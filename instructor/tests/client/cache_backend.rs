use super::{config, response, Amount};
use instructor_ai::{BlockingClient, Client, Event, Mode, Provider, ProviderConfig, Request};
use mockito::Server;
use serde_json::json;
use std::{
    sync::Arc,
    thread,
    time::{Duration, SystemTime},
};

// --8<-- [start:adapter]
use instructor_ai::{CacheRecord, ResponseCache};
use std::{collections::HashMap, io, sync::Mutex};

type Entries = HashMap<String, Vec<u8>>;

#[derive(Default)]
struct JsonStore(Mutex<HashMap<String, Entries>>);

impl ResponseCache for JsonStore {
    fn get(&self, scope: &str, key: &str) -> io::Result<Option<CacheRecord>> {
        let store = self.0.lock().unwrap();
        match store.get(scope).and_then(|entries| entries.get(key)) {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    fn put(&self, scope: &str, key: &str, record: CacheRecord) -> io::Result<()> {
        let bytes = serde_json::to_vec(&record)?;
        let mut store = self.0.lock().unwrap();
        store
            .entry(scope.into())
            .or_default()
            .insert(key.into(), bytes);
        Ok(())
    }

    fn clear(&self, scope: &str) -> io::Result<()> {
        self.0.lock().unwrap().remove(scope);
        Ok(())
    }
}
// --8<-- [end:adapter]

#[test]
fn shared_backend_uses_disk_identity_and_record_across_client_styles() {
    let mut server = Server::new();
    let raw = response(r#"{"value":3}"#, Some(17));
    let mock = server
        .mock("POST", "/chat/completions")
        .expect(2)
        .with_body(raw.to_string())
        .create();
    let store = Arc::new(JsonStore::default());
    let client = BlockingClient::new(config(&server))
        .unwrap()
        .with_cache_backend(store.clone(), "receipts")
        .unwrap();
    let request = Request::new("receipt");
    let ttl = Duration::from_secs(60);
    let before = SystemTime::now();
    client
        .create::<Amount>(request.clone().cache_ttl(ttl))
        .unwrap();
    let after = SystemTime::now();
    let dir = tempfile::tempdir().unwrap();
    BlockingClient::new(config(&server))
        .unwrap()
        .with_disk_cache(dir.path(), "receipts")
        .unwrap()
        .create::<Amount>(request.clone())
        .unwrap();
    {
        let items = store.0.lock().unwrap();
        let (scope, fields) = items.iter().next().unwrap();
        let (key, bytes) = fields.iter().next().unwrap();
        let stored: CacheRecord = serde_json::from_slice(bytes).unwrap();
        assert!((before + ttl..=after + ttl).contains(&stored.expires.unwrap()));
        assert_eq!(stored.raw, raw);
        let file = dir.path().join(format!("instructor-{scope}/{key}.json"));
        let disk: CacheRecord = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        assert_eq!(disk.raw, raw);
        assert_eq!(disk.expires, None);
    }
    let client = Client::new(config(&server))
        .unwrap()
        .with_cache_backend(store, "receipts")
        .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let clone = client.clone();
        let (left, right) = tokio::join!(
            client.create_with_metadata::<Amount>(request.clone()),
            clone.create_with_metadata::<Amount>(request)
        );
        for hit in [left.unwrap(), right.unwrap()] {
            assert_eq!(hit.attempts, 0);
            assert_eq!(hit.total_tokens, Some(0));
            assert_eq!(hit.raw, raw);
            assert_eq!(hit.value.value, 3);
        }
    });
    mock.assert();
}

#[test]
fn backend_separates_namespace_credential_and_context_and_clears_one_scope() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/chat/completions")
        .expect(5)
        .with_body(response(r#"{"value":3}"#, None).to_string())
        .create();
    let store = Arc::new(JsonStore::default());
    let client = |credential, namespace: &str| {
        let config = ProviderConfig::new(Provider::OpenAI, "test", credential)
            .with_mode(Mode::JsonSchema)
            .with_base_url(server.url());
        BlockingClient::new(config)
            .unwrap()
            .with_cache_backend(store.clone(), namespace)
    };
    assert!(client("x", " ").is_err());
    let a = client("x", "a").unwrap();
    let b = client("x", "b").unwrap();
    let other = client("y", "a").unwrap();
    let attempts = |client: &BlockingClient, min: u32| {
        let request = Request::new("value").context(json!({"min":min}));
        client
            .create_with_metadata::<Amount>(request)
            .unwrap()
            .attempts
    };
    for client in [&a, &b, &other] {
        assert_eq!(attempts(client, 1), 1);
    }
    assert_eq!(attempts(&a, 2), 1);
    a.clear_cache().unwrap();
    assert_eq!(attempts(&a, 2), 1);
    assert_eq!(attempts(&b, 1), 0);
    assert_eq!(attempts(&other, 1), 0);
    mock.assert();
}

#[test]
fn backend_bypasses_zero_ttl_and_replaces_expired_and_rejected_records() {
    let mut server = Server::new();
    let mocks: Vec<_> = (1..=4)
        .map(|i| {
            server
                .mock("POST", "/chat/completions")
                .with_body(response(&json!({"value":i}).to_string(), None).to_string())
                .create()
        })
        .collect();
    let store = Arc::new(JsonStore::default());
    let client = BlockingClient::new(config(&server))
        .unwrap()
        .with_cache_backend(store.clone(), "receipts")
        .unwrap();
    let req = Request::new("query");
    let value = |req| client.create::<Amount>(req).unwrap().value;
    assert_eq!(value(req.clone().cache_ttl(Duration::from_nanos(1))), 1);
    assert_eq!(value(req.clone()), 2);
    assert_eq!(value(req.clone().cache_ttl(Duration::ZERO)), 3);
    assert_eq!(value(req.clone()), 2);
    let validated = client
        .create_validated::<Amount>(req.clone(), |amount| {
            if amount.value < 4 {
                Err("invalid".into())
            } else {
                Ok(Amount { value: 5 })
            }
        })
        .unwrap();
    assert_eq!(validated.value, 5);
    assert_eq!(value(req), 4);
    for mock in mocks {
        mock.assert();
    }
}

struct Unavailable(thread::ThreadId);

impl ResponseCache for Unavailable {
    fn get(&self, _: &str, _: &str) -> io::Result<Option<CacheRecord>> {
        assert_ne!(self.0, thread::current().id());
        Err(io::Error::other("read"))
    }
    fn put(&self, _: &str, _: &str, _: CacheRecord) -> io::Result<()> {
        assert_ne!(self.0, thread::current().id());
        Err(io::Error::other("write"))
    }
    fn clear(&self, _: &str) -> io::Result<()> {
        assert_ne!(self.0, thread::current().id());
        Err(io::Error::other("clear"))
    }
}

#[tokio::test]
async fn backend_operations_leave_executor_and_report_storage_errors() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/chat/completions")
        .with_body(response(r#"{"value":4}"#, None).to_string())
        .create_async()
        .await;
    let errors = Arc::new(Mutex::new(Vec::new()));
    let log = errors.clone();
    let backend = Arc::new(Unavailable(thread::current().id()));
    let client = Client::new(config(&server))
        .unwrap()
        .with_cache_backend(backend, "receipts")
        .unwrap()
        .on_event(move |n, e| {
            if let Event::CacheError(error) = e {
                log.lock().unwrap().push((n, error.to_string()));
            }
        });
    assert_eq!(client.create::<Amount>("query").await.unwrap().value, 4);
    assert_eq!(
        *errors.lock().unwrap(),
        [(0, "read".into()), (1, "write".into())]
    );
    assert_eq!(
        client.clear_cache_async().await.unwrap_err().to_string(),
        "clear"
    );
    mock.assert_async().await;
}
