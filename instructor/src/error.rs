use thiserror::Error;

/// Failure decoding or validating a structured result.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("could not parse structured output: {0}")]
    Parse(String),
    #[error("validation failed: {0}")]
    Validation(String),
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Parse(error.to_string())
    }
}
