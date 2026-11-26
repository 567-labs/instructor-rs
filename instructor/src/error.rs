use serde_json::Value;
use thiserror::Error;

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
    Http { status: u16, body: String },
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
    pub(crate) fn retryable(&self) -> bool {
        matches!(self, Self::Parse(_) | Self::Validation(_))
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Parse(error.to_string())
    }
}
