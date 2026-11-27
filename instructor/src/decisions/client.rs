use super::{DecisionError, DecisionModel, DecisionRequest, PreparedDecision, Route, Value};
use crate::error::http_failure;
use reqwest::{blocking, StatusCode};
use std::{sync::Arc, time::Duration};
use tokio::runtime::Runtime;

/// Configure an endpoint and its transport. Keys are resolved at build time.
pub struct DecisionsConfig {
    route: Route,
    endpoint: String,
    api_key: Option<String>,
    timeout: Option<Option<Duration>>,
}

impl DecisionsConfig {
    /// Split on the first slash, preserving namespaced model IDs.
    pub fn new(route: &str) -> Result<Self, DecisionError> {
        let route = Route::new(route)?;
        Ok(Self {
            endpoint: route.provider.endpoint().to_owned(),
            route,
            api_key: None,
            timeout: None,
        })
    }

    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Replace the complete URL, including its path.
    pub fn endpoint(mut self, url: impl Into<String>) -> Self {
        self.endpoint = url.into();
        self
    }

    /// Set an overall deadline per request. None disables the default on
    /// built-in transports. Supplied transports retain their own timeout
    /// unless a duration is passed.
    pub fn timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn build(self) -> Result<DecisionsClient, DecisionError> {
        let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
        if let Some(timeout) = self.owned_timeout() {
            builder = builder.timeout(timeout);
        }
        self.with_client(builder.build()?)
    }

    pub fn build_blocking(self) -> Result<BlockingDecisionsClient, DecisionError> {
        let client = blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(self.owned_timeout())
            .build()?;
        self.with_blocking_client(client)
    }

    /// Reuse a transport by passing a cheap clone of its handle.
    pub fn with_client(self, client: reqwest::Client) -> Result<DecisionsClient, DecisionError> {
        self.finish(client)
    }

    pub fn with_blocking_client(
        self,
        client: blocking::Client,
    ) -> Result<BlockingDecisionsClient, DecisionError> {
        Ok(BlockingDecisionsClient(BlockingTransport::Http(
            self.finish(client)?,
        )))
    }

    fn owned_timeout(&self) -> Option<Duration> {
        self.timeout.unwrap_or(Some(Duration::from_secs(60)))
    }

    fn finish<C>(self, http: C) -> Result<Client<C>, DecisionError> {
        let key = self.route.provider.key();
        let api_key = self
            .api_key
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var(key).ok().filter(|s| !s.is_empty()))
            .ok_or_else(|| DecisionError::Request(format!("set {key} or supply an API key")))?;
        Ok(Client {
            route: self.route,
            endpoint: self.endpoint,
            api_key,
            http,
            timeout: self.timeout.flatten(),
        })
    }
}

#[derive(Clone)]
pub struct Client<C> {
    route: Route,
    api_key: String,
    endpoint: String,
    timeout: Option<Duration>,
    http: C,
}

/// Async Decisions client. Use from a Tokio runtime.
pub type DecisionsClient = Client<reqwest::Client>;

/// Synchronous Decisions client. Use from a blocking thread.
#[derive(Clone)]
pub struct BlockingDecisionsClient(BlockingTransport);

#[derive(Clone)]
enum BlockingTransport {
    Http(Client<blocking::Client>),
    Shared(DecisionsClient, Arc<Runtime>),
}

impl crate::Client {
    /// Share this OpenAI or OpenRouter client's model, credentials,
    /// base URL, connection pool and timeout with a Decisions client.
    pub fn decisions(&self) -> Result<DecisionsClient, DecisionError> {
        self.config
            .decisions_config()?
            .with_client(self.http.clone())
    }
}

impl crate::BlockingClient {
    /// Share this OpenAI or OpenRouter client's configuration, runtime
    /// and connection pool with a synchronous Decisions client.
    pub fn decisions(&self) -> Result<BlockingDecisionsClient, DecisionError> {
        let client = self.client.decisions()?;
        Ok(BlockingDecisionsClient(BlockingTransport::Shared(
            client,
            self.runtime.clone(),
        )))
    }
}

impl<C> Client<C> {
    pub fn model(&self) -> &str {
        &self.route.model
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl DecisionsClient {
    /// Build a client with the provider endpoint and environment key.
    pub fn from_provider(route: &str) -> Result<Self, DecisionError> {
        DecisionsConfig::new(route)?.build()
    }

    pub async fn create<T: DecisionModel>(
        &self,
        request: impl Into<DecisionRequest>,
    ) -> Result<T, DecisionError> {
        self.create_with_completion(request)
            .await
            .map(|(result, _)| result)
    }

    /// Return the typed result and the provider's entire JSON response.
    pub async fn create_with_completion<T: DecisionModel>(
        &self,
        request: impl Into<DecisionRequest>,
    ) -> Result<(T, Value), DecisionError> {
        let prepared = PreparedDecision::prepare(&self.route, request.into())?;
        let mut builder = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(prepared.body());
        if let Some(timeout) = self.timeout {
            builder = builder.timeout(timeout);
        }
        let response = builder.send().await?;
        let failure = http_failure(response.status(), response.headers());
        finish(prepared, failure, &response.bytes().await?)
    }
}

impl BlockingDecisionsClient {
    pub fn model(&self) -> &str {
        match &self.0 {
            BlockingTransport::Http(client) => client.model(),
            BlockingTransport::Shared(client, _) => client.model(),
        }
    }

    pub fn endpoint(&self) -> &str {
        match &self.0 {
            BlockingTransport::Http(client) => client.endpoint(),
            BlockingTransport::Shared(client, _) => client.endpoint(),
        }
    }

    pub fn from_provider(route: &str) -> Result<Self, DecisionError> {
        DecisionsConfig::new(route)?.build_blocking()
    }

    pub fn create<T: DecisionModel>(
        &self,
        request: impl Into<DecisionRequest>,
    ) -> Result<T, DecisionError> {
        self.create_with_completion(request)
            .map(|(result, _)| result)
    }

    pub fn create_with_completion<T: DecisionModel>(
        &self,
        request: impl Into<DecisionRequest>,
    ) -> Result<(T, Value), DecisionError> {
        let client = match &self.0 {
            BlockingTransport::Http(client) => client,
            BlockingTransport::Shared(client, runtime) => {
                return runtime.block_on(client.create_with_completion(request))
            }
        };
        let prepared = PreparedDecision::prepare(&client.route, request.into())?;
        let mut builder = client
            .http
            .post(&client.endpoint)
            .bearer_auth(&client.api_key)
            .json(prepared.body());
        if let Some(timeout) = client.timeout {
            builder = builder.timeout(timeout);
        }
        let response = builder.send()?;
        let failure = http_failure(response.status(), response.headers());
        finish(prepared, failure, &response.bytes()?)
    }
}

fn finish<T: DecisionModel>(
    prepared: PreparedDecision<T>,
    failure: Option<(StatusCode, Option<String>)>,
    bytes: &[u8],
) -> Result<(T, Value), DecisionError> {
    if let Some((status, request_id)) = failure {
        return Err(DecisionError::Http {
            status,
            body: bytes.to_vec(),
            request_id,
        });
    }
    let raw = serde_json::from_slice(bytes).map_err(DecisionError::Json)?;
    Ok((prepared.parse(&raw)?, raw))
}
