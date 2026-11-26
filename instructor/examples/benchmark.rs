//! Rust worker for benchmarks/compare.py. Reads the same fixtures as the Python worker.
use std::{hint::black_box, io, time::Instant};

use futures_util::{stream, TryStreamExt};
use instructor_ai::{
    decisions::{Choices, DecisionModel, DecisionsConfig, Noul, PreparedDecision, Score},
    model, parse_response,
    streaming::{self, PartialEvent},
    BlockingClient, InstructMacro, Mode, PartialModel, Provider, ProviderConfig, Request,
};
use serde::Serialize;
use serde_json::{json, Value};

#[model]
struct Record {
    id: i64,
    title: String,
}

#[model]
struct Batch {
    tasks: Vec<Record>,
}

#[derive(instructor_ai::ParallelTools, Serialize)]
#[serde(untagged)]
enum Output {
    One(Record),
    Many(Batch),
}

#[model]
#[validate(context = "check_length")]
struct LimitedBatch {
    tasks: Vec<Record>,
}

fn check_length(items: &mut LimitedBatch, context: &Value) -> Result<(), String> {
    if items.tasks.len() as u64 > context["max_items"].as_u64().unwrap() {
        return Err("too many items".into());
    }
    Ok(())
}

/// Which topic?
#[derive(Choices, Serialize)]
#[serde(rename_all = "lowercase")]
enum Topic {
    Billing,
    Other,
}

#[derive(DecisionModel, Serialize)]
struct Ticket {
    topic: Topic,
    #[decision(Noul::new("Refund for {{ team }}?"))]
    refund: f64,
    #[decision(Score::new("Urgent?", ["low", "normal", "high"]).scale(0.0, 10.0))]
    urgency: f64,
}

fn string<'a>(object: &'a Value, name: &str) -> &'a str {
    object[name].as_str().expect("required string in fixture")
}

fn measure<T>(target_ns: u128, call: impl Fn() -> T, proof: impl FnOnce(T) -> Value) -> Value {
    let witness = proof(call());
    let batch = |iterations: u64| {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(call());
        }
        start.elapsed().as_nanos()
    };
    let mut iterations = 1;
    while batch(iterations) < target_ns {
        iterations *= 2;
    }
    let elapsed_ns = batch(iterations);
    json!({"witness": witness, "elapsed_ns": elapsed_ns, "iterations": iterations})
}

fn value<T: Serialize>(out: T) -> Value {
    serde_json::to_value(out).unwrap()
}

fn snapshot<T: Serialize + PartialModel>(out: PartialEvent<T>) -> Value
where
    T::Partial: Serialize,
{
    match out {
        PartialEvent::Partial(v) => value(v),
        PartialEvent::Complete(v) => value(v),
    }
}

fn snapshots<T: Serialize + PartialModel>(out: Vec<PartialEvent<T>>) -> Value
where
    T::Partial: Serialize,
{
    assert!(matches!(out.last(), Some(PartialEvent::Complete(_))));
    Value::Array(out.into_iter().map(snapshot).collect())
}

fn http_config(provider: &str, endpoint: &str) -> ProviderConfig {
    let (provider, model) = match provider {
        "retry" => (Provider::OpenAI, "retry"),
        other => (other.parse().unwrap(), "fixture"),
    };
    let (suffix, mode) = match provider {
        Provider::Cohere => ("/v2", Mode::JsonSchema),
        Provider::Google => ("/v1beta", Mode::JsonSchema),
        _ => ("/v1", Mode::Tools),
    };
    ProviderConfig::new(provider, model, "local")
        .with_base_url(format!("{endpoint}{suffix}"))
        .with_mode(mode)
}

fn run(case: &Value, endpoint: &str, ns: u128) -> Value {
    let op = string(case, "op");
    match op {
        "schema" => measure(ns, Batch::json_schema, value),
        "validate" | "validate_error" => {
            let text = string(case, "text");
            let context = &case["context"];
            let parse = || parse_response::<LimitedBatch>(black_box(text), context);
            if op == "validate_error" {
                measure(ns, || parse().is_err(), value)
            } else {
                measure(ns, || parse().unwrap(), value)
            }
        }
        "parse" | "invalid" => {
            let text = string(case, "text");
            let parse = || parse_response::<Batch>(black_box(text), &Value::Null);
            if op == "invalid" {
                measure(ns, || parse().is_err(), value)
            } else {
                measure(ns, || parse().unwrap(), value)
            }
        }
        "partial" | "iterable" | "partial_first" | "iterable_first" | "partial_async"
        | "iterable_async" => {
            let chunks: Vec<_> = case["chunks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap())
                .collect();
            let fragments = || chunks.iter().map(Ok);
            let partial = || streaming::partial::<Batch>(fragments()).map(Result::unwrap);
            let iterable = || streaming::iterable::<Record>(fragments()).map(Result::unwrap);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            let source = || stream::iter(fragments());
            match op {
                "partial_async" => {
                    let fetch = || {
                        runtime
                            .block_on(streaming::partial_async::<Batch>(source()).try_collect())
                            .unwrap()
                    };
                    measure(ns, fetch, snapshots)
                }
                "iterable_async" => {
                    let fetch = || {
                        runtime
                            .block_on(
                                streaming::iterable_async::<Record>(source())
                                    .try_collect::<Vec<_>>(),
                            )
                            .unwrap()
                    };
                    measure(ns, fetch, value)
                }
                "partial" => measure(ns, || partial().collect(), snapshots),
                "iterable" => measure(ns, || iterable().collect::<Vec<_>>(), value),
                "partial_first" => measure(ns, || partial().next().unwrap(), snapshot),
                "iterable_first" => measure(ns, || iterable().next().unwrap(), value),
                _ => unreachable!(),
            }
        }
        "batch_prepare" | "batch_parse" => {
            let provider = string(case, "provider");
            let client = BlockingClient::new(http_config(provider, endpoint)).unwrap();
            let batch = client.batch::<Record>().unwrap();
            if op == "batch_prepare" {
                let prompts: Vec<_> = case["prompts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p.as_str().unwrap())
                    .collect();
                measure(
                    ns,
                    || batch.prepare(prompts.iter().copied()).unwrap(),
                    value,
                )
            } else {
                let text = string(case, "text");
                measure(
                    ns,
                    || batch.parse(text),
                    |rows| {
                        Value::Array(rows.into_iter().map(|row| json!({
                        "custom_id":row.custom_id, "result":row.result.unwrap(), "success":true
                    })).collect())
                    },
                )
            }
        }
        "http" | "http_stream" | "http_parallel" | "cache" => {
            let provider = string(case, "provider");
            let client = BlockingClient::new(http_config(provider, endpoint)).unwrap();
            let client = if op == "cache" {
                client.with_cache(10)
            } else {
                client
            };
            let request = || {
                let req = Request::new("Extract ticket").max_retries(1);
                if provider == "anthropic" {
                    req.option("max_tokens", 1024)
                } else {
                    req
                }
            };
            if op == "http_parallel" {
                measure(
                    ns,
                    || client.create_parallel::<Output>(request()).unwrap(),
                    value,
                )
            } else if op != "http_stream" {
                measure(ns, || client.create::<Record>(request()).unwrap(), value)
            } else {
                let stream = || {
                    client
                        .create_partial::<Record>(request())
                        .unwrap()
                        .map(Result::unwrap)
                        .collect()
                };
                measure(ns, stream, snapshots)
            }
        }
        "questions" | "answers" | "reject" | "http_decision" => {
            let provider = string(case, "provider");
            let route = format!("{provider}/{}", string(case, "model"));
            let input = || case["context"].clone();
            let prepare = || PreparedDecision::<Ticket>::new(&route, input()).unwrap();
            match op {
                "http_decision" => {
                    let client = DecisionsConfig::new(&route)
                        .unwrap()
                        .api_key("local")
                        .endpoint(format!("{endpoint}/{provider}"))
                        .build_blocking()
                        .unwrap();
                    measure(ns, || client.create::<Ticket>(input()).unwrap(), value)
                }
                "questions" => measure(ns, prepare, |p| p.body().clone()),
                _ => {
                    let prepared = prepare();
                    let raw = &case["answer"];
                    let decode = || prepared.parse(black_box(raw));
                    if op == "answers" {
                        measure(ns, || decode().unwrap(), value)
                    } else {
                        measure(ns, || decode().is_err(), value)
                    }
                }
            }
        }
        _ => panic!("Unknown benchmark {op}"),
    }
}

fn main() {
    let request: Value = serde_json::from_reader(io::stdin().lock()).unwrap();
    let target = request["target_ns"].as_u64().unwrap() as u128;
    let endpoint = string(&request, "endpoint");
    let mut results = json!({});
    for case in request["cases"].as_array().unwrap() {
        results[string(case, "name")] = run(case, endpoint, target);
    }
    println!("{results}");
}
