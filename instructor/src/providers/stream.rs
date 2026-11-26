use super::indexed::IndexedCalls;
use super::response::{
    arguments, check_error, check_finish, check_name, check_prompt_block, check_refusal,
    check_responses_status, collect_text, entries, responses_output,
};
use super::{tool_name, Mode, Provider, ProviderConfig};
use crate::streaming::{SseDecoder, SseEvent};
use crate::Error;
use futures_util::{stream::BoxStream, StreamExt};
use serde_json::Value;
use std::collections::HashSet;

impl ProviderConfig {
    pub(crate) fn deltas(
        &self,
        response: reqwest::Response,
        schema: &Value,
        iterable: bool,
    ) -> BoxStream<'static, Result<String, Error>> {
        let mut parser = Parser::new(self.provider, self.mode, tool_name(schema).to_owned());
        if iterable
            && matches!(self.mode, Mode::Tools | Mode::ToolsStrict)
            && matches!(
                self.provider,
                Provider::OpenAI | Provider::OpenRouter | Provider::Mistral
            )
        {
            parser.indexed = Some(IndexedCalls::default());
        }
        Box::pin(async_stream::try_stream! {
            let mut bytes = response.bytes_stream();
            let mut decoder = SseDecoder::default();
            loop {
                let (events,eof) = match bytes.next().await {
                    Some(bytes) => (decoder.feed(&bytes?),false),
                    None => (decoder.finish(),true),
                };
                for event in events {
                    let fragment = parser.event(event?)?;
                    if !fragment.is_empty() {
                        parser.has_output = true;
                        yield fragment;
                    }
                    if parser.done { break; }
                }
                if parser.done || eof { break; }
            }
            parser.finish()?;
            if let Some(indexed) = parser.indexed {
                for fragment in indexed.finish(&parser.expected) {
                    yield fragment?;
                }
            }
        })
    }
}

// State belongs to one HTTP response. Never concatenate independent calls,
// candidates, reasoning text or auxiliary tool output into a model's JSON.
struct Parser {
    indexed: Option<IndexedCalls>,
    provider: Provider,
    mode: Mode,
    expected: String,
    selected: Option<u64>,
    streamed_name: String,
    pending_empty_tool: bool,
    ignored: HashSet<u64>,
    has_output: bool,
    received_finish: bool,
    done: bool,
}

impl Parser {
    fn new(provider: Provider, mode: Mode, expected: String) -> Self {
        Self {
            indexed: None,
            provider,
            mode,
            expected,
            selected: None,
            streamed_name: String::new(),
            pending_empty_tool: false,
            ignored: HashSet::new(),
            has_output: false,
            received_finish: false,
            done: false,
        }
    }

    fn event(&mut self, event: SseEvent) -> Result<String, Error> {
        let data = event.data.trim();
        if event.event.as_deref() == Some("error") {
            return Err(Error::Provider(data.into()));
        }
        if data.is_empty() || event.event.as_deref() == Some("ping") {
            return Ok(String::new());
        }
        if data == "[DONE]" {
            if !matches!(
                self.provider,
                Provider::OpenAI | Provider::OpenRouter | Provider::Mistral
            ) || self.mode == Mode::ResponsesTools
            {
                return Err(Error::Incomplete("Unexpected end sentinel".into()));
            }
            self.done = true;
            return Ok(String::new());
        }
        let raw: Value = serde_json::from_str(data)
            .map_err(|err| Error::Protocol(format!("Invalid JSON in SSE event: {err}")))?;
        if !raw.is_object() {
            return Err(Error::Protocol("Expected SSE object".into()));
        }
        let kind = raw["type"]
            .as_str()
            .or(event.event.as_deref())
            .unwrap_or("");
        check_error(&raw)?;
        if kind == "error" {
            return Err(Error::Provider(data.into()));
        }
        match self.provider {
            Provider::OpenAI if self.mode == Mode::ResponsesTools => self.responses(kind, &raw),
            Provider::OpenAI | Provider::OpenRouter | Provider::Mistral => self.chat(&raw),
            Provider::Google => self.google(&raw),
            Provider::Cohere => self.cohere(kind, &raw),
            Provider::Anthropic => self.anthropic(kind, &raw),
        }
    }

    fn finish(&self) -> Result<(), Error> {
        if !self.received_finish && !self.done {
            return Err(Error::Incomplete(
                "Stream ended before a provider finish marker".into(),
            ));
        }
        if !self.streamed_name.is_empty() && self.streamed_name != self.expected {
            return Err(Error::Parse(format!(
                "Expected tool {}, received {}",
                self.expected, self.streamed_name
            )));
        }
        if !self.has_output && !self.indexed.as_ref().is_some_and(IndexedCalls::has_calls) {
            return Err(Error::Parse("Empty result stream".into()));
        }
        Ok(())
    }

    fn select(&mut self, index: u64) -> Result<(), Error> {
        if self.selected.is_some_and(|prev| prev != index) {
            return Err(Error::Parse(
                "Expected exactly one structured output".into(),
            ));
        }
        self.selected = Some(index);
        Ok(())
    }

    fn chat(&mut self, raw: &Value) -> Result<String, Error> {
        let choices = raw["choices"]
            .as_array()
            .ok_or_else(|| Error::Protocol("Missing choices in stream".into()))?;
        if choices.is_empty() {
            return Ok(String::new());
        } // Usage-only chunk.
        if choices.len() != 1 || index(&choices[0])? != 0 {
            return Err(Error::Protocol(
                "Expected a single streamed candidate".into(),
            ));
        }
        let choice = &choices[0];
        check_error(choice)?;
        self.received_finish |= check_finish(&choice["finish_reason"])?;
        let delta = &choice["delta"];
        check_refusal(delta)?;
        if !self.mode.is_tool() {
            return collect_text(&delta["content"]).map(std::borrow::Cow::into_owned);
        }
        if let Some(indexed) = &mut self.indexed {
            return indexed.feed(&delta["tool_calls"], &self.expected);
        }
        let mut out = String::new();
        for (position, call) in entries(&delta["tool_calls"]).enumerate() {
            if position != 0 {
                return Err(Error::Parse(
                    "Expected a single streamed function call".into(),
                ));
            }
            self.select(index(call)?)?;
            out.push_str(&self.function_delta(&call["function"])?);
        }
        Ok(out)
    }

    fn function_delta(&mut self, func: &Value) -> Result<String, Error> {
        if let Some(name) = func["name"].as_str() {
            if self.streamed_name != self.expected || name != self.expected {
                self.streamed_name.push_str(name);
            }
            if !self.expected.starts_with(&self.streamed_name) {
                return Err(Error::Parse(format!(
                    "Unexpected tool {}",
                    self.streamed_name
                )));
            }
        }
        if func["arguments"].is_null() {
            return Ok(String::new());
        }
        arguments(&func["arguments"]).map(std::borrow::Cow::into_owned)
    }

    fn anthropic(&mut self, kind: &str, raw: &Value) -> Result<String, Error> {
        let idx = index(raw)?;
        let delta = &raw["delta"];
        match kind {
            "content_block_start" => {
                let block = &raw["content_block"];
                check_refusal(block)?;
                if self.mode.is_tool() && block["type"] == "tool_use" {
                    if self.selected.is_some() {
                        return Err(Error::Parse("Multiple streamed tool blocks".into()));
                    }
                    check_name(&block["name"], &self.expected)?;
                    self.select(idx)?;
                    if block["input"].as_object().is_some_and(|v| !v.is_empty()) {
                        return arguments(&block["input"]).map(std::borrow::Cow::into_owned);
                    }
                    self.pending_empty_tool = block["input"].is_object();
                }
                if !self.mode.is_tool() && block["type"] == "text" {
                    return delta_text(block, "text");
                }
            }
            "content_block_delta" => match delta["type"].as_str() {
                Some("input_json_delta") if self.mode.is_tool() && self.selected == Some(idx) => {
                    let fragment = delta_text(delta, "partial_json")?;
                    if !fragment.is_empty() {
                        self.pending_empty_tool = false;
                    }
                    return Ok(fragment);
                }
                Some("text_delta") if !self.mode.is_tool() => return delta_text(delta, "text"),
                _ => (),
            },
            "content_block_stop" if self.selected == Some(idx) && self.pending_empty_tool => {
                self.pending_empty_tool = false;
                return Ok("{}".into());
            }
            "message_delta" => {
                check_finish(&delta["stop_reason"])?;
            }
            "message_stop" => self.done = true,
            _ => (),
        }
        Ok(String::new())
    }

    fn cohere(&mut self, kind: &str, raw: &Value) -> Result<String, Error> {
        let delta = &raw["delta"];
        let message = &delta["message"];
        match kind {
            "tool-call-start" | "tool-call-delta" if self.mode.is_tool() => {
                self.select(index(raw)?)?;
                self.function_delta(&message["tool_calls"]["function"])
            }
            "content-start" | "content-delta" if !self.mode.is_tool() => {
                let content = &message["content"];
                if !matches!(content["type"].as_str(), None | Some("text")) {
                    return Ok(String::new());
                }
                Ok(content["text"].as_str().unwrap_or_default().to_owned())
            }
            "message-end" => {
                check_finish(&delta["finish_reason"])?;
                self.done = true;
                Ok(String::new())
            }
            _ => Ok(String::new()),
        }
    }

    fn google(&mut self, raw: &Value) -> Result<String, Error> {
        check_prompt_block(raw)?;
        let mut out = String::new();
        for (i, c) in entries(&raw["candidates"]).enumerate() {
            if i != 0 || index(c)? != 0 {
                return Err(Error::Protocol("Expected a single Gemini candidate".into()));
            }
            self.done |= check_finish(&c["finishReason"])?;
            let parts = &c["content"]["parts"];
            if !self.mode.is_tool() {
                out.push_str(&collect_text(parts)?);
            } else {
                for call in entries(parts)
                    .filter(|p| p["thought"] != true)
                    .filter_map(|p| p.get("functionCall"))
                {
                    if self.selected.is_some() {
                        return Err(Error::Parse("Expected one Gemini function call".into()));
                    }
                    self.select(0)?;
                    check_name(&call["name"], &self.expected)?;
                    out.push_str(&arguments(&call["args"])?);
                }
            }
        }
        Ok(out)
    }

    fn responses(&mut self, kind: &str, raw: &Value) -> Result<String, Error> {
        let index = raw["output_index"].as_u64().unwrap_or_default();
        match kind {
            "response.output_item.added" => {
                let item = &raw["item"];
                if (item["type"] == "function_call" && item["name"] != self.expected)
                    || item["phase"] == "commentary"
                {
                    self.ignored.insert(index);
                }
            }
            "response.function_call_arguments.delta" | "response.output_text.delta" => {
                let fragment = delta_text(raw, "delta")?;
                if !self.ignored.contains(&index) && !fragment.is_empty() {
                    self.select(index)?;
                    return Ok(fragment);
                }
            }
            "response.refusal.delta" | "response.refusal.done" => {
                return Err(Error::Refusal(
                    raw["delta"]
                        .as_str()
                        .or(raw["refusal"].as_str())
                        .unwrap_or("Model declined")
                        .into(),
                ));
            }
            "response.incomplete" => {
                return Err(Error::Incomplete(
                    raw["response"]["incomplete_details"].to_string(),
                ))
            }
            "response.failed" => return Err(Error::Provider(raw["response"]["error"].to_string())),
            "response.completed" => {
                let response = &raw["response"];
                check_responses_status(response)?;
                self.done = true;
                if !self.has_output {
                    return responses_output(response, &self.expected)
                        .map(std::borrow::Cow::into_owned);
                }
                // Validate any final refusal or unexpected multiplicity reported in
                // the canonical output, even after preview fragments were emitted.
                if entries(&response["output"]).next().is_some() {
                    responses_output(response, &self.expected)?;
                }
            }
            _ => (),
        }
        Ok(String::new())
    }
}

fn delta_text(raw: &Value, key: &str) -> Result<String, Error> {
    raw[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::Protocol(format!("Expected string delta {key}")))
}

fn index(raw: &Value) -> Result<u64, Error> {
    match raw.get("index") {
        None | Some(Value::Null) => Ok(0),
        Some(index) => index
            .as_u64()
            .ok_or_else(|| Error::Protocol("Invalid stream index".into())),
    }
}
