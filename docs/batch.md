# Run a batch of independent extractions

Use `client.batch::<T>()` to prepare requests sharing a response type and submit them as a provider-managed job. Keep the job ID to check status and download results from a later process. Each input gets its own custom ID, which identifies its result even if the provider reorders the records.

| Provider | Default batch mode | Status for retrieval |
| --- | --- | --- |
| OpenAI | `JsonSchema` | `completed`, plus available files from `cancelled` or `expired` jobs |
| Anthropic | `Tools` | `ended` |
| Mistral | `JsonSchema` | `SUCCESS` |

## Prepare, submit, and retrieve

Set the selected provider's key. The `prepare` command builds the JSON Lines payload locally. The `submit` command creates a provider job and prints its ID. Set `BATCH_ID` to that ID, check the status, and request the results when the job is ready.

```sh
cargo run -p instructor-ai --example batch -- openai/gpt-6-luna prepare
cargo run -p instructor-ai --example batch -- openai/gpt-6-luna submit
cargo run -p instructor-ai --example batch -- openai/gpt-6-luna status "$BATCH_ID"
cargo run -p instructor-ai --example batch -- openai/gpt-6-luna results "$BATCH_ID"
```

For Claude, use `anthropic/claude-haiku-5-5` with `ANTHROPIC_API_KEY` for each command. Use the same model type and client configuration when parsing the results.

```rust
--8<-- "instructor/examples/batch.rs"
```

`batch.prepare` accepts prompts or `BatchItem` values. You can construct an item with a `Request` to include provider options per record. The generated IDs start at `request-0`. Use `.id(...)` to carry identifiers from your dataset. Preparation checks IDs before submission. To save input atomically for a later submission, use `batch.prepare_file(requests, path)?` and read it with `std::fs::read_to_string`. For OpenAI or Mistral JSON Schema mode, `batch.strict(false)` permits open-ended maps.

## Recover individual successes and failures

The outer `Result` from `batch.results(id)` covers job retrieval. Each record's `result` describes its extraction, with `raw` holding the provider record and `custom_id` identifying the input when available. Match on the per-record `Result` and retain the ID when collecting failures to resubmit. OpenAI can return separate output and error files. Anthropic includes the record status with each result.

`results_raw` returns the original result JSON Lines, and `parse` converts a saved string into typed records locally. If a validator needs request-specific data, keep the context by custom ID in your application and supply a lookup closure to `parse_with_context`. This also works when results arrive in a different order.

## Manage a job

`status` exposes the provider status, counts, and native job JSON. Use `list` for job discovery, `cancel` to request a stop, and `download` to save result data. Mistral supports deleting a job. Set submission options with `submit_with_options` and select an explicit output mode with `client.batch_with_mode::<T>(mode)`.

The async and blocking clients both expose the batch processor. On an async client, await the network operations. Preparation and local parsing run synchronously. Python's `BatchProcessor` covers the same three providers. [Python batch guide](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/batch.md) · [Provider credentials](providers.md)
