//! Native Responses contracts exercised through the default client configuration.
use super::*;
use std::sync::mpsc;

const ROUTES: [(Provider, &str); 3] = [
    (Provider::OpenAI, "gpt-6-luna"),
    (Provider::OpenRouter, "openai/gpt-6.2-sol"),
    (Provider::Mistral, "mistral-large-4"),
];

fn route(server: &Server, provider: Provider, model: &str) -> ProviderConfig {
    ProviderConfig::new(provider, model, "fixture-key")
        .with_base_url(format!("{}/v1", server.url()))
}

fn endpoint(server: &mut Server, accept: &str) -> Mock {
    server
        .mock("POST", "/v1/responses")
        .match_header("authorization", "Bearer fixture-key")
        .match_header("content-type", "application/json")
        .match_header("accept", accept)
}

// Capture the body on receipt so assertions report the exact mismatched field.
async fn reply(server: &mut Server, output: Value) -> mpsc::Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    endpoint(server, "application/json")
        .with_body_from_request(move |req| {
            tx.send(serde_json::from_slice(req.body().unwrap()).unwrap())
                .unwrap();
            serde_json::to_vec(&json!({"object":"response","status":"completed","output": output}))
                .unwrap()
        })
        .create_async()
        .await;
    rx
}

#[model]
#[derive(Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Priority {
    Routine,
    NeedsReview,
}

#[model]
#[derive(Debug, PartialEq)]
struct Ticket {
    priority: Priority,
    owner: Option<String>,
}

const TICKET: &str = r#"{"priority":"needs_review","owner":null}"#;

fn answer() -> Value {
    json!({"type":"message","role":"assistant","status":"completed","id":"msg_1",
        "content":[{"type":"output_text","text":TICKET,"annotations":[]}]})
}

#[tokio::test]
async fn default_schema_decodes_a_typed_enum_and_translates_generation_options() {
    for (provider, model) in ROUTES {
        let mut server = Server::new_async().await;
        let client = Client::new(route(&server, provider, model)).unwrap();
        for strict in [true, false] {
            let sent = reply(&mut server, json!([answer()])).await;
            let mut req = Request::new("Flag this ticket for review")
                .option("max_completion_tokens", 1024)
                .option("reasoning_effort", "high");
            if !strict {
                req = req.strict(false);
            }
            let ticket = client.create::<Ticket>(req).await.unwrap();
            assert_eq!(ticket.priority, Priority::NeedsReview);
            let body = sent.try_recv().unwrap();
            assert_eq!(body["model"], model);
            assert_eq!(body["store"], false);
            assert_eq!(
                body["input"],
                json!([{"role":"user","content":"Flag this ticket for review"}])
            );
            assert_eq!(body["max_output_tokens"], 1024);
            assert_eq!(body["reasoning"], json!({"effort":"high"}));
            for key in [
                "tools",
                "messages",
                "response_format",
                "strict",
                "reasoning_effort",
                "max_completion_tokens",
            ] {
                assert!(body.get(key).is_none(), "{key}");
            }
            let format = &body["text"]["format"];
            assert_eq!(format["type"], "json_schema");
            assert_eq!(format["name"], "Ticket");
            assert_eq!(format["strict"], strict);
            let required = &format["schema"]["required"];
            assert_eq!(
                required.as_array().unwrap().contains(&json!("owner")),
                strict
            );
        }
    }
}

#[tokio::test]
async fn raw_state_options_follow_the_endpoint_storage_contract() {
    for (provider, model) in ROUTES {
        let mut server = Server::new_async().await;
        let client = Client::new(route(&server, provider, model)).unwrap();
        let options = [
            ("store", json!(true)),
            ("previous_response_id", json!("resp_old")),
            ("conversation", json!("conv_1")),
        ];
        for (key, value) in options {
            let req = Request::new("Continue").option(key, value.clone());
            if provider == Provider::OpenAI {
                let sent = reply(&mut server, json!([answer()])).await;
                client.create_raw(req).await.unwrap();
                let body = sent.try_recv().unwrap();
                assert_eq!(body[key], value);
                assert!(body.get("text").is_none());
            } else {
                assert!(
                    matches!(client.create_raw(req).await, Err(Error::Configuration(_))),
                    "{provider}/{key}"
                );
            }
        }
    }
}

#[derive(instructor_ai::ParallelTools, Debug, PartialEq)]
enum Action {
    Flag(Ticket),
    Assign(Person),
}

fn call(id: &str, name: &str, args: &str) -> Value {
    json!({"type":"function_call","id":format!("fc_{id}"),"call_id":format!("call_{id}"),"name":name,"arguments":args})
}

#[tokio::test]
async fn parallel_reask_replays_reasoning_and_resolves_each_call_in_order() {
    for (provider, model) in ROUTES {
        for mode in [Mode::JsonSchema, Mode::Tools] {
            let mut server = Server::new_async().await;
            let client = Client::new(route(&server, provider, model).with_mode(mode)).unwrap();
            let mut reasoning = json!({"type":"reasoning","id":"rs_1","summary":[]});
            if provider == Provider::Mistral {
                reasoning["content"] =
                    json!([{"type":"reasoning_text","text":"A review is needed."}]);
            } else {
                reasoning["encrypted_content"] = "opaque".into();
            }
            let assign = call("a", "Person", r#"{"name":"Renée","age":41}"#);
            let bad = call("b", "Ticket", r#"{"priority":"other"}"#);
            let first = reply(&mut server, json!([reasoning, assign, bad])).await;
            let retry = reply(&mut server, json!([assign, call("c", "Ticket", TICKET)])).await;
            let actions = client
                .create_parallel::<Action>(Request::new("Assign and flag").max_retries(1))
                .await
                .unwrap();
            assert!(
                matches!(actions.as_slice(), [Action::Assign(p),Action::Flag(t)] if p.name == "Renée" && t.priority == Priority::NeedsReview)
            );
            let body = first.try_recv().unwrap();
            assert_eq!(body["parallel_tool_calls"], true);
            assert_eq!(body["tool_choice"], "required");
            assert!(body["text"].get("format").is_none());
            let tools = body["tools"].as_array().unwrap();
            assert_eq!(tools.len(), 2);
            for (function, name) in tools.iter().zip(["Ticket", "Person"]) {
                assert_eq!(function["type"], "function");
                assert_eq!(function["name"], name);
                assert_eq!(function["parameters"]["type"], "object");
                assert_eq!(function["strict"], mode != Mode::Tools);
            }
            let followup = retry.try_recv().unwrap();
            assert_eq!(followup["tools"], body["tools"]);
            let input = followup["input"].as_array().unwrap();
            assert_eq!(input.len(), 6);
            assert_eq!(input[0], body["input"][0]);
            assert_eq!(&input[1..4], [reasoning, assign, bad]);
            for (result, id) in input[4..].iter().zip(["call_a", "call_b"]) {
                assert_eq!(result["type"], "function_call_output");
                assert_eq!(result["call_id"], id);
                assert!(result["output"].as_str().unwrap().contains("other"));
            }
        }
    }
}

#[tokio::test]
async fn typed_text_stream_requires_a_successful_lifecycle_event() {
    for (provider, model) in ROUTES {
        let mut server = Server::new_async().await;
        let client = Client::new(route(&server, provider, model)).unwrap();
        let prefix = [
            json!({"type":"response.reasoning_text.delta","output_index":0,"delta":"reason"}),
            json!({"type":"response.output_text.delta","output_index":1,"delta":r#"{"priority":"needs_review","#}),
            json!({"type":"response.output_text.delta","output_index":1,"delta":r#""owner":null}"#}),
        ].iter().map(frame).collect::<String>();
        for completed in [true, false] {
            let last = if completed {
                json!({"type":"response.completed","response":{"status":"completed","output":[answer()]}})
            } else {
                json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}})
            };
            let stream = format!("{prefix}{}", frame(&last));
            let wire = endpoint(&mut server, "text/event-stream").match_body(Matcher::PartialJson(
                json!({"stream":true,"text":{"format":{"type":"json_schema"}}}),
            ));
            let server_reply = fragmented(wire, stream).create_async().await;
            let mut snapshots = client
                .create_partial::<Ticket>(Request::new("Flag").max_retries(0))
                .await
                .unwrap();
            assert!(
                matches!(snapshots.next().await.unwrap().unwrap(), PartialEvent::Partial(ticket) if matches!(ticket.priority, Some(PriorityPartial::NeedsReview)))
            );
            let rest: Result<Vec<_>, _> = snapshots.try_collect().await;
            if completed {
                assert!(
                    matches!(rest.unwrap().last(), Some(PartialEvent::Complete(ticket)) if ticket.priority == Priority::NeedsReview)
                );
            } else {
                assert!(
                    matches!(rest, Err(Error::Incomplete(reason)) if reason.contains("max_output_tokens"))
                );
            }
            server_reply.assert_async().await;
        }
    }
}
