# Configure a provider

OpenAI, Anthropic, and OpenRouter are the primary integrations. Gemini, Cohere, and Mistral complete the set. The five direct providers come from the original Rust roadmap and share a Python test contract for list extraction, nesting, validation, raw completions, and both kinds of streaming. OpenRouter adds a gateway to routed models and Jev. [Rust roadmap at the comparison baseline](https://github.com/567-labs/instructor-rs/blob/cb9e730e312b4c8653afd04b613ff482c3033587/docs/index.md) · [Python provider capabilities](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/llm/test_core_providers/capabilities.py)

## Select the model and credential

`Client::from_provider` and `BlockingClient::from_provider` split a `provider/model` identifier at the first slash and look up the selected provider's key. Use the credential for the provider you are calling. The model IDs below come from the Python guide tests. Model availability and mode support depend on your account and model. [Python guide model IDs](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/llm/test_current_doc_models.py)

| Provider | Example argument | Environment variable | Default mode | HTTP API |
| --- | --- | --- | --- | --- |
| OpenAI | `openai/gpt-6-luna` | `OPENAI_API_KEY` | `Tools` | `/v1/chat/completions` |
| Anthropic | `anthropic/claude-haiku-5-5` | `ANTHROPIC_API_KEY` | `Tools` | `/v1/messages` |
| OpenRouter | `openrouter/google/gemini-3.8-flash` | `OPENROUTER_API_KEY` | `Tools` | `/api/v1/chat/completions` |
| Google Gemini | `google/gemini-3.8-flash` | `GOOGLE_API_KEY` or `GEMINI_API_KEY` | `JsonSchema` | `/v1beta/models/{model}:generateContent` |
| Cohere | `cohere/command-a-03-2025` | `COHERE_API_KEY` | `Tools` | `/v2/chat` |
| Mistral | `mistral/mistral-small-latest` | `MISTRAL_API_KEY` | `Tools` | `/v1/chat/completions` |

Pass any row's argument to the extraction examples. Each of the commands below uses one provider's credential.

```sh
cargo run -p instructor-ai --example extract -- openai/gpt-6-luna
cargo run -p instructor-ai --example stream_partial -- anthropic/claude-haiku-5-5
cargo run -p instructor-ai --example stream_iterable -- openrouter/google/gemini-3.8-flash
```

The `gemini/` prefix is an alias for `google/`. Both use the Gemini Developer API, with `GOOGLE_API_KEY` taking precedence. Gemini streams via `:streamGenerateContent?alt=sse`. The other APIs select streaming through a field in the request body.

## Choose an output mode

The mode controls how the provider receives the schema and encodes the output. Local parsing and validation apply in each mode.

| Rust mode | When to use it |
| --- | --- |
| `Mode::Tools` | Ask a model that supports function calling to return arguments to a named tool. The arguments form the result. |
| `Mode::ToolsStrict` | Use strict function calling with OpenAI or a compatible OpenRouter route. |
| `Mode::ResponsesTools` | Use the OpenAI Responses API with a named extraction function. |
| `Mode::JsonSchema` | Ask the provider to constrain its text output with its native JSON Schema setting. |
| `Mode::Json` | Put the schema in the prompt and request JSON syntax where the provider offers it. |
| `Mode::MarkdownJson` | Put the schema in the prompt and read JSON from text or a Markdown code fence. |

`ProviderConfig::from_env` uses the same credentials as `from_provider`. Set the mode before constructing the client. Provider parameters go on the request.

```rust
use instructor_ai::{BlockingClient, Error, Message, Mode, ProviderConfig, Request};

fn main() -> Result<(), Error> {
    let config = ProviderConfig::from_env("openai/gpt-6-luna")?
        .with_mode(Mode::JsonSchema);
    let client = BlockingClient::new(config)?;
    let request = Request::messages(vec![
        Message::system("Extract the count as an integer."),
        Message::user("There were forty-two visitors."),
    ])
    .option("max_completion_tokens", 2048);
    let count: u32 = client.create(request)?;
    println!("{count}");
    Ok(())
}
```

Use `ProviderConfig::new(Provider::Anthropic, model, api_key)` for an explicitly supplied credential. To use a compatible endpoint or a local test server, pass its base including the API version to `with_base_url`, for example `http://localhost:8080/v1` for an OpenAI-compatible chat endpoint. The client appends the operation path and sends the configured credential there.

To reuse an existing transport and connection pool, pass a Reqwest client to `Client::with_http_client(config, http.clone())?`. Use `BlockingClient::from_client(client)?` to wrap a configured client for callers outside an async runtime.

### Set strictness for one Chat request

For OpenAI Chat and OpenRouter, `Request::new(prompt).strict(true)` closes objects and requires every named property in the transmitted schema, including nested properties. An optional field can take `null`. Use `.strict(false)` to send the model’s original required fields and allow omitted optional properties or open-ended maps. Serde and local validators check the answer in either case. [Python wire tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_public_strict_schemas.py)

`Tools` defaults to permissive, and `ToolsStrict` and `JsonSchema` default to strict. Either client accepts an override for one typed call, including streams and the individual functions in a parallel call. An OpenAI batch item can override the batch setting. The equivalent `.option("strict", value)` accepts a boolean. Other providers, modes and raw calls return `Error::Configuration` for a request-level override. [Batch defaults](batch.md)

## OpenAI

The `Tools` default uses Chat Completions. Choose `ToolsStrict` for strict function arguments or `JsonSchema` for schema-constrained text. Native strict schemas require closed objects and a value for every property, with nullable fields represented using `null`.

`ResponsesTools` selects `/v1/responses` and adds an extraction function with matching structured text format. Use the same `create`, `create_with_completion`, `create_partial`, and `create_iterable` methods with that config. The provider option for the generation limit is `max_output_tokens`.

```rust
use instructor_ai::{BlockingClient, Error, Mode, ProviderConfig, Request};

fn main() -> Result<(), Error> {
    let config = ProviderConfig::from_env("openai/gpt-6-luna")?
        .with_mode(Mode::ResponsesTools);
    let client = BlockingClient::new(config)?;
    let request = Request::new("How many guests arrived? A dozen guests arrived.")
        .option("max_output_tokens", 2048);
    let (count, raw) = client.create_with_completion::<u32>(request)?;
    println!("Guests {count}, usage {}", raw["usage"]);
    Ok(())
}
```

To let the model use a server-managed tool before extraction, add `.option("tools", serde_json::json!([{"type": "web_search"}]))` to the request. The client parses extraction function arguments or final structured output text. Use the Decisions client for typed judgments from the OpenAI Decisions endpoint. [Decisions](decisions.md)

For conversations containing prior tool results or other native input items, use `Request::native_messages`. Pass JSON objects shaped for the selected API. For example, in Responses mode, supply a previous response ID and its matching tool call ID.

```rust
use instructor_ai::Request;
use serde_json::json;

fn continuation(response_id: &str, call_id: &str, output: &str) -> Request {
    Request::native_messages(vec![json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output
    })])
    .option("previous_response_id", response_id)
}
```

Native messages go to the provider as supplied. Typed messages added to `request.messages` follow the native items and support template rendering. For a separate system prompt on Anthropic or Gemini, use a typed system message.

## Anthropic

Messages sent with `Message::system` become Anthropic's separate system prompt. Set `max_tokens` for the generation limit. For a Claude model that supports adaptive thinking, the `thinking` option in tool mode lets the model reason before calling the extraction function. The client selects automatic tool choice for thinking. Add `serde_json = "1"` to an external project's dependencies to construct options using `json!`.

```rust
use instructor_ai::{BlockingClient, Error, Request};
use serde_json::json;

fn main() -> Result<(), Error> {
    let client = BlockingClient::from_provider("anthropic/claude-haiku-5-5")?;
    let request = Request::new("We booked six tables of eight guests. How many seats?")
        .option("max_tokens", 2048)
        .option("thinking", json!({"type": "adaptive"}));
    let seats: u32 = client.create(request)?;
    println!("{seats}");
    Ok(())
}
```

For an incremental display, pass the same model ID to the `stream_partial` example. The parser reads streamed tool arguments. [Typed snapshots](streaming.md)

### Reuse a prompt prefix

For Anthropic or a compatible OpenRouter route, `Message::system(long_prefix).cache()` puts an ephemeral prompt cache marker on the last content block of that message. The default lifetime is five minutes. Use `.cache_for(CacheTtl::OneHour)` for one hour. A user message with media can also have a marker after its last attachment. The provider decides eligibility, including minimum token count and supported TTL. Reuse an identical prefix before a changing user question and inspect the raw completion's usage to see whether it hit the provider cache. This mechanism reuses provider work on the prefix. The local response cache reuses a complete answer. [Response cache](concepts.md#observe-attempts-and-reuse-responses)

## OpenRouter

The first path component selects the gateway. For `openrouter/google/gemini-3.8-flash`, Instructor sends `google/gemini-3.8-flash` as the model to OpenRouter with the OpenRouter credential. Choose a route offering the requested tool or schema capability. To reproduce Python's `extra_body={"provider": {"require_parameters": True}}`, set the native `provider` field on the Rust request. [OpenRouter model catalog](https://openrouter.ai/models)

```rust
use instructor_ai::{BlockingClient, Error, Request};
use serde_json::json;

fn main() -> Result<(), Error> {
    let client = BlockingClient::from_provider("openrouter/google/gemini-3.8-flash")?;
    let request = Request::new("List the attendees. Ali, Chen and Bo all attended.")
        .option("provider", json!({"require_parameters": true}));
    let names: Vec<String> = client.create(request)?;
    println!("{names:?}");
    Ok(())
}
```

For Jev via OpenRouter, use the Decisions client with a decision model. [Jev on OpenRouter](decisions.md)

## Gemini, Cohere, and Mistral

- **Gemini** sends text incrementally in `JsonSchema` mode. Its tool mode returns function arguments as an object, so previews can arrive together. Pass generation settings under `generationConfig`, for example `.option("generationConfig", serde_json::json!({"maxOutputTokens": 2048}))`.
- **Cohere** uses the v2 Chat API and a native required tool call in `Tools` mode. Select a model that supports `tool_choice`.
- **Mistral** uses its own Chat Completions endpoint. Check the model's function calling or structured output support when choosing a mode.

Provider-specific options use the field names of the native HTTP API. Each client manages model selection, messages, streaming, and schema placement. The shared message type carries text and user-supplied attachments. [Media input](media.md) · [Streaming](streaming.md) · [Validation and retries](concepts.md) · [Scope of the Python comparison](parity.md)
