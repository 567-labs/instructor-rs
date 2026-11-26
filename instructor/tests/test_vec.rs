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
        common::{GPT4, GPT4_O},
    };
    use serde::{Deserialize, Serialize};

    use super::*;

    #[test]
    fn test_simple_vec() {
        if support::run_loopback_in_child("tests::test_simple_vec") {
            return;
        }
        let (client, _server) = support::client(
            "UserIds",
            &[json!({"user_ids": ["12", "13", "14", "15", "24"]})],
        );
        let instructor_client = from_openai(client);

        #[derive(InstructMacro, Debug, Serialize, Deserialize)]
        struct UserIds {
            #[description("This is a list of user ids that we extracted from the message")]
            user_ids: Vec<String>,
        }

        let req = ChatCompletionRequest::new(
            GPT4.to_string(),
            vec![chat_completion::ChatCompletionMessage {
                role: chat_completion::MessageRole::user,
                content: chat_completion::Content::Text(String::from(
                    "User IDs are 12, 13, 14, 15,24",
                )),
                name: None,
            }],
        );

        let result = instructor_client
            .chat_completion::<UserIds>(req, 3)
            .unwrap();

        assert_eq!(result.user_ids, vec!["12", "13", "14", "15", "24"]);
    }

    #[test]
    fn test_complex_vec() {
        if support::run_loopback_in_child("tests::test_complex_vec") {
            return;
        }
        let (client, _server) = support::client(
            "Users",
            &[
                json!({"users": [{"name": "Jason", "age": "20"}, {"name": "Sarah", "age": "30"}, {"name": "John", "age": "40"}]}),
            ],
        );
        let instructor_client = from_openai(client);

        #[derive(InstructMacro, Debug, Serialize, Deserialize, PartialEq)]
        #[description("This is a user that we extracted from the text")]
        struct User {
            name: String,
            age: String,
        }

        #[derive(InstructMacro, Debug, Serialize, Deserialize)]
        #[description("Users that are present in the sentence provided")]
        struct Users {
            users: Vec<User>,
        }

        let req = ChatCompletionRequest::new(
            GPT4_O.to_string(),
            vec![chat_completion::ChatCompletionMessage {
                role: chat_completion::MessageRole::user,
                content: chat_completion::Content::Text(String::from(
                    "Jason is 20, Sarah is 30, and John is 40",
                )),
                name: None,
            }],
        );

        let result = instructor_client.chat_completion::<Users>(req, 3).unwrap();

        assert!(result.users.contains(&User {
            name: "Jason".to_string(),
            age: "20".to_string()
        }));
        assert!(result.users.contains(&User {
            name: "Sarah".to_string(),
            age: "30".to_string()
        }));
        assert!(result.users.contains(&User {
            name: "John".to_string(),
            age: "40".to_string()
        }));
    }
}
