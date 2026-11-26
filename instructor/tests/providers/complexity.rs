//! Execute the live suite's generation assertions on known wire responses.
//! Enum fixtures also verify the exact wire labels in the submitted schema.
use super::generation::{Generation, User};
use super::*;

fn routes() -> impl Iterator<Item = (Provider, Mode)> {
    PROVIDERS
        .into_iter()
        .map(|p| (p, p.default_mode()))
        .chain([(Provider::OpenAI, Mode::ResponsesTools)])
}

fn params(body: &Value, provider: Provider, mode: Mode) -> &Value {
    match (provider, mode) {
        (_, Mode::ResponsesTools) => &body["tools"][0]["parameters"],
        (Provider::Google, Mode::JsonSchema) => &body["generationConfig"]["responseJsonSchema"],
        (Provider::Google, Mode::Tools) => {
            &body["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"]
        }
        (Provider::Anthropic, _) => &body["tools"][0]["input_schema"],
        _ => &body["tools"][0]["function"]["parameters"],
    }
}

fn manifest() -> Value {
    json!({
        "depot":"North",
        "packages":[
            {"tracking_id":"A1","state":"packed","carrier":null},
            {"tracking_id":"B2","state":"in_transit","carrier":"Swift"},
        ]
    })
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
            json!({"value":42}),
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
    ($case:ident, $name:expr, $body:expr) => {
        #[tokio::test]
        async fn $case() {
            let mut server = Server::new_async().await;
            for (p, mode) in routes() {
                let m = reply(&mut server, p, mode, $name, $body)
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

structured!(test_int, &integer_name(), json!({"content":42}));
structured!(
    test_simple_extraction,
    "User",
    json!({"name":"Jason","age":25})
);
structured!(
    test_enum_extraction,
    "ShipmentState",
    json!({"content":"in_transit"})
);
structured!(test_nested_enum_collection, "Manifest", manifest());
structured!(
    test_list_extraction,
    &collection_name(),
    json!({"tasks":[
        {"name":"Alice","age":30},{"name":"Bob","age":25},{"name":"Charlie","age":35}
    ]})
);
structured!(
    test_nested_model_extraction,
    "UserWithAddress",
    json!({"name":"John Doe","age":28,
        "address":{"street":"123 Main St","city":"Springfield","country":"USA"}
    })
);

#[tokio::test]
async fn test_validation_with_retries() {
    let mut server = Server::new_async().await;
    for (p, mode) in routes() {
        let valid = json!({"name":" Sarah Johnson ","age":25});
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
        let mut unknown = manifest();
        unknown["packages"][1]["state"] = json!("lost");
        let bad = reply(&mut server, p, mode, "Manifest", unknown)
            .create_async()
            .await;
        let good = reply(&mut server, p, mode, "Manifest", manifest())
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
