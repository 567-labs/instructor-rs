# Port a Python Instructor application to Rust

Replace your Pydantic model with a Rust struct annotated with `#[model]`. Where Python passes `response_model=Ticket`, Rust infers `Ticket` from the result or accepts it as a type parameter. Select the provider with the same `provider/model` string. Use `Client` with `.await?` in a Tokio application, or `BlockingClient` with `?` in synchronous code. [First extraction](index.md) · [Port the field types](types.md)

| In your Python application | In an async Rust application | Guide |
| --- | --- | --- |
| Extract one ticket | `let ticket: Ticket = client.create(prompt).await?;` | [Validation and reasks](concepts.md) |
| Inspect the original API response | `let (ticket, raw) = client.create_with_completion::<Ticket>(prompt).await?;` | [Parse a saved response](types.md#parse-a-saved-completion) |
| Preview individual fields as they arrive | `client.create_partial::<Ticket>(prompt).await?` yields `PartialEvent::Partial` with optional fields, then `Complete(Ticket)` after validation. | [Typed partials](streaming.md) |
| Process a list as it arrives | `client.create_iterable::<Ticket>(prompt).await?` yields one validated ticket at a time. | [Iterable streaming](streaming.md) |
| Classify a ticket and score its urgency | Derive `DecisionModel` for your questions. Use `DecisionsClient::from_provider("openrouter/typesafe/jev-1.14")?` or select the direct Jev or OpenAI route. | [Decisions](decisions.md) |
| Run a provider-managed job for a dataset | Use `client.batch::<Ticket>()?`, save the job ID and parse the downloaded JSON Lines by custom ID. | [Batch jobs](batch.md) |

## Choose the generation API

The provider prefix chooses the endpoint and the default output mode. You supply the model ID. Each route supports typed creation, partial previews and iterable streams. If your application uses native messages, tools or saved responses, check the protocol before porting those values. [Model IDs, credentials and mode selection](providers.md)

| Provider | Rust generation endpoint and default mode | What to do when porting | API reference |
| --- | --- | --- | --- |
| OpenAI | `/v1/responses`, `JsonSchema` | Supply your prompt as usual. For native calls, use Responses input items. `create_with_completion` returns an `output` array. OpenAI batch preparation also uses Responses. | [Responses migration](https://developers.openai.com/api/docs/guides/migrate-to-responses) |
| Anthropic | `/v1/messages`, version `2026-10-21`, `Tools` | Set `max_tokens`. Remove `temperature`, `top_p` and `top_k` from existing options. Keep the system prompt separate from user turns in native histories. | [Messages migration](https://platform.claude.com/docs/en/api/migration-2026-10-21) |
| OpenRouter | `/api/v1/responses`, `JsonSchema` | Keep the full routed model ID after `openrouter/`. Select a route that accepts your schema and settings. Supply the history with each request. | [Stateless Responses](https://openrouter.ai/docs/api/reference/responses/overview) |
| Google Gemini | `/v1/interactions`, `JsonSchema` | The Python reference uses `generateContent`. Use steps for native history and `generation_config` for generation options. Saved `generateContent` completions can be parsed locally. | [Interactions migration](https://aistudio.google.com/docs/migrate-to-interactions) |
| Cohere | `/v2/chat`, `Tools` | Choose a model with tool support for the default mode. The typed result comes from the tool arguments. | [Chat v2](https://docs.cohere.com/v2/reference/chat) |
| Mistral | `/v1/responses`, `JsonSchema` | Send history with each request. Mistral batch jobs use `/v1/chat/completions`. | [Stateless Responses](https://docs.mistral.ai/inference/conversations/responses) |

For an existing OpenAI, OpenRouter or Mistral integration that reads `choices[0].message` or sends Chat history, set `ProviderConfig::with_chat_completions()`. Select `Mode::Tools` as well if your application expects a named function. Setting a tool mode by itself uses the default endpoint. Rust accepts saved Chat and Responses completions through `parse_completion`. [Provider configuration](providers.md) · [Saved completions](types.md#parse-a-saved-completion)

## Comparison baseline

The tables compare the Rust guides with **Python Instructor 1.18.0** at `b01fff4`. **Supported** means the described workflow has a Rust API and deterministic tests. **Partial** identifies a difference that can affect your application. **Unsupported** means the port needs another client or application code. The links pair Python documentation or contract tests with the Rust guide. Check the endpoint results for the model and protocol exercised by an actual account. [Python client](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/core/client.py) · [Python provider tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/llm/test_core_providers/README.md) · [Remaining gaps](#prioritized-next-steps) · [Historical live results](#historical-endpoint-verification)

If you used the original Rust OpenAI SDK wrapper at `cb9e730`, enable `legacy-openai` for its `from_openai` constructor. New applications can use the native blocking or async client with the default features. [Original API](https://github.com/567-labs/instructor-rs/blob/cb9e730e312b4c8653afd04b613ff482c3033587/instructor/src/lib.rs) · [Rust migration](index.md#moving-from-the-original-rust-client)

## Provider and client APIs

| Workflow | Support | Rust behavior and limits | References |
| --- | --- | --- | --- |
| Provider selection, blocking and async create | Supported | `BlockingClient::from_provider` and `Client::from_provider` accept a provider/model string. `ProviderConfig` controls endpoint, key, and mode. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/from_provider.md) · [Rust](index.md) |
| Chat Completions compatibility | Supported | Explicit Chat requests on OpenAI, OpenRouter and Mistral support typed output, validation retries, partial previews and iterable streams. Tool and schema support depends on the model. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/openai.md) · [Rust](providers.md) |
| OpenAI Responses | Supported | Use native JSON Schema for structured text or a named function for tool arguments. You can also include server-managed tools in a Responses request. The client distinguishes function arguments, final text, refusals and incomplete output. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/openai-responses.md) · [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_responses_sdk_contract.py) · [Rust](providers.md#openai) |
| Anthropic Messages | Supported | Native tool blocks, separate system instructions, structured text and streamed arguments. Reasks preserve tool IDs and thinking signatures. The same version header applies to Message Batches. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/anthropic.md) · [Rust](providers.md#anthropic) |
| OpenRouter Responses | Supported | Use the gateway key with the routed model ID. Native routing options reach the gateway. Supply the conversation history in the input for each turn. Check capabilities on your selected route. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/openrouter.md) · [Rust](providers.md#openrouter) |
| Per-request schema strictness | Supported | On Responses and on OpenAI or OpenRouter Chat, set `Request::new(prompt).strict(false)` in schema or tool mode to permit omitted optional fields. With `.strict(true)`, named fields become required, nullable fields can be `null`, and objects are closed. Serde and your validators run in either case. | [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_public_strict_schemas.py) · [Rust modes](providers.md) · [Required nullable fields](types.md#require-a-field-that-accepts-null) |
| Caller-selected tools | Supported | Pass `.option("tool_choice", choice)` for Responses and Anthropic tool requests. Without an override, Anthropic models that require automatic choice receive a tool-only instruction. Select the expected typed call or inspect accompanying text through `create_with_completion`. | [Python Responses](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/test_openai_responses_tools.py#L121) · [Python Anthropic](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/providers/anthropic/handlers.py#L405) · [Rust](providers.md) |
| Provider error details | Supported | Call `http_details()` on a generation, batch or Decisions error to read the HTTP status, request ID and provider code when available. Generation errors expose details through the final retry error. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_responses_sdk_contract.py#L174) · [Rust](concepts.md#diagnose-a-failed-http-request) |
| Gemini Developer API | Partial | Interactions supports JSON Schema, function calls, media, signed steps on retry and incremental text or tool arguments. Vertex AI authentication, background polling and long-decode continuation require additional client APIs. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/google.md) · [Rust](providers.md) |
| Cohere and Mistral | Supported | Cohere v2 Chat and Mistral Responses can both return typed data or stream it. Mistral supports stateless function calls. Select Chat explicitly for a Mistral integration using Chat history. | [Python Cohere](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/cohere.md) · [Python Mistral](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/mistral.md) · [Rust](providers.md) |
| Raw completion and creation without a model | Supported | `create_with_completion` returns `(T, serde_json::Value)`, including for lists. `create_raw` returns the native JSON envelope. Metadata is available on that envelope. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/raw_response.md) · [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_create_with_completion_raw.py) · [Rust](index.md#choose-an-operation) |
| Revalidate a saved completion or stream | Supported | `parse_completion` accepts a native non-streaming envelope, its provider and mode, and a validation context. Local partial and iterable decoders accept extracted JSON fragments and context through iterator and async APIs. These operations run without a client. | [Python completion parser](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/core/function_calls.py#L168) · [Python partial parser](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/dsl/partial.py#L540) · [Rust envelopes](types.md#parse-a-saved-completion) · [Rust fragments](streaming.md#replay-fragments-locally) |
| Additional provider adapters | Unsupported | Python also has native Azure, Bedrock, Vertex, xAI, and other clients. A compatible gateway can use a custom base URL when its protocol matches an implemented API. | [Python integrations](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/index.md) · [Rust provider list](providers.md) |
| Schema analysis and mode selection | Supported | `analyze_schema` reports complexity, token estimates, and findings. `select_mode` returns an advisory mode based on provider and schema. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/core/auto_mode.py) · [Rust](types.md#analyze-a-schema-before-selecting-a-mode) |
| SDK patching | Unsupported | Use the native clients directly. The original OpenAI SDK wrapper is available with the `legacy-openai` Cargo feature. | [Python patching](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/patching.md) · [Rust migration](index.md#moving-from-the-original-rust-client) |

## Types, streaming, and validation

| Workflow | Support | Rust behavior and limits | References |
| --- | --- | --- | --- |
| Inferred output type, primitives, and containers | Supported | A binding or return type selects `T` for `create`. Scalars, lists, nullable values, and string-keyed maps use native Rust types. The client wraps and unwraps provider object envelopes. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/types.md) · [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/dsl/test_simple_types.py) · [Rust](types.md) |
| Models, descriptions, aliases, defaults, and unions | Supported | `#[model]` combines Serde, Schemars, validation, and partial derives. Tagged enums represent unions. Generic and recursive models have typed schemas and snapshots. Provider schema subsets still apply. | [Python models](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/models.md) · [Python unions](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/unions.md) · [Rust](types.md) |
| Dynamic model construction and coercion policies | Partial | Rust JSON Schema and return types derive from compile-time types. Serde controls conversion and rejects type mismatches by default. Python can generate new models at runtime and choose Pydantic-specific coercion, validators, and defaults that depend on earlier validated fields. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/models.md) · [Default factory tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/dsl/test_partial_default_factories.py) · [Rust](types.md) |
| Typed partial previews | Supported | Use `Partial<T>` for the preview type generated by `#[model]`. Match `PartialEvent::Partial` to update a display and `Complete(T)` to save the validated record. Nested structs, enums and lists retain their Rust types. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/partial.md) · [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/dsl/test_partial.py) · [Rust](streaming.md) |
| Partial validation and truncation | Partial | Nested validators can run as a child closes, including inside a tagged variant or a single flattened field. Python can wait for siblings. Untagged alternatives, multiple flattened fields and custom deserializers can require the enclosing model. On truncated root JSON, Rust ends with an error. Python can yield a final provisional snapshot. Persist only `Complete(T)`. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/partial.md) · [Rust](streaming.md#when-nested-validators-run) |
| Iterable streaming and cleanup | Supported | Process validated items in arrival order. An error after delivery ends the stream. To stop early, drop the stream or leave the loop that owns it. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/iterable.md) · [Cleanup tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_iterable_stream_cleanup.py) · [Rust](streaming.md) |
| Markdown JSON selection | Supported | For one model, select the last complete JSON candidate. Iterable streams prefer fenced candidates and defer unfenced candidates until the end. Native modes permit earlier delivery. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/iterable.md) · [Rust](streaming.md#handle-errors-and-cancellation) |
| Parallel tool calls | Supported | For different kinds of calls, derive `ParallelTools` on an enum and use `create_parallel`. It returns a validated vector in call order, including repetitions. Use `create_iterable` for incremental delivery of one record type. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/parallel.md) · [Rust](parallel.md) |
| Interleaved iterable tool calls | Supported | On explicit Chat routes for OpenAI, OpenRouter and Mistral, `create_iterable` can assemble interleaved fragments by call index. On Responses, use a collection schema for streaming or `create_parallel` to receive several named calls together. | [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_openai_streaming_parallel_tool_calls.py) · [Rust](streaming.md) |
| Field, model, and context validation | Supported | `#[validate]` hooks run recursively, including on fields inside containers. A contextual validator receives the request's JSON and can normalize a value. `create_validated` accepts an additional check, including an async check on `Client`. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/validation.md) · [Rust](concepts.md) |
| Model-based semantic validation and moderation | Supported | `validate_text` takes a rule with an optional override and `moderate_text` returns flagged categories from OpenAI. Available on blocking and async clients. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/semantic_validation.md) · [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/test_llm_validator_allow_override.py) · [Rust](concepts.md#check-text-with-a-model) |
| Missing results and citation matching | Partial | `Maybe<T>` models missing output with a reason. `Citations` verifies quotes against one or several sources. Rust exposes UTF-8 byte ranges. Fuzzy alignments with tied scores may differ. | [Python Maybe](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/maybe.md) · [Python citations](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/citation.md) · [Rust](grounded-extraction.md) |

## Request lifecycle and other operations

| Workflow | Support | Rust behavior and limits | References |
| --- | --- | --- | --- |
| Integer retries and provider-specific corrections | Supported | The limit counts additional attempts. Reasks use a request-local history and include validation feedback. Exhaustion exposes the rejected completions and last error. Streams retry before the first delivered value. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/retrying.md) · [Isolation tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_retry_isolation_contract.py) · [Rust](concepts.md) |
| Usage and token budgets | Supported | For typed non-streaming calls, `create_with_usage` and `on_usage` expose input, output, cache reads and writes, reasoning, and totals across attempts. Each count tracks availability independently. Reasks stop when the budget is reached or usage is unknown. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/usage.md) · [Budget tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_retry_budget.py) · [Rust](concepts.md#set-retry-limits) · [Accounting](concepts.md#account-for-tokens-across-retries) |
| Time limits and custom retry policies | Supported | Per-attempt HTTP timeout and a separate elapsed retry timeout. A request policy inspects a failed attempt and selects a delay or stops. Attempt, token, and time limits apply. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/retrying.md) · [Rust](concepts.md#set-retry-limits) |
| Message templating | Partial | MiniJinja renders text messages from JSON context with strict undefined variables. Python's template environment can also handle Python objects and provider-specific message structures. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/templating.md) · [Rust](concepts.md#validate-against-the-source) |
| Hooks and logging | Partial | Client and request hooks receive attempt numbers and events for requests, responses, errors, usage and cache hits. Use `Event::LastAttempt` to record exhaustion. Python also passes maximum attempts and a last-attempt flag to error hooks. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/hooks.md) · [Rust](concepts.md#observe-attempts-and-reuse-responses) |
| Response and schema caching | Partial | Rust reuses schemas by type. Choose memory, disk or an application-supplied `ResponseCache` for responses on the client. Select a tenant with `Request::cache_namespace` and a lifetime with `cache_ttl`. Hits are revalidated. Python also accepts the store on each call. In Rust, use separate client configurations for different stores. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/caching.md) · [Rust](concepts.md#observe-attempts-and-reuse-responses) · [Cache adapters](concepts.md#share-an-application-cache) |
| Prompt caching and provider-managed context | Partial | For Anthropic Messages or a compatible OpenRouter Chat route, add a five-minute or one-hour ephemeral prefix marker. OpenAI Decisions accepts explicit cache controls. Python also configures cached content for Gemini generateContent. Its settings differ from the Interactions API. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/prompt_caching.md) · [Rust](providers.md) |
| Multimodal input | Partial | `Media` attaches explicit bytes or a provider-readable URL. Gemini Interactions requires HTTPS for remote files, as does Mistral Responses for remote PDFs. For local files, read the bytes and specify the MIME type. Python also provides media detection and loading helpers. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/multimodal.md) · [Rust format matrix](media.md) |
| Provider-managed batch jobs | Partial | Use `download` and `parse_reader` for a large OpenAI, Anthropic or Mistral job. A successful transfer replaces the destination atomically and local parsing yields individual outcomes. OpenAI accepts saved Chat and Responses input files, with one endpoint per job. Python also offers a batch CLI. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/batch.md) · [Retrieval tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_batch_result_retrieval.py) · [Rust](batch.md) |
| Distillation | Partial | Typed teacher calls and JSON Lines export using raw or message formats. Rust uses an explicit definition. Python also supports decorators with function reflection and automatic logging. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/distillation.md) · [Rust](distillation.md) |
| CLI and fine-tuning management | Unsupported | Python offers job and usage commands. Rust exposes batch jobs through the library and a Cargo example. | [Python CLI](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/cli/index.md) · [Rust batch](batch.md) |

## Decisions

| Workflow | Support | Rust behavior and limits | References |
| --- | --- | --- | --- |
| Jev on OpenRouter and Typesafe | Supported | Use `openrouter/typesafe/jev-1.14` or direct `typesafe/jev-1.14.0`. Both routes receive your context as state and return typed choices, probabilities and scores. Use `create_with_completion` to inspect the original distributions. Jev’s experimental Box primitive lies outside both Instructor bindings. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/decisions.md) · [Provider tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/decisions/test_provider_contracts.py) · [Rust](decisions.md) · [Jev models](https://docs.typesafe.ai/models) |
| OpenAI Decisions | Supported | Use `openai/gpt-6-luna` on `/v1/decisions`. Supply shared input or per-question text, images and JSON. Rust accepts boolean and string choices, named score levels, prompt cache controls and a safety identifier. It validates answer type, identity, order and bounds and reports refusals by field. | [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/decisions/test_openai_decisions.py) · [Rust](decisions.md#use-native-openai-input) · [OpenAI API](https://developers.openai.com/api/docs/guides/decisions) |
| Choice, probability, and score models | Supported | `Choices` and `DecisionModel` derives, inline labels, `Question`, `Noul`, `Score`, criteria, examples, descriptions, templates, and optional score scaling. Tests cover malformed specifications and answers. | [Python schema tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/decisions/test_decisions_schema.py) · [Rust](decisions.md#define-the-questions) |
| Validation and transport configuration | Supported | Blocking and async clients, provider-scoped keys, endpoint/timeout overrides, reusable HTTP clients, and contextual strictness passed to application validation. Decisions is a single non-streaming request in both libraries. | [Python configuration tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/decisions/test_client_configuration.py) · [Rust](decisions.md#configure-the-transport) |

## Reproduce the checks

From the Rust workspace root, run the tests with the recorded dependency versions. The default feature set runs local tests including mock HTTP exchanges, stream parsing, validation, batch jobs and Decisions. The 75 model tests are opt-in. For a comparison with Python on the same prompts, use the local replay. [Replay without credentials](ci-credentials.md#replay-the-paired-tests-without-credentials)

```sh
if [ ! -e Cargo.lock ]; then
  cp benchmarks/recorded/Cargo.lock Cargo.lock
fi
cargo test --locked --workspace
```

To try an extraction without an account, run `cargo run -p instructor-ai --example types` to inspect a schema and parse a sample, or `cargo run -p instructor-ai --example replay` to watch a partial response. [Getting started](index.md)

??? example "Reproduce the earlier Rust comparison at 61718fc"

    At this revision the Rust suite passed **201 tests**, with **75 live tests** ignored. Formatting, Clippy and the strict documentation build passed. The accompanying 49-workload benchmark used identical Rust sources at `494da59` and Python `b01fff4e`. The report records the measured revisions, source fingerprint and timing conditions. [Rust checkout](https://github.com/567-labs/instructor-rs/tree/61718fce4b81d4d48055d432d175a375174b2fca) · [49-workload report](https://github.com/567-labs/instructor-rs/blob/4328ec17dd5905d25210fb5b39f707768a88c527/benchmarks/recorded/report.json) · [Benchmark guide](benchmarks.md)

    For the original SDK tests, unsetting `OPENAI_API_KEY` selects loopback fixtures. Install the checkout's `requirements.txt` in a Python environment to build its guides.

    ```sh
    if [ ! -e Cargo.lock ]; then
      cp benchmarks/recorded/Cargo.lock Cargo.lock
    fi
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    env -u OPENAI_API_KEY cargo test --locked --workspace --no-fail-fast
    RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps
    cargo check --locked --workspace --examples
    cargo run -p instructor-ai --example types
    cargo run -p instructor-ai --example replay
    cargo run -p instructor-ai --example grounded_extraction
    mkdocs build --strict
    ```

### Python reference tests

The reference run for Python 1.18.0 passed 1,948 distinct tests across the provider, parsing and validation contracts. It skipped 33 cases.

??? example "Reproduce the Python reference run at b01fff4e"

    The combined Python reference suite uses commit `b01fff4e01ecab2bdcea6ad24e795bc3f549b89d`, CPython 3.12.14 and pytest 9.2.0. After setting up `ref_dir` with the frozen development and provider extras from the benchmark guide, run the following in Bash. The suite exercises provider contracts, DSL behaviors, streaming, decisions, batching, validators and local benchmark workloads. Tests marked `llm` are excluded. [Reference environment](benchmarks.md)

    ```bash
    cd "$ref_dir"
    .venv/bin/python -m pytest -q -rs --tb=short --asyncio-mode=auto -m 'not llm' \
      tests/benchmarks tests/dsl tests/v2/decisions \
      tests/v2/test_{iterable_streaming,iterable_stream_cleanup,json_stream_bounds,request_local_streaming,openai_streaming}.py \
      tests/v2/test_{retry_budget,retry_isolation_contract,retry_runtime,response_schema_compat,response_schema_cache,public_strict_schemas}.py \
      tests/v2/test_{create_with_completion_raw,simple_type_fixes,provider_result_finalization,async_validator_contracts,async_validators_execution}.py \
      tests/v2/test_{anthropic_handlers,cohere_handlers,genai_handlers_deterministic,mistral_handlers,openai_compat_handlers}.py \
      tests/v2/test_{json_helpers,json_sanitization,templating,token_limit_contract,messages_not_mutated}.py \
      tests/coverage/test_{partial_completeness,partial_streaming_aliases,dsl_partial_coverage}.py \
      tests/v2/test_{cache_identity_contracts,cache_response_round_trip,core_multimodal_runtime,genai_multimodal_runtime}.py \
      tests/v2/test_{parallel_contracts,parallel_tools_wrapper,openai_streaming_parallel_tool_calls}.py \
      tests/v2/test_{responses_sdk_contract,openai_responses_client,reasoning_content_contract,typed_dict_reserved_keys,batch_strict_schema}.py \
      tests/coverage/test_core_hooks_registry_coverage.py tests/test_batch*.py tests/test_anthropic_batch_errors.py \
      tests/coverage/test_batch*.py tests/v2/test_batch_result_retrieval.py tests/v2/test_citation.py \
      tests/coverage/test_{auto_mode,schema_analyzer,streaming_json_selection,legacy_distil_coverage}.py
    ```

    The combined command passed **1,935 tests** in 107.25 s, skipped 33 and reported 76 warnings. The skip breakdown was 23 tests requiring `xai_sdk`, four requiring `vertexai`, four requiring `grpc`, one decisions case targeting Pydantic versions before 2.11 (installed 2.11.7), and one unconditionally skipped Mistral placeholder. The Mistral SDK was installed. [Mistral skip](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_mistral_handlers.py#L333)

    A separate selector covered LLM validation and moderation contracts in two additional files. It passed 13 tests and deselected 14, for **1,948 distinct passing tests** across the two commands.

    ```bash
    .venv/bin/python -m pytest -q -o addopts= \
      tests/test_llm_validator_allow_override.py tests/coverage/test_validation_coverage.py \
      -k 'llm_validator or moderation or TestAllowOverride'
    ```

### Opt-in live checks

To verify a model your application will use, set its key and select one exact test from the Rust workspace. For example, with `OPENROUTER_API_KEY` configured, run the nested extraction on the current generation route or classify a refund ticket with Jev 1.14. Each test sends billable requests. The first command only lists available cases. [Provider IDs](providers.md) · [CI comparison models and overrides](ci-credentials.md#run-the-same-contracts-in-both-languages)

```sh
cargo test -p instructor-ai --test live -- --ignored --list
cargo test -p instructor-ai --test live openrouter_responses::test_nested_enum_collection -- --ignored --exact
cargo test -p instructor-ai --test live openrouter_jev -- --ignored --exact
```

The standalone tests use the models in the Rust provider guide. The nested OpenRouter test above uses `openai/gpt-6.2-sol`. To test another route available on your account, set `INSTRUCTOR_LIVE_OPENROUTER_MODEL` to its catalog ID. To run a provider's generation suite, filter by the module name, for example `cargo test -p instructor-ai --test live anthropic_messages:: -- --ignored`. The paired workflow selects the pinned Python model for each case in both languages. Its summary lists which cases actually executed. [Provider models](providers.md) · [Test selection and cost](ci-credentials.md#run-the-same-contracts-in-both-languages)

### Historical endpoint verification

On November 26, 2026, the Python repository ran a pinned workflow with both clients. Each live pair used one credential from the Python job. The caller was at `e24f62c`, the Python contracts at `0ab894f`, and the Rust contracts at `c630e8a`. The Rust repository ran the same comparison with its own Actions credentials. Both columns report the outcome for the two clients together. These runs predate the new Rust defaults, including the Anthropic version header and the move to Gemini Interactions. Routed Jev selected 1.13. [Caller](https://github.com/567-labs/instructor/blob/e24f62c3fc451f76d6dad76b590a40fa73dfb7d7/.github/workflows/paired-live.yml) · [Python contracts](https://github.com/567-labs/instructor/tree/0ab894f0661a6968b17e67cf5b7657f1fcb3b9f7/tests/llm) · [Rust contracts](https://github.com/567-labs/instructor-rs/blob/c630e8af30e8743e03f981b618d68a79e916075d/instructor/tests/live.rs)

| Route at c630e8a | Pairs | With the Python repository’s keys | With the Rust repository’s keys |
| --- | ---: | --- | --- |
| OpenAI Chat Completions | 10 | Passed | HTTP 401 |
| OpenAI Responses | 9 | Passed | HTTP 401 |
| OpenAI Decisions | 2 | Passed | HTTP 401 |
| Anthropic Messages | 10 | Passed | Collected |
| Cohere | 10 | Passed | Collected |
| Google Gemini | 10 | HTTP 402 | Collected |
| OpenRouter generation | 10 | Collected | Collected |
| OpenRouter Jev | 2 | Collected | Collected |
| Direct Typesafe Jev | 2 | Collected | Collected |
| Mistral | 10 | Collected | Collected |

**Passed** records success in both languages. HTTP 401 indicates an invalid OpenAI credential, and HTTP 402 indicates depleted Google credits. **Collected** means both test names were found and the job had no credential bound for that provider. In total, the Python job executed 51 pairs with 41 passing. The Rust job executed 21 pairs and received 21 authentication failures. The remaining cases only exercised discovery. [Python results](https://github.com/567-labs/instructor/actions/runs/45898277923/attempts/1) · [Rust results](https://github.com/567-labs/instructor-rs/actions/runs/45898236914/attempts/1)

### Compare later changes locally

All 75 typed test pairs pass with fixed answers over loopback HTTP, including the six Decisions cases. At the Python CI pin, 74 pairs also match on model, input, answer, streaming and attempt count. The Gemini retry case matches after correcting the Python test to count requests. The replay report records the chosen endpoints, prompts, answers and request counts. This lets you check whether your port receives the same typed data and reasks after the same failures, including when the two clients use different APIs. To check whether a model accepts your schema on your account, run a selected live test. [Replay commands and results](ci-credentials.md#replay-the-paired-tests-without-credentials)

## Prioritized next steps

| Priority | Who would benefit | Expected benefit and effort |
| --- | --- | --- |
| 1. Check the deployed model and protocol together | Applications moving to Responses, Gemini Interactions, the new Messages version or Jev 1.14 | A selected live check can confirm schema acceptance and the delivered value for that account. Start with a nested extraction, a partial stream or a ticket classification for your chosen route. This uses existing tests and limits paid requests to the selected cases. The historical jobs found the OpenRouter and direct Jev tests and had no credentials for those routes. [Results](#historical-endpoint-verification) · [Select one case](#opt-in-live-checks) |
| 2. Report token usage when a typed stream finishes | Services that account for usage per tenant or operation | Both libraries expose cumulative counts for typed non-streaming calls. An optional completion hook for streams could cover the same accounting without another model request. It would need to preserve missing counts across retries, cancellation and providers that send usage after the final content. Until then, use `create_with_usage` to inspect totals from a non-streaming request. [Python limits](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/usage.md#L96) · [Rust accounting](concepts.md#account-for-tokens-across-retries) |
| 3. Select a response cache store per call | Services whose tenants must use physically separate storage | Python accepts a backend on each call. In Rust, use a configured client per store, or `cache_namespace` for isolation within one store. A per-call backend would simplify the former case. The main work would be preserving expiry, validation and isolation on concurrent calls. [Python caching](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/caching.md#L25) · [Rust scopes](concepts.md#share-an-application-cache) |
| 4. Support additional cloud authentication | Applications tied to Vertex AI, Azure or Bedrock deployments | A native adapter would allow these users to keep their deployment and identity provider. Each requires authentication, endpoint selection and independent streaming checks. Use the direct provider routes when your deployment permits them. [Python integrations](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/index.md) · [Rust routes](providers.md) |

For providers you can already access, the remaining differences most likely to affect a port are Pydantic coercion, validation timing during a partial stream, native message formats and model-specific schema constraints. Try a saved completion with your Rust type and validation context before running a paid request. [Parse a saved completion](types.md#parse-a-saved-completion) · [Replay JSON fragments](streaming.md#replay-fragments-locally)
