use super::*;
#[model]
#[derive(Debug, PartialEq)]
struct Color {
    name: String,
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
