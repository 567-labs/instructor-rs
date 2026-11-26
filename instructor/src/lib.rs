//! Typed structured outputs.
//!
//! ```no_run
//! use instructor_ai::{model, from_provider};
//!
//! #[model]
//! struct Person {
//!     name: String,
//!     age: u32,
//! }
//!
//! # async fn example() -> Result<(), instructor_ai::Error> {
//! let client = from_provider("openai/gpt-4.1-mini")?;
//! let person: Person = client.create("Ada is 36").await?;
//! assert_eq!(person.age, 36);
//! # Ok(())
//! # }
//! ```
extern crate self as instructor_ai;

mod analysis;
mod cache;
mod client;
mod error;
mod helpers;
mod legacy;
mod media;
pub mod providers;
mod request;
mod retry;
mod schema;

pub use analysis::{analyze_schema, select_mode, SchemaAnalysis, SchemaFinding, Severity};
pub use client::{from_provider, BlockingClient, Client, Completion, HookHandle};
pub use error::{Error, FailedAttempt, StopReason};
pub use instruct_macros::{model, InstructMacro, PartialModel};
pub use instruct_macros_types::{InstructMacro, Partial, PartialModel};
pub use legacy::{from_openai, InstructorClient};
pub use media::Media;
pub use providers::{Mode, Provider, ProviderConfig};
pub use request::{CacheTtl, Event, Message, Request};
pub use schema::{parse_response, response_schema, ResponseModel};

#[doc(hidden)]
pub use instruct_macros_types as __private;

mod markdown;
