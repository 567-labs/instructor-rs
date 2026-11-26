//! Native HTTP protocols for the supported Instructor providers.

mod indexed;
mod parallel;
mod reask;
mod request;
mod response;
mod stream;

pub(crate) use request::WireRequest;
pub(crate) use response::check_finish;

use crate::Error;
use serde_json::Value;
use std::{env, fmt, str::FromStr};

/// Upstream API selected by a provider/model identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAI,
    OpenRouter,
    Anthropic,
    Google,
    Cohere,
    Mistral,
}

impl Provider {
    /// Environment variable used by [ProviderConfig::from_env].
    pub fn api_key_env(self) -> &'static str {
        match self {
            Self::OpenAI => "OPENAI_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::Google => "GOOGLE_API_KEY",
            Self::Cohere => "COHERE_API_KEY",
            Self::Mistral => "MISTRAL_API_KEY",
        }
    }

    /// Base endpoint including the API version.
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAI => "https://api.openai.com/v1",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::Google => "https://generativelanguage.googleapis.com/v1beta",
            Self::Cohere => "https://api.cohere.com/v2",
            Self::Mistral => "https://api.mistral.ai/v1",
        }
    }

    pub fn default_mode(self) -> Mode {
        match self {
            Self::Google => Mode::JsonSchema,
            _ => Mode::Tools,
        }
    }
}

impl FromStr for Provider {
    type Err = Error;

    fn from_str(prefix: &str) -> Result<Self, Error> {
        match prefix.to_ascii_lowercase().as_str() {
            "openai" => Ok(Self::OpenAI),
            "openrouter" => Ok(Self::OpenRouter),
            "anthropic" => Ok(Self::Anthropic),
            "google" | "gemini" => Ok(Self::Google),
            "cohere" => Ok(Self::Cohere),
            "mistral" => Ok(Self::Mistral),
            _ => Err(Error::Configuration(format!("Unknown provider {prefix:?}. Supported providers: openai, openrouter, anthropic, google, cohere, mistral"))),
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OpenAI => "openai",
            Self::OpenRouter => "openrouter",
            Self::Anthropic => "anthropic",
            Self::Google => "google",
            Self::Cohere => "cohere",
            Self::Mistral => "mistral",
        })
    }
}

/// Wire representation for a typed output. Local decoding and validation run in every mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Request a single named function with structured arguments.
    Tools,
    /// Request an OpenAI function with strict schema enforcement.
    ToolsStrict,
    /// Use OpenAI Responses with a named function and matching text schema.
    ResponsesTools,
    /// Request text generated with the provider's native structured output schema.
    JsonSchema,
    /// Include the schema in the prompt and request JSON syntax where supported.
    Json,
    /// Include the schema in the prompt and extract JSON from text or a Markdown fence.
    MarkdownJson,
}

impl Mode {
    pub(crate) fn is_tool(self) -> bool {
        matches!(self, Self::Tools | Self::ToolsStrict | Self::ResponsesTools)
    }
}

/// Credentials, model and endpoint for one provider.
#[derive(Clone)]
pub struct ProviderConfig {
    provider: Provider,
    model: String,
    api_key: String,
    base_url: String,
    mode: Mode,
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl ProviderConfig {
    pub(crate) fn cache_identity(&self) -> (String, &str, &str, &str, String) {
        (
            self.provider.to_string(),
            self.model(),
            &self.base_url,
            &self.api_key,
            format!("{:?}", self.mode),
        )
    }

    /// Configure a provider directly with its model identifier and API key.
    pub fn new(provider: Provider, model: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
            api_key: api_key.into(),
            base_url: provider.default_base_url().to_owned(),
            mode: provider.default_mode(),
        }
    }

    /// Parse a provider/model string and load the corresponding API key.
    /// Google also accepts GEMINI_API_KEY as a fallback.
    pub fn from_env(spec: &str) -> Result<Self, Error> {
        Self::from_vars(spec, |key| env::var(key).ok())
    }

    fn from_vars(spec: &str, read: impl Fn(&str) -> Option<String>) -> Result<Self, Error> {
        let (prefix, model) = spec
            .split_once('/')
            .filter(|(_, model)| !model.trim().is_empty())
            .ok_or_else(|| Error::Configuration("Expected provider/model".into()))?;
        let provider: Provider = prefix.parse()?;
        let key_name = provider.api_key_env();
        let mut key = read(key_name).filter(|value| !value.trim().is_empty());
        if key.is_none() && provider == Provider::Google {
            key = read("GEMINI_API_KEY").filter(|value| !value.trim().is_empty());
        }
        let key = key.ok_or_else(|| Error::Configuration(format!("Missing {key_name}")))?;
        let config = Self::new(provider, model, key);
        config.validate()?;
        Ok(config)
    }

    /// Override the endpoint base, including its version prefix.
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into();
        self
    }

    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// Reuse the model, endpoint base and credentials for a Decisions client.
    pub fn decisions_config(
        &self,
    ) -> Result<crate::decisions::DecisionsConfig, crate::decisions::DecisionError> {
        use crate::decisions::{DecisionError, DecisionsConfig};
        let route = format!("{}/{}", self.provider, self.model);
        let config = DecisionsConfig::new(&route)?;
        self.validate()
            .map_err(|e| DecisionError::Request(e.to_string()))?;
        let base = self.base_url.trim_end_matches('/');
        let endpoint = if self.provider == Provider::OpenRouter {
            match base.strip_suffix("/v1") {
                Some(prefix) => format!("{prefix}/alpha/decisions"),
                None => format!("{base}/decisions"),
            }
        } else {
            format!("{base}/decisions")
        };
        Ok(config.api_key(&self.api_key).endpoint(endpoint))
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.model.trim().is_empty() || self.model.chars().any(char::is_whitespace) {
            return Err(Error::Configuration("Expected a model identifier".into()));
        }
        if self.provider == Provider::Google {
            let model = self.model.strip_prefix("models/").unwrap_or(&self.model);
            if model.is_empty()
                || matches!(model, "." | "..")
                || !model
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
            {
                return Err(Error::Configuration(
                    "Invalid Gemini model identifier".into(),
                ));
            }
        }
        if self.api_key.trim().is_empty() {
            return Err(Error::Configuration(format!(
                "Missing {}",
                self.provider.api_key_env()
            )));
        }
        if self.mode == Mode::ToolsStrict
            && !matches!(self.provider, Provider::OpenAI | Provider::OpenRouter)
        {
            return Err(Error::Configuration(
                "ToolsStrict requires OpenAI or OpenRouter".into(),
            ));
        }
        if self.mode == Mode::ResponsesTools && self.provider != Provider::OpenAI {
            return Err(Error::Configuration(
                "ResponsesTools requires OpenAI".into(),
            ));
        }
        let url = reqwest::Url::parse(&self.base_url)
            .map_err(|_| Error::Configuration("Invalid base URL".into()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Configuration(
                "Expected an HTTP(S) base URL with a host and optional path".into(),
            ));
        }
        Ok(())
    }
}

// Schema construction normalizes the title once per Rust type.
fn tool_name(schema: &Value) -> &str {
    schema["title"].as_str().unwrap_or("Response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_identifiers_preserve_model_names_and_select_credentials() {
        for (spec, provider) in [
            ("OpenAI/org/model", Provider::OpenAI),
            ("anthropic/model", Provider::Anthropic),
            ("openrouter/openai/model", Provider::OpenRouter),
            ("gemini/models/model", Provider::Google),
            ("cohere/model", Provider::Cohere),
            ("mistral/model", Provider::Mistral),
        ] {
            let config = ProviderConfig::from_vars(spec, |key| {
                (key == provider.api_key_env()).then(|| "mock-secret".into())
            })
            .unwrap();
            assert_eq!(config.provider(), provider);
            assert_eq!(config.mode(), provider.default_mode());
            assert_eq!(config.model(), spec.split_once('/').unwrap().1);
            assert!(!format!("{config:?}").contains("mock-secret"));
        }
        let fallback = ProviderConfig::from_vars("google/model", |key| {
            (key == "GEMINI_API_KEY").then(|| "fallback".into())
        })
        .unwrap();
        assert_eq!(fallback.api_key, "fallback");
    }

    #[test]
    fn rejects_invalid_configurations_before_sending() {
        for spec in [
            "gpt",
            "/model",
            "openai/ ",
            "provider/model",
            "google/model?key=x",
            "google/models/",
        ] {
            assert!(ProviderConfig::from_vars(spec, |_| Some("key".into())).is_err());
        }
        let err = ProviderConfig::from_vars("openai/model", |_| None).unwrap_err();
        assert!(err.to_string().contains("OPENAI_API_KEY"));
        let config = ProviderConfig::new(Provider::Mistral, "model", "key");
        for endpoint in [
            "file:///tmp/model",
            "https://user@example.com",
            "https://example.com/?key=x",
            "relative/path",
        ] {
            assert!(config.clone().with_base_url(endpoint).validate().is_err());
        }
        assert!(config.with_mode(Mode::ToolsStrict).validate().is_err());
    }
}
