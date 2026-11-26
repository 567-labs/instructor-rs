//! Typed structured outputs, streaming, and finite decisions.
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
pub mod batch;
mod cache;
mod client;
pub mod decisions;
pub mod distillation;
mod error;
mod helpers;
mod legacy;
mod media;
mod models;
mod parallel;
pub mod providers;
mod request;
mod retry;
mod schema;
pub mod streaming;
pub mod validation;

pub use analysis::{analyze_schema, select_mode, SchemaAnalysis, SchemaFinding, Severity};
pub use client::{
    from_provider, BlockingClient, Client, Completion, HookHandle, OutputIterator, OutputStream,
};
pub use error::{Error, FailedAttempt, StopReason};
pub use instruct_macros::{model, InstructMacro, ParallelTools, PartialModel};
pub use instruct_macros_types::{InstructMacro, Partial, PartialModel};
pub use legacy::{from_openai, InstructorClient};
pub use media::Media;
pub use models::{Citations, CitationsPartial, Maybe, MaybePartial};
pub use parallel::{ParallelTool, ParallelTools};
pub use providers::{Mode, Provider, ProviderConfig};
pub use request::{CacheTtl, Event, Message, Request};
pub use schema::{parse_response, response_schema, ResponseModel};
pub use streaming::PartialEvent;
pub use validation::TextRule;

#[doc(hidden)]
pub use instruct_macros_types as __private;
pub(crate) use request::render;
