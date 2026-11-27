use instructor_ai::{
    batch::{BatchItem, BatchOptions},
    model, BlockingClient, Client, Error, Message, Mode, Provider, ProviderConfig, Request,
};
use mockito::{Matcher, Server};
use serde_json::{json, Value};

#[path = "batch/download.rs"]
mod download;

#[model]
#[derive(Debug)]
struct Amount {
    #[validate(context = "check")]
    value: u32,
}

fn check(value: &mut u32, ctx: &Value) -> Result<(), String> {
    if ctx["forbid"] == *value {
        return Err("forbidden amount".into());
    }
    Ok(())
}

fn config(provider: Provider, base: &str) -> ProviderConfig {
    let model = match provider {
        Provider::OpenAI => "gpt-6-luna",
        Provider::Anthropic => "claude-opus-5",
        _ => "test",
    };
    ProviderConfig::new(provider, model, "test-key").with_base_url(format!("{base}/v1"))
}

fn client(provider: Provider) -> Client {
    Client::new(config(provider, "https://example.test")).unwrap()
}

fn item() -> BatchItem {
    BatchItem::new(
        Request::messages(vec![
            Message::system("Extract amount"),
            Message::user("{{ amount }}"),
        ])
        .context(json!({"amount":300}))
        .option("max_tokens", 23),
    )
    .id("bill_1")
}

fn lines(jsonl: &str) -> Vec<Value> {
    jsonl
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn per_provider_preparation_and_defaults() {
    for p in [Provider::OpenAI, Provider::Anthropic, Provider::Mistral] {
        let prepared = client(p)
            .batch::<Amount>()
            .unwrap()
            .prepare([item(), BatchItem::new("hello")])
            .unwrap();
        let records = lines(&prepared);
        assert!(prepared.ends_with('\n'));
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["custom_id"], "bill_1");
        assert_eq!(records[1]["custom_id"], "request-1");
        if p == Provider::Anthropic {
            let params = &records[0]["params"];
            assert_eq!(params["system"], "Extract amount");
            assert_eq!(params["messages"], json!([{"role":"user","content":"300"}]));
            assert!(params.get("temperature").is_none());
            assert_eq!(params["tools"][0]["name"], "extract_data");
            assert_eq!(
                params["tool_choice"],
                json!({"type":"tool","name":"extract_data"})
            );
            let schema = &params["tools"][0]["input_schema"];
            assert_eq!(schema["title"], "Amount");
            assert_eq!(schema["additionalProperties"], false);
            assert_eq!(
                params["tools"][0]["input_schema"]["properties"]["value"]["type"],
                "integer"
            );
            assert_eq!(params["max_tokens"], 23);
            assert_eq!(records[1]["params"]["max_tokens"], 1000);
            assert!(params.get("stream").is_none());
            continue;
        }
        let body = &records[0]["body"];
        assert!(body.get("stream").is_none());
        let format = if p == Provider::OpenAI {
            assert_eq!(records[0]["url"], "/v1/responses");
            assert_eq!(records[0]["method"], "POST");
            assert_eq!(body["input"][1]["content"], "300");
            assert_eq!(body["max_output_tokens"], 23);
            assert_eq!(records[1]["body"]["max_output_tokens"], 1000);
            for key in [
                "messages",
                "response_format",
                "temperature",
                "reasoning_effort",
                "max_tokens",
                "max_completion_tokens",
            ] {
                assert!(body.get(key).is_none(), "unexpected Responses option {key}");
            }
            assert_eq!(body["text"]["format"]["type"], "json_schema");
            &body["text"]["format"]
        } else {
            assert_eq!(body["messages"][1]["content"], "300");
            assert_eq!(body["temperature"], 0.1);
            assert_eq!(body["max_tokens"], 23);
            assert!(body.get("model").is_none());
            assert!(body.get("input").is_none());
            &body["response_format"]["json_schema"]
        };
        assert_eq!(format["strict"], true);
        assert_eq!(format["schema"]["additionalProperties"], false);
    }
}

#[test]
fn atomic_preparation_and_validation() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("batch.jsonl");
    std::fs::write(&dest, "original").unwrap();
    let batch = client(Provider::Anthropic).batch::<Amount>().unwrap();
    for bad in [
        "bad id",
        "ünicode",
        "bill_1",
        "",
        "a.b",
        "x\n",
        &"x".repeat(65),
    ] {
        let outcome = batch.prepare_file([item(), BatchItem::new("prompt").id(bad)], &dest);
        assert!(outcome.is_err());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "original");
    }
    batch
        .prepare_file([BatchItem::new("prompt").id("x".repeat(64))], &dest)
        .unwrap();
    assert_eq!(
        lines(&std::fs::read_to_string(&dest).unwrap())[0]["custom_id"],
        "x".repeat(64)
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(client(Provider::Google).batch::<Amount>().is_err());
    assert!(batch.prepare(Vec::<Request>::new()).is_err());
    assert!(batch
        .prepare([Request::new("prompt").token_budget(10)])
        .is_err());
}

#[test]
fn openai_strictness_and_responses_mode() {
    #[model]
    struct Catalog {
        labels: std::collections::HashMap<String, String>,
    }
    let client = client(Provider::OpenAI);
    let batch = client.batch::<Catalog>().unwrap();
    assert!(batch.prepare(["prompt"]).is_err());
    let local = batch
        .prepare([Request::new("prompt").strict(false)])
        .unwrap();
    let batch = batch.strict(false);
    let input = batch.prepare(["prompt"]).unwrap();
    assert_eq!(input, local);
    assert!(batch
        .prepare([Request::new("prompt").strict(true)])
        .is_err());
    let rec = &lines(&input)[0];
    assert!(rec["body"].get("response_format").is_none());
    assert_eq!(rec["body"]["text"]["format"]["strict"], false);
    assert_eq!(
        rec["body"]["text"]["format"]["schema"]["properties"]["labels"]["additionalProperties"]
            ["type"],
        "string"
    );
    let response = client
        .batch_with_mode::<Amount>(Mode::ResponsesTools)
        .unwrap();
    let input = response.prepare([item()]).unwrap();
    let rec = &lines(&input)[0];
    assert_eq!(rec["url"], "/v1/responses");
    assert_eq!(rec["body"]["max_output_tokens"], 23);
    assert_eq!(rec["body"]["input"][1]["content"], "300");
}

#[test]
fn openai_reads_saved_chat_and_responses_result_envelopes() {
    let batch = client(Provider::OpenAI).batch::<Amount>().unwrap();
    let response = json!({"custom_id":"response","response":{"status_code":200,"body":{
        "status":"completed","output":[
            {"type":"reasoning","summary":[]},
            {"type":"message","phase":"commentary","content":[{"type":"output_text","text":"Checking"}]},
            {"type":"message","phase":"final_answer","content":[{"type":"output_text","text":"{\"value\":15}"}]}
        ]
    }}});
    let mut truncated = response.clone();
    truncated["response"]["body"]["status"] = "incomplete".into();
    let mut refused = response.clone();
    refused["response"]["body"]["output"][2]["content"] =
        json!([{"type":"refusal","refusal":"declined"}]);
    let input = format!("{response}\n{}\n{truncated}\n{refused}", good());
    let rows = batch.parse(&input);
    assert_eq!(rows[0].raw, response);
    assert_eq!(rows[0].result.as_ref().unwrap().value, 15);
    assert_eq!(rows[1].result.as_ref().unwrap().value, 23);
    assert_eq!(
        rows[2].result.as_ref().unwrap_err().kind,
        "incomplete_output"
    );
    assert_eq!(rows[3].result.as_ref().unwrap_err().kind, "refusal");
}

#[test]
fn saved_chat_requests_determine_the_job_endpoint_and_mixed_files_fail_before_upload() {
    let mut server = Server::new();
    let config = config(Provider::OpenAI, &server.url());
    let legacy = BlockingClient::new(config.clone().with_chat_completions())
        .unwrap()
        .batch::<Amount>()
        .unwrap();
    let input = legacy.prepare([item()]).unwrap();
    let rec = &lines(&input)[0];
    assert_eq!(rec["url"], "/v1/chat/completions");
    assert_eq!(rec["body"]["messages"][1]["content"], "300");
    assert_eq!(rec["body"]["temperature"], 0.1);
    assert_eq!(rec["body"]["max_completion_tokens"], 23);
    assert_eq!(rec["body"]["reasoning_effort"], "none");
    assert_eq!(
        rec["body"]["response_format"]["json_schema"]["strict"],
        true
    );
    let batch = BlockingClient::new(config)
        .unwrap()
        .batch::<Amount>()
        .unwrap();
    let new = batch.prepare(["other prompt"]).unwrap();
    for invalid in [
        format!("{new}{input}"),
        input.replace("/v1/chat/completions", "/unknown"),
        new.replace("POST", "GET"),
    ] {
        assert!(matches!(
            batch.submit(&invalid),
            Err(Error::Configuration(_))
        ));
    }
    let upload = server
        .mock("POST", "/v1/files")
        .match_body(Matcher::Regex("/v1/chat/completions".into()))
        .with_body(r#"{"id":"file_legacy"}"#)
        .create();
    let job = server
        .mock("POST", "/v1/batches")
        .match_body(Matcher::PartialJson(
            json!({"endpoint":"/v1/chat/completions", "input_file_id":"file_legacy"}),
        ))
        .with_body(r#"{"id":"job"}"#)
        .create();
    assert_eq!(batch.submit(&input).unwrap(), "job");
    upload.assert();
    job.assert();
}

fn good() -> Value {
    json!({"custom_id":"bill_1","response":{"status_code":200,"body":{
        "choices":[{"finish_reason":"stop","message":{"content":"{\"value\":23}"}}]
    }}})
}

fn error() -> Value {
    json!({"custom_id":"bad","error":{"code":"rate_limited","message":"retry later"}})
}

#[test]
fn individual_failures_preserve_id_and_raw_and_keep_other_rows() {
    let mut truncated = good();
    truncated["response"]["body"]["choices"][0]["finish_reason"] = "length".into();
    let responses = [
        (truncated, "incomplete_output"),
        (error(), "rate_limited"),
        (json!({"response":{"status_code":404}}), "openai_error"),
        (
            json!({"response":{"body":{"error":{"type":"specific","code":"less_specific"}}}}),
            "specific",
        ),
        (json!({"error":"provider error"}), "openai_error"),
        (json!({"response":null}), "extraction_error"),
    ];
    let batch = client(Provider::OpenAI).batch::<Amount>().unwrap();
    for (mut raw, kind) in responses {
        raw["custom_id"] = "error_id".into();
        let result = batch.parse(&format!("\n{raw}\n{{\n{}", good()));
        assert_eq!(result[0].raw, raw);
        assert_eq!(result[0].custom_id.as_deref(), Some("error_id"));
        assert_eq!(result[0].result.as_ref().unwrap_err().kind, kind);
        assert_eq!(
            result[1].result.as_ref().unwrap_err().kind,
            "json_parse_error"
        );
        assert_eq!(result[1].raw["raw_line"], "{");
        assert_eq!(result[2].result.as_ref().unwrap().value, 23);
    }
    let out = batch.parse_with_context(&good().to_string(), |id| {
        assert_eq!(id, "bill_1");
        json!({"forbid":23})
    });
    assert!(out[0]
        .result
        .as_ref()
        .unwrap_err()
        .message
        .contains("forbidden"));
}

#[test]
fn openai_blocking_submit_and_job_management() {
    let mut server = Server::new();
    let batch = BlockingClient::new(config(Provider::OpenAI, &server.url()))
        .unwrap()
        .batch::<Amount>()
        .unwrap();
    let jsonl = batch.prepare([item()]).unwrap();
    let upload = server
        .mock("POST", "/v1/files")
        .match_header("authorization", "Bearer test-key")
        .match_header("content-type", Matcher::Regex("multipart/form-data".into()))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("batch.jsonl".into()),
            Matcher::Regex("bill_1".into()),
            Matcher::Regex("purpose".into()),
            Matcher::Regex("/v1/responses".into()),
        ]))
        .with_body(r#"{"id":"file_1"}"#)
        .create();
    let job =
        json!({"id":"job","status":"in_progress","request_counts":{"total":2},"created_at":12});
    let submit=server.mock("POST","/v1/batches").match_body(Matcher::Json(json!({
        "input_file_id":"file_1", "endpoint":"/v1/responses","completion_window":"24h","metadata":{"run":"test"}
    }))).with_body(job.to_string()).create();
    let id = batch
        .submit_with_options(
            &jsonl,
            BatchOptions {
                metadata: Some(json!({"run":"test"})),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(id, "job");
    upload.assert();
    submit.assert();
    let status = server
        .mock("GET", "/v1/batches/job")
        .with_body(job.to_string())
        .expect(2)
        .create();
    let row = batch.status(&id).unwrap();
    assert_eq!(row.raw, job);
    assert_eq!(row.request_counts["total"], 2);
    assert!(batch.results(&id).is_err());
    status.assert();
    let list = server
        .mock("GET", "/v1/batches?limit=7")
        .with_body(json!({"data":[job]}).to_string())
        .create();
    assert_eq!(batch.list(7).unwrap()[0].id, id);
    list.assert();
    let cancel = server
        .mock("POST", "/v1/batches/job/cancel")
        .with_body(r#"{"status":"cancelling"}"#)
        .create();
    assert_eq!(batch.cancel(&id).unwrap()["status"], "cancelling");
    cancel.assert();
    assert!(batch.delete(&id).is_err());
    assert!(matches!(
        batch.status("../job"),
        Err(Error::Configuration(_))
    ));
}

#[tokio::test]
async fn openai_terminal_jobs_read_output_and_error_file_once() {
    let mut server = Server::new_async().await;
    let batch = Client::new(config(Provider::OpenAI, &server.url()))
        .unwrap()
        .batch::<Amount>()
        .unwrap();
    let output = server
        .mock("GET", "/v1/files/out/content")
        .with_body(good().to_string())
        .expect(3)
        .create_async()
        .await;
    let errors = server
        .mock("GET", "/v1/files/errs/content")
        .with_body(error().to_string())
        .expect(3)
        .create_async()
        .await;
    for state in ["completed", "cancelled", "expired"] {
        let status=server.mock("GET","/v1/batches/job").with_body(json!({
            "id":"job","status":state,"output_file_id":"out","error_file_id":"errs","request_counts":{"completed":1,"failed":1}
        }).to_string()).create_async().await;
        let rows = batch.results("job").await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].result.as_ref().unwrap().value, 23);
        assert_eq!(rows[1].result.as_ref().unwrap_err().message, "retry later");
        status.assert_async().await;
        status.remove_async().await;
    }
    output.assert_async().await;
    errors.assert_async().await;
}

#[tokio::test]
async fn second_file_failure_leaves_existing_file_and_download_is_recoverable() {
    let mut server = Server::new_async().await;
    let batch = Client::new(config(Provider::OpenAI, &server.url()))
        .unwrap()
        .batch::<Amount>()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("results");
    std::fs::write(&dest, "original").unwrap();
    let status = server
        .mock("GET", "/v1/batches/job")
        .with_body(
            json!({
                "id":"job","status":"completed","output_file_id":"out","error_file_id":"err"
            })
            .to_string(),
        )
        .expect(2)
        .create_async()
        .await;
    let output = server
        .mock("GET", "/v1/files/out/content")
        .with_body(good().to_string())
        .expect(2)
        .create_async()
        .await;
    let bad = server
        .mock("GET", "/v1/files/err/content")
        .with_status(502)
        .with_header("x-request-id", "req_file")
        .with_body("bad upstream")
        .create_async()
        .await;
    let err = batch.download("job", &dest).await.unwrap_err();
    let details = err.http_details().unwrap();
    assert_eq!(details.status, 502);
    assert_eq!(details.request_id.as_deref(), Some("req_file"));
    assert_eq!(details.code, None);
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "original");
    bad.assert_async().await;
    bad.remove_async().await;
    let errors = server
        .mock("GET", "/v1/files/err/content")
        .with_body(error().to_string())
        .create_async()
        .await;
    batch.download("job", &dest).await.unwrap();
    assert_eq!(
        lines(&std::fs::read_to_string(&dest).unwrap()),
        vec![good(), error()]
    );
    errors.assert_async().await;
    status.assert_async().await;
    output.assert_async().await;
}

fn anth_success(reason: &str, blocks: Value) -> Value {
    json!({"custom_id":"ok","result":{"type":"succeeded","message":{"stop_reason":reason,"content":blocks}}})
}

#[tokio::test]
async fn anthropic_inline_requests_and_result_variants() {
    let mut server = Server::new_async().await;
    let batch = Client::new(config(Provider::Anthropic, &server.url()))
        .unwrap()
        .batch::<Amount>()
        .unwrap();
    let input = batch.prepare([item()]).unwrap();
    let submit = server
        .mock("POST", "/v1/messages/batches")
        .match_header("anthropic-version", "2026-10-21")
        .match_header("x-api-key", "test-key")
        .match_body(Matcher::Json(json!({"requests":lines(&input)})))
        .with_body(r#"{"id":"job"}"#)
        .create_async()
        .await;
    let id = batch.submit(&input).await.unwrap();
    let status = server
        .mock("GET", "/v1/messages/batches/job")
        .with_body(r#"{"id":"job","processing_status":"ended"}"#)
        .expect(2)
        .create_async()
        .await;
    submit.assert_async().await;
    let tool = json!([{"type":"tool_use","name":"extract_data","input":{"value":23}}]);
    let text = json!([{"type":"text","text":"prose"},{"type":"text","text":"{\"value\":2}"}]);
    let err = json!({"custom_id":"bad","result":{"type":"errored","error":{"type":"error","error":{"type":"invalid_request","message":"unknown input"}}}});
    let merged = [
        anth_success("tool_use", tool.clone()),
        anth_success("end_turn", text),
        anth_success("max_tokens", tool),
        err.clone(),
    ]
    .iter()
    .map(ToString::to_string)
    .collect::<Vec<_>>()
    .join("\n");
    let raw = server
        .mock("GET", "/v1/messages/batches/job/results")
        .with_body(merged)
        .create_async()
        .await;
    let rows = batch.results(&id).await.unwrap();
    assert_eq!(rows[0].result.as_ref().unwrap().value, 23);
    assert_eq!(rows[1].result.as_ref().unwrap().value, 2);
    assert_eq!(
        rows[2].result.as_ref().unwrap_err().kind,
        "incomplete_output"
    );
    assert_eq!(rows[3].result.as_ref().unwrap_err().kind, "invalid_request");
    raw.assert_async().await;
    raw.remove_async().await;
    let failed = server
        .mock("GET", "/v1/messages/batches/job/results")
        .with_body(err.to_string())
        .create_async()
        .await;
    let rows = batch.results(&id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].result.as_ref().unwrap_err().message,
        "unknown input"
    );
    failed.assert_async().await;
    status.assert_async().await;
}

#[test]
fn mistral_file_jobs_and_deletion() {
    let mut server = Server::new();
    let batch = BlockingClient::new(config(Provider::Mistral, &server.url()))
        .unwrap()
        .batch::<Amount>()
        .unwrap();
    let upload = server
        .mock("POST", "/v1/files")
        .with_body(r#"{"id":"file"}"#)
        .create();
    let submit = server
        .mock("POST", "/v1/batch/jobs")
        .match_body(Matcher::Json(json!({
            "endpoint":"/v1/chat/completions","model":"test","input_files":["file"],"metadata":{}
        })))
        .with_body(r#"{"id":"job"}"#)
        .create();
    let id = batch.submit(&batch.prepare([item()]).unwrap()).unwrap();
    upload.assert();
    submit.assert();
    let status = server
        .mock("GET", "/v1/batch/jobs/job")
        .with_body(r#"{"id":"job","status":"SUCCESS","output_file":"out","succeeded_requests":1}"#)
        .create();
    let output = server
        .mock("GET", "/v1/files/out/content")
        .with_body(good().to_string())
        .create();
    assert_eq!(
        batch.results(&id).unwrap()[0]
            .result
            .as_ref()
            .unwrap()
            .value,
        23
    );
    status.assert();
    output.assert();
    let delete = server
        .mock("DELETE", "/v1/batch/jobs/job")
        .with_status(204)
        .create();
    assert_eq!(batch.delete(&id).unwrap(), Value::Null);
    delete.assert();
}

#[test]
fn reader_matches_string_parser_for_each_provider() {
    use std::io::{BufReader, Seek, Write};

    for provider in [Provider::OpenAI, Provider::Anthropic, Provider::Mistral] {
        let batch = client(provider).batch::<Amount>().unwrap();
        let (ok, bad) = if provider == Provider::Anthropic {
            (
                json!({"custom_id":"bill_1","result":{"type":"succeeded","message":{
                    "content":[{"type":"tool_use","input":{"value":23}}]
                }}}),
                json!({"custom_id":"bad","result":{"type":"errored","error":{
                    "type":"rate_limited","message":"retry later"
                }}}),
            )
        } else {
            (good(), error())
        };
        let lines = format!("\n \t\r\n{ok}\r\n{bad}\n{{incomplete☂\r\n{ok}");
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(lines.as_bytes()).unwrap();
        file.rewind().unwrap();
        let mut records = batch.parse_reader(BufReader::with_capacity(2, file));
        for expected in batch.parse(&lines) {
            let actual = records.next().unwrap().unwrap();
            assert_eq!(actual.raw, expected.raw);
            assert_eq!(actual.custom_id, expected.custom_id);
            let value = |result: Result<Amount, instructor_ai::batch::BatchFailure>| {
                result.map(|a| a.value).map_err(|e| (e.kind, e.message))
            };
            assert_eq!(value(actual.result), value(expected.result));
        }
        assert!(records.next().is_none());
    }
}

#[test]
fn blocking_reader_defers_context_and_recovers_after_invalid_bytes() {
    use std::cell::Cell;

    let batch = BlockingClient::new(config(Provider::Mistral, "https://example.test"))
        .unwrap()
        .batch::<Amount>()
        .unwrap();
    let count = Cell::new(0);
    let mut second = good();
    second["custom_id"] = json!("bill_2");
    let mut input = b"\xff\r\n".to_vec();
    input.extend_from_slice(format!("{}\n{second}", good()).as_bytes());
    let mut records = batch.parse_reader_with_context(input.as_slice(), |id| {
        count.set(count.get() + 1);
        if id == "bill_1" {
            json!({"forbid":23})
        } else {
            Value::Null
        }
    });
    assert_eq!(count.get(), 0);
    let invalid = records.next().unwrap().unwrap();
    assert_eq!(invalid.raw["raw_line"], "\u{fffd}");
    assert_eq!(invalid.result.unwrap_err().kind, "json_parse_error");
    assert_eq!(count.get(), 0);
    let rejected = records.next().unwrap().unwrap();
    assert_eq!(rejected.custom_id.as_deref(), Some("bill_1"));
    assert!(rejected
        .result
        .unwrap_err()
        .message
        .contains("forbidden amount"));
    assert_eq!(count.get(), 1);
    let accepted = records.next().unwrap().unwrap();
    assert_eq!(accepted.custom_id.as_deref(), Some("bill_2"));
    assert_eq!(accepted.result.unwrap().value, 23);
    assert_eq!(count.get(), 2);
    assert!(records.next().is_none());
    assert!(batch.parse_reader(&b""[..]).next().is_none());
}

#[test]
fn reader_returns_io_error_once_after_complete_rows() {
    use std::cell::Cell;
    use std::io::{self, BufReader, Read};

    struct Broken<'a>(&'a Cell<usize>);
    impl Read for Broken<'_> {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.0.set(self.0.get() + 1);
            Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "read failed",
            ))
        }
    }

    let batch = client(Provider::OpenAI).batch::<Amount>().unwrap();
    let reads = Cell::new(0);
    let first = format!("{}\n{{", good());
    let input = first.as_bytes().chain(Broken(&reads));
    let mut records = batch.parse_reader(BufReader::new(input));
    assert_eq!(reads.get(), 0);
    assert_eq!(records.next().unwrap().unwrap().result.unwrap().value, 23);
    assert_eq!(reads.get(), 0);
    let failure = records.next().unwrap().unwrap_err();
    assert_eq!(failure.kind(), io::ErrorKind::ConnectionReset);
    assert_eq!(failure.to_string(), "read failed");
    assert!(records.next().is_none());
    assert!(records.next().is_none());
    assert_eq!(reads.get(), 1);
}
