use super::{tool_name, Mode, Provider, ProviderConfig};
use crate::Error;
use serde_json::Value;

impl ProviderConfig {
    pub(crate) fn extract(&self, raw: &Value, schema: &Value) -> Result<String, Error> {
        check_error(raw)?;
        let name = tool_name(schema);
        match self.provider {
            Provider::OpenAI if self.mode == Mode::ResponsesTools => responses_output(raw, &name),
            Provider::OpenAI | Provider::OpenRouter | Provider::Mistral => {
                let choice = candidate(raw, "choices")?;
                check_finish(&choice["finish_reason"])?;
                chat_output(&choice["message"], self.mode, &name)
            }
            Provider::Cohere => {
                check_finish(&raw["finish_reason"])?;
                chat_output(&raw["message"], self.mode, &name)
            }
            Provider::Anthropic => {
                check_finish(&raw["stop_reason"])?;
                if !self.mode.is_tool() {
                    return text_content(&raw["content"]);
                }
                let mut calls = entries(&raw["content"]).filter(|b| b["type"] == "tool_use");
                let call = single_call(&mut calls)?;
                check_name(&call["name"], &name)?;
                arguments(&call["input"])
            }
            Provider::Google => {
                check_prompt_block(raw)?;
                let candidate = candidate(raw, "candidates")?;
                check_finish(&candidate["finishReason"])?;
                let parts = &candidate["content"]["parts"];
                if !self.mode.is_tool() {
                    return text_content(parts);
                }
                let mut calls = entries(parts)
                    .filter(|p| p["thought"] != true)
                    .filter_map(|p| p.get("functionCall"));
                let call = single_call(&mut calls)?;
                check_name(&call["name"], &name)?;
                arguments(&call["args"])
            }
        }
    }

    pub(crate) fn usage(&self, raw: &Value) -> Option<u64> {
        fn sum(values: &Value, fields: &[&str]) -> Option<u64> {
            fields
                .iter()
                .filter_map(|key| values[*key].as_u64())
                .reduce(u64::saturating_add)
        }
        let usage = &raw["usage"];
        match self.provider {
            Provider::Anthropic => sum(
                usage,
                &[
                    "input_tokens",
                    "output_tokens",
                    "cache_creation_input_tokens",
                    "cache_read_input_tokens",
                ],
            ),
            Provider::Google => raw["usageMetadata"]["totalTokenCount"]
                .as_u64()
                .or_else(|| {
                    sum(
                        &raw["usageMetadata"],
                        &[
                            "promptTokenCount",
                            "candidatesTokenCount",
                            "thoughtsTokenCount",
                        ],
                    )
                }),
            Provider::Cohere => sum(&usage["tokens"], &["input_tokens", "output_tokens"]),
            _ => usage["total_tokens"].as_u64().or_else(|| {
                sum(
                    usage,
                    &[
                        "prompt_tokens",
                        "completion_tokens",
                        "input_tokens",
                        "output_tokens",
                    ],
                )
            }),
        }
    }
}

pub(super) fn check_error(raw: &Value) -> Result<(), Error> {
    if !raw["error"].is_null() {
        return Err(Error::Provider(raw["error"].to_string()));
    }
    Ok(())
}

pub(super) fn check_prompt_block(raw: &Value) -> Result<(), Error> {
    if let Some(reason) = raw["promptFeedback"]["blockReason"].as_str() {
        if !reason.is_empty() && reason != "BLOCK_REASON_UNSPECIFIED" {
            return Err(Error::Refusal(format!("Gemini prompt blocked: {reason}")));
        }
    }
    Ok(())
}

pub(super) fn check_refusal(v: &Value) -> Result<(), Error> {
    if v["type"] == "refusal" || !v["refusal"].is_null() {
        return Err(Error::Refusal(
            v["refusal"].as_str().unwrap_or("Model declined").into(),
        ));
    }
    Ok(())
}

/// Returns whether this is a successful finish marker.
pub(crate) fn check_finish(value: &Value) -> Result<bool, Error> {
    if value.is_null() {
        return Ok(false);
    }
    let reason = value
        .as_str()
        .ok_or_else(|| Error::Protocol("Expected textual finish reason".into()))?;
    match reason.to_ascii_lowercase().as_str() {
        "" | "finish_reason_unspecified" => Ok(false),
        "stop" | "end_turn" | "tool_use" | "tool_calls" | "function_call" | "tool_call"
        | "complete" | "stop_sequence" => Ok(true),
        "max_tokens"
        | "length"
        | "model_length"
        | "model_context_window_exceeded"
        | "pause_turn"
        | "continuation" => Err(Error::Incomplete(reason.into())),
        "content_filter"
        | "refusal"
        | "safety"
        | "blocklist"
        | "recitation"
        | "prohibited_content"
        | "spii"
        | "image_safety"
        | "image_prohibited_content"
        | "image_recitation"
        | "error_toxic" => Err(Error::Refusal(reason.into())),
        "malformed_function_call" | "unexpected_tool_call" => Err(Error::Parse(reason.into())),
        _ => Err(Error::Provider(format!("Provider ended with {reason}"))),
    }
}

pub(super) fn entries(array: &Value) -> impl Iterator<Item = &Value> {
    array.as_array().into_iter().flatten()
}

pub(super) fn candidate<'a>(raw: &'a Value, key: &str) -> Result<&'a Value, Error> {
    match raw[key].as_array().map(Vec::as_slice) {
        Some([candidate]) => Ok(candidate),
        _ => Err(Error::Protocol(format!("Expected one candidate in {key}"))),
    }
}

fn single_call<'a>(calls: &mut impl Iterator<Item = &'a Value>) -> Result<&'a Value, Error> {
    let call = calls
        .next()
        .ok_or_else(|| Error::Parse("Missing expected tool call".into()))?;
    if calls.next().is_some() {
        return Err(Error::Parse("Expected exactly one tool call".into()));
    }
    Ok(call)
}

pub(super) fn check_name(candidate: &Value, expected: &str) -> Result<(), Error> {
    if candidate.as_str() != Some(expected) {
        return Err(Error::Parse(format!(
            "Expected tool name {expected:?}, received {candidate}"
        )));
    }
    Ok(())
}

pub(super) fn arguments(arg: &Value) -> Result<String, Error> {
    match arg {
        Value::String(text) => Ok(text.clone()),
        Value::Object(_) => Ok(arg.to_string()),
        _ => Err(Error::Parse("Missing or invalid tool arguments".into())),
    }
}

pub(super) fn collect_text(parts: &Value) -> Result<String, Error> {
    if let Some(text) = parts.as_str() {
        return Ok(text.into());
    }
    let mut result = String::new();
    for part in entries(parts) {
        check_refusal(part)?;
        if part["thought"] == true
            || !matches!(part["type"].as_str(), None | Some("text" | "output_text"))
        {
            continue;
        }
        if let Some(text) = part["text"].as_str() {
            result.push_str(text);
        }
    }
    Ok(result)
}

fn text_content(parts: &Value) -> Result<String, Error> {
    let result = collect_text(parts)?;
    if result.trim().is_empty() {
        return Err(Error::Parse("Missing response text".into()));
    }
    Ok(result)
}

fn chat_output(message: &Value, mode: Mode, name: &str) -> Result<String, Error> {
    check_refusal(message)?;
    if !mode.is_tool() {
        return text_content(&message["content"]);
    }
    let call = single_call(&mut entries(&message["tool_calls"]))?;
    let function = &call["function"];
    check_name(&function["name"], name)?;
    arguments(&function["arguments"])
}

pub(super) fn check_responses_status(raw: &Value) -> Result<(), Error> {
    check_error(raw)?;
    match raw["status"].as_str() {
        None | Some("completed") => Ok(()),
        Some("incomplete") => Err(Error::Incomplete(
            raw["incomplete_details"]["reason"]
                .as_str()
                .unwrap_or("Incomplete response")
                .into(),
        )),
        Some(status) => Err(Error::Provider(format!(
            "Response ended with status {status}"
        ))),
    }
}

pub(super) fn responses_output(raw: &Value, name: &str) -> Result<String, Error> {
    check_responses_status(raw)?;
    let mut tool = None;
    let mut text = String::new();
    for item in entries(&raw["output"]) {
        match item["type"].as_str() {
            Some("function_call" | "tool_call") if item["name"] == name => {
                if tool.is_some() {
                    return Err(Error::Parse("Expected one result function".into()));
                }
                tool = Some(arguments(&item["arguments"])?);
            }
            Some("message") if item["phase"] != "commentary" => {
                text.push_str(&collect_text(&item["content"])?)
            }
            _ => (),
        }
    }
    if let Some(args) = tool {
        return Ok(args);
    }
    if text.is_empty() {
        return text_content(&raw["output_text"]);
    }
    Ok(text)
}
