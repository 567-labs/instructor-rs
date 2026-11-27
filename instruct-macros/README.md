# Instruct Macros

Use `#[model]` on a struct or enum to describe the result you want from a language model. It gives Instructor a schema for the request, a Rust type for the reply, and a partial type for streaming previews.

In an application, add `instructor-ai` and import `instructor_ai::model`. The same dependency provides `ParallelTools` for an enum of named tools and `decisions::{Choices, DecisionModel}` for classification and scoring. [Start a project](../README.md) · [Response types](../docs/types.md) · [Parallel tools](../docs/parallel.md) · [Decisions](../docs/decisions.md)
