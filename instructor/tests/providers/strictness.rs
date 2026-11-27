use super::*;

#[model]
struct Member {
    name: String,
    nickname: Option<String>,
}

#[model]
struct Team {
    people: Vec<Member>,
    region: Option<String>,
}

fn matches_team_schema(body: &Value, mode: Mode, strict: bool) -> bool {
    let (definition, key) = if tool(mode) {
        (&body["tools"][0]["function"], "parameters")
    } else {
        (&body["response_format"]["json_schema"], "schema")
    };
    let schema = &definition[key];
    let item = &schema["properties"]["people"]["items"];
    let member = match item["$ref"].as_str().and_then(|s| s.strip_prefix('#')) {
        Some(path) => schema.pointer(path).unwrap(),
        None => item,
    };
    let closed = strict.then_some(&Value::Bool(false));
    let (outer, inner) = if strict {
        (json!(["people", "region"]), json!(["name", "nickname"]))
    } else {
        (json!(["people"]), json!(["name"]))
    };
    body.get("strict").is_none()
        && definition.get("strict") == strict.then_some(&Value::Bool(true))
        && definition["name"] == "Team"
        && schema["required"] == outer
        && member["required"] == inner
        && schema.get("additionalProperties") == closed
        && member.get("additionalProperties") == closed
}

#[tokio::test]
async fn strictness_is_local_to_each_call_and_separates_response_cache_entries() {
    let schema = response_schema::<Team>();
    let mut server = Server::new_async().await;
    for provider in [Provider::OpenAI, Provider::OpenRouter] {
        for mode in [Mode::Tools, Mode::ToolsStrict, Mode::JsonSchema] {
            let mut endpoints = Vec::new();
            for (strict, payload) in [
                (
                    true,
                    json!({"people":[{"name":"Ada", "nickname":null}], "region":"East"}),
                ),
                (false, json!({"people":[{"name":"Grace"}]})),
            ] {
                let raw = named_completion(provider, mode, "Team", payload);
                let endpoint = mock(&mut server, provider, mode, false)
                    .match_request(move |req| {
                        let body: Value = serde_json::from_slice(req.body().unwrap()).unwrap();
                        matches_team_schema(&body, mode, strict)
                    })
                    .with_body(raw.to_string())
                    .create_async()
                    .await;
                endpoints.push(endpoint);
            }
            let client = Client::new(config(&server, provider, mode))
                .unwrap()
                .with_cache(2);
            let (strict, loose) = tokio::join!(
                client.create::<Team>(request().strict(true)),
                client.create::<Team>(request().strict(false))
            );
            let (strict, loose) = (strict.unwrap(), loose.unwrap());
            assert_eq!(strict.people[0].name, "Ada");
            assert_eq!(strict.region.as_deref(), Some("East"));
            assert_eq!(loose.people[0].name, "Grace");
            assert!(loose.people[0].nickname.is_none() && loose.region.is_none());
            // The default must hit the entry with the same effective wire schema.
            let default = client
                .create_with_metadata::<Team>(request())
                .await
                .unwrap();
            assert_eq!(default.attempts, 0);
            let expected = if mode == Mode::Tools { loose } else { strict };
            assert_eq!(default.value.people[0].name, expected.people[0].name);
            assert_eq!(response_schema::<Team>(), schema);
            for endpoint in endpoints {
                endpoint.assert_async().await;
            }
        }
    }
}

#[tokio::test]
async fn strictness_option_applies_to_partial_streams() {
    let mut server = Server::new_async().await;
    let p = Provider::OpenRouter;
    let mode = Mode::JsonSchema;
    let endpoint = mock(&mut server, p, mode, true)
        .match_request(|req| {
            let body = serde_json::from_slice(req.body().unwrap()).unwrap();
            matches_team_schema(&body, Mode::JsonSchema, false)
        })
        .with_body(events(
            p,
            mode,
            "Team",
            &[r#"{"people":["#, r#"{"name":"Grace"}]}"#],
        ))
        .create_async()
        .await;
    let client = Client::new(config(&server, p, mode)).unwrap();
    let stream = client
        .create_partial::<Team>(request().option("strict", false))
        .await
        .unwrap();
    let items: Vec<_> = stream.try_collect().await.unwrap();
    assert!(
        matches!(items.last(), Some(PartialEvent::Complete(team)) if team.people[0].name == "Grace" && team.people[0].nickname.is_none())
    );
    endpoint.assert_async().await;
}

#[tokio::test]
async fn strictness_requires_a_boolean_and_a_supported_typed_mode() {
    let server = Server::new_async().await;
    let client = Client::new(config(&server, Provider::OpenAI, Mode::Tools)).unwrap();
    for invalid in [json!("false"), Value::Null, json!(0)] {
        assert!(matches!(
            client
                .create::<Person>(request().option("strict", invalid))
                .await,
            Err(Error::Configuration(_))
        ));
    }
    assert!(matches!(
        client.create_raw(request().strict(false)).await,
        Err(Error::Configuration(_))
    ));
    for (p, mode) in [
        (Provider::OpenAI, Mode::MarkdownJson),
        (Provider::OpenRouter, Mode::Json),
        (Provider::Anthropic, Mode::Tools),
        (Provider::Mistral, Mode::JsonSchema),
    ] {
        let client = Client::new(config(&server, p, mode)).unwrap();
        assert!(matches!(
            client.create::<Person>(request().strict(true)).await,
            Err(Error::Configuration(_))
        ));
    }
}
