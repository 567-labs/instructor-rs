use super::{config, Amount, BlockingClient, Client, Error, Provider, Server};
use std::{
    fs, io,
    sync::{mpsc, Mutex},
    time::Duration,
};

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
enum End {
    Success,
    Disconnect,
    Cancel,
}

async fn gated_transfer(end: End) {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("result.jsonl");
    fs::write(&dest, "original").unwrap();
    let mut server = Server::new_async().await;
    let client = Client::new(config(Provider::Anthropic, &server.url())).unwrap();
    let batch = client.batch::<Amount>().unwrap();
    let status = server
        .mock("GET", "/v1/messages/batches/job")
        .with_body(r#"{"id":"job","processing_status":"ended"}"#)
        .create_async()
        .await;
    let (release, wait) = mpsc::channel();
    let wait = Mutex::new(wait);
    let prefix = "x".repeat(65535);
    let expected = format!("{prefix}🦀\r\n");
    let bytes = expected.clone().into_bytes();
    let results = server
        .mock("GET", "/v1/messages/batches/job/results")
        .match_header("x-api-key", "test-key")
        .with_chunked_body(move |writer| {
            // Split the crab across both a wire chunk and a 64 KiB write block.
            writer.write_all(&bytes[..65536])?;
            wait.lock()
                .unwrap()
                .recv_timeout(TIMEOUT)
                .map_err(io::Error::other)?;
            if matches!(end, End::Disconnect) {
                return Err(io::Error::other("connection interrupted"));
            }
            writer.write_all(&bytes[65536..])
        })
        .create_async()
        .await;
    let path = dest.clone();
    let task = tokio::spawn(async move { batch.download("job", path).await });

    tokio::time::timeout(TIMEOUT, async {
        loop {
            let partial = fs::read_dir(dir.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path != &dest);
            if let Some(path) = partial {
                if fs::metadata(&path).unwrap().len() >= prefix.len() as u64 {
                    assert!(fs::read(&path).unwrap().starts_with(prefix.as_bytes()));
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the first block must reach disk before HTTP EOF");
    assert_eq!(fs::read_to_string(&dest).unwrap(), "original");

    match end {
        End::Success => {
            release.send(()).unwrap();
            task.await.unwrap().unwrap();
            assert_eq!(fs::read_to_string(&dest).unwrap(), expected);
        }
        End::Disconnect => {
            release.send(()).unwrap();
            assert!(matches!(task.await.unwrap(), Err(Error::Transport(_))));
            assert_eq!(fs::read_to_string(&dest).unwrap(), "original");
        }
        End::Cancel => {
            // Complete a second download to the same destination during the first.
            let _new_status = server
                .mock("GET", "/v1/messages/batches/new")
                .with_body(r#"{"id":"new","processing_status":"ended"}"#)
                .create_async()
                .await;
            let _new_results = server
                .mock("GET", "/v1/messages/batches/new/results")
                .with_body("new")
                .create_async()
                .await;
            client
                .batch::<Amount>()
                .unwrap()
                .download("new", &dest)
                .await
                .unwrap();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            release.send(()).unwrap();
            assert_eq!(fs::read_to_string(&dest).unwrap(), "new");
        }
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    status.assert_async().await;
    results.assert_async().await;
}

#[tokio::test]
async fn writes_output_before_eof_and_preserves_split_utf8() {
    gated_transfer(End::Success).await;
}

#[tokio::test]
async fn interrupted_body_preserves_existing_file() {
    gated_transfer(End::Disconnect).await;
}

#[tokio::test]
async fn cancelled_transfer_cannot_overwrite_a_concurrent_download() {
    gated_transfer(End::Cancel).await;
}

#[test]
fn openai_output_and_error_files_keep_their_exact_bytes() {
    let mut server = Server::new();
    let batch = BlockingClient::new(config(Provider::OpenAI, &server.url()))
        .unwrap()
        .batch::<Amount>()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("result.jsonl");
    let cases = [
        ("one", "two", "one\ntwo"),
        ("one\r\n\n", "two\n", "one\r\n\ntwo\n"),
        ("", "two", "two"),
        ("one", "", "one\n"),
    ];
    let status = server
        .mock("GET", "/v1/batches/job")
        .with_body(
            r#"{"id":"job","status":"completed","output_file_id":"out","error_file_id":"err"}"#,
        )
        .expect(cases.len() * 2)
        .create();
    for (out, err, merged) in cases {
        let one = server
            .mock("GET", "/v1/files/out/content")
            .with_body(out)
            .expect(2)
            .create();
        let two = server
            .mock("GET", "/v1/files/err/content")
            .with_body(err)
            .expect(2)
            .create();
        assert_eq!(batch.results_raw("job").unwrap(), merged);
        batch.download("job", &dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), merged);
        one.assert();
        two.assert();
        one.remove();
        two.remove();
    }
    status.assert();
}

#[test]
fn mistral_invalid_utf8_fails_without_publishing_and_retry_preserves_permissions() {
    let mut server = Server::new();
    let batch = BlockingClient::new(config(Provider::Mistral, &server.url()))
        .unwrap()
        .batch::<Amount>()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("result.jsonl");
    fs::write(&dest, "original").unwrap();
    let permissions = fs::metadata(&dest).unwrap().permissions();
    let status = server
        .mock("GET", "/v1/batch/jobs/job")
        .with_body(r#"{"id":"job","status":"SUCCESS","output_file":"out"}"#)
        .expect(3)
        .create();
    for bad in [&[0xF0, 0x9F][..], &[0xFF]] {
        let body = server
            .mock("GET", "/v1/files/out/content")
            .with_body(bad)
            .create();
        assert!(
            matches!(batch.download("job",&dest),Err(Error::Protocol(s)) if s.contains("UTF-8"))
        );
        assert_eq!(fs::read_to_string(&dest).unwrap(), "original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        body.assert();
        body.remove();
    }
    let body = server
        .mock("GET", "/v1/files/out/content")
        .with_body("résultats\n")
        .create();
    batch.download("job", &dest).unwrap();
    assert_eq!(fs::read_to_string(&dest).unwrap(), "résultats\n");
    assert_eq!(fs::metadata(&dest).unwrap().permissions(), permissions);
    status.assert();
    body.assert();
}
