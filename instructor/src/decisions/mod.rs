//! Typed, single-request decisions through OpenAI or Jev.
//!
//! ```no_run
//! use instructor_ai::decisions::{Choices, DecisionModel, DecisionsClient, Noul, Score};
//! use serde_json::json;
//!
//! /// Which team handles this ticket?
//! #[derive(Choices, Debug)]
//! enum Team {
//!     #[choice(examples = ["I was charged twice."])]
//!     Billing,
//!     Support,
//! }
//!
//! #[derive(DecisionModel)]
//! struct Ticket {
//!     team: Team,
//!     #[decision(Noul::new("Does the customer want a refund?"))]
//!     refund: f64,
//!     #[decision(Score::new("How urgent?", ["Routine", "Urgent"]).scale(0.0, 10.0))]
//!     urgency: f64,
//! }
//!
//! # async fn run() -> Result<(), instructor_ai::decisions::DecisionError> {
//! let client = DecisionsClient::from_provider("typesafe/jev-latest")?;
//! let ticket: Ticket = client.create(json!({"message": "I was charged twice."})).await?;
//! println!("{:?}: refund probability {}, urgency {}", ticket.team, ticket.refund, ticket.urgency);
//! # Ok(())
//! # }
//! ```
//!
//! A binary decision returns a probability, so a boolean field is rejected at compile time.
//!
//! ```compile_fail
//! use instructor_ai::decisions::{DecisionModel, Noul};
//! #[derive(DecisionModel)]
//! struct Ticket {
//!     #[decision(Noul::new("Duplicate charge?"))]
//!     duplicate: bool,
//! }
//! ```
//!
//! Choice labels must be unique.
//!
//! ```compile_fail
//! use instructor_ai::decisions::Choices;
//! #[derive(Choices)]
//! enum Team {
//!     Support,
//!     #[choice(value = "support")]
//!     Other,
//! }
//! ```

mod client;
mod questions;
mod schema;

pub use client::{BlockingDecisionsClient, DecisionsClient, DecisionsConfig};
pub use instruct_macros::{Choices, DecisionModel};
pub use questions::{
    Answers, Choice, Choices, DecisionModel, DecisionValue, Level, NamedQuestion, Noul, Question,
    Score,
};
pub use schema::PreparedDecision;
pub use serde_json::Value;

/// Failures stay attributable to a stage of the request. HTTP failures
/// carry the exact response bytes, including non-JSON responses.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DecisionError {
    #[error("invalid decision request: {0}")]
    Request(String),
    #[error("decision request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("decision endpoint returned {status}")]
    Http {
        status: reqwest::StatusCode,
        body: Vec<u8>,
        request_id: Option<String>,
    },
    #[error("invalid decision response JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid decision answer: {0}")]
    Answer(String),
    #[error("decision model validation failed: {0}")]
    Validation(String),
}

impl DecisionError {
    /// Inspect the status, request ID and provider code of an HTTP failure.
    pub fn http_details(&self) -> Option<crate::HttpErrorDetails> {
        match self {
            Self::Http {
                status,
                body,
                request_id,
            } => Some(crate::HttpErrorDetails::from_body(
                status.as_u16(),
                request_id.as_deref(),
                body,
            )),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Provider {
    OpenAi,
    TypeSafe,
    OpenRouter,
}

impl Provider {
    fn endpoint(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com/v1/decisions",
            Self::TypeSafe => "https://api.typesafe.ai/v1/systemone",
            Self::OpenRouter => "https://openrouter.ai/api/alpha/decisions",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_API_KEY",
            Self::TypeSafe => "TYPESAFE_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
        }
    }
}

#[derive(Clone)]
struct Route {
    provider: Provider,
    model: String,
}

impl Route {
    fn new(route: &str) -> Result<Self, DecisionError> {
        let (provider, model) = route
            .split_once('/')
            .filter(|(_, model)| !model.trim().is_empty())
            .ok_or_else(|| DecisionError::Request("expected provider/model".into()))?;
        let provider = match provider {
            "openai" => Provider::OpenAi,
            "typesafe" => Provider::TypeSafe,
            "openrouter" => Provider::OpenRouter,
            other => {
                return Err(DecisionError::Request(format!(
                    "unsupported decisions provider {other}"
                )))
            }
        };
        Ok(Self {
            provider,
            model: model.to_owned(),
        })
    }
}

/// Context drives instruction templates and model validation. Jev receives
/// it as state. OpenAI uses its JSON text unless native input is supplied.
#[derive(Debug, Clone)]
pub struct DecisionRequest {
    context: Value,
    input: Option<Value>,
    strict: bool,
}

impl DecisionRequest {
    pub fn context(context: Value) -> Self {
        Self {
            context,
            input: None,
            strict: true,
        }
    }

    /// OpenAI text or a JSON array of native input items.
    pub fn input(input: impl Into<Value>) -> Self {
        Self {
            context: serde_json::json!({}),
            input: Some(input.into()),
            strict: true,
        }
    }

    pub fn with_context(mut self, context: Value) -> Self {
        self.context = context;
        self
    }

    /// Set the flag passed to application validation. Wire types and
    /// ranges are checked in both modes.
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }
}

impl From<Value> for DecisionRequest {
    fn from(context: Value) -> Self {
        Self::context(context)
    }
}
