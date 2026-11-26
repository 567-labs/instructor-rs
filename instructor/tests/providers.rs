//! End-to-end contract tests against deterministic loopback endpoints.
#[path = "providers/complexity.rs"]
mod complexity;
#[path = "support/generation.rs"]
mod generation;
#[path = "providers/parallel.rs"]
mod parallel;
#[path = "providers/usage.rs"]
mod usage;
use futures_util::{StreamExt, TryStreamExt};
use instructor_ai::{
    model, response_schema, BlockingClient, Client, Error, Message, Mode, PartialEvent, Provider,
    ProviderConfig, Request,
};
use mockito::{Matcher, Mock, Server};
use serde_json::{json, Value};

const PROVIDERS: [Provider; 6] = [
    Provider::OpenAI,
    Provider::Anthropic,
    Provider::OpenRouter,
    Provider::Google,
    Provider::Cohere,
    Provider::Mistral,
];

#[model]
#[derive(Debug)]
struct Person {
    name: String,
    age: u32,
}

fn model_id(p: Provider) -> &'static str {
    if p == Provider::OpenRouter {
        "anthropic/claude-sonnet-4.5:exacto"
    } else {
        "model"
    }
}

fn config(server: &Server, p: Provider, mode: Mode) -> ProviderConfig {
    ProviderConfig::new(p, model_id(p), "fake-key")
        .with_base_url(format!("{}/base/", server.url()))
        .with_mode(mode)
}

fn request() -> Request {
    Request::messages(vec![
        Message::system("Extract"),
        Message::assistant("Ready"),
        Message::user("Renée is 41"),
    ])
    .max_retries(0)
}

fn tool(mode: Mode) -> bool {
    matches!(mode, Mode::Tools | Mode::ToolsStrict)
}

fn mock(server: &mut Server, p: Provider, mode: Mode, stream: bool) -> Mock {
    let path = match p {
        Provider::Google if stream => "/base/models/model:streamGenerateContent?alt=sse",
        Provider::Google => "/base/models/model:generateContent",
        Provider::Anthropic => "/base/messages",
        Provider::Cohere => "/base/chat",
        Provider::OpenAI if mode == Mode::ResponsesTools => "/base/responses",
        _ => "/base/chat/completions",
    };
    let mock = server
        .mock("POST", path)
        .match_header("content-type", "application/json")
        .match_header(
            "accept",
            if stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        );
    match p {
        Provider::Google => mock
            .match_header("x-goog-api-key", "fake-key")
            .match_header("authorization", Matcher::Missing),
        Provider::Anthropic => mock
            .match_header("x-api-key", "fake-key")
            .match_header("anthropic-version", "2023-06-01")
            .match_header("authorization", Matcher::Missing),
        _ => mock.match_header("authorization", "Bearer fake-key"),
    }
}

fn structured_request(p: Provider, mode: Mode, stream: bool, name: &str) -> Value {
    let shape = if name == "Person" {
        json!({"type":"object","properties":{"age":{"type":"integer"},"name":{"type":"string"}}})
    } else {
        json!({"type":"object","properties":{"tasks":{"type":"array"}}})
    };
    let mut body = json!({"model":model_id(p),"stream":stream,"messages":[
        {"role":"system","content":"Extract"},{"role":"assistant","content":"Ready"},{"role":"user","content":"Renée is 41"}
    ]});
    match p {
        Provider::Google => {
            body = json!({"contents":[{"role":"model","parts":[{"text":"Ready"}]},{"role":"user","parts":[{"text":"Renée is 41"}]}],
                "systemInstruction":{"parts":[{"text":"Extract"}]}});
            if tool(mode) {
                body["tools"] =
                    json!([{"functionDeclarations":[{"name":name,"parametersJsonSchema":shape}]}]);
                body["toolConfig"] =
                    json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":[name]}});
            } else {
                body["generationConfig"] =
                    json!({"responseMimeType":"application/json","responseJsonSchema":shape});
            }
        }
        Provider::Anthropic => {
            body["messages"].as_array_mut().unwrap().remove(0);
            body["system"] = json!("Extract");
            body["max_tokens"] = json!(1024);
            if tool(mode) {
                body["tools"] = json!([{"name":name,"input_schema":shape}]);
                body["tool_choice"] = json!({"type":"tool","name":name});
            } else {
                body["output_config"] = json!({"format":{"type":"json_schema","schema":shape}});
            }
        }
        _ if tool(mode) => {
            body["tools"] =
                json!([{"type":"function","function":{"name":name,"parameters":shape}}]);
            body["tool_choice"] = match p {
                Provider::Mistral => json!("any"),
                Provider::Cohere => json!("REQUIRED"),
                _ => json!({"type":"function","function":{"name":name}}),
            };
        }
        Provider::Cohere => body["response_format"] = json!({"type":"json_object","schema":shape}),
        _ => {
            body["response_format"] = json!({"type":"json_schema","json_schema":{"name":name,"schema":shape,"strict":true}})
        }
    }
    body
}

fn completion(p: Provider, mode: Mode, args: Value) -> Value {
    named_completion(p, mode, "Person", args)
}

fn named_completion(p: Provider, mode: Mode, name: &str, args: Value) -> Value {
    let call = json!({"id":"call_1","type":"function","function":{"name":name,"arguments":args.to_string()}});
    match p {
        Provider::OpenAI if mode == Mode::ResponsesTools => json!({"status":"completed","output":[
            {"type":"function_call","name":name,"call_id":"call_1","arguments":args.to_string()}
        ]}),
        Provider::Anthropic if tool(mode) => json!({"stop_reason":"tool_use","content":[
            {"type":"thinking","thinking":"…","signature":"opaque"},{"type":"text","text":"Calling"},
            {"type":"tool_use","id":"tool_1","name":name,"input":args}
        ]}),
        Provider::Anthropic => json!({"stop_reason":"end_turn","content":[
            {"type":"thinking","thinking":"…","signature":"opaque"},{"type":"text","text":args.to_string()}]}),
        Provider::Google => {
            let part = if tool(mode) {
                json!({"functionCall":{"name":name,"args":args},"thoughtSignature":"opaque"})
            } else {
                json!({"text":args.to_string()})
            };
            json!({"candidates":[{"content":{"role":"model","parts":[{"thought":true,"text":"thinking"},part]},"finishReason":"STOP"}]})
        }
        Provider::Cohere if tool(mode) => {
            json!({"finish_reason":"TOOL_CALL","message":{"tool_plan":"…","tool_calls":[call]}})
        }
        Provider::Cohere => {
            json!({"finish_reason":"COMPLETE","message":{"content":[{"type":"text","text":args.to_string()}]}})
        }
        _ if tool(mode) => {
            json!({"choices":[{"index":0,"message":{"role":"assistant","tool_calls":[call]},"finish_reason":"tool_calls"}]})
        }
        _ => {
            json!({"choices":[{"index":0,"message":{"role":"assistant","content":args.to_string()},"finish_reason":"stop"}]})
        }
    }
}

fn person() -> Value {
    json!({"name":"Renée","age":41})
}

#[tokio::test]
async fn all_providers_send_native_requests_and_return_original_envelopes() {
    let mut server = Server::new_async().await;
    for provider in PROVIDERS {
        for mode in [Mode::Tools, Mode::JsonSchema] {
            let envelope = completion(provider, mode, person());
            let endpoint = mock(&mut server, provider, mode, false)
                .match_body(Matcher::PartialJson(structured_request(
                    provider, mode, false, "Person",
                )))
                .with_body(envelope.to_string())
                .create_async()
                .await;
            let client = Client::new(config(&server, provider, mode)).unwrap();
            let result = client.create_with_completion::<Person>(request()).await;
            endpoint.assert_async().await;
            let (p, raw) = result.unwrap();
            assert_eq!(p.name, "Renée");
            assert_eq!(p.age, 41);
            assert_eq!(raw, envelope);
        }
    }
}

fn frame(raw: &Value) -> String {
    let event = raw["type"]
        .as_str()
        .map(|k| format!("event: {k}\r\n"))
        .unwrap_or_default();
    format!("{event}data: {raw}\r\n\r\n")
}

fn events(p: Provider, mode: Mode, name: &str, fragments: &[&str]) -> String {
    let mut stream = String::from(": keepalive\r\n\r\n");
    let mut send = |v: Value| stream.push_str(&frame(&v));
    match p {
        Provider::OpenAI if mode == Mode::ResponsesTools => {
            send(
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","name":name}}),
            );
            for fragment in fragments {
                send(
                    json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":fragment}),
                );
            }
            let final_item =
                json!({"type":"function_call","name":name,"arguments":fragments.concat()});
            send(
                json!({"type":"response.completed","response":{"status":"completed","output":[final_item]}}),
            );
        }
        Provider::Google => {
            send(json!({"candidates":[{"content":{"parts":[{"thought":true,"text":"…"}]}}]}));
            if tool(mode) {
                let args: Value = serde_json::from_str(&fragments.concat()).unwrap();
                send(
                    json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":name,"args":args}}]}}]}),
                );
            } else {
                for s in fragments {
                    send(json!({"candidates":[{"content":{"parts":[{"text":s}]}}]}));
                }
            }
            send(
                json!({"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"totalTokenCount":10}}),
            );
        }
        Provider::Anthropic => {
            send(json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
            send(
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            );
            send(
                json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"…"}}),
            );
            send(json!({"type":"content_block_stop","index":0}));
            let block = if tool(mode) {
                json!({"type":"tool_use","id":"tool_1","name":name,"input":{}})
            } else {
                json!({"type":"text","text":""})
            };
            send(json!({"type":"content_block_start","index":1,"content_block":block}));
            send(json!({"type":"ping"}));
            for s in fragments {
                let delta = if tool(mode) {
                    json!({"type":"input_json_delta","partial_json":s})
                } else {
                    json!({"type":"text_delta","text":s})
                };
                send(json!({"type":"content_block_delta","index":1,"delta":delta}));
            }
            send(json!({"type":"content_block_stop","index":1}));
            send(
                json!({"type":"message_delta","delta":{"stop_reason":if tool(mode){"tool_use"}else{"end_turn"}}}),
            );
            send(json!({"type":"message_stop"}));
        }
        Provider::Cohere => {
            send(
                json!({"type":"message-start","id":"id","delta":{"message":{"role":"assistant"}}}),
            );
            if tool(mode) {
                send(
                    json!({"type":"tool-call-start","index":0,"delta":{"message":{"tool_calls":{"id":"call_1","function":{"name":name,"arguments":""}}}}}),
                );
                for s in fragments {
                    send(
                        json!({"type":"tool-call-delta","index":0,"delta":{"message":{"tool_calls":{"function":{"arguments":s}}}}}),
                    );
                }
                send(json!({"type":"tool-call-end","index":0}));
            } else {
                for s in fragments {
                    send(
                        json!({"type":"content-delta","index":0,"delta":{"message":{"content":{"text":s}}}}),
                    );
                }
            }
            send(
                json!({"type":"message-end","delta":{"finish_reason":if tool(mode){"TOOL_CALL"}else{"COMPLETE"}}}),
            );
        }
        _ => {
            send(
                json!({"choices":[{"index":0,"delta":{"role":"assistant","reasoning":"thinking"}}]}),
            );
            if tool(mode) {
                send(
                    json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":name,"arguments":""}}]}}]}),
                );
            }
            for s in fragments {
                let delta = if tool(mode) {
                    json!({"tool_calls":[{"index":0,"function":{"arguments":s}}]})
                } else {
                    json!({"content":s})
                };
                send(json!({"choices":[{"index":0,"delta":delta}]}));
            }
            send(
                json!({"choices":[{"delta":{},"finish_reason":if tool(mode){"tool_calls"}else{"stop"}}]}),
            );
            send(json!({"choices":[],"usage":{"total_tokens":12}}));
            stream.push_str("data: [DONE]\n\n");
        }
    }
    stream
}

fn fragmented(mock: Mock, stream: String) -> Mock {
    mock.with_header("content-type", "text/event-stream; charset=utf-8")
        .with_chunked_body(move |writer| {
            for chunk in stream.as_bytes().chunks(13) {
                writer.write_all(chunk)?;
            }
            Ok(())
        })
}

#[tokio::test]
async fn fragmented_native_streams_yield_typed_partials_then_complete() {
    let mut server = Server::new_async().await;
    for p in PROVIDERS {
        for mode in [Mode::Tools, Mode::JsonSchema] {
            let raw = events(
                p,
                mode,
                "Person",
                &[r#"{"name":"Ren"#, r#"ée","age":"#, "41}"],
            );
            let endpoint = fragmented(
                mock(&mut server, p, mode, true).match_body(Matcher::PartialJson(
                    structured_request(p, mode, true, "Person"),
                )),
                raw,
            )
            .create_async()
            .await;
            let client = Client::new(config(&server, p, mode)).unwrap();
            let stream = client.create_partial::<Person>(request()).await;
            endpoint.assert_async().await;
            let items: Vec<_> = stream.unwrap().try_collect().await.unwrap();
            if p != Provider::Google || !tool(mode) {
                assert!(items.iter().any(|item|matches!(item,PartialEvent::Partial(part) if part.name.as_deref()==Some("Renée") && part.age.is_none())), "{p} {mode:?}");
            }
            assert!(
                matches!(items.last(),Some(PartialEvent::Complete(model)) if model.name=="Renée" && model.age==41)
            );
            assert_eq!(
                items
                    .iter()
                    .filter(|i| matches!(i, PartialEvent::Complete(_)))
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn all_default_provider_streams_yield_iterable_items_in_order() {
    let mut server = Server::new_async().await;
    let schema = response_schema::<Vec<Person>>();
    let name = schema["title"].as_str().unwrap();
    for p in PROVIDERS {
        let mode = p.default_mode();
        let stream = events(
            p,
            mode,
            name,
            &[
                r#"{"tasks":[{"name":"Renée","age":41}"#,
                r#",{"name":"Li","age":2}"#,
                "]}",
            ],
        );
        let endpoint = fragmented(
            mock(&mut server, p, mode, true).match_body(Matcher::PartialJson(structured_request(
                p, mode, true, name,
            ))),
            stream,
        )
        .create_async()
        .await;
        let client = Client::new(config(&server, p, mode)).unwrap();
        let stream = client.create_iterable::<Person>(request()).await;
        endpoint.assert_async().await;
        let people: Vec<Person> = stream.unwrap().try_collect().await.unwrap();
        assert_eq!(
            people
                .iter()
                .map(|p| (p.name.as_str(), p.age))
                .collect::<Vec<_>>(),
            vec![("Renée", 41), ("Li", 2)]
        );
    }
}

#[test]
fn blocking_facade_consumes_streams_for_primary_providers() {
    let mut server = Server::new();
    for p in [Provider::OpenAI, Provider::Anthropic, Provider::OpenRouter] {
        let raw = events(
            p,
            Mode::Tools,
            "Person",
            &[r#"{"name":"Ren"#, r#"ée","age":41}"#],
        );
        let endpoint = fragmented(mock(&mut server, p, Mode::Tools, true), raw).create();
        let client = BlockingClient::new(config(&server, p, Mode::Tools)).unwrap();
        let output: Result<Vec<_>, _> = client
            .create_partial::<Person>("Renée is 41")
            .unwrap()
            .collect();
        assert!(
            matches!(output.unwrap().last(),Some(PartialEvent::Complete(model)) if model.name=="Renée")
        );
        endpoint.assert();
    }
}

#[tokio::test]
async fn native_tool_retries_replay_correlated_messages_and_preserve_signatures() {
    let mut server = Server::new_async().await;
    for p in PROVIDERS {
        let args = json!({"name":"Renée","age":"invalid"});
        let mut invalid = completion(p, Mode::Tools, args);
        if p == Provider::OpenRouter {
            invalid["choices"][0]["message"]["reasoning_details"] =
                json!([{"type":"reasoning.encrypted","data":"opaque"}]);
        }
        let request_body = match p {
            Provider::Google => json!({"contents":[
                {"role":"user"},invalid["candidates"][0]["content"],
                {"role":"user","parts":[{"functionResponse":{"name":"Person","response":{}}}]}]}),
            Provider::Anthropic => json!({"messages":[
                {"role":"user"},{"role":"assistant","content":invalid["content"]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"tool_1","is_error":true}]}]}),
            _ => {
                let mut msg = if p == Provider::Cohere {
                    invalid["message"].clone()
                } else {
                    invalid["choices"][0]["message"].clone()
                };
                msg["role"] = json!("assistant");
                json!({"messages":[{"role":"user"},msg,{"role":"tool","tool_call_id":"call_1"}]})
            }
        };
        let first = mock(&mut server, p, Mode::Tools, false)
            .with_body(invalid.to_string())
            .create_async()
            .await;
        let retry = mock(&mut server, p, Mode::Tools, false)
            .match_body(Matcher::AllOf(vec![
                Matcher::PartialJson(request_body),
                Matcher::Regex("invalid type".into()),
            ]))
            .with_body(completion(p, Mode::Tools, person()).to_string())
            .create_async()
            .await;
        let client = Client::new(config(&server, p, Mode::Tools)).unwrap();
        let req = Request::new("Renée is 41").max_retries(1);
        let result = client.create::<Person>(req.clone()).await;
        first.assert_async().await;
        retry.assert_async().await;
        assert_eq!(result.unwrap().age, 41);
        assert_eq!(req.messages.len(), 1);
    }
}

#[tokio::test]
async fn all_provider_token_budgets_count_native_usage_for_failed_attempts() {
    let mut server = Server::new_async().await;
    for p in PROVIDERS {
        let mut invalid = completion(p, p.default_mode(), json!({}));
        match p {
            Provider::Google => invalid["usageMetadata"] = json!({"totalTokenCount":100}),
            Provider::Anthropic => {
                invalid["usage"] = json!({"input_tokens":60,"output_tokens":10,"cache_creation_input_tokens":25,"cache_read_input_tokens":5})
            }
            Provider::Cohere => {
                invalid["usage"] = json!({"tokens":{"input_tokens":60,"output_tokens":40}})
            }
            _ => invalid["usage"] = json!({"total_tokens":100}),
        }
        let endpoint = mock(&mut server, p, p.default_mode(), false)
            .with_body(invalid.to_string())
            .create_async()
            .await;
        let client = Client::new(config(&server, p, p.default_mode())).unwrap();
        let err = client
            .create::<Person>(Request::new("Renée").token_budget(100))
            .await
            .unwrap_err();
        assert!(
            matches!(err,Error::Retry{total_tokens:Some(100),reason:instructor_ai::StopReason::TokenBudget{budget:100},ref attempts} if attempts.len()==1)
        );
        endpoint.assert_async().await;
    }
}

#[tokio::test]
async fn all_providers_flag_truncation_even_for_parseable_json() {
    let mut server = Server::new_async().await;
    for p in PROVIDERS {
        let mut raw = completion(p, p.default_mode(), person());
        match p {
            Provider::Google => raw["candidates"][0]["finishReason"] = json!("MAX_TOKENS"),
            Provider::Anthropic => raw["stop_reason"] = json!("max_tokens"),
            Provider::Cohere => raw["finish_reason"] = json!("MAX_TOKENS"),
            _ => raw["choices"][0]["finish_reason"] = json!("length"),
        }
        let endpoint = mock(&mut server, p, p.default_mode(), false)
            .with_body(raw.to_string())
            .create_async()
            .await;
        let client = Client::new(config(&server, p, p.default_mode())).unwrap();
        assert!(matches!(
            client.create::<Person>("Renée").await,
            Err(Error::Incomplete(_))
        ));
        endpoint.assert_async().await;
    }
}

#[tokio::test]
async fn errors_after_provisional_output_are_terminal_and_skip_complete() {
    let mut server = Server::new_async().await;
    for p in PROVIDERS {
        let json = r#"{"name":"Renée","age":"#;
        let (content, stop) = match p {
            Provider::Google => (
                json!({"candidates":[{"content":{"parts":[{"text":json}]}}]}),
                json!({"candidates":[{"finishReason":"MAX_TOKENS"}]}),
            ),
            Provider::Cohere => (
                json!({"type":"content-delta","delta":{"message":{"content":{"text":json}}}}),
                json!({"type":"message-end","delta":{"finish_reason":"MAX_TOKENS"}}),
            ),
            Provider::Anthropic => (
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":json}}),
                json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"}}),
            ),
            _ => (
                json!({"choices":[{"delta":{"content":json}}]}),
                json!({"choices":[{"delta":{},"finish_reason":"length"}]}),
            ),
        };
        let endpoint = mock(&mut server, p, Mode::JsonSchema, true)
            .with_body(frame(&content) + &frame(&stop))
            .create_async()
            .await;
        let client = Client::new(config(&server, p, Mode::JsonSchema)).unwrap();
        let mut values = client.create_partial::<Person>("Renée").await.unwrap();
        assert!(matches!(
            values.next().await,
            Some(Ok(PartialEvent::Partial(_)))
        ));
        assert!(matches!(
            values.next().await,
            Some(Err(Error::Incomplete(_)))
        ));
        assert!(values.next().await.is_none());
        endpoint.assert_async().await;
    }
}

#[tokio::test]
async fn openai_and_anthropic_and_router_handle_http_errors_refusals_and_malformed_tools() {
    let mut server = Server::new_async().await;
    for p in [Provider::OpenAI, Provider::Anthropic, Provider::OpenRouter] {
        let mode = Mode::Tools;
        let client = Client::new(config(&server, p, mode)).unwrap();
        let rate_limited = mock(&mut server, p, mode, false)
            .with_status(429)
            .with_body("quota")
            .create_async()
            .await;
        let err = client.create::<Person>("prompt").await.unwrap_err();
        assert!(matches!(err, Error::Http{status:429,body} if body=="quota"));
        rate_limited.assert_async().await;
        rate_limited.remove_async().await;

        let mut raw = completion(p, mode, person());
        let mut invalid = raw.clone();
        if p == Provider::Anthropic {
            raw["stop_reason"] = json!("refusal");
            invalid["content"][2]["name"] = json!("Wrong");
        } else {
            raw["choices"][0]["message"]["refusal"] = json!("declined");
            invalid["choices"][0]["message"]["tool_calls"][0]["function"]["name"] = json!("Wrong");
        }
        let refusal = mock(&mut server, p, mode, false)
            .with_body(raw.to_string())
            .create_async()
            .await;
        let bad_name = mock(&mut server, p, mode, false)
            .with_body(invalid.to_string())
            .create_async()
            .await;
        let err = client.create::<Person>("prompt").await.unwrap_err();
        assert!(matches!(err, Error::Refusal(_)));
        let err = client.create::<Person>(request()).await.unwrap_err();
        assert!(
            matches!(err, Error::Retry{attempts,..} if attempts.len()==1 && attempts[0].error.to_string().contains("Wrong"))
        );
        refusal.assert_async().await;
        bad_name.assert_async().await;
    }
}

#[tokio::test]
async fn openrouter_preserves_routing_and_reports_errors_inside_successful_http() {
    let mut server = Server::new_async().await;
    let p = Provider::OpenRouter;
    let client = Client::new(config(&server, p, Mode::JsonSchema)).unwrap();
    let route = json!({"order":["Anthropic"],"require_parameters":true,"allow_fallbacks":false});
    let endpoint=mock(&mut server,p,Mode::JsonSchema,false).match_body(Matcher::PartialJson(json!({
        "model":model_id(p), "provider":route, "response_format":{"type":"json_schema","json_schema":{"strict":true}}
    }))).with_body(json!({"error":{"code":502,"message":"upstream"}}).to_string()).create_async().await;
    let err = client
        .create::<Person>(request().option("provider", route))
        .await
        .unwrap_err();
    assert!(matches!(err,Error::Provider(msg) if msg.contains("502")));
    endpoint.assert_async().await;
    let endpoint=mock(&mut server,p,Mode::JsonSchema,true).with_body(
        ": OPENROUTER PROCESSING\n\ndata: {\"error\":{\"message\":\"upstream\"},\"choices\":[{\"finish_reason\":\"error\"}]}\n\ndata: [DONE]\n\n"
    ).create_async().await;
    assert!(
        matches!(client.create_partial::<Person>(request()).await,Err(Error::Provider(msg)) if msg.contains("upstream"))
    );
    endpoint.assert_async().await;
}

#[tokio::test]
async fn streams_require_completion_markers_and_well_formed_events() {
    let mut server = Server::new_async().await;
    for suffix in ["", "data: invalid\n\n", "event: error\ndata: overload\n\n"] {
        let endpoint = mock(&mut server, Provider::OpenAI, Mode::JsonSchema, true)
            .with_body(
                frame(&json!({"choices":[{"delta":{"content":person().to_string()}}]})) + suffix,
            )
            .create_async()
            .await;
        let client = Client::new(config(&server, Provider::OpenAI, Mode::JsonSchema)).unwrap();
        let err = client
            .create_partial::<Person>(request())
            .await
            .err()
            .unwrap();
        match suffix {
            "" => assert!(matches!(err, Error::Incomplete(_))),
            "data: invalid\n\n" => assert!(matches!(err, Error::Protocol(_))),
            _ => assert!(matches!(err, Error::Provider(_))),
        }
        endpoint.assert_async().await;
    }
}

#[tokio::test]
async fn anthropic_thinking_tools_use_auto_and_keep_native_options() {
    let mut server = Server::new_async().await;
    for (model, thinking, expected) in [
        ("claude-opus-5.5", None, "auto"),
        ("claude-fable-5-1-20261001", None, "auto"),
        ("claude-opus-5-50", None, "tool"),
        ("model", Some("adaptive"), "auto"),
        ("model", Some("disabled"), "tool"),
    ] {
        let mut req = Request::messages(vec![
            Message::system("A"),
            Message::user("Q"),
            Message::system("B"),
        ])
        .option("max_tokens", 4000);
        let mut body = json!({"model":model,"max_tokens":4000,"tool_choice":{"type":expected},"system":"A\n\nB","messages":[{"role":"user","content":"Q"}]});
        if let Some(mode) = thinking {
            req = req.option("thinking", json!({"type":mode}));
            body["thinking"] = json!({"type":mode});
        }
        let m = mock(&mut server, Provider::Anthropic, Mode::Tools, false)
            .match_body(Matcher::PartialJson(body))
            .with_body(completion(Provider::Anthropic, Mode::Tools, person()).to_string())
            .create_async()
            .await;
        let cfg = ProviderConfig::new(Provider::Anthropic, model, "fake-key")
            .with_base_url(format!("{}/base", server.url()));
        Client::new(cfg)
            .unwrap()
            .create::<Person>(req)
            .await
            .unwrap();
        m.assert_async().await;
    }
}

#[model]
struct Team {
    people: Vec<Person>,
    region: Option<String>,
}

#[tokio::test]
async fn strict_schema_closes_nested_models_without_mutating_cached_schemas() {
    let mut server = Server::new_async().await;
    let schema = response_schema::<Team>();
    let p = Provider::OpenAI;
    let mut shape =
        json!({"type":"object","additionalProperties":false,"required":["people","region"]});
    if schema["properties"]["people"]["items"]
        .get("$ref")
        .is_some()
    {
        shape["$defs"] = json!({"Person":{"type":"object","additionalProperties":false,"required":["age","name"]}});
    } else {
        shape["properties"] = json!({"people":{"items":{"type":"object","additionalProperties":false,"required":["age","name"]}}});
    }
    let mut raw = completion(
        p,
        Mode::ToolsStrict,
        json!({"people":[person()],"region":null}),
    );
    raw["choices"][0]["message"]["tool_calls"][0]["function"]["name"] = json!("Team");
    let endpoint = mock(&mut server, p, Mode::ToolsStrict, false)
        .match_body(Matcher::PartialJson(json!({
            "tools":[{"function":{"name":"Team","parameters":shape,"strict":true}}]
        })))
        .with_body(raw.to_string())
        .create_async()
        .await;
    let cfg = config(&server, p, Mode::ToolsStrict);
    let result = Client::new(cfg).unwrap().create::<Team>(request()).await;
    endpoint.assert_async().await;
    let team = result.unwrap();
    assert_eq!(team.people[0].age, 41);
    assert!(team.region.is_none());
    assert_eq!(response_schema::<Team>(), schema);
}

#[tokio::test]
async fn json_and_markdown_json_prompt_for_schema_and_raw_skips_formatting() {
    let mut server = Server::new_async().await;
    for p in [Provider::OpenAI, Provider::Anthropic, Provider::OpenRouter] {
        for mode in [Mode::Json, Mode::MarkdownJson] {
            let mut raw = completion(p, mode, person());
            if mode == Mode::MarkdownJson {
                let fence = "\x60\x60\x60";
                let text = json!(format!(
                    "Here is the result:\n{fence}json\n{}\n{fence}",
                    person()
                ));
                if p == Provider::Anthropic {
                    raw["content"][1]["text"] = text;
                } else {
                    raw["choices"][0]["message"]["content"] = text;
                }
            }
            let matcher = Matcher::Regex("matching this JSON Schema.*Person".into());
            let matcher = if p != Provider::Anthropic && mode == Mode::Json {
                Matcher::AllOf(vec![
                    matcher,
                    Matcher::PartialJson(json!({"response_format":{"type":"json_object"}})),
                ])
            } else {
                matcher
            };
            let m = mock(&mut server, p, mode, false)
                .match_body(matcher)
                .with_body(raw.to_string())
                .create_async()
                .await;
            Client::new(config(&server, p, mode))
                .unwrap()
                .create::<Person>(request())
                .await
                .unwrap();
            m.assert_async().await;
        }
        let mut body =
            json!({"stream":false,"model":model_id(p),"messages":[{"role":"user","content":"Hi"}]});
        if p == Provider::Anthropic {
            body["max_tokens"] = json!(1024);
        }
        let endpoint = mock(&mut server, p, Mode::JsonSchema, false)
            .match_body(Matcher::Json(body))
            .with_body(r#"{"id":"response_id"}"#)
            .create_async()
            .await;
        let resp = Client::new(config(&server, p, Mode::JsonSchema))
            .unwrap()
            .create_raw("Hi")
            .await
            .unwrap();
        assert_eq!(resp["id"], "response_id");
        endpoint.assert_async().await;
    }
}

#[tokio::test]
async fn options_cannot_overwrite_mode_payloads() {
    let server = Server::new_async().await;
    let client = Client::new(config(&server, Provider::OpenAI, Mode::Tools)).unwrap();
    for (key, v) in [
        ("stream", json!(true)),
        ("messages", json!([])),
        ("tools", json!([])),
        ("tool_choice", json!("none")),
        ("model", json!("other")),
        ("n", json!(2)),
        ("generationConfig", json!({"responseJsonSchema":{}})),
        ("output_config", json!({"format":{}})),
        ("text", json!(1)),
        ("generationConfig", json!("bad")),
    ] {
        let err = client
            .create::<Person>(request().option(key, v))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Configuration(_)));
    }
}

#[tokio::test]
async fn responses_uses_input_and_named_tool_with_aligned_text_format() {
    let mut server = Server::new_async().await;
    let client = Client::new(config(&server, Provider::OpenAI, Mode::ResponsesTools)).unwrap();
    let params = json!({"type":"object","additionalProperties":false,"required":["age","name"]});
    let matcher = Matcher::PartialJson(json!({
        "input":[{"role":"user","content":"Renée"}],"max_output_tokens":250,
        "tool_choice":{"type":"function","name":"Person"},
        "tools":[{"type":"function","name":"Person","parameters":params}],
        "text":{"format":{"type":"json_schema","name":"Person","schema":params,"strict":true},"verbosity":"low"}
    }));
    let raw = json!({"status":"completed","id":"response_1","output":[
        {"type":"reasoning","summary":[]},
        {"type":"function_call","name":"Foreign","arguments":"bad"},
        {"type":"function_call","name":"Person","arguments":person().to_string(),"call_id":"call_1"}
    ]});
    let endpoint = mock(&mut server, Provider::OpenAI, Mode::ResponsesTools, false)
        .match_body(matcher)
        .with_body(raw.to_string())
        .create_async()
        .await;
    let (p, envelope) = client
        .create_with_completion::<Person>(
            Request::new("Renée")
                .option("max_tokens", 250)
                .option("text", json!({"verbosity":"low"})),
        )
        .await
        .unwrap();
    assert_eq!(p.age, 41);
    assert_eq!(envelope, raw);
    endpoint.assert_async().await;
}

#[tokio::test]
async fn responses_can_return_text_with_builtins_and_ignore_commentary() {
    let mut server = Server::new_async().await;
    let client = Client::new(config(&server, Provider::OpenAI, Mode::ResponsesTools)).unwrap();
    let m=mock(&mut server,Provider::OpenAI,Mode::ResponsesTools,false).match_body(Matcher::PartialJson(json!({
        "tools":[{"type":"web_search"},{"type":"function","name":"Person"}]
    }))).with_body(json!({"status":"completed","output":[
        {"type":"message","phase":"commentary","content":[{"type":"output_text","text":"Searching…"}]},
        {"type":"web_search_call","status":"completed"},
        {"type":"message","phase":"final_answer","content":[{"type":"output_text","text":person().to_string()}]}
    ]}).to_string()).create_async().await;
    let result: Person = client
        .create(request().option("tools", json!([{"type":"web_search"}])))
        .await
        .unwrap();
    assert_eq!(result.name, "Renée");
    m.assert_async().await;
}

#[tokio::test]
async fn responses_stream_selects_one_matching_output_and_skips_auxiliary_deltas() {
    let mut server = Server::new_async().await;
    for (ty, delta) in [
        ("function_call", "response.function_call_arguments.delta"),
        ("message", "response.output_text.delta"),
    ] {
        let data=[
            json!({"type":"response.created","response":{"status":"in_progress"}}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","name":"foreign"}}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"bad"}),
            json!({"type":"response.output_item.added","output_index":2,"item":{"type":"message","phase":"commentary"}}),
            json!({"type":"response.output_text.delta","output_index":2,"delta":"Searching…"}),
            json!({"type":"response.output_item.added","output_index":3,"item":{"type":ty,"name":"Person"}}),
            json!({"type":delta,"output_index":3,"delta":r#"{"name":"Renée","age":"#}),
            json!({"type":delta,"output_index":3,"delta":"41}"}),
            json!({"type":"response.completed","response":{"status":"completed"}}),
        ].iter().map(frame).collect();
        let m = fragmented(
            mock(&mut server, Provider::OpenAI, Mode::ResponsesTools, true),
            data,
        )
        .create_async()
        .await;
        let client = Client::new(config(&server, Provider::OpenAI, Mode::ResponsesTools)).unwrap();
        let mut output = client.create_partial::<Person>(request()).await.unwrap();
        let preview = output.next().await.unwrap().unwrap();
        assert!(
            matches!(preview,PartialEvent::Partial(p) if p.name.as_deref()==Some("Renée") && p.age.is_none())
        );
        let rest: Vec<_> = output.try_collect().await.unwrap();
        assert!(matches!(rest.last(),Some(PartialEvent::Complete(p)) if p.age==41));
        m.assert_async().await;
    }
}

#[tokio::test]
async fn responses_failures_refusals_and_incomplete_states_are_typed() {
    let mut server = Server::new_async().await;
    let client = Client::new(config(&server, Provider::OpenAI, Mode::ResponsesTools)).unwrap();
    let examples = [
        (
            "failed",
            json!({"status":"failed","error":{"message":"broken"}}),
        ),
        (
            "incomplete",
            json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}),
        ),
        (
            "completed",
            json!({"status":"completed","output":[{"type":"message","content":[{"type":"refusal","refusal":"declined"}]}]}),
        ),
    ];
    for (state, raw) in examples {
        for stream in [false, true] {
            let body = if stream {
                frame(&json!({"type":format!("response.{state}"),"response":raw}))
            } else {
                raw.to_string()
            };
            let m = mock(&mut server, Provider::OpenAI, Mode::ResponsesTools, stream)
                .with_body(body)
                .create_async()
                .await;
            let err = if stream {
                client
                    .create_partial::<Person>(request())
                    .await
                    .err()
                    .unwrap()
            } else {
                client.create::<Person>(request()).await.unwrap_err()
            };
            match state {
                "failed" => assert!(matches!(err, Error::Provider(_))),
                "incomplete" => assert!(matches!(err, Error::Incomplete(_))),
                _ => assert!(matches!(err, Error::Refusal(_))),
            }
            m.assert_async().await;
        }
    }
}

#[tokio::test]
async fn native_history_is_preserved_before_retries_and_after_schema_instructions() {
    let mut server = Server::new_async().await;
    for (p, mode) in [
        (Provider::OpenAI, Mode::ResponsesTools),
        (Provider::OpenAI, Mode::Json),
        (Provider::Anthropic, Mode::Json),
        (Provider::OpenRouter, Mode::Tools),
        (Provider::Cohere, Mode::JsonSchema),
        (Provider::Mistral, Mode::Tools),
        (Provider::Google, Mode::JsonSchema),
    ] {
        let (key, history) = match (p, mode) {
            (_, Mode::ResponsesTools) => (
                "input",
                vec![
                    json!({"type":"reasoning","id":"rs_1","encrypted_content":"opaque","summary":[]}),
                    json!({"type":"function_call","call_id":"call_0","name":"Lookup","arguments":"{}"}),
                    json!({"type":"function_call_output","call_id":"call_0","output":"{{literal}}"}),
                ],
            ),
            (Provider::Anthropic, _) => (
                "messages",
                vec![
                    json!({"role":"assistant","content":[{"type":"tool_use","id":"tool_0","name":"Lookup","input":{}}]}),
                    json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"tool_0","content":"{{literal}}"}]}),
                ],
            ),
            (Provider::Google, _) => (
                "contents",
                vec![
                    json!({"role":"model","parts":[{"functionCall":{"name":"Lookup","args":{}},"thoughtSignature":"opaque"}]}),
                    json!({"role":"user","parts":[{"functionResponse":{"name":"Lookup","response":{"value":"{{literal}}"}}}]}),
                ],
            ),
            _ => (
                "messages",
                vec![
                    json!({"role":"assistant","tool_calls":[{"id":"call_0","type":"function","function":{"name":"Lookup","arguments":"{}"}}]}),
                    json!({"role":"tool","tool_call_id":"call_0","content":"{{literal}}"}),
                ],
            ),
        };
        let request = Request::native_messages(history.clone())
            .context(json!({"literal":"replacement"}))
            .max_retries(1);
        let offset = usize::from(mode == Mode::Json && p == Provider::OpenAI);
        let mut endpoints = Vec::new();
        for (attempt, data) in [json!({}), person()].into_iter().enumerate() {
            let history = history.clone();
            let raw = if mode == Mode::ResponsesTools {
                json!({"output":[{"type":"function_call","name":"Person","arguments":data.to_string()}]})
            } else {
                completion(p, mode, data)
            };
            let endpoint = mock(&mut server, p, mode, false)
                .match_request(move |req| {
                    let body: Value = serde_json::from_slice(req.body().unwrap()).unwrap();
                    let items = body[key].as_array().unwrap();
                    let end = offset + history.len();
                    items.get(offset..end) == Some(&history[..])
                        && ((attempt == 0) == (items.len() == end))
                })
                .with_body(raw.to_string())
                .create_async()
                .await;
            endpoints.push(endpoint);
        }
        let client = Client::new(config(&server, p, mode)).unwrap();
        let result = client.create::<Person>(request).await;
        for endpoint in endpoints {
            endpoint.assert_async().await;
        }
        assert_eq!(result.unwrap().age, 41);
    }
}

#[tokio::test]
async fn native_items_and_typed_messages_compose_in_order() {
    let mut server = Server::new_async().await;
    let p = Provider::OpenAI;
    let mode = Mode::ResponsesTools;
    let item = json!({"type":"item_reference","id":"msg_1"});
    let mut req = Request::native_messages(vec![item.clone()]).context(json!({"name":"John"}));
    req.messages.push(Message::user("What about {{name}}?"));
    let m = mock(&mut server, p, mode, false)
        .match_body(Matcher::PartialJson(json!({
            "input":[item,{"role":"user","content":"What about John?"}]
        })))
        .with_body("{}")
        .create_async()
        .await;
    let client = Client::new(config(&server, p, mode)).unwrap();
    client.create_raw(req).await.unwrap();
    m.assert_async().await;
    for history in [vec![], vec![Value::Null]] {
        assert!(matches!(
            client.create_raw(Request::native_messages(history)).await,
            Err(Error::Configuration(_))
        ));
    }
}
