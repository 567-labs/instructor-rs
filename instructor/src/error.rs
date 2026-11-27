use serde_json::Value;
use thiserror::Error;

/// Diagnostics for one rejected HTTP request. The error retains the full body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpErrorDetails {
    pub status: u16,
    /// Provider request identifier from the headers or response body.
    pub request_id: Option<String>,
    /// Provider error code, or error type when no code field is present.
    pub code: Option<String>,
}

impl HttpErrorDetails {
    pub(crate) fn from_body(status: u16, header_id: Option<&str>, body: &[u8]) -> Self {
        let raw: Value = serde_json::from_slice(body).unwrap_or_default();
        let request_id = header_id
            .or_else(|| raw["request_id"].as_str().filter(|id| !id.is_empty()))
            .map(str::to_owned);
        let code = raw
            .pointer("/error/code")
            .or_else(|| raw.get("code"))
            .or_else(|| raw.pointer("/error/type"));
        let code = match code {
            Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
            Some(Value::Number(value)) => Some(value.to_string()),
            _ => None,
        };
        Self {
            status,
            request_id,
            code,
        }
    }
}

pub(crate) fn http_failure(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
) -> Option<(reqwest::StatusCode, Option<String>)> {
    if status.is_success() {
        return None;
    }
    let request_id = ["x-request-id", "request-id"]
        .into_iter()
        .find_map(|name| headers.get(name)?.to_str().ok().filter(|id| !id.is_empty()))
        .map(str::to_owned);
    Some((status, request_id))
}

/// One rejected response and the reason it failed validation.
#[derive(Debug)]
pub struct FailedAttempt {
    pub attempt: u64,
    pub completion: Value,
    pub error: Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StopReason {
    #[error("terminal error")]
    Terminal,
    #[error("retry timeout reached")]
    Timeout,
    #[error("retry policy stopped")]
    Policy,
    #[error("retries exhausted")]
    RetriesExhausted,
    #[error("token budget of {budget} exhausted")]
    TokenBudget { budget: u64 },
    #[error("usage unavailable for token budget of {budget}")]
    UsageUnavailable { budget: u64 },
}

/// Terminal failures and the history of rejected structured completions.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid configuration: {0}")]
    Configuration(String),
    #[error("HTTP transport: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("HTTP {status}: {body}")]
    Http {
        status: u16,
        body: String,
        request_id: Option<String>,
    },
    #[error("provider error: {0}")]
    Provider(String),
    #[error("invalid provider response: {0}")]
    Protocol(String),
    #[error("incomplete output: {0}")]
    Incomplete(String),
    #[error("provider refused request: {0}")]
    Refusal(String),
    #[error("could not parse structured output: {0}")]
    Parse(String),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("{reason} after {} attempts: {}", .attempts.len(), .attempts.last().map(|a| a.error.to_string()).unwrap_or_default())]
    Retry {
        reason: StopReason,
        total_tokens: Option<u64>,
        attempts: Vec<FailedAttempt>,
    },
}

impl Error {
    /// Inspect an HTTP failure, following the last attempt of a retry.
    /// The provider code is decoded from the saved response body on demand.
    pub fn http_details(&self) -> Option<HttpErrorDetails> {
        match self {
            Self::Http {
                status,
                body,
                request_id,
            } => Some(HttpErrorDetails::from_body(
                *status,
                request_id.as_deref(),
                body.as_bytes(),
            )),
            Self::Retry { attempts, .. } => attempts.last()?.error.http_details(),
            _ => None,
        }
    }

    pub(crate) fn retryable(&self) -> bool {
        matches!(self, Self::Parse(_) | Self::Validation(_))
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Parse(error.to_string())
    }
}
