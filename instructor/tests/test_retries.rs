#![cfg(feature = "legacy-openai")]

extern crate instruct_macros;
extern crate instruct_macros_types;

use instruct_macros::{validate, InstructMacro};
use instructor_ai::from_openai;
mod support;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use openai_api_rs::v1::{
        chat_completion::{self, ChatCompletionRequest},
        common::GPT3_5_TURBO,
    };
    use serde::{Deserialize, Serialize};

    use super::*;

    #[test]
    fn test_from_openai() {
        if support::run_loopback_in_child("tests::test_from_openai") {
            return;
        }
        let (client, _server) = support::client(
            "UserInfo",
            &[
                json!({"name": "John Doe", "age": 30}),
                json!({"name": "JOHN DOE", "age": 30}),
            ],
        );
        let instructor_client = from_openai(client);

        #[derive(InstructMacro, Debug, Serialize, Deserialize)]
        // This represents a single user
        struct UserInfo {
            // This represents the name of the user
            #[validate(custom = "validate_uppercase")]
            name: String,
            // This represents the age of the user
            age: u8,
        }

        #[validate]
        fn validate_uppercase(name: &String) -> Result<String, String> {
            if name.chars().any(|c| c.is_lowercase()) {
                return Err(format!(
                    "Name '{}' should be entirely in uppercase. Examples: 'TIMOTHY', 'JANE SMITH'",
                    name
                ));
            }
            Ok(name.to_uppercase())
        }

        let req = ChatCompletionRequest::new(
            GPT3_5_TURBO.to_string(),
            vec![chat_completion::ChatCompletionMessage {
                role: chat_completion::MessageRole::user,
                content: chat_completion::Content::Text(String::from(
                    "John Doe is a 30 year old software engineer",
                )),
                name: None,
            }],
        );

        let result = instructor_client
            .chat_completion::<UserInfo>(req, 3)
            .unwrap();
        assert_eq!(result.age, 30);
        assert_eq!(result.name, "JOHN DOE");
    }
}
