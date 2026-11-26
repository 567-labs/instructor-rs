# Instructor for Rust

Use a Rust type to specify what to extract from an LLM response. Instructor derives the JSON Schema, sends it to the provider, deserializes the answer, and runs your validators. A failed extraction can send feedback to the model for another attempt.

```rust
use instructor_ai::{model, BlockingClient, Error};

#[model]
struct Person {
    name: String,
    age: u8,
}

fn main() -> Result<(), Error> {
    let client = BlockingClient::from_provider("openai/gpt-6-luna")?;
    let person: Person = client.create("Extract the person. Ada Chen is 37 years old.")?;
    println!("{} is {}", person.name, person.age);
    Ok(())
}
```

These APIs are in the workspace checkout. To use them from a sibling Cargo project, add the path dependency below to its `Cargo.toml`.

```toml
[dependencies]
instructor-ai = { path = "../instructor-rs/instructor" }
```

## Try it

The source for each command is a Cargo example. [Runnable examples](instructor/examples)

Replay a fragmented JSON response locally and see typed previews followed by a validated `Person`.

```sh
cargo run -p instructor-ai --example replay
```

For live extraction, set the credential for the selected provider. The examples work with model IDs from OpenAI, Anthropic, OpenRouter, Gemini, Cohere, and Mistral. Each command starts a separate provider request. [Provider IDs and credentials](docs/providers.md)

```sh
cargo run -p instructor-ai --example extract -- openai/gpt-6-luna
cargo run -p instructor-ai --example stream_partial -- anthropic/claude-haiku-5-5
cargo run -p instructor-ai --example stream_iterable -- openrouter/google/gemini-3.8-flash
cargo run -p instructor-ai --example validate_quote -- openai/gpt-6-luna
```

## Guides

- Choose a blocking or async client and configure your provider. [Getting started](docs/index.md) · [Providers and modes](docs/providers.md)
- Derive schemas, write context-aware validators, and inspect the raw completion. [Response types](docs/types.md) · [Validation and retries](docs/concepts.md)
- Display fields as they arrive or process a sequence of completed records. [Streaming](docs/streaming.md)
- Supply media to supported routes. [Media input](docs/media.md)
