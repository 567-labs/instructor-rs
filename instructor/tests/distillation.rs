use std::io::{self, Write};

use instructor_ai::{
    distillation::{Arguments, FinetuneFormat, Instructions},
    model, BlockingClient, Client, Error, Provider, ProviderConfig,
};
use mockito::{Matcher, Server};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Serialize)]
struct Input {
    count: i32,
    label: String,
}

#[model]
#[derive(Debug, PartialEq)]
struct ResultLabel {
    #[validate(custom = "nonempty")]
    name: String,
}
fn nonempty(s: &str) -> Result<(), String> {
    if s.is_empty() {
        Err("empty".into())
    } else {
        Ok(())
    }
}

const DEF: &str = "fn label(count:i32, label:String) -> ResultLabel";
fn instructions() -> Instructions<Input, ResultLabel> {
    Instructions::new("tag", DEF).with_arguments(|input: &Input| {
        Ok(Arguments {
            args: vec![json!(input.count)],
            kwargs: [("label".into(), json!(input.label))].into_iter().collect(),
        })
    })
}

#[test]
fn teacher_returns_typed_result_and_each_line_is_independently_decodable() {
    let inst = instructions();
    let mut out = Vec::new();
    let input = Input {
        count: 42,
        label: "a\n\"雪".into(),
    };
    let mut calls = 0;
    for _ in 0..2 {
        let value = inst
            .distil(&input, &mut out, |args| {
                calls += 1;
                Ok(ResultLabel {
                    name: args.label.clone(),
                })
            })
            .unwrap();
        assert_eq!(value.name, input.label);
    }
    assert_eq!(calls, 2);
    let lines = std::str::from_utf8(&out)
        .unwrap()
        .lines()
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    let expected = format!("Return `tag(42, label={})`", json!(input.label));
    for line in lines {
        let record: Value = serde_json::from_str(line).unwrap();
        let messages = &record["messages"];
        assert_eq!(
            messages[0]["content"],
            format!("Predict the results of this function:\n\n{DEF}")
        );
        assert_eq!(messages[1]["content"], expected);
        assert_eq!(messages[2]["role"], "assistant");
        let call = &messages[2]["function_call"];
        assert_eq!(call["name"], "ResultLabel");
        let output: ResultLabel =
            serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(output.name, input.label);
        let f = &record["functions"][0];
        assert_eq!(f["name"], call["name"]);
        assert_eq!(f["parameters"]["required"], json!(["name"]));
    }
}

#[test]
fn raw_format_and_input_conventions() {
    let input = Input {
        count: 2,
        label: "x".into(),
    };
    let output = ResultLabel { name: "xx".into() };
    let v = instructions()
        .format(FinetuneFormat::Raw)
        .record(&input, &output)
        .unwrap();
    assert_eq!(v["fn_name"], "tag");
    assert_eq!(v["fn_repr"], DEF);
    assert_eq!(v["args"], json!([2]));
    assert_eq!(v["kwargs"], json!({"label":"x"}));
    assert_eq!(v["resp"], json!({"name":"xx"}));
    assert_eq!(v["schema"]["title"], "ResultLabel");
    for (value, args, kwargs) in [
        (Value::Null, json!([]), json!({})),
        (json!([1, 2]), json!([1, 2]), json!({})),
        (json!(false), json!([false]), json!({})),
        (json!({"x":7}), json!([]), json!({"x":7})),
    ] {
        let rec = Instructions::new("f", "fn f")
            .format(FinetuneFormat::Raw)
            .record(&value, &output)
            .unwrap();
        assert_eq!(rec["args"], args);
        assert_eq!(rec["kwargs"], kwargs);
    }
    let wrapped = Instructions::new("scalar", "scalar")
        .record(&(), &2i32)
        .unwrap();
    let args: Value = serde_json::from_str(
        wrapped["messages"][2]["function_call"]["arguments"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(args, json!({"content":2}));
}

#[test]
fn errors_propagate_and_failed_teacher_records_are_skipped() {
    let inst = instructions();
    let input = Input {
        count: 0,
        label: "".into(),
    };
    let mut out = Vec::new();
    assert!(matches!(
        inst.distil(&input, &mut out, |_| Err(Error::Provider("failure".into()))),
        Err(Error::Provider(_))
    ));
    assert!(matches!(
        inst.track(
            &input,
            &ResultLabel {
                name: String::new()
            },
            &mut out
        ),
        Err(Error::Validation(_))
    ));
    assert!(out.is_empty());
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    assert!(matches!(
        inst.track(&input, &ResultLabel { name: "ok".into() }, Broken),
        Err(Error::Io(_))
    ));
}

fn envelope() -> String {
    json!({"status":"completed", "output":[{"type":"message","content":[{"type":"output_text","text":r#"{"name":"student"}"#}]}]}).to_string()
}

#[tokio::test]
async fn dispatch_matches_teacher_messages_and_uses_configured_student() {
    let mut server = Server::new_async().await;
    let inst = instructions();
    let input = Input {
        count: 7,
        label: "east".into(),
    };
    let record = inst
        .record(
            &input,
            &ResultLabel {
                name: "teacher".into(),
            },
        )
        .unwrap();
    let calls = server
        .mock("POST", "/responses")
        .match_body(Matcher::PartialJson(json!({
            "model": "student-model",
            "input":[record["messages"][0],record["messages"][1]],
            "text":{"format":{"name":"ResultLabel"}},
        })))
        .with_body(envelope())
        .create_async()
        .await;
    let config =
        ProviderConfig::new(Provider::OpenAI, "student-model", "mock").with_base_url(server.url());
    let result = inst
        .dispatch(&Client::new(config).unwrap(), &input)
        .await
        .unwrap();
    assert_eq!(result.name, "student");
    calls.assert_async().await;
}

#[test]
fn blocking_dispatch() {
    let mut server = Server::new();
    let call = server
        .mock("POST", "/responses")
        .with_body(envelope())
        .create();
    let config = ProviderConfig::new(Provider::OpenAI, "test", "mock").with_base_url(server.url());
    let client = BlockingClient::new(config).unwrap();
    let output = instructions()
        .dispatch_blocking(
            &client,
            &Input {
                count: 1,
                label: "a".into(),
            },
        )
        .unwrap();
    assert_eq!(output.name, "student");
    call.assert();
}
