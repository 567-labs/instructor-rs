//! Derive schemas and validate typed structured outputs.
extern crate self as instructor_ai;

mod error;
mod helpers;
mod legacy;
mod schema;

pub use error::Error;
pub use instruct_macros::{model, InstructMacro, PartialModel};
pub use instruct_macros_types::{InstructMacro, Partial, PartialModel};
pub use legacy::{from_openai, InstructorClient};
pub use schema::{parse_response, response_schema, ResponseModel};

#[doc(hidden)]
pub use instruct_macros_types as __private;
