//! Execute the live suite's generation assertions on known wire responses.
//! Enum fixtures also verify the exact wire labels in the submitted schema.
use super::generation::{Generation, User};
use super::*;

fn routes() -> impl Iterator<Item = (Provider, Mode)> {
    PROVIDERS
        .into_iter()
        .map(|p| {
            (
                p,
                if p == Provider::Google {
                    Mode::JsonSchema
                } else {
                    Mode::Tools
                },
            )
        })
        .chain([(Provider::OpenAI, Mode::ResponsesTools)])
}

fn params(body: &Value, provider: Provider, mode: Mode) -> &Value {
    match (provider, mode) {
        (_, Mode::ResponsesTools) => &body["tools"][0]["parameters"],
        (Provider::Google, Mode::JsonSchema) => &body["response_format"]["schema"],
        (Provider::Google, Mode::Tools) => &body["tools"][0]["parameters"],
        (Provider::Anthropic, _) => &body["tools"][0]["input_schema"],
        _ => &body["tools"][0]["function"]["parameters"],
    }
}

fn answer(case: &str) -> Value {
    let fixtures: Value = serde_json::from_str(include_str!("../support/generation.json")).unwrap();
    fixtures[case]
        .get("answer")
        .expect("missing generation fixture")
        .clone()
}

fn collection_name() -> String {
    response_schema::<Vec<User>>()["title"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn integer_name() -> String {
    response_schema::<i32>()["title"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn reply(server: &mut Server, p: Provider, mode: Mode, name: &str, args: Value) -> Mock {
    let envelope = named_completion(p, mode, name, args);
    let name = name.to_owned();
    mock(server, p, mode, false)
        .match_request(move |req| {
            let Ok(body) = serde_json::from_slice::<Value>(req.body().unwrap()) else {
                return false;
            };
            let schema = params(&body, p, mode);
            let props = &schema["properties"];
            let labels = json!(["packed", "in_transit", "delivered"]);
            match name.as_str() {
                "ShipmentState" => props["content"]["enum"] == labels,
                "Manifest" => {
                    let packages = &props["packages"];
                    let fields = &packages["items"]["properties"];
                    packages["type"] == "array"
                        && fields["state"]["enum"] == labels
                        && fields["tracking_id"]["type"] == "string"
                        && fields["carrier"].is_object()
                }
                "UserWithAddress" => props["address"]["properties"]["street"]["type"] == "string",
                "User" | "RetriedUser" => props["age"]["type"] == "integer",
                "ExtractedNumber" => props["value"]["type"] == "integer",
                _ if name == integer_name() => props["content"]["type"] == "integer",
                _ if name == collection_name() => props["tasks"]["items"]["type"] == "object",
                _ => false,
            }
        })
        .with_body(envelope.to_string())
}

#[tokio::test]
async fn test_current_documented_model() {
    let mut server = Server::new_async().await;
    for p in PROVIDERS {
        let m = reply(
            &mut server,
            p,
            Mode::Tools,
            "ExtractedNumber",
            answer("test_current_documented_model"),
        )
        .create_async()
        .await;
        Generation::new(config(&server, p, Mode::Tools))
            .unwrap()
            .test_current_documented_model()
            .await
            .unwrap();
        m.assert_async().await;
    }
}

macro_rules! structured {
    ($case:ident, $name:expr) => {
        #[tokio::test]
        async fn $case() {
            let mut server = Server::new_async().await;
            for (p, mode) in routes() {
                let m = reply(&mut server, p, mode, $name, answer(stringify!($case)))
                    .create_async()
                    .await;
                let suite = Generation::new(config(&server, p, mode)).unwrap();
                let result = suite.$case().await;
                m.assert_async().await;
                result.unwrap_or_else(|err| panic!("{p} {mode:?}: {err}"));
            }
        }
    };
}

structured!(test_int, &integer_name());
structured!(test_simple_extraction, "User");
structured!(test_enum_extraction, "ShipmentState");
structured!(test_nested_enum_collection, "Manifest");
structured!(test_list_extraction, &collection_name());
structured!(test_nested_model_extraction, "UserWithAddress");

#[tokio::test]
async fn test_validation_with_retries() {
    let mut server = Server::new_async().await;
    for (p, mode) in routes() {
        let valid = answer("test_validation_with_retries");
        let m = reply(&mut server, p, mode, "RetriedUser", valid)
            .expect(2)
            .create_async()
            .await;
        Generation::new(config(&server, p, mode))
            .unwrap()
            .test_validation_with_retries()
            .await
            .unwrap();
        m.assert_async().await;
    }
}

#[tokio::test]
async fn test_partial_streaming() {
    let mut server = Server::new_async().await;
    for (p, mode) in routes() {
        let body = events(
            p,
            mode,
            "User",
            &[r#"{"name":"Jason"#, r#" Liu","age":"#, "30}"],
        );
        let m = fragmented(mock(&mut server, p, mode, true), body)
            .create_async()
            .await;
        Generation::new(config(&server, p, mode))
            .unwrap()
            .test_partial_streaming()
            .await
            .unwrap();
        m.assert_async().await;
    }
}

#[tokio::test]
async fn test_iterable_streaming() {
    let mut server = Server::new_async().await;
    for (p, mode) in routes() {
        // Python's Mistral tools handler expects a complete tasks list
        // per argument delta. Also test this framing through Rust.
        let chunks = if p == Provider::Mistral {
            [
                r#"{"tasks":[{"name":"Alice","age":25}]}"#,
                r#"{"tasks":[{"name":"Bob","age":30}]}"#,
                r#"{"tasks":[{"name":"Carol","age":35}]}"#,
            ]
        } else {
            [
                r#"{"tasks":[{"name":"Alice","age":25}"#,
                r#",{"name":"Bob","age":30}"#,
                r#",{"name":"Carol","age":35}]}"#,
            ]
        };
        let body = events(p, mode, &collection_name(), &chunks);
        let m = fragmented(mock(&mut server, p, mode, true), body)
            .create_async()
            .await;
        Generation::new(config(&server, p, mode))
            .unwrap()
            .test_iterable_streaming()
            .await
            .unwrap();
        m.assert_async().await;
    }
}

#[tokio::test]
async fn invalid_enum_in_nested_record_causes_a_reask() {
    let mut server = Server::new_async().await;
    for (p, mode) in routes() {
        let mut unknown = answer("test_nested_enum_collection");
        unknown["packages"][1]["state"] = json!("lost");
        let bad = reply(&mut server, p, mode, "Manifest", unknown)
            .create_async()
            .await;
        let good = reply(
            &mut server,
            p,
            mode,
            "Manifest",
            answer("test_nested_enum_collection"),
        )
        .match_body(Matcher::Regex("unknown variant.*lost".into()))
        .create_async()
        .await;
        let suite = Generation::new(config(&server, p, mode)).unwrap();
        let result = suite.test_nested_enum_collection().await;
        bad.assert_async().await;
        good.assert_async().await;
        result.unwrap();
    }
}
