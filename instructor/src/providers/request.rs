use super::{tool_name, Mode, Provider, ProviderConfig};
use crate::{CacheTtl, Error, Message, Request};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT};
use serde_json::{json, Value};

/// Own the prepared payload so retries preserve the caller's original request.
pub(crate) struct WireRequest {
    url: String,
    headers: HeaderMap,
    pub(crate) body: Value,
}

impl WireRequest {
    pub(crate) async fn send(&self, client: &reqwest::Client) -> Result<reqwest::Response, Error> {
        let response = client
            .post(&self.url)
            .headers(self.headers.clone())
            .json(&self.body)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Http {
                status: status.as_u16(),
                body: response.text().await?,
            });
        }
        Ok(response)
    }
}

impl ProviderConfig {
    pub(crate) fn request(
        &self,
        req: &Request,
        schema: Option<&Value>,
        stream: bool,
    ) -> Result<WireRequest, Error> {
        self.validate()?;
        let mut body = Value::Object(req.options.clone());
        check_options(&body, self.mode)?;
        let instruction = match (self.mode, schema) {
            (Mode::Json | Mode::MarkdownJson, Some(s)) =>
                Some(format!("Return a single JSON object matching this JSON Schema. Respond with JSON only.\n{s}")),
            _ => None,
        };
        let mut messages = Vec::new();
        let mut system = Vec::new();
        let instruction = instruction.map(Message::system);
        for msg in instruction.iter().chain(req.messages.iter()) {
            let role = msg.role.as_str();
            let content = msg.content.as_str();
            let attachments = msg.attachments.as_slice();
            if msg.cache_control.is_some()
                && !matches!(self.provider, Provider::Anthropic | Provider::OpenRouter)
            {
                return Err(Error::Configuration(
                    "Prompt-cache breakpoints require Anthropic or OpenRouter".into(),
                ));
            }
            if !matches!(role, "system" | "developer" | "assistant" | "user") {
                return Err(Error::Configuration(format!(
                    "Unsupported message role {role:?}"
                )));
            }
            if !attachments.is_empty() && role != "user" {
                return Err(Error::Configuration(
                    "attachments require a user message".into(),
                ));
            }
            match self.provider {
                Provider::Google | Provider::Anthropic
                    if matches!(role, "system" | "developer") =>
                {
                    system.push(msg)
                }
                Provider::Google => {
                    let role = if role == "assistant" { "model" } else { "user" };
                    let parts =
                        crate::media::content(self.provider, self.mode, content, attachments)?;
                    messages.push(json!({"role":role,"parts":parts}));
                }
                _ => {
                    let role = if self.provider != Provider::OpenAI && role == "developer" {
                        "system"
                    } else {
                        role
                    };
                    let mut content =
                        crate::media::content(self.provider, self.mode, content, attachments)?;
                    if let Some(ttl) = msg.cache_control {
                        if content.is_string() {
                            content = json!([{"type":"text","text":content}]);
                        }
                        if let Some(last) = content.as_array_mut().and_then(|v| v.last_mut()) {
                            last["cache_control"] = cache_control(ttl);
                        }
                    }
                    messages.push(json!({"role":role,"content":content}));
                }
            }
        }
        if !req.native.is_empty() {
            if !req.native.iter().all(Value::is_object) {
                return Err(Error::Configuration(
                    "Expected native message objects".into(),
                ));
            }
            // Chat endpoints include the generated instruction as the first
            // message. Anthropic and Gemini put it in a separate system field.
            let pos = usize::from(
                instruction.is_some()
                    && !matches!(self.provider, Provider::Anthropic | Provider::Google),
            );
            messages.splice(pos..pos, req.native.iter().cloned());
        } else if req.messages.is_empty() || messages.is_empty() {
            return Err(Error::Configuration("Expected at least one message".into()));
        }
        if self.provider == Provider::Google {
            body["contents"] = messages.into();
            if !system.is_empty() {
                body["systemInstruction"] = json!({"parts":[{"text":system_prompt(&system)}]});
            }
        } else {
            if self.mode == Mode::ResponsesTools {
                body["input"] = messages.into();
                if let Some(max_tokens) = body.as_object_mut().and_then(|b| b.remove("max_tokens"))
                {
                    body["max_output_tokens"] = max_tokens;
                }
            } else {
                body["messages"] = messages.into();
            }
            body["model"] = json!(self.model);
            body["stream"] = json!(stream);
            if self.provider == Provider::Anthropic {
                if body.get("max_tokens").is_none() {
                    body["max_tokens"] = json!(1024);
                }
                if !system.is_empty() {
                    body["system"] = system_prompt(&system);
                }
            }
        }
        if let Some(schema) = schema {
            self.set_output(&mut body, schema)?;
        }
        Ok(WireRequest {
            url: self.endpoint(stream),
            headers: self.headers(stream)?,
            body,
        })
    }

    fn endpoint(&self, stream: bool) -> String {
        let base = self.base_url.trim_end_matches('/');
        match self.provider {
            Provider::OpenAI if self.mode == Mode::ResponsesTools => format!("{base}/responses"),
            Provider::OpenAI | Provider::OpenRouter | Provider::Mistral => {
                format!("{base}/chat/completions")
            }
            Provider::Anthropic => format!("{base}/messages"),
            Provider::Cohere => format!("{base}/chat"),
            Provider::Google => {
                let model = self.model.strip_prefix("models/").unwrap_or(&self.model);
                let method = if stream {
                    "streamGenerateContent?alt=sse"
                } else {
                    "generateContent"
                };
                format!("{base}/models/{model}:{method}")
            }
        }
    }

    fn headers(&self, stream: bool) -> Result<HeaderMap, Error> {
        let mut h = HeaderMap::new();
        h.insert(
            ACCEPT,
            HeaderValue::from_static(if stream {
                "text/event-stream"
            } else {
                "application/json"
            }),
        );
        let (name, value) = match self.provider {
            Provider::Anthropic => {
                h.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
                ("x-api-key", self.api_key.clone())
            }
            Provider::Google => ("x-goog-api-key", self.api_key.clone()),
            _ => ("authorization", format!("Bearer {}", self.api_key)),
        };
        let mut token = HeaderValue::from_str(&value)
            .map_err(|_| Error::Configuration("Invalid characters in API key".into()))?;
        token.set_sensitive(true);
        h.insert(name, token);
        Ok(h)
    }

    pub(super) fn set_output(&self, body: &mut Value, schema: &Value) -> Result<(), Error> {
        let name = tool_name(schema);
        let description = crate::schema::tool_description(schema);
        match self.mode {
            Mode::MarkdownJson => (),
            Mode::ResponsesTools => {
                let mut schema = schema.clone();
                constrain_schema(&mut schema, self.provider)?;
                let mut tools = body["tools"].as_array().cloned().unwrap_or_default();
                if tools.is_empty() {
                    body["tool_choice"] = json!({"type":"function","name":name});
                }
                tools.push(json!({"type":"function","name":name,"parameters":schema,"description":description,"strict":true}));
                body["tools"] = tools.into();
                body["text"]["format"] =
                    json!({"type":"json_schema","name":name,"schema":schema,"strict":true});
            }
            Mode::Json => match self.provider {
                Provider::Anthropic => (),
                Provider::Google => {
                    body["generationConfig"]["responseMimeType"] = json!("application/json")
                }
                _ => body["response_format"] = json!({"type":"json_object"}),
            },
            Mode::Tools | Mode::ToolsStrict => {
                let mut params = crate::schema::tool_parameters(schema);
                match self.provider {
                    Provider::Google => {
                        body["tools"] = json!([{"functionDeclarations":[{
                            "name":name,"description":description,"parametersJsonSchema":params
                        }]}]);
                        body["toolConfig"] = json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":[name]}});
                    }
                    Provider::Anthropic => {
                        body["tools"] =
                            json!([{"name":name,"description":description,"input_schema":params}]);
                        body["tool_choice"] =
                            if requires_auto_choice(&self.model, &body["thinking"]) {
                                json!({"type":"auto"})
                            } else {
                                json!({"type":"tool","name":name})
                            };
                    }
                    _ => {
                        let strict = self.mode == Mode::ToolsStrict;
                        if strict {
                            constrain_schema(&mut params, self.provider)?;
                        }
                        let mut function =
                            json!({"name":name,"description":description,"parameters":params});
                        if strict {
                            function["strict"] = json!(true);
                        }
                        body["tools"] = json!([{"type":"function","function":function}]);
                        body["tool_choice"] = match self.provider {
                            Provider::Mistral => json!("any"),
                            Provider::Cohere => json!("REQUIRED"),
                            _ => json!({"type":"function","function":{"name":name}}),
                        };
                    }
                }
            }
            Mode::JsonSchema => {
                let mut schema = schema.clone();
                constrain_schema(&mut schema, self.provider)?;
                match self.provider {
                    Provider::Google => {
                        let config = &mut body["generationConfig"];
                        config["responseMimeType"] = json!("application/json");
                        config["responseJsonSchema"] = schema;
                    }
                    Provider::Cohere => {
                        body["response_format"] = json!({"type":"json_object","schema":schema})
                    }
                    Provider::Anthropic => {
                        body["output_config"]["format"] =
                            json!({"type":"json_schema","schema":schema});
                    }
                    Provider::OpenAI | Provider::OpenRouter | Provider::Mistral => {
                        body["response_format"] = json!({"type":"json_schema","json_schema":{
                            "name":name,"schema":schema,"strict":true
                        }});
                    }
                }
            }
        }
        Ok(())
    }
}

fn cache_control(ttl: CacheTtl) -> Value {
    match ttl {
        CacheTtl::FiveMinutes => json!({"type":"ephemeral"}),
        CacheTtl::OneHour => json!({"type":"ephemeral","ttl":"1h"}),
    }
}

fn system_prompt(system: &[&Message]) -> Value {
    if !system.iter().any(|m| m.cache_control.is_some()) {
        return json!(system
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n"));
    }
    system
        .iter()
        .map(|msg| {
            let mut block = json!({"type":"text","text":msg.content});
            if let Some(ttl) = msg.cache_control {
                block["cache_control"] = cache_control(ttl);
            }
            block
        })
        .collect()
}

fn check_options(options: &Value, mode: Mode) -> Result<(), Error> {
    if let Some(tools) = options.get("tools") {
        if mode != Mode::ResponsesTools
            || !tools.as_array().is_some_and(|tools| {
                tools.iter().all(|tool| {
                    tool["type"]
                        .as_str()
                        .is_some_and(|kind| !matches!(kind, "function" | "custom"))
                })
            })
        {
            return Err(Error::Configuration(
                "Use the tools option for built-in tools in ResponsesTools mode".into(),
            ));
        }
    }
    for key in [
        "model",
        "messages",
        "stream",
        "contents",
        "system",
        "systemInstruction",
        "system_instruction",
        "input",
        "tool_choice",
        "toolConfig",
        "tool_config",
        "response_format",
        "output_format",
        "functions",
        "function_call",
        "generation_config",
    ] {
        if options.get(key).is_some() {
            return Err(Error::Configuration(format!(
                "Reserved request option: {key}"
            )));
        }
    }
    for (root, keys) in [
        (
            "generationConfig",
            &[
                "responseMimeType",
                "responseJsonSchema",
                "responseSchema",
                "response_mime_type",
                "response_json_schema",
                "response_schema",
                "_responseJsonSchema",
            ][..],
        ),
        ("output_config", &["format"][..]),
        ("text", &["format"][..]),
    ] {
        if let Some(config) = options.get(root) {
            if !config.is_object() {
                return Err(Error::Configuration(format!("Expected object for {root}")));
            }
            for key in keys {
                if config.get(*key).is_some() {
                    return Err(Error::Configuration(format!(
                        "Reserved request option: {root}.{key}"
                    )));
                }
            }
        }
    }
    for pointer in [
        "/n",
        "/generationConfig/candidateCount",
        "/generationConfig/candidate_count",
    ] {
        if let Some(n) = options.pointer(pointer) {
            if n.as_u64() != Some(1) {
                return Err(Error::Configuration(
                    "Expected a single completion candidate".into(),
                ));
            }
        }
    }
    Ok(())
}

// Python shares these rules between Claude providers in v2/providers/claude.py.
fn requires_auto_choice(model: &str, thinking: &Value) -> bool {
    if matches!(thinking["type"].as_str(), Some("enabled" | "adaptive")) {
        return true;
    }
    let lower = model.to_ascii_lowercase();
    lower.match_indices("claude-").any(|(i, _)| {
        if i > 0 && !b"./:".contains(&lower.as_bytes()[i - 1]) {
            return false;
        }
        let tail = lower[i..].replace('.', "-");
        [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-fable-5-1",
            "claude-mythos-5-1",
        ]
        .iter()
        .any(|m| {
            tail.strip_prefix(*m)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', ':', '@']))
        })
    })
}

// Visit schema positions, preserving ordinary properties called "items", "default", etc.
fn constrain_schema(schema: &mut Value, provider: Provider) -> Result<(), Error> {
    let Some(map) = schema.as_object_mut() else {
        return Ok(());
    };
    let is_object = match map.get("type") {
        Some(Value::Array(types)) => types.iter().any(|t| t == "object"),
        Some(t) => t == "object",
        _ => false,
    };
    let close = !matches!(provider, Provider::Cohere | Provider::Google);
    if close && (is_object || map.contains_key("properties")) {
        if map
            .get("additionalProperties")
            .is_some_and(|v| v.is_object() || *v == true)
            || (!map.contains_key("properties") && !map.contains_key("additionalProperties"))
            || map.contains_key("patternProperties")
        {
            return Err(Error::Configuration(
                "Dynamic object keys require a non-strict output mode".into(),
            ));
        }
        map.insert("additionalProperties".into(), json!(false));
        if provider != Provider::Anthropic {
            if let Some(props) = map.get("properties").and_then(Value::as_object) {
                let required = Value::Array(props.keys().map(|k| json!(k)).collect());
                map.insert("required".into(), required);
            }
        }
    }
    if let Some(variants) = map.remove("oneOf") {
        map.insert("anyOf".into(), variants);
    }
    if let Some(value) = map.remove("const") {
        map.insert("enum".into(), json!([value]));
    }
    map.remove("default");
    for key in ["properties", "definitions", "$defs"] {
        if let Some(children) = map.get_mut(key).and_then(Value::as_object_mut) {
            for value in children.values_mut() {
                constrain_schema(value, provider)?
            }
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(value) = map.get_mut(key) {
            constrain_schema(value, provider)?;
        }
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(children) = map.get_mut(key).and_then(Value::as_array_mut) {
            for value in children {
                constrain_schema(value, provider)?
            }
        }
    }
    move_unsupported_to_description(map, provider);
    Ok(())
}

// Schemars emits Rust numeric formats (uint32, int64, etc.) and bounds.
// Native grammars accept different subsets. As in Anthropic's SDK transform,
// describe omitted constraints to the model and retain Rust-side checks.
fn move_unsupported_to_description(map: &mut serde_json::Map<String, Value>, provider: Provider) {
    let mut hints = serde_json::Map::new();
    if let Some(format) = map.get("format").and_then(Value::as_str) {
        let supported = match format {
            "time" | "date" | "date-time" | "uuid" => true,
            "duration" | "email" | "hostname" | "ipv4" | "ipv6" => provider != Provider::Cohere,
            "uri" => provider == Provider::Anthropic,
            _ => false,
        };
        if !supported {
            hints.insert("format".into(), json!(format));
            map.remove("format");
        }
    }
    if matches!(provider, Provider::Anthropic | Provider::Cohere) {
        for key in [
            "minimum",
            "maximum",
            "exclusiveMinimum",
            "exclusiveMaximum",
            "multipleOf",
            "minLength",
            "maxLength",
            "minItems",
            "maxItems",
            "uniqueItems",
        ] {
            if let Some(value) = map.remove(key) {
                hints.insert(key.into(), value);
            }
        }
    }
    if !hints.is_empty() {
        let existing = map
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let suffix = Value::Object(hints);
        let description = if existing.is_empty() {
            suffix.to_string()
        } else {
            format!("{existing}\n\n{suffix}")
        };
        map.insert("description".into(), json!(description));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_schema_translates_rust_formats_and_preserves_field_names() {
        let schema = json!({
            "type":"object","properties":{
                "default":{"type":"integer","format":"uint32","minimum":0,"description":"Count"},
                "oneOf":{"oneOf":[{"const":"literal"},{"$ref":"#/$defs/Item"}]}
            },
            "required":["default"],
            "$defs":{"Item":{"type":"object","properties":{"id":{"type":"string","format":"uuid"}}}}
        });
        for provider in [
            Provider::OpenAI,
            Provider::Anthropic,
            Provider::OpenRouter,
            Provider::Cohere,
        ] {
            let mut normalized = schema.clone();
            constrain_schema(&mut normalized, provider).unwrap();
            let props = &normalized["properties"];
            assert_eq!(props["oneOf"]["anyOf"][0]["enum"], json!(["literal"]));
            assert_eq!(props["oneOf"]["anyOf"][1]["$ref"], "#/$defs/Item");
            assert!(props["default"].get("format").is_none());
            assert!(props["default"]["description"]
                .as_str()
                .unwrap()
                .contains("uint32"));
            assert_eq!(
                normalized["$defs"]["Item"]["properties"]["id"]["format"],
                "uuid"
            );
            if matches!(provider, Provider::Anthropic | Provider::Cohere) {
                assert!(props["default"].get("minimum").is_none());
                assert_eq!(normalized["required"], schema["required"]);
            } else {
                assert_eq!(props["default"]["minimum"], 0);
                assert_eq!(normalized["required"], json!(["default", "oneOf"]));
            }
        }
    }
}
