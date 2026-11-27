# Validate a response and ask for a correction

The requested type gives the provider a schema and gives the local parser a Rust target. After deserialization, Instructor runs nested and field validators, then the whole-model validators. A failed parse or validation can trigger another request that includes feedback. This is useful when a schema checks the shape of a value and your application also needs to check its meaning.

## Check a field

Use `#[validate(custom = "function_name")]` on a field or a model. The function accepts a reference to the value and returns an error with a message the provider can use to correct it. Run the same check locally with `parse_response` and an empty context.

```rust
use instructor_ai::{model, parse_response, Error};
use serde_json::Value;

#[model]
struct Reservation {
    #[validate(custom = "check_count")]
    guests: u16,
}

fn check_count(guests: &u16) -> Result<(), String> {
    if *guests == 0 {
        return Err("Supply a positive number of guests.".into());
    }
    Ok(())
}

fn main() {
    let result = parse_response::<Reservation>(r#"{"guests": 0}"#, &Value::Null);
    assert!(matches!(result, Err(Error::Validation(_))));
}
```

## Validate against the source

Give the request a JSON context to use in both message templates and validators. This example asks the model to cite a specific passage. The validator checks the returned text against the original passage. A fabricated quote causes one additional attempt with an explanation.

```sh
cargo run -p instructor-ai --example validate_quote -- openai/gpt-6-luna
```

```rust
--8<-- "instructor/examples/validate_quote.rs"
```

`Request::context` accepts a `serde_json::Value`. Add `serde_json = "1"` to use the `json!` macro in your application. With a context set, MiniJinja renders the messages before sending them. An undefined variable in the template returns a configuration error before the first request. The validator receives the original context with a mutable reference to the field or model, allowing normalization. To pass source text to the model, include it in the template, as in `{{ source }}`.

`create_with_completion` returns the validated value paired with the successful provider response as JSON. Use `create_raw` for an ordinary provider request with no output schema. Raw response fields follow the provider's native format.

## Validate inside an async workflow

`Client::create_validated` accepts a function that returns a future. It receives the typed output after the model's validators pass. Return a value to accept the result, or an error message to ask for a correction. An async function can query a database or service as part of the check. The blocking client accepts a synchronous function.

```rust
use instructor_ai::{Client, Error, Request};

async fn check_room(room: String) -> Result<String, String> {
    let rooms = ["Cedar", "Oak"];
    if !rooms.contains(&room.as_str()) {
        return Err("Select Cedar or Oak exactly as named on the schedule.".into());
    }
    Ok(room)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let client = Client::from_provider("anthropic/claude-haiku-5-5")?;
    let request = Request::new("Which room holds the review? The review is in Cedar.")
        .max_retries(1);
    let room = client.create_validated(request, check_room).await?;
    println!("{room}");
    Ok(())
}
```

## Check text with a model

`validate_text` sends a candidate and rule to the configured generation model. A passing check returns the original text. When overrides are enabled, a rejected candidate can produce the model's suggested replacement. Returning the replacement ends the check. If the rule fails without a permitted replacement, the call returns `Error::Validation`. A rule can be a string, or a `TextRule` with options. Claude uses the model’s default sampling. For other providers, `TextRule` sets the temperature to `0.0`. Use `.temperature(None)` to select the model’s default.

`moderate_text` calls OpenAI's moderation endpoint with `omni-moderation-latest`. Flagged text returns a validation error describing the categories. The following example performs two requests on success.

```rust
use instructor_ai::{BlockingClient, Error, TextRule};

fn main() -> Result<(), Error> {
    let client = BlockingClient::from_provider("openai/gpt-6-luna")?;
    let rule = TextRule::new("Use a single complete sentence.")
        .allow_override(true)
        .temperature(None);
    let sentence = client.validate_text(rule, "Arriving tomorrow")?;
    let approved = client.moderate_text(&sentence)?;
    println!("{approved}");
    Ok(())
}
```

Both clients expose the checks. To use one inside `create_validated`, pass it the candidate text in the validation callback and map a validation failure to the callback's error message. Each check makes its own provider request. [Python semantic validation](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/semantic_validation.md)

## Set retry limits

- `max_retries(0)` allows one attempt. `max_retries(2)` allows three attempts. The default is three additional attempts. The limit applies separately to each call.
- `token_budget(n)` checks cumulative usage after a rejected typed response. Once usage reaches the budget or cannot be determined, Instructor stops asking for corrections. A successful response can exceed the budget. Use the provider's generation limit to bound an individual response. A budget must be positive and requires a non-streaming structured request.
- The HTTP timeout applies per attempt, including reading its body. `Client::with_timeout(config, duration)` customizes the default of 180 seconds. To bound a whole async operation, wrap the operation, including any stream consumption, in `tokio::time::timeout`.
- By default, parse and validation failures can trigger a correction. A stream can retry before returning the first value. Once a caller receives a value, further errors terminate that stream.
- `Request::retry_timeout(duration)` sets an elapsed-time limit for starting retries. `retry_policy` takes a closure examining a `FailedAttempt`. Return `None` to stop or `Some(delay)` to retry, including for a selected transport error. Attempt, time, and token limits still apply. Refusals and incomplete output end the operation.

When corrections stop, `Error::Retry` includes a `StopReason`, the optional cumulative token count, and ordered `FailedAttempt` values. Each rejected completion includes the attempt number, native response, and local error. A token count of `None` indicates unavailable usage. The last successful raw completion is available via `create_with_completion`.

## Diagnose a failed HTTP request

For an incident report, call `error.http_details()` to recover the status, provider request ID and machine-readable code. This works for generation, opening a stream, moderation, batch operations and `DecisionError`. On `Error::Retry` it examines the final attempt. To inspect an earlier failure, call `attempt.error.http_details()` on that entry in `attempts`.

```rust
use instructor_ai::Error;

fn report(error: &Error) {
    if let Some(details) = error.http_details() {
        eprintln!("HTTP {} request={} code={}",
            details.status,
            details.request_id.as_deref().unwrap_or("unavailable"),
            details.code.as_deref().unwrap_or("unavailable"));
    }
}
```

OpenAI supplies `x-request-id`, and Anthropic supplies `request-id`. A top-level `request_id` in JSON serves as a fallback. The accessor reads `error.code`, then a top-level `code`, then `error.type` when the code field is absent. Numeric codes, such as OpenRouter's `429`, become decimal strings. Missing details are `None`. A plain-text failure can still carry a request ID in its headers. For the response body, match `Error::Http { status, body, .. }` or `DecisionError::Http { status, body, .. }`. Add `..` to existing patterns that list only status and body. [Python Responses error contract](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_responses_sdk_contract.py#L174)

For accounting on success, call `create_with_metadata::<T>`. Its `Completion<T>` exposes `value`, `raw`, `attempts`, and `total_tokens`. The total covers the attempts of the current operation and is available when all attempts report usage. A cache hit has zero attempts and `Some(0)` tokens, while the raw response includes the original provider metadata.

## Account for tokens across retries

Call `create_with_usage::<T>` for a `(Completion<T>, TokenUsage)` pair. The counts include rejected responses and the final success. To inspect a single response, pass its native envelope to `TokenUsage::from_response(provider, &result.raw)`. On retry failure, `error.retry_usage(provider)` reconstructs the counts from the saved history. A local response-cache hit returns `TokenUsage::ZERO` for the current call.

```rust
use instructor_ai::{BlockingClient, Error, Provider, Request, TokenUsage};

fn main() -> Result<(), Error> {
    let client = BlockingClient::from_provider("openai/gpt-6-luna")?;
    let req = Request::new("How many guests? Five guests attended.")
        .max_retries(1)
        .on_usage(|attempt, counts| {
            eprintln!("Attempt {attempt}, cumulative tokens {:?}", counts.total_tokens);
        });
    let (result, counts) = client.create_with_usage::<u32>(req)?;
    let last = TokenUsage::from_response(Provider::OpenAI, &result.raw);
    println!("{} guests after {} attempts", result.value, result.attempts);
    println!("All attempts {counts:?}, final response {last:?}");
    Ok(())
}
```

For a typed non-streaming request, `on_usage` receives an owned snapshot after each provider response and before validation, or at attempt zero on a local cache hit. Register it on either client or on a single request. Use `add_usage_hook` for a removable client registration. On the async client, await `create_with_usage`.

Each `Option<u64>` tracks one counter. `Some(0)` represents a known zero. `None` means that at least one attempt lacked the information needed for that counter. A response reporting only a total contributes to `total_tokens`. If a response omits usage entirely or an HTTP attempt fails before returning a completion, all cumulative counts become unknown. A native total takes precedence where the provider supplies one. Deriving a total requires both inclusive input and output counts, so a token budget stops further corrections when either is missing and the total is unavailable.

| Provider | How to interpret the counts |
| --- | --- |
| OpenAI Chat, OpenRouter, Mistral | Prompt and completion counts map to input and output. Prompt details can supply `cache_read_tokens` and `cache_write_tokens`. Completion details can supply `reasoning_tokens`. |
| OpenAI Responses | Input and output fields map directly, including cache and reasoning details. |
| Anthropic | Input includes uncached input plus reported cache reads and creation. For 60 uncached, 10 read, 30 created and 50 output, total usage is 150. The 5-minute and 1-hour portions of creation are available as `cache_write_5m_tokens` and `cache_write_1h_tokens`. Output includes thinking. |
| Gemini | Input includes cached content. Output includes candidate and thought tokens. |
| Cohere | Input and output come from `usage.tokens`. The raw response carries billed units separately. |

Cache and reasoning counts describe portions of the inclusive counts. Use `total_tokens` to avoid counting those portions twice. Run `cargo test --offline -p instructor-ai --test providers usage::` for loopback examples covering all six providers and both OpenAI routes. [Python usage](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/usage.md) · [Python accounting tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/test_usage_accounting_contracts.py)

## Observe attempts and reuse responses

Install a hook with `on_event` on a client or request. The callback receives the attempt number and a borrowed `Event`. For structured calls it can inspect prepared request bodies, provider responses, cumulative token usage, parse errors, completion errors, and exhausted retries. Attempts start at one, and a cache hit uses zero. Hooks run inline.

To manage registrations on a mutable client, save the handle returned by `add_hook(callback)`. Pass it to `remove_hook(&handle)`, or use `clear_hooks()` to remove all client callbacks. Cloning a client copies its registrations into an independently editable list. Request-local hooks run with the client callbacks for the duration of that request.

```rust
use instructor_ai::{BlockingClient, Error, Event, Request};

fn main() -> Result<(), Error> {
    let client = BlockingClient::from_provider("openrouter/google/gemini-3.8-flash")?
        .with_cache(16)
        .on_event(|attempt, event| match event {
            Event::Usage { total_tokens } => eprintln!("Attempt {attempt}, tokens {total_tokens}"),
            Event::CacheHit => eprintln!("Reused completion"),
            _ => {}
        });
    let request = Request::new("How many guests? Five guests attended.")
        .max_retries(1)
        .token_budget(1024);
    let first: u32 = client.create(request.clone())?;
    let second: u32 = client.create(request)?;
    assert_eq!(first, second);
    Ok(())
}
```

The in-memory response cache is opt-in and bounded by entry count. Clones of a client share its cache. A key includes the response type, prepared request body, and context. A hit re-parses the cached provider JSON and runs validation again. New entries evict the least recently used entry when full. This cache applies to successful non-streaming structured responses. The schema cache separately reuses the derived schema for a Rust type across requests.

Use `.with_disk_cache(path, namespace)?` to reuse completions across processes. Disk entries are atomically written JSON files containing the raw completion. The namespace and provider configuration, including the model, credential, endpoint, and mode, scope the cache. Choose a namespace that identifies the consuming application.

For all response caches, `request.cache_ttl(duration)` sets the lifetime on write. A hit retains its original expiration. A zero duration bypasses both reads and writes. `client.clear_cache()?` clears the active scope synchronously. On the async client, `client.clear_cache_async().await?` offloads storage I/O to Tokio's blocking pool.

### Share an application cache

Python accepts a thread-safe `BaseCache` through the `cache=` argument. In Rust, implement `ResponseCache` and pass an `Arc` to `.with_cache_backend(store, namespace)?` on either client. The adapter receives the same two SHA-256 digests as the disk store. The scope identifies the namespace and provider configuration. The entry key identifies the response type, prepared request body and validation context. Give tenants distinguished by custom HTTP headers separate namespaces. [Python cache interface](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/cache/__init__.py#L43) · [Python caching guide](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/caching.md)

This adapter serializes entries to JSON bytes in a process-local map. Its outer map is keyed by scope, so clearing one tenant leaves the others available. For a database, use `(scope, key)` as the composite primary key, replace a single row atomically on `put`, and restrict deletion to rows with the requested scope on `clear`.

```rust
--8<-- "instructor/tests/client/cache_backend.rs:adapter"
```

With `JsonStore` defined above, two clients can use the same record. The second call checks the cached provider response against the requested type and validators. `attempts` counts provider calls made by that invocation.

```rust
use instructor_ai::{BlockingClient, Request};
use std::{sync::Arc, time::Duration};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(JsonStore::default());
    let client = || {
        BlockingClient::from_provider("openai/gpt-6-luna")?
            .with_cache_backend(store.clone(), "guest-counts")
    };
    let req = Request::new("How many guests? Five guests attended.")
        .cache_ttl(Duration::from_secs(600));
    let first: u32 = client()?.create(req.clone())?;
    let second = client()?.create_with_metadata::<u32>(req)?;
    assert_eq!(second.value, first);
    assert_eq!(second.attempts, 0);
    client()?.clear_cache()?;
    Ok(())
}
```

`CacheRecord.raw` holds the original provider envelope, including generated content and usage. `expires` holds an optional absolute `SystemTime`. The client checks that timestamp on every read. Configure eviction in your store to reclaim expired records. Expired entries and entries that fail parsing or validation trigger a fresh provider request. A successful response can replace the old record. Concurrent misses can each send a provider request, and an in-flight write can repopulate a cleared scope.

Both clients run adapter reads and writes on Tokio's blocking pool. An I/O error from `get` or `put` emits `Event::CacheError`. A failed read proceeds to the provider, and a failed write returns the validated result to the caller. Explicit clearing returns storage errors directly. Run the adapter contract checks locally with `cargo test -p instructor-ai --test client cache_backend::`.
