# Extract your first typed response

Instructor uses the type of your result to build the request and validate the answer. With `let person: Person = client.create(prompt)?;`, the fields are available as ordinary Rust values. A string prompt creates a single user message.

## Try the parser locally

From the repository root, replay a short response to see the difference between a preview and a completed model. This command runs entirely on local data.

```sh
cargo run -p instructor-ai --example replay
```

## Make a provider request

Set `OPENAI_API_KEY` in your environment and pass a model your account can access. The example takes the provider and model as a single argument.

```sh
cargo run -p instructor-ai --example extract -- openai/gpt-6-luna
```

`instructor/examples/extract.rs` defines the schema and makes the blocking request.

```rust
--8<-- "instructor/examples/extract.rs"
```

The provider sees the field names, their schema types, and the description on `name`. Serde checks that both fields are present and the age fits in `u8`. Pass `anthropic/claude-haiku-5-5` with `ANTHROPIC_API_KEY` to use the same model type with Claude. [Providers and credentials](providers.md)

## Use it in a Cargo project

For a project beside this checkout, add `instructor-ai` to the dependencies in `Cargo.toml`. Adjust the path for your directory layout. The `#[model]` attribute provides the derives for a response model.

```toml
[dependencies]
instructor-ai = { path = "../instructor-rs/instructor" }
```

The default build includes all six generation providers over HTTPS, both streaming APIs, Decisions, batches, validation, caching and the model derives.

Use `Client` inside a Tokio runtime. To run the following example, add `tokio = { version = "1", features = ["macros", "rt"] }` to the same dependencies.

```rust
use instructor_ai::{model, Client, Error};

#[model]
struct Person {
    name: String,
    age: u8,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let client = Client::from_provider("openai/gpt-6-luna")?;
    let person = client.create::<Person>("Extract the person. Ada Chen is 37.").await?;
    println!("{} is {}", person.name, person.age);
    Ok(())
}
```

## Choose an operation

Both clients expose the following methods. The async client returns a future that resolves to a `Result`. The blocking client returns the `Result` directly.

| Goal | Method | Value on success |
| --- | --- | --- |
| Read one model, scalar, or collection | `create::<T>(request)` | `T` |
| Inspect provider metadata | `create_with_completion::<T>(request)` | `(T, serde_json::Value)` |
| Count attempts and tokens | `create_with_metadata::<T>(request)` | `Completion<T>` |
| Read the provider response as JSON | `create_raw(request)` | `serde_json::Value` |
| Show a preview of one object | `create_partial::<T>(request)` | Stream or iterator of `Result<PartialEvent<T>, Error>` |
| Process completed records as they arrive | `create_iterable::<T>(request)` | Stream or iterator of `Result<T, Error>` |
| Apply a check after deserialization | `create_validated(request, check)` | `T` |
| Receive several named tool calls | `create_parallel::<T>(request)` | `Vec<T>` |

For choices, probabilities, and scores, use the dedicated Decisions client. [Decisions with OpenAI and Jev](decisions.md)

## Moving from the original Rust client

If you import `from_openai` or `InstructorClient`, enable the `legacy-openai` feature on your existing dependency when upgrading. This includes the `openai-api-rs` SDK and its HTTP and TLS dependencies.

```toml
[dependencies]
instructor-ai = { path = "../instructor-rs/instructor", features = ["legacy-openai"] }
```

To migrate, put the model name in `BlockingClient::from_provider`, replace `chat_completion::<T>(request, attempts)` with `create::<T>(prompt)`, and use `#[model]` on your data types. For a limit of three total attempts, pass `Request::new(prompt).max_retries(2)` to `create`. Remove the feature after you migrate the calls. [Response types](types.md) · [Validation and retries](concepts.md)

## Inspect the dependency graph

For a Linux x86-64 consumer with `instructor-ai` as its only dependency, Cargo 1.100.0 selected the following package versions. The counts include Instructor and its transitive normal and build dependencies. Each distinct package name and version counts once. The consumer's own package and test dependencies are excluded.

| Configuration | Packages | Build dependencies affected |
| --- | ---: | --- |
| Baseline at `e820e17` | 160 | Includes both the legacy SDK and the provider-neutral HTTP client. |
| Defaults | 152 | Provider requests use Reqwest with Rustls 0.23. |
| `features = ["legacy-openai"]` | 158 | Adds `openai-api-rs`, `minreq`, `rustls` 0.21, `rustls-webpki` 0.101, `sct` and `webpki-roots` 0.25. |

To reproduce the counts from a fresh workspace checkout with cached dependencies, use the recorded lockfile. Run the same command at the baseline to compare. The target platform, lockfile and other dependencies in your application affect the counts.

```sh
cp benchmarks/recorded/Cargo.lock Cargo.lock
count_packages() {
  cargo tree --locked --offline -p instructor-ai --edges normal,build \
    --prefix none --format '{p}' "$@" | awk '{print $1, $2}' | sort -u | wc -l
}
count_packages
count_packages --features legacy-openai
```
