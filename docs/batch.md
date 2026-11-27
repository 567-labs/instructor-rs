# Run a batch of independent extractions

Use `client.batch::<T>()` to prepare requests sharing a response type and submit them as a provider-managed job. Keep the job ID to check status and download results from a later process. Each input gets its own custom ID, which identifies its result even if the provider reorders the records.

| Provider | Default batch mode | Status for retrieval |
| --- | --- | --- |
| OpenAI | Responses with `JsonSchema` | `completed`, plus available files from `cancelled` or `expired` jobs |
| Anthropic | `Tools` | `ended` |
| Mistral | Chat Completions with `JsonSchema` | `SUCCESS` |

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

## Work with OpenAI batch files

New OpenAI requests use `/v1/responses` with the prompt in `input` and the schema in `text.format`. Set `Request::new(prompt).option("max_output_tokens", 2000)` to override the default limit of 1,000 tokens. Set `.strict(false)` on an individual request to allow a flexible schema for that record.

To submit a saved Chat Completions JSONL file, pass its contents directly to `batch.submit`. Each row's `url` determines the job endpoint, and all rows in a file must agree. The batch parser reads saved results from both APIs. To prepare new Chat Completions files, pass a configuration built with `ProviderConfig::from_env(model)?.with_chat_completions()` to `Client::new` or `BlockingClient::new`. [OpenAI Batch guide](https://developers.openai.com/api/docs/guides/batch/)

## Recover individual successes and failures

The outer `Result` from `batch.results(id)` covers job retrieval. Each record's `result` describes its extraction, with `raw` holding the provider record and `custom_id` identifying the input when available. Match on the per-record `Result` and retain the ID when collecting failures to resubmit. OpenAI can return separate output and error files. Anthropic includes the record status with each result.

`results_raw` returns the original result JSON Lines, and `parse` converts a saved string into typed records locally. If a validator needs request-specific data, keep the context by custom ID in your application and supply a lookup closure to `parse_with_context`. This also works when results arrive in a different order.

## Download a large batch and read it one result at a time

Call `batch.download(id, path).await?` on the async client or `batch.download(id, path)?` on the blocking client. For OpenAI, Anthropic and Mistral, this streams the result body into a temporary file alongside the destination, validating UTF-8 in blocks of up to 64 KiB. OpenAI's error records follow its output records. The destination is replaced atomically after all files have arrived and the writes finish. If the transfer fails or you drop the future while it's downloading, the previous file stays available. Retry with the same job ID.

Pass a `BufReader<File>` to `parse_reader` on either batch client. Advance the iterator to read the next nonblank JSONL record. For context-sensitive validation, use `parse_reader_with_context(reader, |id| ...)` with the same lookup as `parse_with_context`.

```rust
use instructor_ai::{batch::BlockingBatch, model, Error};
use std::{fs::File, io::BufReader, path::Path};

#[model]
struct Person {
    name: String,
    age: u8,
}

fn download_and_inspect(batch: &BlockingBatch<Person>, id: &str, path: &Path) -> Result<(), Error> {
    batch.download(id, path)?;
    let file = BufReader::new(File::open(path)?);
    for row in batch.parse_reader(file) {
        let record = row?;
        let id = record.custom_id.as_deref().unwrap_or("unknown");
        match record.result {
            Ok(person) => println!("{id} {} {}", person.name, person.age),
            Err(error) => eprintln!("{id} {error}"),
        }
    }
    Ok(())
}
```

A malformed line yields a `BatchResult` with a `json_parse_error` and the line in `raw["raw_line"]`. Invalid UTF-8 bytes appear there as replacement characters. Iteration continues after malformed records, provider errors and validation failures. A read failure yields an outer `io::Error` once and ends the iterator. Lines may end in LF, CRLF or EOF.

The iterator reads synchronously and retains a buffer sized for the longest line. Process each record in the loop to keep memory use independent of the number of records. Use `results_raw` when you need the complete JSONL string in memory. The client also reads job metadata and HTTP error bodies into memory. [Python file parser](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/batch/__init__.py#L74)

## Manage a job

`status` exposes the provider status, counts, and native job JSON. Use `list` for job discovery, `cancel` to request a stop, and `download` to save result data. Mistral supports deleting a job. Set submission options with `submit_with_options` and select an explicit output mode with `client.batch_with_mode::<T>(mode)`.

The async and blocking clients both expose the batch processor. On an async client, await the network operations. Preparation and local parsing run synchronously. Python's `BatchProcessor` covers the same three providers. [Python batch guide](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/batch.md) · [Provider credentials](providers.md) · [Comparison](parity.md)
