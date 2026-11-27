use super::{
    response::{
        arguments, candidate, check_error, check_finish, check_prompt_block, check_refusal, entries,
    },
    Mode, Provider, ProviderConfig, WireRequest,
};
use crate::{Error, Request};
use serde_json::{json, Value};

impl ProviderConfig {
    pub(crate) fn parallel_request<'s>(
        &self,
        req: &Request,
        schemas: impl Iterator<Item = &'s Value>,
    ) -> Result<WireRequest, Error> {
        if !(self.mode.is_tool() || (self.uses_responses() && self.mode == Mode::JsonSchema)) {
            return Err(Error::Configuration(
                "parallel calls require a tool mode".into(),
            ));
        }
        let mut schemas = schemas.peekable();
        let strict = self.schema_strictness(req)?;
        let Some(first) = schemas.peek().copied() else {
            return Err(Error::Configuration("expected at least one tool".into()));
        };
        if self.uses_responses() {
            let mut wire = self.request_with_strictness(req, None, false, strict)?;
            if !req.options.contains_key("tool_choice") {
                wire.body["tool_choice"] = json!("required");
            }
            wire.body["parallel_tool_calls"] = true.into();
            let strict = strict.unwrap_or(self.mode != Mode::Tools);
            for schema in schemas {
                self.set_responses_output(Mode::Tools, &mut wire.body, schema, Some(strict))?;
            }
            return Ok(wire);
        }
        let mut wire = self.request_with_strictness(req, Some(first), false, strict)?;
        let google = self.provider == Provider::Google;
        let mut declarations = Vec::new();
        let mut names = Vec::new();
        // Reuse the exact schema lowering used for single-model tools.
        for (i, schema) in schemas.enumerate() {
            if i > 0 {
                self.set_output(&mut wire.body, schema, strict)?;
            }
            let declared = &mut wire.body["tools"][0];
            if google {
                declarations.push(declared["functionDeclarations"][0].take());
                names.push(super::tool_name(schema));
            } else {
                declarations.push(declared.take());
            }
        }
        if google {
            wire.body["tools"] = json!([{"functionDeclarations": declarations}]);
            wire.body["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"] = json!(names);
        } else {
            wire.body["tools"] = declarations.into();
        }
        match self.provider {
            Provider::Anthropic if !req.options.contains_key("tool_choice") => {
                wire.body["tool_choice"] = json!({"type": "auto"});
            }
            Provider::OpenAI | Provider::OpenRouter => {
                wire.body["tool_choice"] = json!("required");
                wire.body["parallel_tool_calls"] = json!(true);
            }
            Provider::Mistral => wire.body["parallel_tool_calls"] = json!(true),
            _ => (),
        }
        Ok(wire)
    }

    pub(crate) fn parallel_calls<'a>(
        &self,
        raw: &'a Value,
    ) -> Result<Vec<(&'a str, std::borrow::Cow<'a, str>)>, Error> {
        check_error(raw)?;
        let mut calls = Vec::new();
        let mut add = |name: &'a Value, args: &'a Value| -> Result<(), Error> {
            let name = name
                .as_str()
                .ok_or_else(|| Error::Parse("missing tool name".into()))?;
            calls.push((name, arguments(args)?));
            Ok(())
        };
        match self.provider {
            _ if self.uses_responses() => {
                super::response::check_responses_status(raw)?;
                for item in entries(&raw["output"]) {
                    if item["type"] == "function_call" {
                        add(&item["name"], &item["arguments"])?;
                    } else if item["type"] == "message" {
                        for part in entries(&item["content"]) {
                            check_refusal(part)?;
                        }
                    }
                }
            }
            Provider::Google => {
                check_prompt_block(raw)?;
                let choice = candidate(raw, "candidates")?;
                check_finish(&choice["finishReason"])?;
                for part in entries(&choice["content"]["parts"]) {
                    if part["thought"] != true {
                        if let Some(call) = part.get("functionCall") {
                            add(&call["name"], &call["args"])?;
                        }
                    }
                }
            }
            Provider::Anthropic => {
                check_finish(&raw["stop_reason"])?;
                for block in entries(&raw["content"]) {
                    check_refusal(block)?;
                    if block["type"] == "tool_use" {
                        add(&block["name"], &block["input"])?;
                    }
                }
            }
            _ => {
                let choice = if self.provider == Provider::Cohere {
                    raw
                } else {
                    candidate(raw, "choices")?
                };
                check_finish(&choice["finish_reason"])?;
                let message = &choice["message"];
                check_refusal(message)?;
                for call in entries(&message["tool_calls"]) {
                    add(&call["function"]["name"], &call["function"]["arguments"])?;
                }
            }
        }
        if calls.is_empty() {
            return Err(Error::Parse("expected at least one tool call".into()));
        }
        Ok(calls)
    }
}
