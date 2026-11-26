use std::{
    future::{ready, Future},
    path::Path,
    sync::Arc,
    time::Duration,
};

use crate::cache::{Cache, ResponseCache};
use futures_util::{stream::BoxStream, StreamExt};
use serde_json::Value;
use tokio::runtime::Runtime;

use crate::{
    providers::WireRequest,
    request::{Callback, Hook},
    retry::Attempts,
    schema::{schema, Schema},
    streaming, Error, Event, Mode, PartialEvent, PartialModel, ProviderConfig, Request,
    ResponseModel, TokenUsage,
};

pub type OutputStream<T> = BoxStream<'static, Result<T, Error>>;

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
pub struct HookHandle(std::sync::Weak<Callback>);

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
    /// timeout and redirect policy. For cache sharing, use a namespace
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
        self.add_hook(hook);
        self
    }

    /// Observe cumulative usage after each structured JSON response, or zero on a cache hit.
    pub fn on_usage(mut self, hook: impl Fn(u64, TokenUsage) + Send + Sync + 'static) -> Self {
        self.add_usage_hook(hook);
        self
    }

    /// Register a removable client callback. Clones inherit their own callback
    /// lists, so removal affects subsequent calls through this client.
    pub fn add_hook(
        &mut self,
        callback: impl Fn(u64, Event<'_>) + Send + Sync + 'static,
    ) -> HookHandle {
        self.register(Callback::Event(Box::new(callback)))
    }

    /// Register a removable callback for cumulative usage snapshots.
    pub fn add_usage_hook(
        &mut self,
        callback: impl Fn(u64, TokenUsage) + Send + Sync + 'static,
    ) -> HookHandle {
        self.register(Callback::Usage(Box::new(callback)))
    }

    fn register(&mut self, callback: Callback) -> HookHandle {
        let hook = Arc::new(callback);
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

    /// Use application storage in place of any previously configured cache.
    /// The nonempty namespace scopes entries together with the provider, model,
    /// endpoint, credential and mode. Use a distinct namespace for each tenant.
    pub fn with_cache_backend(
        mut self,
        backend: Arc<dyn ResponseCache>,
        namespace: &str,
    ) -> Result<Self, Error> {
        self.cache = Some(Cache::backend(&self.config, backend, namespace)?);
        Ok(self)
    }

    /// Clear this client's cache scope synchronously.
    pub fn clear_cache(&self) -> std::io::Result<()> {
        self.cache.as_ref().map_or(Ok(()), Cache::clear)
    }

    /// Clear this client's cache scope, offloading storage I/O to the blocking pool.
    pub async fn clear_cache_async(&self) -> std::io::Result<()> {
        match &self.cache {
            Some(cache) => cache.clear_async().await,
            None => Ok(()),
        }
    }

    pub(crate) fn emit(&self, request: &Request, attempt: u64, event: Event<'_>) {
        Self::notify(self.hooks.iter().chain(&request.hooks), attempt, event);
    }

    fn notify<'a>(hooks: impl Iterator<Item = &'a Hook>, attempt: u64, event: Event<'_>) {
        for hook in hooks {
            if let Callback::Event(callback) = hook.as_ref() {
                callback(attempt, event);
            }
        }
    }

    fn emit_usage(&self, request: &Request, attempt: u64, usage: TokenUsage) {
        for hook in self.hooks.iter().chain(&request.hooks) {
            if let Callback::Usage(callback) = hook.as_ref() {
                callback(attempt, usage);
            }
        }
    }

    pub async fn create<T: ResponseModel>(&self, request: impl Into<Request>) -> Result<T, Error> {
        self.create_with_usage(request)
            .await
            .map(|(result, _)| result.value)
    }

    /// Return the validated value and its original successful provider response.
    pub async fn create_with_completion<T: ResponseModel>(
        &self,
        request: impl Into<Request>,
    ) -> Result<(T, Value), Error> {
        self.create_with_usage(request)
            .await
            .map(|(result, _)| result.into_pair())
    }

    /// Return the full result including the number of attempts and cumulative usage.
    pub async fn create_with_metadata<T: ResponseModel>(
        &self,
        request: impl Into<Request>,
    ) -> Result<Completion<T>, Error> {
        self.create_with_usage(request)
            .await
            .map(|(completion, _)| completion)
    }

    /// Return metadata and a breakdown of tokens consumed across all attempts.
    pub async fn create_with_usage<T: ResponseModel>(
        &self,
        request: impl Into<Request>,
    ) -> Result<(Completion<T>, TokenUsage), Error> {
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
            .map(|(c, _)| c.value)
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
        schema.parse_completion(self.config.provider(), self.config.mode(), raw, context)
    }

    async fn complete<T, F, Fut>(
        &self,
        request: Request,
        validate: F,
    ) -> Result<(Completion<T>, TokenUsage), Error>
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
    ) -> Result<(Completion<T>, TokenUsage), Error>
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
                        self.emit_usage(req, 0, TokenUsage::ZERO);
                        return Ok((
                            Completion {
                                value: model,
                                raw,
                                attempts: 0,
                                total_tokens: Some(0),
                            },
                            TokenUsage::ZERO,
                        ));
                    }
                }
                cache.reject(key, raw);
            }
        }

        let mut attempts = Attempts::new(req.retry_timeout);
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
            let usage =
                attempts.record_usage(TokenUsage::from_response(self.config.provider(), &raw));
            self.emit_usage(req, attempt, usage);
            let total_tokens = usage.total_tokens;
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
                    return Ok((
                        Completion {
                            value,
                            raw,
                            attempts: attempt,
                            total_tokens,
                        },
                        usage,
                    ));
                }
                Err(error) => {
                    self.reject(req, &mut wire, &mut attempts, raw, error)
                        .await?
                }
            }
        }
    }

    /// Stream provisional typed snapshots followed by one fully validated model.
    pub async fn create_partial<T>(
        &self,
        request: impl Into<Request>,
    ) -> Result<OutputStream<PartialEvent<T>>, Error>
    where
        T: ResponseModel + PartialModel + Send,
        T::Partial: Send + 'static,
    {
        let schema = schema::<T>();
        self.stream(request.into(), schema, false, |fragments, context| {
            streaming::partial_async_with_context::<T>(fragments, context).boxed()
        })
        .await
    }

    /// Stream validated items as each element of the tasks array ends.
    pub async fn create_iterable<T: ResponseModel + Send>(
        &self,
        request: impl Into<Request>,
    ) -> Result<OutputStream<T>, Error> {
        self.stream(
            request.into(),
            schema::<Vec<T>>(),
            true,
            |fragments, context| {
                streaming::iterable_async_with_context::<T>(fragments, context).boxed()
            },
        )
        .await
    }

    async fn stream<T: Send + 'static>(
        &self,
        request: Request,
        schema: Arc<Schema>,
        iterable: bool,
        decode: fn(OutputStream<String>, Value) -> OutputStream<T>,
    ) -> Result<OutputStream<T>, Error> {
        let req = request.prepare(false)?;
        let mut wire = self.config.request(&req, Some(&schema.value), true)?;
        let mut attempts = Attempts::new(req.retry_timeout);
        loop {
            let num = attempts.next();
            self.emit(&req, num, Event::Request(&wire.body));
            let resp = match wire.send(&self.http).await {
                Ok(resp) => resp,
                Err(error) => {
                    self.reject(&req, &mut wire, &mut attempts, Value::Null, error)
                        .await?;
                    continue;
                }
            };
            let mut fragments = self.config.deltas(resp, &schema.value, iterable);
            if self.config.mode() == Mode::MarkdownJson {
                fragments = streaming::markdown_fragments(fragments, iterable);
            }
            let mut stream = decode(fragments, req.context.clone());
            // Replay is safe only before handing the caller the first item.
            match stream.next().await {
                Some(Err(error)) => {
                    drop(stream);
                    self.reject(&req, &mut wire, &mut attempts, Value::Null, error)
                        .await?;
                }
                Some(Ok(first)) => {
                    // Once a stream has yielded, only the terminal callbacks are needed.
                    let client_hooks = self.hooks.clone();
                    let request_hooks = req.hooks;
                    return Ok(async_stream::stream! {
                        yield Ok(first);
                        while let Some(result) = stream.next().await {
                            if let Err(error) = result {
                                Self::notify(client_hooks.iter().chain(&request_hooks), num, Event::LastAttempt(&error));
                                drop(stream);
                                yield Err(error);
                                break;
                            }
                            yield result;
                        }
                    }
                    .boxed());
                }
                None => return Ok(futures_util::stream::empty().boxed()),
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

/// Dropping an iterator releases its underlying HTTP stream.
pub struct OutputIterator<T> {
    stream: OutputStream<T>,
    runtime: Arc<Runtime>,
}

impl<T> Iterator for OutputIterator<T> {
    type Item = Result<T, Error>;
    fn next(&mut self) -> Option<Self::Item> {
        self.runtime.block_on(self.stream.next())
    }
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

    /// Observe cumulative usage after each structured JSON response, or zero on a cache hit.
    pub fn on_usage(mut self, hook: impl Fn(u64, TokenUsage) + Send + Sync + 'static) -> Self {
        self.client = self.client.on_usage(hook);
        self
    }

    /// Register a removable callback for cumulative usage snapshots.
    pub fn add_usage_hook(
        &mut self,
        callback: impl Fn(u64, TokenUsage) + Send + Sync + 'static,
    ) -> HookHandle {
        self.client.add_usage_hook(callback)
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

    /// Use application storage, with the same scoping as [`Client::with_cache_backend`].
    pub fn with_cache_backend(
        mut self,
        backend: Arc<dyn ResponseCache>,
        namespace: &str,
    ) -> Result<Self, Error> {
        self.client = self.client.with_cache_backend(backend, namespace)?;
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

    /// Return metadata and a breakdown of tokens consumed across all attempts.
    pub fn create_with_usage<T: ResponseModel>(
        &self,
        request: impl Into<Request>,
    ) -> Result<(Completion<T>, TokenUsage), Error> {
        self.runtime
            .block_on(self.client.create_with_usage(request))
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

    pub fn create_partial<T>(
        &self,
        request: impl Into<Request>,
    ) -> Result<OutputIterator<PartialEvent<T>>, Error>
    where
        T: ResponseModel + PartialModel + Send,
        T::Partial: Send + 'static,
    {
        let stream = self
            .runtime
            .block_on(self.client.create_partial::<T>(request))?;
        Ok(OutputIterator {
            stream,
            runtime: self.runtime.clone(),
        })
    }

    pub fn create_iterable<T: ResponseModel + Send>(
        &self,
        request: impl Into<Request>,
    ) -> Result<OutputIterator<T>, Error> {
        let stream = self
            .runtime
            .block_on(self.client.create_iterable::<T>(request))?;
        Ok(OutputIterator {
            stream,
            runtime: self.runtime.clone(),
        })
    }
}
