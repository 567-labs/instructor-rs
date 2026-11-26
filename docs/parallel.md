# Request several tool calls in one response

Use `create_parallel` when the provider should choose from several named tools and may call each tool more than once. Define each tool's arguments as a model, then derive `ParallelTools` on an enum with one variant per argument type. Instructor decodes the calls and returns typed values for your application to handle.

The example requests two weather lookups and a museum search. It prints the proposed calls. Run it with `OPENAI_API_KEY` or `ANTHROPIC_API_KEY` and the corresponding model ID.

```sh
cargo run -p instructor-ai --example parallel -- openai/gpt-6-luna
cargo run -p instructor-ai --example parallel -- anthropic/claude-haiku-5-5
```

```rust
--8<-- "instructor/examples/parallel.rs"
```

Python uses `Iterable[Forecast | Search]` as the response model. The Rust equivalent is `create_parallel::<Action>` returning `Vec<Action>`. Match on each variant to decide what to do with its arguments. A single model also works with `create_parallel::<Forecast>` to receive several calls to one tool. Names must be distinct after the provider normalizes schema titles.

## Choose the operation

| Goal | Call | Delivery |
| --- | --- | --- |
| Request several native tool calls | `create_parallel::<Action>` | A vector in provider order once every call validates |
| Include the original tool calls and IDs | `create_parallel_with_completion::<Action>` | The vector paired with the raw provider JSON |
| Return several values under one schema | `create::<Vec<T>>` | One complete typed list |
| Process a collection incrementally | `create_iterable::<T>` | One validated item per stream event |

Parallel requests use the configured validators, request context, retries, token budget, hooks, and cache. A rejected set can trigger a correction of the whole response. Use `.await?` with the async client. The native parallel API returns a complete vector. Streaming a collection uses a single array schema. [Streaming](streaming.md)

Select a model and route supporting parallel function calling. The default tool mode works for OpenAI, Anthropic, and OpenRouter. To select tools on Gemini, set `.with_mode(Mode::Tools)` on its configuration. [Provider configuration](providers.md)
