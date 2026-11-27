use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};
type RetryPolicy = Arc<dyn Fn(&crate::FailedAttempt) -> Option<Duration> + Send + Sync>;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{Error, TokenUsage};

/// Lifetime requested for a provider-side prompt-cache breakpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheTtl {
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<crate::Media>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheTtl>,
}

impl Message {
    /// Request a five-minute prompt-cache breakpoint after this message.
    pub fn cache(self) -> Self {
        self.cache_for(CacheTtl::FiveMinutes)
    }

    /// Cache the prompt prefix through the last block of this message.
    /// Available for Anthropic and supported OpenRouter routes.
    pub fn cache_for(mut self, ttl: CacheTtl) -> Self {
        self.cache_control = Some(ttl);
        self
    }

    /// Append media to a user message, in order after its text.
    pub fn attach(mut self, media: crate::Media) -> Self {
        self.attachments.push(media);
        self
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: text.into(),
            attachments: Vec::new(),
            cache_control: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            ..Self::user(text)
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            ..Self::user(text)
        }
    }
}

/// Notifications are borrowed for the duration of the hook call.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Event<'a> {
    Request(&'a Value),
    Response(&'a Value),
    Usage { total_tokens: u64 },
    CompletionError(&'a Error),
    ParseError(&'a Error),
    LastAttempt(&'a Error),
    CacheHit,
    CacheError(&'a std::io::Error),
}

pub(crate) type Hook = Arc<Callback>;

pub(crate) enum Callback {
    Event(Box<dyn Fn(u64, Event<'_>) + Send + Sync>),
    Usage(Box<dyn Fn(u64, TokenUsage) + Send + Sync>),
}

/// Request-local settings. Cloning a request allows reuse across concurrent calls.
#[derive(Clone)]
pub struct Request {
    pub messages: Vec<Message>,
    pub(crate) native: Vec<Value>,
    pub options: Map<String, Value>,
    pub(crate) context: Value,
    pub(crate) retry_timeout: Option<Duration>,
    pub(crate) retry_policy: Option<RetryPolicy>,
    pub(crate) max_retries: u32,
    pub(crate) cache_ttl: Option<Duration>,
    pub(crate) token_budget: Option<u64>,
    pub(crate) hooks: Vec<Hook>,
}

impl Request {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self::messages(vec![Message::user(prompt)])
    }

    pub fn messages(messages: Vec<Message>) -> Self {
        Self {
            messages,
            native: Vec::new(),
            options: Map::new(),
            context: Value::Null,
            max_retries: 3,
            retry_timeout: None,
            retry_policy: None,
            token_budget: None,
            cache_ttl: None,
            hooks: Vec::new(),
        }
    }

    /// Start with provider-native history. Each object is copied verbatim into
    /// messages, Gemini contents, or OpenAI Responses input, without template
    /// rendering. Items added to the typed messages vector follow this history.
    /// Use a typed system message for Anthropic or Gemini system instructions.
    pub fn native_messages(items: Vec<Value>) -> Self {
        Self {
            native: items,
            ..Self::messages(Vec::new())
        }
    }

    /// Additional attempts after a parse or validation error.
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Set an elapsed time limit for starting another attempt, including backoff.
    /// Configure HTTP timeouts separately with Client::with_timeout.
    pub fn retry_timeout(mut self, timeout: Duration) -> Self {
        self.retry_timeout = Some(timeout);
        self
    }

    /// Decide whether to retry and how long to wait. Called for parse, validation,
    /// transport, HTTP and provider errors within the attempt and token limits.
    /// Returning None stops. Refusals and truncated outputs always stop.
    pub fn retry_policy(
        mut self,
        policy: impl Fn(&crate::FailedAttempt) -> Option<Duration> + Send + Sync + 'static,
    ) -> Self {
        self.retry_policy = Some(Arc::new(policy));
        self
    }

    /// Stop reasking when cumulative tokens reach this threshold.
    pub fn token_budget(mut self, budget: u64) -> Self {
        self.token_budget = Some(budget);
        self
    }

    /// Set the expiration time of a newly stored response. Zero bypasses the cache.
    pub fn cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = Some(ttl);
        self
    }

    /// Partition entries within the client's response cache, for example by tenant.
    /// The namespace must be nonempty and is consumed locally.
    pub fn cache_namespace(self, namespace: impl Into<String>) -> Self {
        self.option("cache_namespace", namespace.into())
    }

    /// Context for prompt templates and contextual validators.
    pub fn context(mut self, context: Value) -> Self {
        self.context = context;
        self
    }

    /// Set a native provider request parameter.
    pub fn option(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.options.insert(name.into(), value.into());
        self
    }

    /// Override wire schema strictness for OpenAI Chat or OpenRouter tool and
    /// JSON Schema requests. Rust deserialization and validators still apply.
    pub fn strict(self, strict: bool) -> Self {
        self.option("strict", strict)
    }

    /// Observe this request. Attempt numbers start at one, or zero for a cache hit.
    pub fn on_event(mut self, hook: impl Fn(u64, Event<'_>) + Send + Sync + 'static) -> Self {
        self.hooks.push(Arc::new(Callback::Event(Box::new(hook))));
        self
    }

    /// Observe cumulative usage after each structured JSON response, or zero on a cache hit.
    pub fn on_usage(mut self, hook: impl Fn(u64, TokenUsage) + Send + Sync + 'static) -> Self {
        self.hooks.push(Arc::new(Callback::Usage(Box::new(hook))));
        self
    }

    pub(crate) fn prepare(mut self, supports_budget: bool) -> Result<Self, Error> {
        if self.token_budget == Some(0) {
            return Err(Error::Configuration("token budget must be positive".into()));
        }
        if self.token_budget.is_some() && !supports_budget {
            return Err(Error::Configuration(
                "token budget requires a non-streaming structured request".into(),
            ));
        }
        if !self.context.is_null() {
            for msg in &mut self.messages {
                render(&mut msg.content, &self.context)
                    .map_err(|e| Error::Configuration(format!("template: {e}")))?;
            }
        }
        Ok(self)
    }
}

impl From<&str> for Request {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Request {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<Vec<Message>> for Request {
    fn from(value: Vec<Message>) -> Self {
        Self::messages(value)
    }
}

pub(crate) fn render(template: &mut String, context: &Value) -> Result<(), minijinja::Error> {
    // The default MiniJinja syntax uses {{, {% and {# as opening delimiters.
    if !template.contains('{') {
        return Ok(());
    }
    static ENV: LazyLock<minijinja::Environment<'static>> = LazyLock::new(|| {
        let mut env = minijinja::Environment::new();
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
        env.set_keep_trailing_newline(true);
        env
    });
    *template = ENV.render_str(template, context)?;
    Ok(())
}
