# What carries over from Python Instructor?

The Rust checkout supports typed creation, validation and reasking, partial and iterable streaming, and Decisions including Jev. The primary integrations are OpenAI, Anthropic, and OpenRouter. The comparison below records where a Python application can port a workflow directly, where its behavior changes, and where a port still requires application code.

## Comparison baseline

The reference is Python Instructor 1.18.0 at `b01fff4`. Its client accepts a response model, translates it into the provider's request format, and parses the completion. Its tests exercise those contracts at the model, parser, and HTTP boundaries. [Public client](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/core/client.py) · [Response model preparation](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/core/response_model.py) · [Core provider tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/llm/test_core_providers/README.md)

The Rust baseline at `cb9e730` exposed a blocking OpenAI client for extracting one struct through a tool call. Its derives handled fields, unit enums, options, lists, and nested structs. Five client integration test files used OpenAI credentials. The three guide pages covered setup, macro expansion, and field types. The roadmap listed additional providers, context validation, and unions as future work. [Original client](https://github.com/567-labs/instructor-rs/blob/cb9e730e312b4c8653afd04b613ff482c3033587/instructor/src/lib.rs) · [Original retry test](https://github.com/567-labs/instructor-rs/blob/cb9e730e312b4c8653afd04b613ff482c3033587/instructor/tests/test_retries.rs) · [Original guide](https://github.com/567-labs/instructor-rs/blob/cb9e730e312b4c8653afd04b613ff482c3033587/docs/index.md) · [Original types](https://github.com/567-labs/instructor-rs/blob/cb9e730e312b4c8653afd04b613ff482c3033587/docs/types.md)

**Supported** means the named behavior has a Rust entry point and deterministic test coverage. **Partial** identifies a supported subset or a difference visible to callers. **Unsupported** identifies a Python workflow that still needs a Rust implementation. Offline provider tests run the clients against synthetic responses in the providers' HTTP formats. Live model availability, output quality, and server acceptance of a particular schema depend on the selected model and account. Rust test filenames below are relative to `instructor/tests/` except `typed_models.rs`, which is in `instruct-macros/tests/`. Python links pin the compared revision. [Prioritized gaps](#prioritized-next-steps) · [Historical endpoint results](#historical-endpoint-verification)

## Provider and client APIs

| Workflow | Support | Rust behavior and limits | Guides and tests |
| --- | --- | --- | --- |
| Provider selection, blocking and async create | Supported | `BlockingClient::from_provider` and `Client::from_provider` accept a provider/model string. `ProviderConfig` controls endpoint, key, and mode. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/from_provider.md) · [Rust](index.md) · `providers.rs` |
| OpenAI Chat Completions | Supported | Tool, strict tool, JSON Schema, JSON, and Markdown JSON modes. Fixtures exercise native payloads, original completions, validation retries, and both stream forms. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/openai.md) · [Rust](providers.md#openai) · `providers.rs` |
| OpenAI Responses | Supported | `ResponsesTools` uses `/responses`, optional server-managed tools, and typed function arguments or final structured text. Accepts typed messages, attachments, and native input items. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/openai-responses.md) · [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_responses_sdk_contract.py) · [Rust](providers.md#openai) · `providers.rs` |
| Anthropic Messages | Supported | Native tool blocks, separate system instructions, schema output, thinking-aware tool choice, correlated reasks, and streamed input JSON. Fixtures check thinking signatures on retry. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/anthropic.md) · [Rust](providers.md#anthropic) · `providers.rs` |
| OpenRouter Chat Completions | Supported | Preserves the routed model name, scoped key, and native routing options. Handles embedded provider errors and supports both streams. Tool and schema availability varies by route. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/openrouter.md) · [Rust](providers.md#openrouter) · `providers.rs` |
| Per-request schema strictness | Partial | OpenAI Chat and OpenRouter select strict tool arguments via `ToolsStrict`. Their `JsonSchema` mode always requests a strict schema. Python accepts a per-call override, including permissive JSON Schema with omittable properties. | [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_public_strict_schemas.py#L49) · [OpenRouter handler](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/providers/openai/handlers.py#L391) · [Rust modes](providers.md) |
| Caller-selected tools | Partial | Rust manages `tool_choice` for structured requests. Python accepts an explicit choice on OpenAI Responses and Anthropic. On Anthropic models that require automatic choice, Python additionally puts a tool-only instruction in the system prompt. | [Python Responses tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/test_openai_responses_tools.py#L121) · [Anthropic handler](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/providers/anthropic/handlers.py#L405) · [Rust providers](providers.md) |
| Provider error details | Partial | Rust's generation client exposes the HTTP status and body. The Python OpenAI SDK error retained in the retry history exposes the provider request ID from the headers and a machine-readable error code. | [Python error contract](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_responses_sdk_contract.py#L174) · [Rust errors](concepts.md#set-retry-limits) |
| Gemini Developer API | Partial | Supports generate and stream-generate. Rust defaults to JSON Schema for incremental text. Tool arguments may arrive as a complete object. Python also supports Vertex AI credentials and deployment settings. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/google.md) · [Rust](providers.md) · `providers.rs` |
| Cohere and Mistral | Partial | Native Cohere v2 Chat and Mistral Chat Completions support both kinds of streams. Cohere tool mode requires native tool support. Python also handles legacy SDK versions. | [Cohere](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/cohere.md) · [Mistral](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/mistral.md) · [Rust](providers.md) · `providers.rs` |
| Raw completion and creation without a model | Supported | `create_with_completion` returns `(T, serde_json::Value)`, including for lists. `create_raw` returns the native JSON envelope. Metadata is available on that envelope. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/raw_response.md) · [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_create_with_completion_raw.py) · [Rust](index.md#choose-an-operation) · `providers.rs` |
| Revalidate a saved completion or stream | Supported | `parse_completion` accepts a native non-streaming envelope, its provider and mode, and a validation context. Local partial and iterable decoders accept extracted JSON fragments and context through iterator and async APIs. These operations run without a client. | [Python completion parser](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/core/function_calls.py#L168) · [Python partial parser](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/dsl/partial.py#L540) · [Rust envelopes](types.md#parse-a-saved-completion) · [Rust fragments](streaming.md#replay-fragments-locally) · `providers.rs`, `streaming.rs` |
| Additional provider adapters | Unsupported | Python also has native Azure, Bedrock, Vertex, xAI, and other clients. A compatible gateway can use a custom base URL when its protocol matches an implemented API. | [Python integrations](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/integrations/index.md) · [Rust provider list](providers.md) |
| Schema analysis and mode selection | Supported | `analyze_schema` reports complexity, token estimates, and findings. `select_mode` returns an advisory mode based on provider and schema. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/core/auto_mode.py) · [Rust](types.md#analyze-a-schema-before-selecting-a-mode) · `analysis.rs` |
| SDK patching | Unsupported | Rust uses native HTTP clients and explicit configuration. Its legacy `from_openai` wrapper accepts the original Rust SDK client. | [Python patching](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/patching.md) · [Rust migration](index.md#moving-from-the-original-rust-client) |

## Types, streaming, and validation

| Workflow | Support | Rust behavior and limits | Guides and tests |
| --- | --- | --- | --- |
| Inferred output type, primitives, and containers | Supported | A binding or return type selects `T` for `create`. Scalars, lists, nullable values, and string-keyed maps use native Rust types. The client wraps and unwraps provider object envelopes. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/types.md) · [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/dsl/test_simple_types.py) · [Rust](types.md) · `typed_models.rs`, `client.rs` |
| Models, descriptions, aliases, defaults, and unions | Supported | `#[model]` combines Serde, Schemars, validation, and partial derives. Tagged enums represent unions. Generic and recursive models have typed schemas and snapshots. Provider schema subsets still apply. | [Python models](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/models.md) · [Python unions](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/unions.md) · [Rust](types.md) · `typed_models.rs` |
| Dynamic model construction and coercion policies | Partial | Rust JSON Schema and return types derive from compile-time types. Serde controls conversion and rejects type mismatches by default. Python can generate new models at runtime and choose Pydantic-specific coercion, validators, and defaults that depend on earlier validated fields. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/models.md) · [Default factory tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/dsl/test_partial_default_factories.py) · [Rust](types.md) |
| Typed partial previews | Supported | `create_partial::<T>` emits `PartialEvent::Partial(T::Partial)` with recursive optional fields. It emits `Complete(T)` once on successful termination. Tests cover nested fields, fragmented escapes, defaults, enums, and context. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/partial.md) · [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/dsl/test_partial.py) · [Rust](streaming.md) · `streaming.rs`, `typed_models.rs` |
| Partial validation and truncation | Partial | Rust validates closed children inside tagged variants and a single flattened field. Python tracks sibling completion and may validate later. Untagged alternatives, multiple flattened fields and custom deserializers can require the enclosing model. Rust reports incomplete root JSON as an error. Python can finish with a provisional snapshot on truncation. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/partial.md) · [Rust](streaming.md#when-nested-validators-run) · `partial_completeness.rs`, `streaming.rs` |
| Iterable streaming and cleanup | Supported | `create_iterable::<T>` yields validated records in order. A delayed failure is terminal, and dropping the stream releases the source. Parsers retain nested values and handle fragmentation inside strings and arrays. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/iterable.md) · [Cleanup tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_iterable_stream_cleanup.py) · [Rust](streaming.md) · `streaming.rs`, `providers.rs` |
| Markdown JSON selection | Supported | For one model, select the last complete JSON candidate. Iterable streams prefer fenced candidates and defer unfenced candidates until the end. Native modes permit earlier delivery. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/iterable.md) · [Rust](streaming.md#handle-errors-and-cancellation) · `instructor/src/streaming/markdown.rs` |
| Parallel tool calls | Supported | `#[derive(ParallelTools)]` maps named calls to enum variants. The client validates the full vector and preserves order and repetitions. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/parallel.md) · [Rust](parallel.md) · `providers/parallel.rs` |
| Interleaved iterable tool calls | Supported | For a homogeneous iterable in OpenAI Chat, OpenRouter, and Mistral, fragments are assembled by call index. Tests check ordering, split tokens, and truncation within a call. | [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_openai_streaming_parallel_tool_calls.py) · [Rust](streaming.md) · `providers/parallel.rs` |
| Field, model, and context validation | Supported | `#[validate]` hooks run recursively, including on fields inside containers. A contextual validator receives the request's JSON and can normalize a value. `create_validated` accepts an additional check, including an async check on `Client`. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/validation.md) · [Rust](concepts.md) · `typed_models.rs`, `client.rs` |
| Model-based semantic validation and moderation | Supported | `validate_text` takes a rule with an optional override and `moderate_text` returns flagged categories from OpenAI. Available on blocking and async clients. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/semantic_validation.md) · [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/test_llm_validator_allow_override.py) · [Rust](concepts.md#check-text-with-a-model) · `validation.rs` |
| Missing results and citation matching | Partial | `Maybe<T>` models missing output with a reason. `Citations` verifies quotes against one or several sources. Rust exposes UTF-8 byte ranges. Fuzzy alignments with tied scores may differ. | [Python Maybe](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/maybe.md) · [Python citations](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/citation.md) · [Rust](grounded-extraction.md) · `models.rs` |

## Request lifecycle and other operations

| Workflow | Support | Rust behavior and limits | Guides and tests |
| --- | --- | --- | --- |
| Integer retries and provider-specific corrections | Supported | The limit counts additional attempts. Reasks use a request-local history and include validation feedback. Exhaustion exposes the rejected completions and last error. Streams retry before the first delivered value. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/retrying.md) · [Isolation tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_retry_isolation_contract.py) · [Rust](concepts.md) · `client.rs`, `providers.rs` |
| Usage and token budgets | Supported | For typed non-streaming calls, `create_with_usage` and `on_usage` expose input, output, cache reads and writes, reasoning, and totals across attempts. Each count tracks availability independently. Reasks stop when the budget is reached or usage is unknown. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/usage.md) · [Budget tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_retry_budget.py) · [Rust](concepts.md#set-retry-limits) · `client.rs`, `providers.rs` · [Accounting](concepts.md#account-for-tokens-across-retries) · `providers/usage.rs` |
| Time limits and custom retry policies | Supported | Per-attempt HTTP timeout and a separate elapsed retry timeout. A request policy inspects a failed attempt and selects a delay or stops. Attempt, token, and time limits apply. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/retrying.md) · [Rust](concepts.md#set-retry-limits) · `client.rs` |
| Message templating | Partial | MiniJinja renders text messages from JSON context with strict undefined variables. Python's template environment can also handle Python objects and provider-specific message structures. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/templating.md) · [Rust](concepts.md#validate-against-the-source) · `client.rs` |
| Hooks and logging | Partial | Request and client hooks receive typed events for attempts, errors, usage, and cache hits. Client handlers support removal and clearing. Python also provides richer retry metadata and logging integrations. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/hooks.md) · [Rust](concepts.md#observe-attempts-and-reuse-responses) · `client.rs` |
| Response and schema caching | Partial | Response caches include memory, disk, and application-supplied stores through `ResponseCache`. Clients scope entries, enforce expiry and revalidate hits. A schema cache reuses type metadata. Python supports per-call cache and namespace selection and documents an application-supplied Redis adapter. Rust configures the store on the client. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/caching.md) · [Rust](concepts.md#observe-attempts-and-reuse-responses) · `client.rs` · [Cache adapters](concepts.md#share-an-application-cache) · `client/cache_backend.rs` |
| Prompt caching and provider-managed context | Partial | Anthropic and OpenRouter ephemeral prompt cache markers support five-minute and one-hour lifetimes. Python also provides Gemini cached-content lifecycle helpers. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/prompt_caching.md) · [Rust](providers.md#reuse-a-prompt-prefix) · `validation.rs` |
| Multimodal input | Partial | `Media` attaches explicit bytes or a provider-readable URL. Image, PDF, and audio support varies by protocol. Python also detects media, loads files, and can retrieve remote content locally. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/multimodal.md) · [Rust format matrix](media.md) · `client.rs` |
| Provider-managed batch jobs | Partial | Typed JSON Lines preparation, job management, and parsing for OpenAI, Anthropic, and Mistral. `parse_reader` processes local JSONL incrementally with per-record outcomes. Remote downloads buffer their response. Python also has a batch CLI. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/batch.md) · [Retrieval tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_batch_result_retrieval.py) · [Rust](batch.md) · `batch.rs` |
| Distillation | Partial | Typed teacher calls and JSON Lines export using raw or message formats. Rust uses an explicit definition. Python also supports decorators with function reflection and automatic logging. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/distillation.md) · [Rust](distillation.md) · `distillation.rs` |
| CLI and fine-tuning management | Unsupported | Python offers job and usage commands. Rust exposes batch jobs through the library and a Cargo example. | [Python CLI](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/cli/index.md) · [Rust batch](batch.md) |

## Decisions

| Workflow | Support | Rust behavior and limits | Guides and tests |
| --- | --- | --- | --- |
| Jev on OpenRouter and Typesafe | Supported | Separate routes and credentials. Both receive the context as state. `create` returns a decision model, and `create_with_completion` includes the opaque answer distributions and metadata. | [Python](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/decisions.md) · [Provider tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/decisions/test_provider_contracts.py) · [Rust](decisions.md) · `decisions.rs` |
| OpenAI Decisions | Supported | Converts typed questions to ordered native questions, accepts context or native input parts, and verifies answer count, name, order, and numeric bounds. Successful responses include raw output. A refusal yields a field-specific error. | [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/decisions/test_openai_decisions.py) · [Rust](decisions.md#use-native-openai-input) · `decisions.rs` |
| Choice, probability, and score models | Supported | `Choices` and `DecisionModel` derives, inline labels, `Question`, `Noul`, `Score`, criteria, examples, descriptions, templates, and optional score scaling. Tests cover malformed specifications and answers. | [Python schema tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/decisions/test_decisions_schema.py) · [Rust](decisions.md#define-the-questions) · `decisions.rs` |
| Validation and transport configuration | Supported | Blocking and async clients, provider-scoped keys, endpoint/timeout overrides, reusable HTTP clients, and contextual strictness passed to application validation. Decisions is a single non-streaming request in both libraries. | [Python configuration tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/decisions/test_client_configuration.py) · [Rust](decisions.md#configure-the-transport) · `decisions.rs` |

## Reproduce the checks

The Rust workspace at `61718fc` passed **201 tests** with **75 credentialed tests** ignored in the offline run. Formatting, Clippy with warnings denied and the strict MkDocs build passed. The 49 paired benchmarks used the Rust library and harness sources available at that revision and Python `b01fff4e`. The benchmark report identifies the measured checkout, source fingerprint and timing conditions. [Rust checkout](https://github.com/567-labs/instructor-rs/tree/61718fce4b81d4d48055d432d175a375174b2fca) · [Benchmark results](benchmarks.md#recorded-results)

Run the Rust unit, integration, and documentation tests from the workspace root. On Unix, unsetting `OPENAI_API_KEY` for the test command selects loopback fixtures for the original SDK tests. The local examples exercise schema generation, partial replay, and citation matching. To build the guides, install the repository's `requirements.txt` in a Python environment and run MkDocs from the same directory.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
env -u OPENAI_API_KEY cargo test --locked --workspace --no-fail-fast
RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps
cargo check --locked --workspace --examples
cargo run -p instructor-ai --example types
cargo run -p instructor-ai --example replay
cargo run -p instructor-ai --example grounded_extraction
mkdocs build --strict
```

### Python reference tests

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

`instructor/tests/live.rs` has 75 individually selectable live tests. The generation suite has nine core cases for each of seven API routes and six checks of the documented model IDs, for 69 cases. The Decisions suite adds one async and one blocking case for each of three routes. Ordinary Cargo runs ignore the live tests and run a local check of model selection. To run a live test, configure its credential and pass `--ignored`. Each test sends requests that can incur provider charges. [Response types and offline fixtures](types.md#test-increasingly-complex-response-types)

#### Generation

Each route exercises scalar, flat, enum, list, nested, and nested collection outputs. The three other core cases exercise a validation retry, partial updates, and an iterable stream. The documented model check asks for `42` in a record using tool mode. The OpenAI chat route covers the OpenAI guide model. [Python documented models](https://github.com/567-labs/instructor/blob/0ab894f0661a6968b17e67cf5b7657f1fcb3b9f7/tests/llm/test_current_doc_models.py) · [Python core models](https://github.com/567-labs/instructor/blob/0ab894f0661a6968b17e67cf5b7657f1fcb3b9f7/tests/llm/shared_config.py)

| Rust test module | Credential | Core model | Documented model | Local override |
| --- | --- | --- | --- | --- |
| `openai_chat` | `OPENAI_API_KEY` | `gpt-6-luna` | `gpt-6-luna` | `INSTRUCTOR_LIVE_OPENAI_MODEL` |
| `openai_responses` | `OPENAI_API_KEY` | `gpt-6-luna` | Covered by `openai_chat` | `INSTRUCTOR_LIVE_OPENAI_MODEL` |
| `anthropic_messages` | `ANTHROPIC_API_KEY` | `claude-haiku-5-5` | `claude-haiku-5-5` | `INSTRUCTOR_LIVE_ANTHROPIC_MODEL` |
| `openrouter_chat` | `OPENROUTER_API_KEY` | `google/gemini-3.8-flash` | `google/gemini-3.8-flash` | `INSTRUCTOR_LIVE_OPENROUTER_MODEL` |
| `google_genai` | `GOOGLE_API_KEY` | `GOOGLE_GENAI_MODEL` or `gemini-3.8-flash` | `gemini-3.8-flash` | `INSTRUCTOR_LIVE_GOOGLE_MODEL` |
| `cohere_chat` | `COHERE_API_KEY` | `command-a-03-2025` | `command-a-03-2025` | `INSTRUCTOR_LIVE_COHERE_MODEL` |
| `mistral_chat` | `MISTRAL_API_KEY` | `ministral-8b-latest` | `mistral-small-latest` | `INSTRUCTOR_LIVE_MISTRAL_MODEL` |

The local override replaces both model IDs for that provider. Use a model ID in the format shown in the table. `GOOGLE_GENAI_MODEL` also accepts the `google/` prefix used in Python CI. Gemini core cases use native JSON Schema and the OpenAI Responses cases use `ResponsesTools`. The other core cases use `Tools`.

List all live tests, run one exact case, or use a module prefix to run its entire suite.

```sh
cargo test -p instructor-ai --test live -- --ignored --list
cargo test -p instructor-ai --test live openai_chat::test_nested_enum_collection -- --ignored --exact
cargo test -p instructor-ai --test live anthropic_messages:: -- --ignored
```

#### Decisions

Each test classifies a refund ticket, checks the decoded choice, probability, and scaled score, and validates the raw distributions. The model variables below match Python. The canonical variable takes precedence over the older Rust alias. [Python Decisions route configuration](https://github.com/567-labs/instructor/blob/0ab894f0661a6968b17e67cf5b7657f1fcb3b9f7/tests/llm/test_decisions/conftest.py) · [Decisions in Rust](decisions.md)

| Async test | Blocking test | Credential | Default model | Model override | Older Rust alias |
| --- | --- | --- | --- | --- | --- |
| `openai_decisions` | `openai_decisions_blocking` | `OPENAI_API_KEY` | `gpt-6-luna` | `OPENAI_DECISIONS_MODEL` | `INSTRUCTOR_LIVE_OPENAI_DECISIONS_MODEL` |
| `openrouter_jev` | `openrouter_jev_blocking` | `OPENROUTER_API_KEY` | `typesafe/jev-1.13` | `OPENROUTER_DECISIONS_MODEL` | `INSTRUCTOR_LIVE_OPENROUTER_JEV_MODEL` |
| `typesafe_jev` | `typesafe_jev_blocking` | `TYPESAFE_API_KEY` | `jev-latest` | `TYPESAFE_DECISIONS_MODEL` | `INSTRUCTOR_LIVE_TYPESAFE_JEV_MODEL` |

```sh
cargo test -p instructor-ai --test live openrouter_jev -- --ignored --exact
cargo test -p instructor-ai --test live typesafe_jev_blocking -- --ignored --exact
```

With every credential configured, run `cargo test -p instructor-ai --test live -- --ignored` for all 75 checks. GitHub Actions matches each Rust test with its Python counterpart. Open the paired workflow run summary to inspect credential availability, selection, and outcomes for both implementations. [Credential setup and paired CI](ci-credentials.md)

### Historical endpoint verification

On November 26, 2026, the Python repository ran a pinned workflow with both clients. Each live pair used one credential from the Python job. The caller was at `e24f62c`, the Python contracts at `0ab894f`, and the Rust contracts at `c630e8a`. The Rust repository ran the same comparison with its own Actions credentials. The results below apply to those revisions and those runs. [Caller](https://github.com/567-labs/instructor/blob/e24f62c3fc451f76d6dad76b590a40fa73dfb7d7/.github/workflows/paired-live.yml) · [Python contracts](https://github.com/567-labs/instructor/tree/0ab894f0661a6968b17e67cf5b7657f1fcb3b9f7/tests/llm) · [Rust contracts](https://github.com/567-labs/instructor-rs/blob/c630e8af30e8743e03f981b618d68a79e916075d/instructor/tests/live.rs)

| Route | Pairs | Python Actions job | Rust Actions job |
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

The credential-free runner exercises the same 75 pairs with fixed answers over loopback HTTP. All 75 assertion pairs passed against the pinned Python contracts. Comparing the request traces found that the Gemini retry test accepted two validations after one Python request. Rust sent two requests. A local Python correction counting completion attempts produced matching traces for all 75 pairs. The runner and the endpoint jobs answer separate questions. Use replay for reproducible client behavior and the historical jobs for the observed model access at their pinned revisions. [Replay commands, trace fields and retry comparison](ci-credentials.md#replay-the-paired-tests-without-credentials)

## Prioritized next steps

| Priority | Caller need | Smallest Rust change to investigate | Acceptance test |
| --- | --- | --- | --- |
| 1 | Let an OpenAI Chat or OpenRouter model omit an optional property in schema mode. | Resolve strictness when preparing each tool or JSON Schema definition. Reuse the permissive schema used by batch preparation. | Send strict, permissive, then default requests for the same nested type on one client. Inspect the actual HTTP bodies for `required`, `additionalProperties` and placement of `strict`. Deserialize an answer with the property omitted. |
| 2 | Control tool selection for OpenAI Responses with built-in tools and for Anthropic with thinking. | Accept a caller's choice on the routes that support it. Add the tool-only instruction to Anthropic's automatic choice where Python supplies it. | Inspect explicit and default choices, cached system blocks, and a retry. Require the instruction exactly once and count the extra prompt bytes. |
| 3 | Diagnose a failed request from its provider request ID and error code. | Retain the ID at the HTTP error boundary. Read the code from the existing response body on demand. | Return HTTP 400 with a request ID only in the header and a code only in the body. Recover both from the final error and count exactly one request. |
| 4 | Save large batch results with bounded memory. | Start with Anthropic, whose Python download reads lines incrementally. Write chunks to the temporary file behind `download` and replace the destination on completion. | Gate the last chunk on a write of the first. Check bytes including split UTF-8. Interrupt a later download and verify the original file survives. [Python implementation](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/batch/providers/anthropic.py#L131) |

For multi-tenant services, Python also supports selecting a cache namespace on each call. A Rust implementation can reuse `ResponseCache` and the existing scope digest. An isolation test should interleave identical prompts for two tenants and require a cache hit only inside the originating tenant. [Python cache namespaces](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/caching.md#L25) · [Rust cache scopes](concepts.md#share-an-application-cache)

Both libraries lack final cumulative usage on typed streams. A future Rust hook could reuse `TokenUsage` when terminal counts arrive. Test a stream with usage after its last content delta, a canceled stream, and a retry before the first preview. Only report counts the provider supplied or that can be derived from those counts. [Python usage limitations](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/usage.md#L96)

For verification, the first outstanding endpoint checks are the 12 OpenRouter pairs covering generation and Jev, followed by direct Jev. For local comparisons, count requests and inspect retry feedback against the corrected Python assertion. Use the paired benchmark workloads to measure the cost of each change at pinned revisions. [Endpoint results](#historical-endpoint-verification) · [Local comparison](#compare-later-changes-locally) · [Rust and Python performance](benchmarks.md)
