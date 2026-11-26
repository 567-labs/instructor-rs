extern crate instruct_macros;
extern crate instruct_macros_types;

use instruct_macros::InstructMacro;
use instructor_ai::from_openai;
mod support;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use openai_api_rs::v1::{
        chat_completion::{self, ChatCompletionRequest},
        common::GPT4_O,
    };
    use serde::{Deserialize, Serialize};

    use super::*;

    #[test]
    fn test_simple_option() {
        if support::run_loopback_in_child("tests::test_simple_option") {
            return;
        }
        let (client, _server) = support::client(
            "MaybeUser",
            &[json!({"name": null, "error_message": "missing"})],
        );
        let instructor_client = from_openai(client);

        #[derive(InstructMacro, Debug, Serialize, Deserialize)]
        struct MaybeUser {
            #[description(
                "This is an optional name of a person. If no user name can be found, the field will be null"
            )]
            name: Option<String>,
            error_message: String,
        }

        let req = ChatCompletionRequest::new(
            GPT4_O.to_string(),
            vec![chat_completion::ChatCompletionMessage {
                role: chat_completion::MessageRole::user,
                content: chat_completion::Content::Text(String::from("It's a beautiful day out")),
                name: None,
            }],
        );

        let result = instructor_client
            .chat_completion::<MaybeUser>(req, 3)
            .unwrap();

        assert!(result.name.is_none());
    }

    #[test]
    fn test_complex_option() {
        if support::run_loopback_in_child("tests::test_complex_option") {
            return;
        }
        let (client, _server) = support::client(
            "MaybeUser",
            &[json!({"user": null, "error_message": "missing"})],
        );
        let instructor_client = from_openai(client);

        #[derive(InstructMacro, Debug, Serialize, Deserialize)]
        struct UserInfo {
            name: String,
            age: u8,
        }

        #[derive(InstructMacro, Debug, Serialize, Deserialize)]
        struct MaybeUser {
            #[description("This is an optional user field. If the user is not present, the field will be null")]
            user: Option<UserInfo>,
            error_message: String,
        }

        let req = ChatCompletionRequest::new(
            GPT4_O.to_string(),
            vec![chat_completion::ChatCompletionMessage {
                role: chat_completion::MessageRole::user,
                content: chat_completion::Content::Text(String::from(
                    "Try to extract a user from the following sentence: `It's a beautiful day out`",
                )),
                name: None,
            }],
        );

        let result = instructor_client
            .chat_completion::<MaybeUser>(req, 3)
            .unwrap();

        assert!(result.user.is_none());
    }
}
