use crate::helpers;
use instruct_macros_types::{InstructMacro, InstructMacroResult};
use openai_api_rs::v1::{
    api::Client,
    chat_completion::{self, ChatCompletionRequest, FinishReason},
    error::APIError,
};
use serde::de::DeserializeOwned;

/// Compatibility wrapper around openai-api-rs 4.x.
pub struct InstructorClient {
    client: Client,
}

impl InstructorClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// The legacy max_retries argument counts total attempts.
    pub fn chat_completion<T: InstructMacro + DeserializeOwned>(
        &self,
        req: ChatCompletionRequest,
        max_retries: u8,
    ) -> Result<T, APIError> {
        let InstructMacroResult::Struct(info) = T::get_info() else {
            return Err(APIError {
                message: "Expected a struct response model".into(),
            });
        };
        let name = info.name.clone();
        let function = chat_completion::Function {
            name: name.clone(),
            description: Some(info.description.clone()),
            parameters: helpers::get_response_model(info),
        };
        let mut req = req
            .tools(vec![chat_completion::Tool {
                r#type: chat_completion::ToolType::Function,
                function,
            }])
            .tool_choice(chat_completion::ToolChoiceType::Auto);
        let mut last_error = "No attempts requested".to_string();
        for _ in 0..max_retries {
            let response = self.client.chat_completion(req.clone())?;
            let choice = response.choices.first().ok_or_else(|| APIError {
                message: "No completion choices".into(),
            })?;
            if matches!(
                choice.finish_reason,
                Some(FinishReason::length | FinishReason::content_filter)
            ) {
                return Err(APIError {
                    message: format!("Incomplete output: {:?}", choice.finish_reason),
                });
            }
            let calls = choice.message.tool_calls.as_deref().unwrap_or_default();
            match parse_call::<T>(calls, &name) {
                Ok(value) => return Ok(value),
                Err(error) => last_error = error,
            }
            req.messages.push(chat_completion::ChatCompletionMessage {
                role: chat_completion::MessageRole::user,
                content: chat_completion::Content::Text(format!(
                    "Please correct the previous response: {last_error}"
                )),
                name: None,
            });
        }
        Err(APIError {
            message: format!("Unable to derive model: {last_error}"),
        })
    }
}

fn parse_call<T: InstructMacro + DeserializeOwned>(
    calls: &[chat_completion::ToolCall],
    name: &str,
) -> Result<T, String> {
    let [call] = calls else {
        return Err(format!(
            "Expected one tool call to {name}, received {}",
            calls.len()
        ));
    };
    if call.function.name.as_deref() != Some(name) {
        return Err(format!("Expected tool name {name}"));
    }
    let args = call
        .function
        .arguments
        .as_deref()
        .ok_or("Missing tool arguments")?;
    let value: T =
        serde_json::from_str(args).map_err(|error| format!("{error} for arguments {args}"))?;
    value.validate()?;
    Ok(value)
}

pub fn from_openai(client: Client) -> InstructorClient {
    InstructorClient::new(client)
}
