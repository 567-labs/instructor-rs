# Extract your first typed response

Ask for a `Person` and use `person.age` as a `u8` in your application. Instructor derives the schema from your type, sends it with your prompt, and checks that the model supplies both fields and that the age fits in a byte.

## Add Instructor to your project

Run this command in your application's directory. For a new executable, start with `cargo new people` and `cd people`. The Git revision selects the version described throughout these guides. The current crates.io release predates these APIs.

```sh
cargo add instructor-ai --git https://github.com/567-labs/instructor-rs \
  --rev 278dcced36a2b5b076d7b1fd20606eaabc7a75de
```

The default build includes all six generation providers, streaming, Decisions and the model derives. For a project beside a local checkout, you can use `cargo add instructor-ai --path ../instructor-rs/instructor`.

## Call from Tokio

Use `Client` for async requests. To try this in a new executable, add Tokio and replace `src/main.rs` with the program below.

```sh
cargo add tokio@1 --features macros,rt-multi-thread
```

```rust
use instructor_ai::{model, Client, Error};

#[model]
struct Person {
    name: String,
    age: u8,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client = Client::from_provider("openai/gpt-6-luna")?;
    let person = client.create::<Person>("Extract the person. Ada Chen is 37.").await?;
    println!("{} is {}", person.name, person.age);
    Ok(())
}
```

Set `OPENAI_API_KEY` in your shell and run `cargo run`. The program asks the model to extract the name and age, then prints a line such as `Ada Chen is 37`. To use Claude, change the model to `anthropic/claude-haiku-5-5` and set `ANTHROPIC_API_KEY`. [Model IDs and credentials](providers.md)

## Call from synchronous code

Use `BlockingClient` for a CLI or other synchronous application. This version takes the model as an argument and only needs the `instructor-ai` dependency. With the program in `src/main.rs` and `OPENAI_API_KEY` set, run `cargo run -- openai/gpt-6-luna`.

```rust
--8<-- "instructor/examples/extract.rs"
```

## Try a stream without a key

From the root of your Instructor repository checkout, replay a response split into fragments. Watch the name and age appear in typed previews, followed by a completed `Person`. To copy the program into your own project, use the source in the streaming guide. [Replay a stream locally](streaming.md#replay-fragments-locally)

```sh
cargo run -p instructor-ai --example replay
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

If you import `from_openai` or `InstructorClient`, add `--features legacy-openai` to the `cargo add instructor-ai` command when upgrading. This includes the `openai-api-rs` SDK and its HTTP and TLS dependencies.

To migrate, put the model name in `BlockingClient::from_provider`, replace `chat_completion::<T>(request, attempts)` with `create::<T>(prompt)`, and use `#[model]` on your data types. For a limit of three total attempts, pass `Request::new(prompt).max_retries(2)` to `create`. Remove the feature after you migrate the calls. [Response types](types.md) · [Validation and retries](concepts.md)
