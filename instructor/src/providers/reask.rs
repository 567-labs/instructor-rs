use super::response::entries;
use super::{Provider, ProviderConfig, WireRequest};
use serde_json::{json, Value};

impl ProviderConfig {
    /// Append a correction in the provider's turn format. A failed stream may
    /// lack a replayable response, in which case feedback is a new user turn.
    pub(crate) fn reask(&self, wire: &mut WireRequest, raw: &Value, feedback: &str) {
        match self.provider {
            Provider::Google => {
                if let Some(history) = wire.body["input"].as_array_mut() {
                    google(history, &raw["steps"], feedback);
                }
            }
            _ if self.uses_responses() => {
                if let Some(input) = wire.body["input"].as_array_mut() {
                    responses(input, &raw["output"], feedback);
                }
            }
            provider => {
                if let Some(history) = wire.body["messages"].as_array_mut() {
                    match provider {
                        Provider::Anthropic => anthropic(history, &raw["content"], feedback),
                        Provider::Cohere => chat(history, &raw["message"], feedback),
                        _ => chat(history, &raw["choices"][0]["message"], feedback),
                    }
                }
            }
        }
    }
}

fn user_feedback(text: &str) -> Value {
    json!({"role":"user","content":text})
}

// Replay response items in order so encrypted reasoning and message phases
// survive stateless retries. Every client function call needs a matching output.
fn responses(input: &mut Vec<Value>, output: &Value, feedback: &str) {
    let mut results = Vec::new();
    for item in entries(output).filter(|i| i["type"] == "function_call") {
        let Some(id) = item["call_id"].as_str().filter(|s| !s.is_empty()) else {
            input.push(user_feedback(feedback));
            return;
        };
        results.push(json!({"type":"function_call_output","call_id":id,"output":feedback}));
    }
    input.extend(entries(output).cloned());
    if results.is_empty() {
        input.push(user_feedback(feedback));
    } else {
        input.extend(results);
    }
}

fn chat(history: &mut Vec<Value>, message: &Value, feedback: &str) {
    let mut results = Vec::new();
    let mut calls = Vec::new();
    for call in entries(&message["tool_calls"]) {
        let (Some(id), Some(name)) = (call["id"].as_str(), call["function"]["name"].as_str())
        else {
            history.push(user_feedback(feedback));
            return;
        };
        let args = &call["function"]["arguments"];
        let arguments = args
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| args.to_string());
        calls.push(
            json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments}}),
        );
        results.push(json!({"role":"tool","tool_call_id":id,"content":feedback}));
    }
    if !calls.is_empty() || !message["content"].is_null() {
        let mut prior = json!({"role":"assistant"});
        for key in [
            "content",
            "reasoning_details",
            "reasoning",
            "reasoning_content",
            "tool_plan",
        ] {
            if !message[key].is_null() {
                prior[key] = message[key].clone();
            }
        }
        if !calls.is_empty() {
            prior["tool_calls"] = calls.into();
        }
        history.push(prior);
    }
    if results.is_empty() {
        results.push(user_feedback(feedback));
    }
    history.extend(results);
}

fn anthropic(history: &mut Vec<Value>, content: &Value, feedback: &str) {
    let mut results = Vec::new();
    for block in entries(content).filter(|b| b["type"] == "tool_use") {
        let Some(id) = block["id"].as_str() else {
            history.push(user_feedback(feedback));
            return;
        };
        results.push(
            json!({"type":"tool_result","tool_use_id":id,"is_error":true,"content":feedback}),
        );
    }
    if entries(content).next().is_some() {
        // Signed thinking blocks must survive the round trip with the tool use.
        history.push(json!({"role":"assistant","content":content}));
    }
    if results.is_empty() {
        history.push(user_feedback(feedback));
    } else {
        history.push(json!({"role":"user","content":results}));
    }
}

fn google(history: &mut Vec<Value>, steps: &Value, feedback: &str) {
    let content = json!([{"type":"text","text":feedback}]);
    let mut results = Vec::new();
    for step in entries(steps).filter(|s| s["type"] == "function_call") {
        let (Some(id), Some(name)) = (step["id"].as_str(), step["name"].as_str()) else {
            history.push(json!({"type":"user_input","content":content}));
            return;
        };
        results.push(json!({"type":"function_result","call_id":id,"name":name,"is_error":true,"result":content}));
    }
    // Stateless interactions replay all model steps, including signed thoughts.
    history.extend(entries(steps).cloned());
    if results.is_empty() {
        history.push(json!({"type":"user_input","content":content}));
    }
    history.extend(results);
}
