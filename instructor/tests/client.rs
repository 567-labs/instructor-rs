//! Cross-provider client semantics pinned to Python Instructor 1.18's core contracts.
#[path = "client/cache_backend.rs"]
mod cache_backend;

use std::sync::{Arc, Mutex};

use instructor_ai::{
    model, parse_response, response_schema, BlockingClient, Client, Error, Event, Media, Message,
    Mode, Provider, ProviderConfig, Request, StopReason,
};
use mockito::{Matcher, Server};
use serde_json::{json, Value};

#[model]
#[derive(Debug, PartialEq)]
struct Amount {
    #[validate(context = "minimum")]
    value: u32,
}

fn minimum(value: &mut u32, context: &Value) -> Result<(), String> {
    if u64::from(*value) < context["min"].as_u64().unwrap_or(0) {
        return Err("below minimum".into());
    }
    Ok(())
}

fn config(server: &Server) -> ProviderConfig {
    ProviderConfig::new(Provider::OpenAI, "test-model", "test-key")
        .with_mode(Mode::JsonSchema)
        .with_base_url(server.url())
}

fn response(text: &str, tokens: Option<u64>) -> Value {
    json!({
        "id": "raw-id",
        "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":text}}],
        "usage":{"total_tokens":tokens}
    })
}

#[test]
fn reask_accumulates_usage_and_preserves_request_and_raw_response() {
    let mut server = Server::new();
    let first = server
        .mock("POST", "/chat/completions")
        .match_body(Matcher::PartialJson(
            json!({"messages":[{"role":"user","content":"minimum 4"}],"temperature":0}),
        ))
        .with_body(response(r#"{"value":3}"#, Some(10)).to_string())
        .create();
    let raw = response(r#"{"value":8}"#, Some(5));
    let second = server
        .mock("POST", "/chat/completions")
        .match_body(Matcher::Regex("below minimum".into()))
        .with_body(raw.to_string())
        .create();
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = events.clone();
    let client = BlockingClient::new(config(&server))
        .unwrap()
        .on_event(move |n, event| {
            let name = match event {
                Event::Request(_) => "request".to_string(),
                Event::Response(_) => "response".into(),
                Event::Usage { total_tokens } => format!("tokens {total_tokens}"),
                Event::ParseError(_) => "parse".into(),
                other => panic!("unexpected {other:?}"),
            };
            log.lock().unwrap().push(format!("{n} {name}"));
        });
    let req = Request::new("minimum {{ min }}")
        .context(json!({"min":4}))
        .token_budget(11)
        .max_retries(1)
        .option("temperature", 0);
    let (parsed, completion): (Amount, _) = client.create_with_completion(req.clone()).unwrap();
    assert_eq!(parsed.value, 8);
    assert_eq!(completion, raw);
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].content, "minimum {{ min }}");
    assert_eq!(
        *events.lock().unwrap(),
        [
            "1 request",
            "1 response",
            "1 tokens 10",
            "1 parse",
            "2 request",
            "2 response",
            "2 tokens 15"
        ]
    );
    first.assert();
    second.assert();
}

#[test]
fn exact_attempt_counts_and_failed_history() {
    for retries in [0, 1, 3] {
        let mut server = Server::new();
        let expected = (retries + 1) as usize;
        let mock = server
            .mock("POST", "/chat/completions")
            .expect(expected)
            .with_body(response("broken", Some(3)).to_string())
            .create();
        let result = BlockingClient::new(config(&server))
            .unwrap()
            .create::<Amount>(Request::new("").max_retries(retries));
        let Err(Error::Retry {
            reason: StopReason::RetriesExhausted,
            total_tokens,
            attempts,
        }) = result
        else {
            panic!("{result:?}")
        };
        assert_eq!(attempts.len(), expected);
        assert_eq!(total_tokens, Some(3 * expected as u64));
        assert_eq!(attempts[0].completion["id"], "raw-id");
        assert!(matches!(attempts[0].error, Error::Parse(_)));
        mock.assert();
    }
}

#[test]
fn token_budget_uses_cumulative_usage_and_fails_closed() {
    for (last_tokens, reason) in [
        (Some(4), StopReason::TokenBudget { budget: 10 }),
        (None, StopReason::UsageUnavailable { budget: 10 }),
    ] {
        let mut server = Server::new();
        let first = server
            .mock("POST", "/chat/completions")
            .with_body(response("broken", Some(6)).to_string())
            .create();
        let second = server
            .mock("POST", "/chat/completions")
            .with_body(response("broken", last_tokens).to_string())
            .create();
        let result = BlockingClient::new(config(&server))
            .unwrap()
            .create::<Amount>(Request::new("").token_budget(10));
        let Err(Error::Retry {
            reason: actual,
            total_tokens,
            attempts,
        }) = result
        else {
            panic!("{result:?}")
        };
        assert_eq!(reason, actual);
        assert_eq!(attempts.len(), 2);
        assert_eq!(total_tokens, last_tokens.map(|t| t + 6));
        first.assert();
        second.assert();
    }
}

#[test]
fn invalid_budget_and_undefined_templates_are_rejected_before_io() {
    let server = Server::new();
    let client = BlockingClient::new(config(&server)).unwrap();
    let req = Request::new("x").token_budget(1);
    assert!(matches!(
        client.create_raw(req.clone()),
        Err(Error::Configuration(_))
    ));
    assert!(matches!(
        client.create_partial::<Amount>(req.clone()),
        Err(Error::Configuration(_))
    ));
    assert!(matches!(
        client.create::<Amount>(req.token_budget(0)),
        Err(Error::Configuration(_))
    ));
    let req = Request::new("{{ name }}").context(json!({}));
    assert!(matches!(
        client.create::<Amount>(req),
        Err(Error::Configuration(_))
    ));
}

#[tokio::test]
async fn async_validator_runs_inside_the_retry_loop() {
    let mut server = Server::new_async().await;
    let first = server
        .mock("POST", "/chat/completions")
        .with_body(response(r#"{"value":3}"#, None).to_string())
        .create_async()
        .await;
    let second = server
        .mock("POST", "/chat/completions")
        .match_body(Matcher::Regex("must be even".into()))
        .with_body(response(r#"{"value":8}"#, None).to_string())
        .create_async()
        .await;
    let client = Client::new(config(&server)).unwrap();
    let amount: Amount = client
        .create_validated("extract", |mut amount: Amount| async move {
            tokio::task::yield_now().await;
            if !amount.value.is_multiple_of(2) {
                return Err("must be even".into());
            }
            amount.value /= 2;
            Ok(amount)
        })
        .await
        .unwrap();
    assert_eq!(amount.value, 4);
    first.assert_async().await;
    second.assert_async().await;
}

#[tokio::test]
async fn concurrent_calls_keep_templates_validation_and_retry_history_local() {
    let mut server = Server::new_async().await;
    let low = server
        .mock("POST", "/chat/completions")
        .match_body(Matcher::Regex("min=1".into()))
        .with_body(response(r#"{"value":2}"#, Some(7)).to_string())
        .create_async()
        .await;
    let high = server
        .mock("POST", "/chat/completions")
        .match_body(Matcher::Regex("min=9".into()))
        .with_body(response(r#"{"value":2}"#, Some(1)).to_string())
        .create_async()
        .await;
    let retry = server
        .mock("POST", "/chat/completions")
        .match_body(Matcher::Regex("min=9.*below minimum".into()))
        .with_body(response(r#"{"value":10}"#, Some(2)).to_string())
        .create_async()
        .await;
    let req = Request::new("min={{ min }}").token_budget(2);
    let client = Client::new(config(&server)).unwrap().with_cache(4);
    let (a, b) = tokio::join!(
        client.create::<Amount>(req.clone().context(json!({"min":1}))),
        client.create::<Amount>(req.context(json!({"min":9})))
    );
    assert_eq!(a.unwrap().value, 2);
    assert_eq!(b.unwrap().value, 10);
    low.assert_async().await;
    high.assert_async().await;
    retry.assert_async().await;
}

#[test]
fn cached_raw_completion_is_revalidated_and_bounded() {
    let mut server = Server::new();
    let raw = response(r#"{"value":7}"#, Some(2));
    let mock = server
        .mock("POST", "/chat/completions")
        .expect(5)
        .with_body(raw.to_string())
        .create();
    let client = BlockingClient::new(config(&server)).unwrap().with_cache(1);
    client.create::<Amount>("same").unwrap();
    let (model, body) = client.create_with_completion::<Amount>("same").unwrap();
    assert_eq!(body, raw);
    assert_eq!(model.value, 7);
    // An external validator can change. A rejected hit forces fresh I/O.
    assert!(client
        .create_validated::<Amount>(Request::new("same").max_retries(0), |_| Err("no".into()))
        .is_err());
    client.create::<Amount>("same").unwrap();
    client
        .create::<Amount>(Request::new("same").context(json!({"min":5})))
        .unwrap();
    client.create::<Amount>("same").unwrap();
    mock.assert();
}

#[test]
fn schemas_are_isolated_and_wrappers_restore_native_rust_values() {
    let mut value = response_schema::<Vec<Amount>>();
    value["properties"]["tasks"]["items"] = Value::Null;
    assert_eq!(
        response_schema::<Vec<Amount>>()["properties"]["tasks"]["items"]["type"],
        "object"
    );
    let output: Vec<Amount> = parse_response(r#"{"tasks":[{"value":4}]}"#, &Value::Null).unwrap();
    assert_eq!(output, [Amount { value: 4 }]);
    let output: Option<Amount> = parse_response(r#"{"content":null}"#, &Value::Null).unwrap();
    assert_eq!(output, None);
    assert!(parse_response::<Option<Amount>>("{}", &Value::Null).is_err());
    assert!(
        parse_response::<Vec<Amount>>(r#"{"tasks":[{"value":1}]}"#, &json!({"min":2})).is_err()
    );
    assert_eq!(
        parse_response::<u64>(r#"{"content":42}"#, &Value::Null).unwrap(),
        42
    );
}

#[test]
fn raw_requests_carry_native_attachment_blocks() {
    let image = Media::bytes("image/png", b"abc");
    let audio = Media::bytes("audio/mpeg", b"abc");
    let pdf = Media::url("application/pdf", "https://example.org/item.pdf");
    for (provider, mode, file, path, expected) in [
        (
            Provider::OpenAI,
            Mode::Tools,
            image.clone(),
            "/chat/completions",
            json!({"type":"image_url","image_url":{"url":"data:image/png;base64,YWJj"}}),
        ),
        (
            Provider::OpenRouter,
            Mode::Tools,
            audio,
            "/chat/completions",
            json!({"type":"input_audio","input_audio":{"format":"mp3","data":"YWJj"}}),
        ),
        (
            Provider::Anthropic,
            Mode::Tools,
            pdf.clone(),
            "/messages",
            json!({"type":"document","source":{"type":"url","url":"https://example.org/item.pdf"}}),
        ),
        (
            Provider::Anthropic,
            Mode::Json,
            image.clone(),
            "/messages",
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"YWJj"}}),
        ),
        (
            Provider::OpenAI,
            Mode::ResponsesTools,
            pdf,
            "/responses",
            json!({"type":"input_file","file_url":"https://example.org/item.pdf"}),
        ),
        (
            Provider::Google,
            Mode::Json,
            image,
            "/models/test:generateContent",
            json!({"inlineData":{"mimeType":"image/png","data":"YWJj"}}),
        ),
    ] {
        let body = match (provider, mode) {
            (Provider::Google, _) => json!({"contents":[{"parts":[{"text":"read"},expected]}]}),
            (_, Mode::ResponsesTools) => {
                json!({"input":[{"content":[{"type":"input_text","text":"read"},expected]}]})
            }
            _ => json!({"messages":[{"content":[{"type":"text","text":"read"},expected]}]}),
        };
        let mut server = Server::new();
        let mock = server
            .mock("POST", path)
            .match_body(Matcher::PartialJson(body))
            .with_body(r#"{"custom":"field"}"#)
            .create();
        let config = ProviderConfig::new(provider, "test", "mock")
            .with_mode(mode)
            .with_base_url(server.url());
        let client = BlockingClient::new(config).unwrap();
        let raw = client
            .create_raw(vec![Message::user("read").attach(file)])
            .unwrap();
        assert_eq!(raw["custom"], "field");
        mock.assert();
    }
}

#[test]
fn malformed_or_unsupported_media_fails_before_io() {
    let server = Server::new();
    let client = BlockingClient::new(config(&server)).unwrap();
    for file in [
        Media::url("image/png", "file:///tmp/local"),
        Media::bytes("image/png", []),
        Media::bytes("text/plain", b"unsupported"),
        Media::url("audio/wav", "https://example.org/file.wav"),
    ] {
        assert!(matches!(
            client.create_raw(vec![Message::user("x").attach(file)]),
            Err(Error::Configuration(_))
        ));
    }
}

fn sse(text: &str) -> String {
    let event = json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":"stop"}]});
    format!("data: {event}\n\ndata: [DONE]\n\n")
}

#[test]
fn streams_retry_only_before_exposing_an_item() {
    let mut server = Server::new();
    let first = server
        .mock("POST", "/chat/completions")
        .with_body(sse(r#"{"tasks":[{"value":1}]}"#))
        .create();
    let retry = server
        .mock("POST", "/chat/completions")
        .match_body(Matcher::Regex("below minimum".into()))
        .with_body(sse(r#"{"tasks":[{"value":8},{"value":1}]}"#))
        .create();
    let events = Arc::new(Mutex::new(Vec::new()));
    let record = |scope| {
        let events = events.clone();
        move |attempt, event: Event<'_>| {
            if matches!(event, Event::LastAttempt(Error::Validation(_))) {
                events.lock().unwrap().push((scope, attempt));
            }
        }
    };
    let req = Request::new("read")
        .context(json!({"min":5}))
        .on_event(record("request"));
    let mut iter = BlockingClient::new(config(&server))
        .unwrap()
        .on_event(record("client"))
        .create_iterable::<Amount>(req)
        .unwrap();
    assert_eq!(iter.next().unwrap().unwrap().value, 8);
    assert!(events.lock().unwrap().is_empty());
    assert!(matches!(iter.next(), Some(Err(Error::Validation(_)))));
    assert!(iter.next().is_none());
    assert_eq!(*events.lock().unwrap(), [("client", 2), ("request", 2)]);
    first.assert();
    retry.assert();
}

#[test]
fn disk_cache_persists_only_with_matching_identity_and_namespace() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Server::new();
    let response = response(r#"{"value":9}"#, None);
    let mock = server
        .mock("POST", "/chat/completions")
        .expect(4)
        .with_body(response.to_string())
        .create();
    let client = |key, namespace: &str| {
        BlockingClient::new(
            ProviderConfig::new(Provider::OpenAI, "test", key)
                .with_mode(Mode::JsonSchema)
                .with_base_url(server.url()),
        )
        .unwrap()
        .with_disk_cache(dir.path(), namespace)
        .unwrap()
    };
    client("a", "domain").create::<Amount>("query").unwrap();
    let (model, raw) = client("a", "domain")
        .create_with_completion::<Amount>("query")
        .unwrap();
    assert_eq!(model.value, 9);
    assert_eq!(raw, response);
    client("a", "other").create::<Amount>("query").unwrap();
    client("b", "domain").create::<Amount>("query").unwrap();
    client("a", "domain").clear_cache().unwrap();
    client("a", "domain").create::<Amount>("query").unwrap();
    client("b", "domain").create::<Amount>("query").unwrap();
    mock.assert();
}

#[test]
fn disk_cache_recovers_from_corrupt_records_and_expiration() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/chat/completions")
        .expect(4)
        .with_body(response(r#"{"value":1}"#, None).to_string())
        .create();
    let errors = Arc::new(Mutex::new(0));
    let log = errors.clone();
    let client = BlockingClient::new(config(&server))
        .unwrap()
        .with_disk_cache(dir.path(), "domain")
        .unwrap()
        .on_event(move |_, e| {
            if matches!(e, Event::CacheError(_)) {
                *log.lock().unwrap() += 1;
            }
        });
    let req = Request::new("query");
    client
        .create::<Amount>(req.clone().cache_ttl(std::time::Duration::from_nanos(1)))
        .unwrap();
    client.create::<Amount>(req.clone()).unwrap();
    let path = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let record = std::fs::read_dir(path)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&record, "not JSON").unwrap();
    client.create::<Amount>(req.clone()).unwrap();
    client.create::<Amount>(req.clone()).unwrap();
    client
        .create::<Amount>(req.cache_ttl(std::time::Duration::ZERO))
        .unwrap();
    assert_eq!(*errors.lock().unwrap(), 1);
    mock.assert();
}

#[test]
fn elapsed_limit_and_policy_predicate_stop_before_reasking() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/chat/completions")
        .expect(2)
        .with_body(response("invalid", None).to_string())
        .create();
    let client = BlockingClient::new(config(&server)).unwrap();
    let req = Request::new("query");
    let stop = req.clone().retry_policy(|failed| {
        assert_eq!(failed.attempt, 1);
        assert_eq!(failed.completion["id"], "raw-id");
        None
    });
    let timeout = req
        .retry_timeout(std::time::Duration::from_secs(1))
        .retry_policy(|_| Some(std::time::Duration::from_secs(2)));
    assert!(matches!(
        client.create::<Amount>(stop),
        Err(Error::Retry {
            reason: StopReason::Policy,
            ..
        })
    ));
    assert!(matches!(
        client.create::<Amount>(timeout),
        Err(Error::Retry {
            reason: StopReason::Timeout,
            ..
        })
    ));
    mock.assert();
}

#[test]
fn custom_policy_can_retry_transient_http_errors_without_changing_the_prompt() {
    let mut server = Server::new();
    let first = server
        .mock("POST", "/chat/completions")
        .with_status(429)
        .create();
    let second = server
        .mock("POST", "/chat/completions")
        .match_body(Matcher::PartialJson(
            json!({"messages":[{"role":"user","content":"query"}]}),
        ))
        .with_body(response(r#"{"value":4}"#, Some(3)).to_string())
        .create();
    let client = BlockingClient::new(config(&server)).unwrap();
    let result = client
        .create::<Amount>(Request::new("query").retry_policy(|failed| {
            matches!(failed.error, Error::Http { status: 429, .. })
                .then_some(std::time::Duration::from_millis(1))
        }))
        .unwrap();
    assert_eq!(result.value, 4);
    first.assert();
    second.assert();
}

#[test]
fn terminal_api_failure_after_reask_preserves_history_and_emits_last_attempt() {
    let mut server = Server::new();
    let first = server
        .mock("POST", "/chat/completions")
        .with_body(response("invalid", Some(9)).to_string())
        .create();
    let second = server
        .mock("POST", "/chat/completions")
        .with_status(403)
        .create();
    let last = Arc::new(Mutex::new(None));
    let log = last.clone();
    let req = Request::new("query").on_event(move |n, e| {
        if matches!(e, Event::LastAttempt(_)) {
            *log.lock().unwrap() = Some(n)
        }
    });
    let result = BlockingClient::new(config(&server))
        .unwrap()
        .create::<Amount>(req);
    let Err(Error::Retry {
        reason: StopReason::Terminal,
        attempts,
        ..
    }) = result
    else {
        panic!("{result:?}")
    };
    assert_eq!(attempts[0].completion["usage"]["total_tokens"], 9);
    assert!(matches!(attempts[1].error, Error::Http { status: 403, .. }));
    assert_eq!(*last.lock().unwrap(), Some(2));
    first.assert();
    second.assert();
}

#[test]
fn metadata_separates_cumulative_usage_from_provider_envelope_and_cached_usage() {
    let mut server = Server::new();
    let invalid = server
        .mock("POST", "/chat/completions")
        .with_body(response("invalid", Some(9)).to_string())
        .create();
    let valid = server
        .mock("POST", "/chat/completions")
        .with_body(response(r#"{"value":9}"#, Some(4)).to_string())
        .create();
    let client = BlockingClient::new(config(&server)).unwrap().with_cache(1);
    let result = client.create_with_metadata::<Amount>("query").unwrap();
    assert_eq!(result.total_tokens, Some(13));
    assert_eq!(result.attempts, 2);
    assert_eq!(result.raw["usage"]["total_tokens"], 4);
    let cached = client.create_with_metadata::<Amount>("query").unwrap();
    assert_eq!(cached.total_tokens, Some(0));
    assert_eq!(cached.attempts, 0);
    assert_eq!(cached.into_pair(), result.into_pair());
    invalid.assert();
    valid.assert();
}

#[tokio::test]
async fn removing_client_hooks_preserves_concurrent_clones_and_request_hooks() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/chat/completions")
        .match_header("x-tenant", "my-app")
        .expect(2)
        .with_body(response(r#"{"value":5}"#, Some(2)).to_string())
        .create_async()
        .await;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-tenant", "my-app".parse().unwrap());
    let http = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    let mut client = Client::with_http_client(config(&server), http).unwrap();
    let global = Arc::new(AtomicUsize::new(0));
    let local = Arc::new(AtomicUsize::new(0));
    let g = global.clone();
    let handle = client.add_hook(move |_, _| {
        g.fetch_add(1, Ordering::SeqCst);
    });
    let mut other = client.clone();
    assert!(client.remove_hook(&handle));
    assert!(!client.remove_hook(&handle));
    let l = local.clone();
    let req = Request::new("q").on_event(move |_, _| {
        l.fetch_add(1, Ordering::SeqCst);
    });
    let (one, two) = tokio::join!(
        client.create::<Amount>(req),
        other.create::<Amount>("other")
    );
    assert_eq!(one.unwrap(), two.unwrap());
    other.clear_hooks();
    assert!(!other.remove_hook(&handle));
    assert_eq!(global.load(Ordering::SeqCst), 3);
    assert_eq!(local.load(Ordering::SeqCst), 3);
    mock.assert_async().await;
}
