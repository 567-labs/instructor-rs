use std::{
    future::{ready, Future},
    path::Path,
    sync::Arc,
    time::Duration,
};

use crate::cache::Cache;
use serde_json::Value;
use tokio::runtime::Runtime;

use crate::{
    markdown,
    providers::WireRequest,
    request::Hook,
    retry::Attempts,
    schema::{schema, Schema},
    Error, Event, Mode, ProviderConfig, Request, ResponseModel,
};

/// A typed result, original provider response and the cost of producing it
/// during this invocation. Historical usage on a cache hit is in `raw`.
#[derive(Debug)]
pub struct Completion<T> {
    pub value: T,
    pub raw: Value,
    /// Number of HTTP attempts. A cache hit makes zero requests.
    pub attempts: u64,
    /// Sum across attempts, if each returned usable token counts.
    pub total_tokens: Option<u64>,
}

impl<T> Completion<T> {
    pub fn into_pair(self) -> (T, Value) {
        (self.value, self.raw)
    }
}

/// Handle used to remove a client-level callback.
#[derive(Clone)]
pub struct HookHandle(std::sync::Weak<dyn Fn(u64, Event<'_>) + Send + Sync>);

/// Asynchronous client. Clones share a connection pool and optional response cache.
#[derive(Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
    pub(crate) config: ProviderConfig,
    hooks: Vec<Hook>,
    cache: Option<Cache>,
}

/// Create an asynchronous client using a provider/model identifier and environment credentials.
pub fn from_provider(spec: &str) -> Result<Client, Error> {
    Client::from_provider(spec)
}

impl Client {
    pub fn new(config: ProviderConfig) -> Result<Self, Error> {
        Self::with_timeout(config, Duration::from_secs(180))
    }

    /// Configure the timeout for each HTTP attempt including its response body.
    pub fn with_timeout(config: ProviderConfig, timeout: Duration) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()?;
        Self::with_http_client(config, http)
    }

    /// Reuse a configured HTTP client, including its connection pool, headers,
    /// timeout and redirect policy. For disk cache sharing, use a namespace
    /// specific to any additional credentials or tenant headers.
    pub fn with_http_client(config: ProviderConfig, http: reqwest::Client) -> Result<Self, Error> {
        config.validate()?;
        Ok(Self {
            config,
            http,
            hooks: Vec::new(),
            cache: None,
        })
    }

    pub fn from_provider(spec: &str) -> Result<Self, Error> {
        Self::new(ProviderConfig::from_env(spec)?)
    }

    /// Observe every structured attempt. The callback runs inline.
    pub fn on_event(mut self, hook: impl Fn(u64, Event<'_>) + Send + Sync + 'static) -> Self {
        self.hooks.push(Arc::new(hook));
        self
    }

    /// Register a removable client callback. Clones inherit their own callback
    /// lists, so removal affects subsequent calls through this client.
    pub fn add_hook(
        &mut self,
        callback: impl Fn(u64, Event<'_>) + Send + Sync + 'static,
    ) -> HookHandle {
        let hook: Hook = Arc::new(callback);
        let handle = HookHandle(Arc::downgrade(&hook));
        self.hooks.push(hook);
        handle
    }

    pub fn remove_hook(&mut self, handle: &HookHandle) -> bool {
        let len = self.hooks.len();
        self.hooks
            .retain(|hook| !handle.0.ptr_eq(&Arc::downgrade(hook)));
        len != self.hooks.len()
    }

    pub fn clear_hooks(&mut self) {
        self.hooks.clear();
    }

    /// Cache successful non-streaming completions in memory. Zero disables caching.
    pub fn with_cache(mut self, capacity: usize) -> Self {
        self.cache = Cache::memory(capacity);
        self
    }

    /// Persist raw JSON responses. Sharing requires matching namespace, endpoint,
    /// credentials, model, mode, response type, request and validation context.
    pub fn with_disk_cache(
        mut self,
        directory: impl AsRef<Path>,
        namespace: &str,
    ) -> Result<Self, Error> {
        self.cache = Some(Cache::disk(&self.config, directory.as_ref(), namespace)?);
        Ok(self)
    }

    /// Clear this client's cache namespace. Disk cleanup is synchronous.
    pub fn clear_cache(&self) -> std::io::Result<()> {
        self.cache.as_ref().map_or(Ok(()), Cache::clear)
    }

    pub(crate) fn emit(&self, request: &Request, attempt: u64, event: Event<'_>) {
        for hook in self.hooks.iter().chain(&request.hooks) {
            hook(attempt, event);
        }
    }

    pub async fn create<T: ResponseModel>(&self, request: impl Into<Request>) -> Result<T, Error> {
        self.create_with_completion(request)
            .await
            .map(|(value, _)| value)
    }

    /// Return the validated value and its original successful provider response.
    pub async fn create_with_completion<T: ResponseModel>(
        &self,
        request: impl Into<Request>,
    ) -> Result<(T, Value), Error> {
        self.create_with_metadata(request)
            .await
            .map(Completion::into_pair)
    }

    /// Return the full result including the number of attempts and cumulative usage.
    pub async fn create_with_metadata<T: ResponseModel>(
        &self,
        request: impl Into<Request>,
    ) -> Result<Completion<T>, Error> {
        self.complete(request.into(), |value| ready(Ok(value)))
            .await
    }

    /// Run an additional asynchronous check after model validation, inside the retry loop.
    pub async fn create_validated<T, F, Fut>(
        &self,
        request: impl Into<Request>,
        validate: F,
    ) -> Result<T, Error>
    where
        T: ResponseModel,
        F: Fn(T) -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        self.complete(request.into(), validate)
            .await
            .map(|c| c.value)
    }

    /// Send a request with no response schema and return the native JSON envelope.
    pub async fn create_raw(&self, request: impl Into<Request>) -> Result<Value, Error> {
        let req = request.into().prepare(false)?;
        let wire = self.config.request(&req, None, false)?;
        self.send_json(&req, &wire, 1)
            .await
            .inspect_err(|err| self.emit(&req, 1, Event::CompletionError(err)))
    }

    async fn send_json(
        &self,
        req: &Request,
        wire: &WireRequest,
        attempt: u64,
    ) -> Result<Value, Error> {
        self.emit(req, attempt, Event::Request(&wire.body));
        let raw = wire.send(&self.http).await?.json::<Value>().await?;
        if let Some(error) = raw.get("error").filter(|error| !error.is_null()) {
            return Err(Error::Provider(error.to_string()));
        }
        self.emit(req, attempt, Event::Response(&raw));
        Ok(raw)
    }

    pub(crate) fn parse<T: ResponseModel>(
        &self,
        raw: &Value,
        schema: &Schema,
        context: &Value,
    ) -> Result<T, Error> {
        let payload = self.config.extract(raw, &schema.value)?;
        let text = if self.config.mode() == Mode::MarkdownJson {
            markdown::extract_markdown_json(&payload)?
        } else {
            &payload
        };
        schema.parse(text, context)
    }

    async fn complete<T, F, Fut>(
        &self,
        request: Request,
        validate: F,
    ) -> Result<Completion<T>, Error>
    where
        T: ResponseModel,
        F: Fn(T) -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        let req = request.prepare(true)?;
        let schema = schema::<T>();
        let wire = self.config.request(&req, Some(&schema.value), false)?;
        self.complete_with(
            &req,
            wire,
            |raw| self.parse(raw, &schema, &req.context),
            validate,
        )
        .await
    }

    pub(crate) async fn complete_with<T: 'static, Fut>(
        &self,
        req: &Request,
        mut wire: WireRequest,
        parse: impl Fn(&Value) -> Result<T, Error>,
        validate: impl Fn(T) -> Fut,
    ) -> Result<Completion<T>, Error>
    where
        Fut: Future<Output = Result<T, String>>,
    {
        let cache_entry = self
            .cache
            .as_ref()
            .filter(|_| req.cache_ttl != Some(Duration::ZERO))
            .map(|cache| (cache, Cache::key::<T>(req, &wire.body)));
        if let Some((cache, key)) = &cache_entry {
            let cached = cache.get(key).await.unwrap_or_else(|e| {
                self.emit(req, 0, Event::CacheError(&e));
                None
            });
            if let Some(raw) = cached {
                if let Ok(model) = parse(&raw) {
                    if let Ok(model) = validate(model).await {
                        self.emit(req, 0, Event::CacheHit);
                        return Ok(Completion {
                            value: model,
                            raw,
                            attempts: 0,
                            total_tokens: Some(0),
                        });
                    }
                }
                cache.reject(key, raw);
            }
        }

        let mut attempts = Attempts::new();
        loop {
            let attempt = attempts.next();
            let raw = match self.send_json(req, &wire, attempt).await {
                Ok(raw) => raw,
                Err(error) => {
                    self.reject(req, &mut wire, &mut attempts, Value::Null, error)
                        .await?;
                    continue;
                }
            };
            let total_tokens = attempts.usage(self.config.usage(&raw));
            if let Some(total_tokens) = total_tokens {
                self.emit(req, attempt, Event::Usage { total_tokens });
            }
            let parsed = match parse(&raw) {
                Ok(model) => validate(model).await.map_err(Error::Validation),
                Err(e) => Err(e),
            };
            match parsed {
                Ok(value) => {
                    if let Some((cache, key)) = cache_entry {
                        if let Err(e) = cache.put(key, raw.clone(), req.cache_ttl).await {
                            self.emit(req, attempt, Event::CacheError(&e));
                        }
                    }
                    return Ok(Completion {
                        value,
                        raw,
                        attempts: attempt,
                        total_tokens,
                    });
                }
                Err(error) => {
                    self.reject(req, &mut wire, &mut attempts, raw, error)
                        .await?
                }
            }
        }
    }
}

/// Synchronous entry point for callers outside an async runtime.
#[derive(Clone)]
pub struct BlockingClient {
    pub(crate) client: Client,
    pub(crate) runtime: Arc<Runtime>,
}

impl BlockingClient {
    pub fn new(config: ProviderConfig) -> Result<Self, Error> {
        Self::from_client(Client::new(config)?)
    }

    pub fn from_provider(spec: &str) -> Result<Self, Error> {
        Self::from_client(Client::from_provider(spec)?)
    }

    pub fn from_client(client: Client) -> Result<Self, Error> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| Error::Configuration(e.to_string()))?;
        Ok(Self {
            client,
            runtime: Arc::new(runtime),
        })
    }

    pub fn on_event(mut self, hook: impl Fn(u64, Event<'_>) + Send + Sync + 'static) -> Self {
        self.client = self.client.on_event(hook);
        self
    }

    pub fn add_hook(
        &mut self,
        callback: impl Fn(u64, Event<'_>) + Send + Sync + 'static,
    ) -> HookHandle {
        self.client.add_hook(callback)
    }

    pub fn remove_hook(&mut self, handle: &HookHandle) -> bool {
        self.client.remove_hook(handle)
    }

    pub fn clear_hooks(&mut self) {
        self.client.clear_hooks()
    }

    pub fn with_cache(mut self, capacity: usize) -> Self {
        self.client = self.client.with_cache(capacity);
        self
    }

    pub fn with_disk_cache(
        mut self,
        directory: impl AsRef<Path>,
        namespace: &str,
    ) -> Result<Self, Error> {
        self.client = self.client.with_disk_cache(directory, namespace)?;
        Ok(self)
    }

    pub fn clear_cache(&self) -> std::io::Result<()> {
        self.client.clear_cache()
    }

    pub fn create<T: ResponseModel>(&self, request: impl Into<Request>) -> Result<T, Error> {
        self.runtime.block_on(self.client.create(request))
    }

    pub fn create_with_completion<T: ResponseModel>(
        &self,
        request: impl Into<Request>,
    ) -> Result<(T, Value), Error> {
        self.runtime
            .block_on(self.client.create_with_completion(request))
    }

    pub fn create_with_metadata<T: ResponseModel>(
        &self,
        request: impl Into<Request>,
    ) -> Result<Completion<T>, Error> {
        self.runtime
            .block_on(self.client.create_with_metadata(request))
    }

    pub fn create_validated<T: ResponseModel>(
        &self,
        request: impl Into<Request>,
        validate: impl Fn(T) -> Result<T, String>,
    ) -> Result<T, Error> {
        self.runtime
            .block_on(self.client.create_validated(request, |value| {
                // Permit synchronous checks that call a BlockingClient themselves.
                ready(tokio::task::block_in_place(|| validate(value)))
            }))
    }

    pub fn create_raw(&self, request: impl Into<Request>) -> Result<Value, Error> {
        self.runtime.block_on(self.client.create_raw(request))
    }
}
