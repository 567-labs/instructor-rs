//! Opt-in per-client memory caching and namespaced persistent response caching.
use crate::{Error, ProviderConfig, Request};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    expires: Option<SystemTime>,
    raw: Value,
}

#[derive(Clone)]
pub(crate) struct Cache(Arc<Store>);

enum Store {
    Memory(Mutex<LruCache<String, Record>>),
    Disk(PathBuf),
}

// Hash a structured tuple to avoid ambiguous boundaries. Request bodies and
// credentials never appear in filenames.
fn digest(value: impl Serialize) -> String {
    let mut hash = Sha256::new();
    serde_json::to_writer(&mut hash, &value)
        .expect("cache identities consist only of strings and JSON values");
    format!("{:x}", hash.finalize())
}

impl Cache {
    pub fn memory(size: usize) -> Option<Self> {
        NonZeroUsize::new(size)
            .map(|size| Self(Arc::new(Store::Memory(Mutex::new(LruCache::new(size))))))
    }

    pub fn disk(config: &ProviderConfig, dir: &Path, namespace: &str) -> Result<Self, Error> {
        if namespace.trim().is_empty() {
            return Err(Error::Configuration(
                "persistent cache requires a nonempty namespace".into(),
            ));
        }
        let key = digest(("v1", namespace, config.cache_identity()));
        let dir = dir.join(format!("instructor-{key}"));
        fs::create_dir_all(&dir)
            .and_then(|_| fs::canonicalize(dir))
            .map(|dir| Self(Arc::new(Store::Disk(dir))))
            .map_err(|e| Error::Configuration(format!("cache directory: {e}")))
    }

    pub fn key<T>(req: &Request, body: &Value) -> String {
        digest((std::any::type_name::<T>(), body, &req.context))
    }

    pub fn clear(&self) -> io::Result<()> {
        match &*self.0 {
            Store::Memory(cache) => cache.lock().unwrap().clear(),
            Store::Disk(dir) => {
                for entry in fs::read_dir(dir)? {
                    let path = entry?.path();
                    if path.extension().is_some_and(|ext| ext == "json")
                        && path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .is_some_and(|key| {
                                key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
                            })
                    {
                        remove_file(&path)?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn access<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Store) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let store = self.0.clone();
        match &*store {
            Store::Memory(_) => f(&store),
            Store::Disk(_) => tokio::task::spawn_blocking(move || f(&store))
                .await
                .map_err(io::Error::other)?,
        }
    }

    pub async fn get(&self, key: &str) -> io::Result<Option<Value>> {
        let key = key.to_string();
        self.access(move |store| {
            let record: Option<Record> = match store {
                Store::Memory(cache) => cache.lock().unwrap().get(&key).cloned(),
                Store::Disk(dir) => match fs::read(dir.join(format!("{key}.json"))) {
                    Ok(bytes) => Some(serde_json::from_slice(&bytes)?),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e),
                },
            };
            Ok(record
                .filter(|r| r.expires.is_none_or(|time| time > SystemTime::now()))
                .map(|r| r.raw))
        })
        .await
    }

    pub async fn put(&self, key: String, raw: Value, ttl: Option<Duration>) -> io::Result<()> {
        let expires = ttl
            .map(|t| {
                SystemTime::now()
                    .checked_add(t)
                    .ok_or_else(|| io::Error::other("cache TTL exceeds clock range"))
            })
            .transpose()?;
        let record = Record { expires, raw };
        self.access(move |store| {
            match store {
                Store::Memory(cache) => {
                    cache.lock().unwrap().put(key, record);
                }
                Store::Disk(dir) => {
                    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
                    serde_json::to_writer(&mut tmp, &record)?;
                    tmp.persist(dir.join(format!("{key}.json")))?;
                }
            }
            Ok(())
        })
        .await
    }

    pub fn reject(&self, key: &str, raw: Value) {
        if let Store::Memory(cache) = &*self.0 {
            let mut cache = cache.lock().unwrap();
            if cache.peek(key).is_some_and(|r| r.raw == raw) {
                cache.pop(key);
            }
        }
        // Disk writers can run in other processes. A fresh successful
        // validation replaces the old file atomically.
    }
}

fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
