use super::*;
use instructor_ai::ParallelTools;

#[model]
#[derive(Debug, PartialEq)]
struct Measurement {
    #[validate(context = "measure")]
    length: i32,
}
fn measure(n: &mut i32, context: &Value) -> Result<(), String> {
    if *n < 0 {
        return Err("length must be nonnegative".into());
    }
    *n += context["offset"].as_i64().unwrap_or_default() as i32;
    Ok(())
}

#[model]
#[derive(Debug, PartialEq)]
struct Color {
    name: String,
}

#[derive(ParallelTools, Debug, PartialEq)]
enum Property<T> {
    Size(Measurement),
    Tint(T),
}

fn response(provider: Provider, calls: &[(&str, Value)]) -> Value {
    let calls:Vec<_> = calls.iter().enumerate().map(|(i,(name,args))|{
        let id=format!("call_{i}");
        match provider {
            Provider::Anthropic=>json!({"type":"tool_use","id":id,"name":name,"input":args}),
            Provider::Google=>json!({"type":"function_call","id":id,"name":name,"arguments":args}),
            _=>json!({"id":id, "type":"function","function":{"name":name,"arguments":args.to_string()}}),
        }
    }).collect();
    match provider {
        Provider::Google => {
            json!({"status":"requires_action","steps":calls})
        }
        Provider::Anthropic => json!({"stop_reason":"tool_use","content":calls}),
        Provider::Cohere => json!({"finish_reason":"TOOL_CALL","message":{"tool_calls":calls}}),
        _ => json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":calls}}]}),
    }
}

fn declarations(provider: Provider) -> Value {
    let first = json!({"type":"object","properties":{"length":{"type":"integer"}}});
    let second = json!({"type":"object","properties":{"name":{"type":"string"}}});
    let fields = [("Measurement", first), ("Color", second)];
    match provider {
        Provider::Google => {
            let tools = fields
                .map(|(name, params)| json!({"type":"function","name":name,"parameters":params}));
            json!({"tools":tools,"generation_config":{"tool_choice":"any"}})
        }
        Provider::Anthropic => {
            let tools = fields.map(|(name, params)| json!({"name":name,"input_schema":params}));
            json!({"tools":tools,"tool_choice":{"type":"auto"}})
        }
        _ => {
            let tools=fields.map(|(name,params)|json!({"type":"function","function":{"name":name,"parameters":params}}));
            let choice = match provider {
                Provider::Cohere => "REQUIRED",
                Provider::Mistral => "any",
                _ => "required",
            };
            json!({"tools":tools,"tool_choice":choice})
        }
    }
}

#[tokio::test]
async fn typed_union_preserves_call_order_and_repetitions_across_providers() {
    let mut server = Server::new_async().await;
    for provider in PROVIDERS {
        let expected = response(
            provider,
            &[
                ("Color", json!({"name":"red"})),
                ("Measurement", json!({"length":3})),
                ("Color", json!({"name":"white"})),
            ],
        );
        let mut expected_body = declarations(provider);
        let mut req = request().context(json!({"offset":4}));
        if matches!(provider, Provider::OpenAI | Provider::OpenRouter) {
            req = req.strict(true);
            for tool in expected_body["tools"].as_array_mut().unwrap() {
                tool["function"]["strict"] = true.into();
                tool["function"]["parameters"]["additionalProperties"] = false.into();
            }
        }
        let endpoint = mock(&mut server, provider, Mode::Tools, false)
            .match_body(Matcher::PartialJson(expected_body))
            .with_body(expected.to_string())
            .create_async()
            .await;
        let client = Client::new(config(&server, provider, Mode::Tools)).unwrap();
        let (values, completion) = client
            .create_parallel_with_completion::<Property<Color>>(req)
            .await
            .unwrap();
        assert_eq!(
            values,
            [
                Property::Tint(Color { name: "red".into() }),
                Property::Size(Measurement { length: 7 }),
                Property::Tint(Color {
                    name: "white".into()
                }),
            ]
        );
        assert_eq!(completion, expected);
        endpoint.assert_async().await;
        endpoint.remove_async().await;
    }
}

#[test]
fn blocking_retries_entire_batch_and_caches_success() {
    let mut server = Server::new();
    let provider = Provider::OpenAI;
    let mode = Mode::ToolsStrict;
    let bad = response(
        provider,
        &[
            ("Measurement", json!({"length":22})),
            ("Measurement", json!({"length":-1})),
        ],
    );
    let first=mock(&mut server,provider,mode,false)
        .match_body(Matcher::PartialJson(json!({
            "parallel_tool_calls":true, "tool_choice":"required",
            "tools":[{"function":{"strict":true, "name":"Measurement","parameters":{"additionalProperties":false}}}]
        })))
        .with_body(bad.to_string()).create();
    let good = response(provider, &[("Measurement", json!({"length":2}))]);
    let second = mock(&mut server, provider, mode, false)
        .match_body(Matcher::Regex(r"length must be nonnegative".into()))
        .with_body(good.to_string())
        .create();
    let req = request().max_retries(1);
    let client = BlockingClient::new(config(&server, provider, mode))
        .unwrap()
        .with_cache(1);
    for _ in 0..2 {
        let items: Vec<Measurement> = client.create_parallel(req.clone()).unwrap();
        assert_eq!(items, [Measurement { length: 2 }]);
    }
    first.assert();
    second.assert();
}

#[tokio::test]
async fn duplicate_names_and_invalid_modes_fail_before_request() {
    let server = Server::new_async().await;
    let client = Client::new(config(&server, Provider::OpenAI, Mode::Tools)).unwrap();
    let result = client
        .create_parallel::<Property<Measurement>>(request())
        .await;
    assert!(
        matches!(result,Err(Error::Configuration(msg)) if msg.contains("duplicate") && msg.contains("Measurement"))
    );
    let client = Client::new(config(&server, Provider::Google, Mode::JsonSchema)).unwrap();
    assert!(matches!(
        client.create_parallel::<Measurement>(request()).await,
        Err(Error::Configuration(_))
    ));
}

#[tokio::test]
async fn unknown_name_missing_calls_and_truncation() {
    let mut server = Server::new_async().await;
    let p = Provider::OpenAI;
    let m = Mode::Tools;
    let client = Client::new(config(&server, p, m)).unwrap();
    for (raw, want) in [
        (
            response(p, &[("Oops", json!({"length":1}))]),
            "Unknown tool call",
        ),
        (response(p, &[]), "expected at least one tool"),
    ] {
        let mock = mock(&mut server, p, m, false)
            .with_body(raw.to_string())
            .create_async()
            .await;
        let Err(Error::Retry { attempts, .. }) =
            client.create_parallel::<Measurement>(request()).await
        else {
            panic!()
        };
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].completion, raw);
        assert!(attempts[0].error.to_string().contains(want));
        mock.assert_async().await;
        mock.remove_async().await;
    }
    let mut raw = response(p, &[("Measurement", json!({"length":1}))]);
    raw["choices"][0]["finish_reason"] = json!("length");
    let mock = mock(&mut server, p, m, false)
        .with_body(raw.to_string())
        .create_async()
        .await;
    let result = client
        .create_parallel::<Measurement>(request().max_retries(2))
        .await;
    assert!(matches!(result, Err(Error::Incomplete(_))));
    mock.assert_async().await;
}

#[model]
#[schemars(title = "size of box")]
struct Renamed {
    size: u8,
}

#[tokio::test]
async fn dispatch_uses_transmitted_schema_title() {
    let mut server = Server::new_async().await;
    let raw = response(Provider::OpenAI, &[("size_of_box", json!({"size":2}))]);
    let endpoint = mock(&mut server, Provider::OpenAI, Mode::Tools, false)
        .match_body(Matcher::PartialJson(
            json!({"tools":[{"function":{"name":"size_of_box"}}]}),
        ))
        .with_body(raw.to_string())
        .create_async()
        .await;
    let client = Client::new(config(&server, Provider::OpenAI, Mode::Tools)).unwrap();
    assert_eq!(
        client.create_parallel::<Renamed>("size 2").await.unwrap()[0].size,
        2
    );
    endpoint.assert_async().await;
}

fn deltas(parts: &[(usize, &str)]) -> String {
    let calls: Vec<_> = parts
        .iter()
        .map(|(i, text)| json!({"index": i,"function":{"arguments":text}}))
        .collect();
    frame(&json!({"choices":[{"index":0,"delta":{"tool_calls":calls}}]}))
}

#[tokio::test]
async fn interleaved_tool_arrays_keep_indices_and_document_boundaries() {
    let mut server = Server::new_async().await;
    let mut text = deltas(&[
        (2, r#"{"tasks":[{"name":"C"#),
        (1, r#"{"name":"B"#),
        (0, r#"{"tasks":[{"name":"A"#),
    ]);
    text.push_str(&deltas(&[
        (1, "lue\"}"),
        (0, "mber\"}]}"),
        (2, "oral\"}]}"),
    ]));
    text.push_str("data: [DONE]\n\n");
    for provider in [Provider::OpenAI, Provider::OpenRouter, Provider::Mistral] {
        let endpoint = mock(&mut server, provider, Mode::Tools, true)
            .with_body(&text)
            .create_async()
            .await;
        let client = Client::new(config(&server, provider, Mode::Tools)).unwrap();
        let values: Vec<_> = client
            .create_iterable::<Color>(request())
            .await
            .unwrap()
            .map(|item| item.unwrap().name)
            .collect()
            .await;
        assert_eq!(values, ["Amber", "Blue", "Coral"]);
        endpoint.assert_async().await;
        endpoint.remove_async().await;
    }
}

#[test]
fn incomplete_call_never_consumes_another_calls_arguments() {
    let mut server = Server::new();
    let p = Provider::OpenAI;
    for index in [0, 1] {
        let text = deltas(&[
            (index, r#"{"tasks":[{"name":"green"},{"name":"#),
            (index + 1, r#""crossed"}]}"#),
        ]) + "data: [DONE]\n\n";
        let endpoint = mock(&mut server, p, Mode::Tools, true)
            .with_body(text)
            .create();
        let client = BlockingClient::new(config(&server, p, Mode::Tools)).unwrap();
        let mut iter = client.create_iterable::<Color>(request()).unwrap();
        assert_eq!(iter.next().unwrap().unwrap().name, "green");
        assert!(matches!(iter.next(), Some(Err(Error::Incomplete(_)))));
        assert!(iter.next().is_none());
        endpoint.assert();
        endpoint.remove();
    }
}

#[tokio::test]
async fn iterable_scalar_outputs_have_no_per_item_wrapper() {
    let mut server = Server::new_async().await;
    let schema = response_schema::<Vec<i32>>();
    let name = schema["title"].as_str().unwrap();
    for provider in PROVIDERS {
        let endpoint = mock(&mut server, provider, Mode::Tools, true)
            .with_body(events(
                provider,
                Mode::Tools,
                name,
                &[r#"{"tasks":[1"#, "23,-5]}"],
            ))
            .create_async()
            .await;
        let client = Client::new(config(&server, provider, Mode::Tools)).unwrap();
        let values: Vec<i32> = client
            .create_iterable::<i32>(request())
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(values, [123, -5]);
        endpoint.assert_async().await;
        endpoint.remove_async().await;
    }
}
