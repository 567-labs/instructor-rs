# Configure a provider

Choose the service that runs your model and set its API key in your application's environment. Pass the corresponding `provider/model` string to `Client::from_provider` (async) or `BlockingClient::from_provider`. The same Rust type works across providers. Start with the default mode for your provider.

## Select the model and credential

| Provider | Example argument | Environment variable | Default mode | HTTP API |
| --- | --- | --- | --- | --- |
| OpenAI | `openai/gpt-6-luna` | `OPENAI_API_KEY` | `JsonSchema` | `/v1/responses` |
| Anthropic | `anthropic/claude-haiku-5-5` | `ANTHROPIC_API_KEY` | `Tools` | `/v1/messages` |
| OpenRouter | `openrouter/openai/gpt-6.2-sol` | `OPENROUTER_API_KEY` | `JsonSchema` | `/api/v1/responses` |
| Google Gemini | `google/gemini-4-hydrogen` | `GOOGLE_API_KEY` or `GEMINI_API_KEY` | `JsonSchema` | `/v1/interactions` |
| Cohere | `cohere/command-a-plus-05-2026` | `COHERE_API_KEY` | `Tools` | `/v2/chat` |
| Mistral | `mistral/mistral-large-4` | `MISTRAL_API_KEY` | `JsonSchema` | `/v1/responses` |

Use a model available on your account that supports the selected output mode. From a checkout of this repository, try one of the following commands.

```sh
cargo run -p instructor-ai --example extract -- openai/gpt-6-luna
cargo run -p instructor-ai --example stream_partial -- anthropic/claude-haiku-5-5
cargo run -p instructor-ai --example stream_iterable -- openrouter/openai/gpt-6.2-sol
```

The `gemini/` prefix is an alias for `google/`. Both use the Gemini Developer API, with `GOOGLE_API_KEY` taking precedence. The client sets `stream` in the request body for incremental results.

## Choose an output mode

The mode controls how the provider receives the schema and encodes the output. Local parsing and validation apply in each mode.

| Rust mode | When to use it |
| --- | --- |
| `Mode::Tools` | Ask a model that supports function calling to return arguments to a named tool. The arguments form the result. |
| `Mode::ToolsStrict` | Require strict tool arguments with OpenAI, Mistral Responses or a compatible OpenRouter route. |
| `Mode::ResponsesTools` | Accept a named extraction function or schema-constrained final text on OpenAI or OpenRouter. |
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
    .option("max_output_tokens", 2048);
    let count: u32 = client.create(request)?;
    println!("{count}");
    Ok(())
}
```

Use `ProviderConfig::new(Provider::Anthropic, model, api_key)` for an explicitly supplied credential. To use a compatible endpoint or a local test server, pass its base including the API version to `with_base_url`, for example `http://localhost:8080/v1` for an OpenAI-compatible endpoint. The client appends the operation path and sends the configured credential there.

To reuse an existing transport and connection pool, pass a Reqwest client to `Client::with_http_client(config, http.clone())?`. Use `BlockingClient::from_client(client)?` to wrap a configured client for callers outside an async runtime.

### Allow an optional field to be omitted

In `JsonSchema` mode, OpenAI, OpenRouter and Mistral Responses require each property in a strict schema. For an `Option<T>` the model can return `null`. If your model should be free to omit a property, or you need open-ended map keys, pass `Request::new(prompt).strict(false)` to `create`. Serde and your validators still check the result. The setting applies to that request, including its retries. You can also set it for a stream or a parallel call. In `Tools` mode, use `.strict(true)` to require every field. [Required and nullable fields](types.md#require-a-field-that-accepts-null) · [Batch settings](batch.md)

## OpenAI

For focused extraction, start with `openai/gpt-6-luna`. For work that needs more reasoning, try `openai/gpt-6.2-sol`. Both use the Responses API. Instructor requests a structured text answer using your Rust type. Choose `Mode::Tools` for function arguments, or call `create_parallel` to receive several named calls. For classification, probability, or scoring, use Decisions with `gpt-6-luna`. [Model selection](https://developers.openai.com/api/docs/guides/latest-model) · [GPT-6 Luna](https://developers.openai.com/api/docs/models/gpt-6-luna) · [Decisions](decisions.md)

To let the model search before answering, supply a built-in tool. This example also limits the output and keeps the original response so you can inspect its sources and usage. Add `serde_json = "1"` for the `json!` macro.

```rust
use instructor_ai::{model, BlockingClient, Error, Request};
use serde_json::json;

#[model]
struct Release {
    version: String,
    source_url: String,
}

fn main() -> Result<(), Error> {
    let client = BlockingClient::from_provider("openai/gpt-6-luna")?;
    let request = Request::new("Find the latest stable Rust release on rust-lang.org and its announcement URL.")
        .option("tools", json!([{"type": "web_search"}]))
        .option("max_output_tokens", 2048);
    let (release, raw) = client.create_with_completion::<Release>(request)?;
    println!("{} {} ({:?} tokens)", release.version, release.source_url, raw["usage"]["total_tokens"]);
    Ok(())
}
```

### Continue a conversation

Responses requests default to `store: false`. To continue locally, build a `Request::native_messages` from your previous input followed by the items in the last response's `output` array. Include any function results with their matching `call_id`, then add your next user message. Keep the output items in order to preserve reasoning and assistant message phases. Instructor does this for you during validation retries. On OpenAI, you can opt into stored responses with `.option("store", true)` on the initial request and supply its ID as `previous_response_id` on the next. OpenRouter and Mistral use local history. [OpenAI conversation state](https://developers.openai.com/api/docs/guides/conversation-state) · [OpenRouter Responses](https://openrouter.ai/docs/api/reference/responses/overview) · [Mistral Responses](https://docs.mistral.ai/inference/conversations/responses)

### Connect to a Chat Completions gateway

When a gateway requires `/chat/completions`, set `.with_chat_completions()` on its `ProviderConfig`. If it expects a named function, also select `.with_mode(Mode::Tools)`. Set both before building the client. Applications reading raw completions should expect an `output` array on Responses and `choices` on Chat Completions. You can revalidate saved replies from either endpoint locally. [OpenAI migration guide](https://developers.openai.com/api/docs/guides/migrate-to-responses) · [Parse a saved completion](types.md#parse-a-saved-completion)

## Anthropic

Claude uses the Messages API version `2026-10-21` for generation, streaming, and batches. Set `max_tokens` for the generation limit. When porting a request, remove `temperature`, `top_p`, and `top_k`. For native histories, place documents in user turns and keep text citations in assistant turns. [Anthropic migration guide](https://platform.claude.com/docs/en/api/migration-2026-10-21)

Messages sent with `Message::system` become Anthropic's separate system prompt. For a Claude model that supports adaptive thinking, the `thinking` option in tool mode lets the model reason before calling the extraction function. The client selects automatic tool choice for thinking. Add `serde_json = "1"` to an external project's dependencies to construct options using `json!`.

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

With thinking enabled or a model that requires automatic tool selection, Instructor asks Claude to return only the tool call. To let Claude include explanatory text alongside its call, supply `.option("tool_choice", json!({"type": "auto"}))` and read the text from `create_with_completion`. The typed result requires a matching call. For an incremental display, use `create_partial` to read its arguments as they arrive. [Claude tool selection](https://platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools) · [Typed snapshots](streaming.md)

### Reuse a prompt prefix

For Anthropic Messages or a compatible OpenRouter route using `.with_chat_completions()`, `Message::system(long_prefix).cache()` puts an ephemeral prompt cache marker on the last content block of that message. The default lifetime is five minutes. Use `.cache_for(CacheTtl::OneHour)` for one hour. A user message with media can also have a marker after its last attachment. The provider decides eligibility, including minimum token count and supported TTL. Reuse an identical prefix before a changing user question and inspect the raw completion's usage to see whether it hit the provider cache. This mechanism reuses provider work on the prefix. The local response cache reuses a complete answer. [Response cache](concepts.md#observe-attempts-and-reuse-responses)

## OpenRouter

Use your OpenRouter key with the full model name from its catalog. For `openrouter/openai/gpt-6.2-sol`, Instructor sends `openai/gpt-6.2-sol` as the model to OpenRouter with the OpenRouter credential. Choose a route offering the requested tool or schema capability. Set `require_parameters` to route the request only to backends that support the requested parameters. [OpenRouter quickstart](https://openrouter.ai/docs/quickstart) · [Model catalog](https://openrouter.ai/models)

```rust
use instructor_ai::{BlockingClient, Error, Request};
use serde_json::json;

fn main() -> Result<(), Error> {
    let client = BlockingClient::from_provider("openrouter/openai/gpt-6.2-sol")?;
    let request = Request::new("List the attendees. Ali, Chen and Bo all attended.")
        .option("provider", json!({"require_parameters": true}));
    let names: Vec<String> = client.create(request)?;
    println!("{names:?}");
    Ok(())
}
```

For Jev via OpenRouter, use the Decisions client with a decision model. [Jev on OpenRouter](decisions.md)

## Gemini

The Gemini Developer API uses `POST /v1/interactions` for every output mode. Google recommends Interactions for new applications and made the stable `v1` version the SDK default in November 2026. The default `JsonSchema` mode supplies a JSON Schema in `response_format`. Set `Mode::Tools` for function calls, including parallel extraction. [API versions](https://aistudio.google.com/api-reference/api-versions) · [Interaction reference](https://aistudio.google.com/api-reference/interactions-api) · [Gemini 4 Hydrogen](https://aistudio.google.com/docs/models/gemini-4-hydrogen)

Use `.option("generation_config", serde_json::json!({"thinking_level":"low"}))` to configure thinking on Hydrogen. To migrate a `generationConfig.maxOutputTokens` setting, supply `generation_config.max_output_tokens`, which caps both thinking and answer tokens. For native history, pass interaction steps to `Request::native_messages`. Typed messages build these steps for you. Requests default to `store: false` and replay the model's signed steps with validation feedback on retries. Saved `generateContent` completions remain readable through `parse_completion`. [Migration guide](https://aistudio.google.com/docs/migrate-to-interactions) · [Stateless function calling](https://aistudio.google.com/docs/function-calling#stateless-function-calling)

Run the partial example to see fields arrive from text deltas. In tool mode the client reads argument deltas from the selected function call. The final value requires both the end of the selected step and a successful interaction status. A `requires_continuation` status produces `Error::Incomplete`. [Streaming events](https://aistudio.google.com/docs/streaming)

```sh
cargo run -p instructor-ai --example stream_partial -- google/gemini-4-hydrogen
```

## Cohere and Mistral

- **Cohere** uses the v2 Chat API and a native required tool call in `Tools` mode. Select a model that supports `tool_choice`. [Cohere models](https://docs.cohere.com/v2/docs/models)
- **Mistral** uses stateless Responses for generation and streaming. Use a vision-capable model for images or PDFs. Its batch service accepts Chat Completions requests, which Instructor prepares when you call `batch`. [Mistral Responses](https://docs.mistral.ai/inference/conversations/responses) · [Mistral batch service](https://docs.mistral.ai/api/endpoint/batch)

Provider-specific options use the field names of the native HTTP API. Each client manages model selection, messages, streaming, and schema placement. The shared message type carries text and user-supplied attachments. [Media input](media.md) · [Streaming](streaming.md) · [Validation and retries](concepts.md) · [Scope of the Python comparison](parity.md)
