//! Provider contracts for cumulative token accounting, including the shapes
//! from Python's tests/test_usage_accounting_contracts.py.
use super::*;
use instructor_ai::{StopReason, TokenUsage};
use std::sync::{Arc, Mutex};

fn native_usage(p: Provider, mode: Mode, n: u64) -> Value {
    match p {
        Provider::Anthropic => json!({
            "input_tokens": 60*n, "output_tokens": 50*n,
            "cache_creation_input_tokens": 30*n, "cache_read_input_tokens": 10*n,
            "cache_creation": {"ephemeral_5m_input_tokens": 20*n, "ephemeral_1h_input_tokens": 10*n},
            "output_tokens_details": {"thinking_tokens": 15*n},
            "server_tool_use": {"web_search_requests": 12}
        }),
        Provider::OpenAI if mode == Mode::ResponsesTools => json!({
            "input_tokens": 100*n, "output_tokens": 50*n, "total_tokens": 150*n,
            "input_tokens_details": {"cached_tokens": 10*n, "cache_write_tokens": 30*n},
            "output_tokens_details": {"reasoning_tokens": 15*n}
        }),
        Provider::Google => json!({
            "promptTokenCount": 100*n, "candidatesTokenCount": 35*n,
            "thoughtsTokenCount": 15*n, "cachedContentTokenCount": 10*n, "totalTokenCount": 150*n
        }),
        Provider::Cohere => json!({
            "tokens": {"input_tokens": 100*n, "output_tokens": 50*n},
            "billed_units": {"input_tokens": 50*n, "output_tokens": 50*n}
        }),
        Provider::Mistral => {
            json!({"prompt_tokens": 100*n, "completion_tokens": 50*n, "total_tokens": 150*n})
        }
        _ => json!({
            "prompt_tokens": 100*n, "completion_tokens": 50*n, "total_tokens": 150*n,
            "prompt_tokens_details": {"cached_tokens": 10*n, "cache_write_tokens": 30*n},
            "completion_tokens_details": {"reasoning_tokens": 15*n, "audio_tokens": 8}
        }),
    }
}

fn check(p: Provider, counts: TokenUsage, n: u64) {
    let has_reads = !matches!(p, Provider::Cohere | Provider::Mistral);
    let has_writes = matches!(
        p,
        Provider::Anthropic | Provider::OpenAI | Provider::OpenRouter
    );
    assert_eq!(counts.input_tokens, Some(100 * n));
    assert_eq!(counts.output_tokens, Some(50 * n));
    assert_eq!(counts.total_tokens, Some(150 * n));
    assert_eq!(counts.cache_read_tokens, has_reads.then_some(10 * n));
    assert_eq!(counts.cache_write_tokens, has_writes.then_some(30 * n));
    assert_eq!(counts.reasoning_tokens, has_reads.then_some(15 * n));
    assert_eq!(
        counts.cache_write_5m_tokens,
        (p == Provider::Anthropic).then_some(20 * n)
    );
    assert_eq!(
        counts.cache_write_1h_tokens,
        (p == Provider::Anthropic).then_some(10 * n)
    );
}

#[tokio::test]
async fn seven_routes_accumulate_reasks_and_keep_original_raw_usage() {
    let mut server = Server::new_async().await;
    for (p, mode) in PROVIDERS
        .map(|p| (p, p.default_mode()))
        .into_iter()
        .chain([(Provider::OpenAI, Mode::ResponsesTools)])
    {
        let key = if p == Provider::Google {
            "usageMetadata"
        } else {
            "usage"
        };
        let mut invalid = completion(p, mode, json!({}));
        invalid[key] = native_usage(p, mode, 1);
        let first = mock(&mut server, p, mode, false)
            .with_body(invalid.to_string())
            .create_async()
            .await;
        let mut valid = completion(p, mode, person());
        valid[key] = native_usage(p, mode, 2);
        let second = mock(&mut server, p, mode, false)
            .with_body(valid.to_string())
            .create_async()
            .await;
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let log = snapshots.clone();
        let req = request()
            .max_retries(1)
            .token_budget(151)
            .on_usage(move |n, u| log.lock().unwrap().push((n, u)));
        let client = Client::new(config(&server, p, mode)).unwrap();
        let (result, counts) = client.create_with_usage::<Person>(req).await.unwrap();
        check(p, counts, 3);
        assert_eq!(result.total_tokens, counts.total_tokens);
        assert_eq!(result.raw, valid);
        assert_eq!(result.attempts, 2);
        first.assert_async().await;
        second.assert_async().await;
        let events = snapshots.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, 1);
        check(p, events[0].1, 1);
        assert_eq!(events[1], (2, counts));
    }
}

const P: Provider = Provider::OpenAI;
const MODE: Mode = Mode::Tools;

fn response(server: &mut Server, payload: Value, usage: Value) -> Mock {
    let mut raw = completion(P, MODE, payload);
    raw["usage"] = usage;
    mock(server, P, MODE, false)
        .with_body(raw.to_string())
        .create()
}

#[test]
fn incomplete_counters_propagate_independently_in_either_order() {
    let partial = json!({"prompt_tokens": 5, "total_tokens": 17,
                         "prompt_tokens_details":{"cached_tokens": 0}});
    let full = native_usage(P, MODE, 1);
    for (first, last) in [(&partial, &full), (&full, &partial)] {
        let mut server = Server::new();
        let _first = response(&mut server, json!({}), first.clone());
        let _last = response(&mut server, person(), last.clone());
        let client = BlockingClient::new(config(&server, P, MODE)).unwrap();
        let (_, counts) = client.create_with_usage::<Person>("Extract").unwrap();
        assert_eq!(counts.input_tokens, Some(105));
        assert_eq!(counts.total_tokens, Some(167));
        assert_eq!(counts.cache_read_tokens, Some(10));
        assert_eq!(counts.cache_write_tokens, None);
        assert_eq!(counts.output_tokens, None);
        assert_eq!(counts.reasoning_tokens, None);
    }
}

#[test]
fn wholly_missing_usage_and_failed_http_attempt_preclude_cumulative_counts() {
    for http in [true, false] {
        let mut server = Server::new();
        let first = if http {
            mock(&mut server, P, MODE, false).with_status(503).create()
        } else {
            response(&mut server, json!({}), Value::Null)
        };
        let last = response(&mut server, person(), native_usage(P, MODE, 1));
        let req = Request::new("Extract").retry_policy(|_| Some(std::time::Duration::ZERO));
        let client = BlockingClient::new(config(&server, P, MODE)).unwrap();
        let (result, counts) = client.create_with_usage::<Person>(req).unwrap();
        assert_eq!(counts, TokenUsage::default());
        check(P, TokenUsage::from_response(P, &result.raw), 1);
        first.assert();
        last.assert();
    }
}

#[test]
fn budget_stop_preserves_detail_and_history_on_failure() {
    let mut server = Server::new();
    let first = response(&mut server, json!({}), native_usage(P, MODE, 1));
    let second = response(&mut server, json!({}), native_usage(P, MODE, 2));
    let client = BlockingClient::new(config(&server, P, MODE)).unwrap();
    let error = client
        .create::<Person>(Request::new("Extract").token_budget(450))
        .unwrap_err();
    check(P, error.retry_usage(P).unwrap(), 3);
    assert!(matches!(
        error,
        Error::Retry {
            reason: StopReason::TokenBudget { budget: 450 },
            total_tokens: Some(450),
            ..
        }
    ));
    first.assert();
    second.assert();
}

#[test]
fn budget_requires_both_components_to_derive_a_total() {
    let mut server = Server::new();
    let first = response(&mut server, json!({}), json!({"prompt_tokens": 2}));
    let client = BlockingClient::new(config(&server, P, MODE)).unwrap();
    let error = client
        .create::<Person>(Request::new("Extract").token_budget(50))
        .unwrap_err();
    let counts = error.retry_usage(P).unwrap();
    assert_eq!(counts.input_tokens, Some(2));
    assert_eq!(counts.total_tokens, None);
    assert!(matches!(
        error,
        Error::Retry {
            reason: StopReason::UsageUnavailable { budget: 50 },
            ..
        }
    ));
    first.assert();
}

#[test]
fn cache_hits_report_zero_current_usage_and_keep_original_usage() {
    let mut server = Server::new();
    let hit = response(&mut server, person(), native_usage(P, MODE, 1));
    let log = Arc::new(Mutex::new(Vec::new()));
    let copy = log.clone();
    let mut client = BlockingClient::new(config(&server, P, MODE))
        .unwrap()
        .with_cache(5)
        .on_usage(move |n, u| copy.lock().unwrap().push((n, u)));
    let handle = client.add_usage_hook(|_, _| panic!("removed hook ran"));
    assert!(client.remove_hook(&handle));
    let (_, fresh) = client.create_with_usage::<Person>("Extract").unwrap();
    let (result, cached) = client.create_with_usage::<Person>("Extract").unwrap();
    check(P, fresh, 1);
    assert_eq!(cached, TokenUsage::ZERO);
    assert_eq!(TokenUsage::from_response(P, &result.raw), fresh);
    assert_eq!(*log.lock().unwrap(), [(1, fresh), (0, cached)]);
    hit.assert();
}

#[test]
fn native_total_wins_and_zero_differs_from_invalid_counts() {
    let raw = json!({"usage":{"total_tokens": 9, "prompt_tokens": 0, "completion_tokens": 0,
        "prompt_tokens_details": {"cached_tokens": true, "cache_write_tokens": -5},
        "completion_tokens_details":{"reasoning_tokens": "3"}}});
    let counts = TokenUsage::from_response(P, &raw);
    assert_eq!(counts.total_tokens, Some(9));
    assert_eq!(counts.input_tokens, Some(0));
    assert_eq!(counts.output_tokens, Some(0));
    assert_eq!(counts.cache_read_tokens, None);
    assert_eq!(counts.cache_write_tokens, None);
    assert_eq!(counts.reasoning_tokens, None);
    let raw = json!({"usage":{"input_tokens": u64::MAX, "output_tokens": 1}});
    assert_eq!(
        TokenUsage::from_response(P, &raw).total_tokens,
        Some(u64::MAX)
    );
}

#[test]
fn anthropic_can_reconstruct_cache_creation_from_both_lifetimes() {
    let mut usage = native_usage(Provider::Anthropic, MODE, 1);
    usage
        .as_object_mut()
        .unwrap()
        .remove("cache_creation_input_tokens");
    check(
        Provider::Anthropic,
        TokenUsage::from_response(Provider::Anthropic, &json!({"usage": usage})),
        1,
    );
    let raw = json!({"usage":{"input_tokens": 5, "output_tokens": 2}});
    let counts = TokenUsage::from_response(Provider::Anthropic, &raw);
    assert_eq!(counts.input_tokens, Some(5));
    assert_eq!(counts.total_tokens, Some(7));
    assert_eq!(counts.cache_write_tokens, None);
}
