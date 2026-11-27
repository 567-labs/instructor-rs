# Python and Rust performance

The suite compares the time to produce the same typed data from the same input in each implementation. Local cases start with strings or decoded JSON. HTTP cases use the public clients and a server on the loopback interface. A run needs no provider credentials. The harness in `benchmarks/compare.py` checks its results before printing any ratios. It builds on the boundaries used in the [Python local benchmark suite](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/benchmarks/README.md).

## Reproduce the run

Start in a fresh Rust checkout containing this suite with Cargo, rustc, uv and Python 3.12 available. The commands resolve the Python dependencies from the reference checkout's lockfile and use the recorded Rust dependency graph. Dependency setup and compilation can require network access. Each timed HTTP request goes to the fixture server.

```sh
ref_dir="$(mktemp -d)"
git clone https://github.com/567-labs/instructor.git "$ref_dir"
git -C "$ref_dir" checkout --detach b01fff4e01ecab2bdcea6ad24e795bc3f549b89d
uv sync --project "$ref_dir" --python 3.12 --frozen \
    --extra dev --extra google-genai --extra cohere --extra mistral
if [ ! -e Cargo.lock ]; then
    cp benchmarks/recorded/Cargo.lock Cargo.lock
fi
"$ref_dir/.venv/bin/python" benchmarks/compare.py --python-repo "$ref_dir" --smoke
"$ref_dir/.venv/bin/python" benchmarks/compare.py --python-repo "$ref_dir" \
    --rounds 6 --target-ms 10 --output /tmp/instructor-timings.json
```

The driver uses the existing workspace lockfile with `--locked` when present. Compare its hash to the recorded hash to check for dependency changes. If your HTTP proxy configuration requires SOCKS support, add it to the reference environment with `uv pip install --python "$ref_dir/.venv/bin/python" 'httpx[socks]==0.28.1'`. The worker verifies that its imported Instructor package comes from `--python-repo`. The reference commit matters because the isolated benchmarks call internal Python functions used by the public clients.

`--smoke` checks every case with a single measured iteration and suppresses timing comparisons. The normal run prints a Markdown table and can write a JSON report with all batch durations, iteration counts, revisions, dependency versions, source fingerprint and lockfile hashes. Increase `--target-ms` for longer batches. Use an even number of rounds to balance execution order.

## Workloads

| Family | Work per invocation |
| --- | --- |
| Schema | Generate a fresh JSON Schema for a batch with a required list of records via `model_json_schema()` and `Batch::json_schema()`. Compare the required properties and types after resolving references. |
| Parsing | Decode compact JSON into typed batches of 1, 16 or 128 records via `_validate_model_from_json(strict=True)` and `parse_response`. Each record has an integer ID and a title. The fenced case includes a Markdown JSON block. The invalid case must reject a string ID. |
| Context | Parse 16 records and apply a custom model validator using a caller-supplied item limit. Limits of 16 and 15 exercise success and rejection. |
| Local streams | Start with the same pre-split ASCII payload and collect all typed snapshots or iterable items. In `16x128`, 16 is the record count and 128 is the maximum bytes per fragment. The synchronous and asynchronous cases call the libraries' fragment parsers directly. The async cases use an already-created event loop or current-thread Tokio runtime with an immediately available source. |
| First yield | Construct a local stream, obtain one typed snapshot or complete item and drop the stream. The first partial contains ID zero and an absent title. These times include cleanup. |
| Local decisions | Build the request body from the same annotated or derived ticket, including a templated instruction, two choices, a Noul probability and a three-level score scaled to 0–10. Parse pre-decoded responses with a prepared model. A raw score of 1.5 maps to 7.5 and a raw score of 3 must fail. Jev uses a map of answers. OpenAI uses an ordered array with a predicate. |
| Batch | Prepare a complete in-memory JSONL buffer from 16 prompts or parse 16 successful result lines with request IDs and typed records. Cases cover OpenAI, Anthropic and Mistral. Python passes `temperature=None` on Anthropic to produce the same request fields as Rust’s Messages API version `2026-10-21`. The Python worker redirects the processor's status messages to the null device. Rust retains the raw envelope in each parsed result. |
| Cache | Repeat a typed OpenAI request using an in-process LRU of capacity 10. The initial proof call populates the cache. The fixture server confirms that subsequent requests hit the cache. |
| HTTP creation | Send the same prompt and record schema, then return one typed record. OpenAI, OpenRouter, Anthropic and Mistral use tools. Google Gemini and Cohere use their native schema output. The retry case receives one invalid OpenAI tool result, reasks with feedback and accepts the corrected record. |
| HTTP Responses | Use the default endpoint and text schema or a named function through OpenAI, OpenRouter and Mistral. Each provider has four cases covering both output modes with complete responses and three streamed partials. OpenRouter uses a provider-qualified model ID. |
| HTTP parallel tools | Send two different tool definitions through OpenAI, Anthropic or OpenRouter, and consume both returned calls in order. The first returns one record and the second a batch of two records. Rust uses a typed enum and Python an iterable over a union. |
| HTTP streaming | Consume a complete OpenAI, OpenRouter or Anthropic tool call delivered in three argument fragments over server-sent events. The fixture writes the framed events together without delays. |
| HTTP decisions | Call the blocking Decisions client with a mixed ticket model against a fixed answer for direct Jev, Jev through OpenRouter or native OpenAI. The measured work includes preparation, HTTP and typed answer validation. |

`benchmarks/workloads.py` generates each fixture once and the driver passes it to both workers. Validation checks independent expected values, then compares the workers' results across languages and rounds. For a full stream it checks every interim snapshot or completed item in order. Rust must terminate partial streams with `PartialEvent::Complete`. Batch preparation compares decoded request records, allowing for provider schema dialect annotations and the field titles, formats and descriptions emitted by Pydantic and Schemars. Batch result comparison checks IDs, success and typed values. OpenAI decision evidence is compared as decoded JSON. The server counts total, streaming and invalid responses to verify that the clients perform the expected requests and retries.

## Timing method

The `responses-text` and `responses-tool` cases send `input` to `/v1/responses`. Rust selects the provider’s default configuration for text or `Mode::Tools` for functions. Python uses `from_openai` with `Mode.RESPONSES_TOOLS` and a loopback base URL for all three compatible endpoints. This handler generates both a function and a text schema. Its text cases pass `tool_choice="none"`. The server checks the schema properties and counts requests for each output mode. In streaming cases it returns the same three text or argument deltas to both clients. The comparisons cover SDK and protocol handling for fixed outputs. [Python Responses handler](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/providers/openai/handlers.py#L1235)

The original OpenAI, OpenRouter and Mistral generation cases explicitly select Chat Completions in Rust, matching Python’s provider integrations. OpenAI and Mistral batch cases prepare and parse Chat jobs. The Google case measures a common schema extraction across two protocols. Rust receives a model output step from `/v1/interactions` and Python a candidate from `/v1beta/models/fixture:generateContent`. The server verifies which protocol each worker used. Anthropic clients call `/v1/messages` with their respective API version headers. [Python Google integration](https://github.com/567-labs/instructor/tree/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/providers/genai)

Each round starts fresh Python and Rust worker processes sequentially. Their order alternates. The driver builds the Rust worker once in release mode. The workers create clients, prepare fragments, initialize model types and materialize a result for comparison outside the timed loop. HTTP clients reuse connections within a workload. The server runs in the driver process with HTTP/1.1 keep-alive and TCP_NODELAY.

Each worker warms up a case by doubling the batch length from one until a trial reaches the target duration. It times a new batch of that length with `time.perf_counter_ns` or `Instant`. Each iteration consumes and releases its result, with Rust using `black_box`. A stream iteration covers the entire indicated consumption pattern. Parsing a decision answer excludes question preparation. Serializing results for the harness, interpreter startup, imports, Cargo compilation and reading fixtures fall outside the timer. Python uses its default garbage collector.

For each language the table reports the median of the round-level values `elapsed_ns / iterations` in microseconds. Python / Rust divides the Python median by the Rust median. A ratio of 2 means Rust took half the Python time. A ratio of 0.5 means Rust took twice the Python time. The last column gives the smallest and largest Python / Rust ratio among individual paired rounds.

## Recorded results

After the smoke run verified all 61 results, timing began on 2026-11-27 at 03:04 UTC. Each workload ran in six alternating pairs with a 10 ms calibration target. The run used an Intel Xeon Platinum 8573C, Linux x86-64, a four-CPU quota, CPython 3.12.14, Instructor 1.18.0 and rustc 1.100.0. Both checkouts were clean. Rust was at `a5de07be4b24c5cde61f0fa7c548bbef462115f1` with default features, Python at `b01fff4e01ecab2bdcea6ad24e795bc3f549b89d`. The Rust and harness source fingerprint was `cfb89040c5ae390649b6f83c8f7f54ed3e0031594976fe5a87ebbee3408acc2a`. The raw samples, versions and lockfile hashes are in `benchmarks/recorded/report.json`. [Rust checkout](https://github.com/567-labs/instructor-rs/tree/a5de07be4b24c5cde61f0fa7c548bbef462115f1) · [Python checkout](https://github.com/567-labs/instructor/tree/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d)

The workers used synthetic credentials inside a Linux network namespace exposing only loopback. The server verified 38,805 requests across output checks, calibration and timing, including 16,104 to Responses, 9,798 streaming responses and 642 invalid responses for retries. The previous 49-case suite used Chat Completions for OpenAI, OpenRouter and Mistral and the earlier Gemini protocol in both languages. The current suite adds 12 Responses cases and exercises Rust's Interactions integration. The new measurement includes the native `response_format.json_schema` field for Cohere v2. [Earlier 61-case run](https://github.com/567-labs/instructor-rs/blob/1bf2316caeccbae6d6b3216edc9fe940f81b4d56/benchmarks/recorded/report.json) · [Previous 49-case run](https://github.com/567-labs/instructor-rs/blob/4328ec17dd5905d25210fb5b39f707768a88c527/benchmarks/recorded/report.json) · [First recorded run](https://github.com/567-labs/instructor-rs/blob/def3761d0fe33db821c4ae2d52d10339ef2617d8/benchmarks/recorded/report.json)

Rust took less time in all 61 workload medians and all 366 individual pairs. Ratios of medians ranged from 3.05× for `batch-parse/anthropic` to 243.74× for `decision/jev-latest/questions`. OpenAI Responses text extraction took 205.38 µs in Rust and 2,401.21 µs in Python. Streaming the same record as three partials took 289.93 µs and 5,933.48 µs respectively. For a Jev decision, the Python/Rust median ratio was 10.66× with direct routing and 10.40× through OpenRouter.

| Workload | Python µs/op | Rust µs/op | Python / Rust | Round ratio range |
| --- | ---: | ---: | ---: | ---: |
| schema/batch | 237.484 | 2.596 | 91.48× | 60.64–94.97 |
| parse/1 | 1.637 | 0.156 | 10.48× | 5.37–10.96 |
| parse/16 | 8.785 | 1.498 | 5.86× | 3.44–6.21 |
| parse/128 | 60.620 | 9.339 | 6.49× | 3.88–6.57 |
| parse/fenced | 20.470 | 1.309 | 15.64× | 8.89–16.39 |
| parse/invalid | 2.359 | 0.455 | 5.18× | 2.86–5.60 |
| context/validate | 9.315 | 1.343 | 6.94× | 4.00–13.99 |
| context/validate_error | 16.159 | 1.415 | 11.42× | 6.91–12.89 |
| partial/1x8 | 81.386 | 3.944 | 20.63× | 11.05–21.22 |
| partial/16x16 | 1234.173 | 185.511 | 6.65× | 4.27–6.94 |
| partial/16x128 | 174.041 | 21.152 | 8.23× | 5.23–8.49 |
| partial/128x128 | 6195.882 | 1509.402 | 4.10× | 3.90–4.67 |
| iterable/1x8 | 10.232 | 0.714 | 14.33× | 10.93–29.26 |
| iterable/16x16 | 81.933 | 5.969 | 13.73× | 9.84–14.73 |
| iterable/16x128 | 67.935 | 5.711 | 11.89× | 7.09–12.53 |
| iterable/128x128 | 542.447 | 44.241 | 12.26× | 8.41–12.64 |
| partial_first/16x20 | 16.170 | 0.932 | 17.35× | 8.29–18.15 |
| iterable_first/16x20 | 7.239 | 0.519 | 13.95× | 6.73–14.59 |
| partial_async/16x16 | 1295.407 | 184.763 | 7.01× | 4.53–7.10 |
| iterable_async/16x16 | 106.381 | 6.303 | 16.88× | 16.14–17.24 |
| decision/jev-latest/questions | 834.850 | 3.425 | 243.74× | 236.97–264.50 |
| decision/jev-latest/answers | 4.440 | 0.374 | 11.88× | 8.51–12.54 |
| decision/jev-latest/reject | 1.496 | 0.429 | 3.49× | 1.93–3.50 |
| decision/gpt-6-luna/questions | 827.589 | 4.590 | 180.32× | 80.13–185.33 |
| decision/gpt-6-luna/answers | 5.505 | 0.404 | 13.62× | 12.97–13.87 |
| decision/gpt-6-luna/reject | 2.576 | 0.447 | 5.77× | 5.02–6.23 |
| batch-prepare/openai | 2588.195 | 86.185 | 30.03× | 28.85–31.88 |
| batch-parse/openai | 120.290 | 27.016 | 4.45× | 4.43–12.49 |
| batch-prepare/anthropic | 2272.003 | 84.353 | 26.93× | 26.26–28.17 |
| batch-parse/anthropic | 108.809 | 35.678 | 3.05× | 2.95–3.21 |
| batch-prepare/mistral | 2233.979 | 84.731 | 26.37× | 25.68–26.95 |
| batch-parse/mistral | 121.976 | 29.042 | 4.20× | 4.10–4.40 |
| cache/hit | 548.085 | 6.641 | 82.53× | 79.96–85.57 |
| http/openai | 1870.701 | 167.355 | 11.18× | 8.98–13.15 |
| http/anthropic | 2113.294 | 176.789 | 11.95× | 2.02–14.37 |
| http/openrouter | 1973.102 | 171.269 | 11.52× | 9.68–25.53 |
| http/google | 4162.610 | 161.034 | 25.85× | 19.13–30.27 |
| http/cohere | 1516.733 | 171.172 | 8.86× | 7.09–10.85 |
| http/mistral | 10467.037 | 189.500 | 55.23× | 22.46–113.61 |
| http/retry | 3868.659 | 346.808 | 11.16× | 8.73–31.09 |
| http-parallel/openai | 3813.306 | 188.944 | 20.18× | 19.52–22.73 |
| http-stream/openai | 5130.124 | 253.388 | 20.25× | 15.03–25.17 |
| http-parallel/anthropic | 3018.992 | 188.024 | 16.06× | 11.97–18.83 |
| http-stream/anthropic | 4267.871 | 269.265 | 15.85× | 11.67–21.00 |
| http-parallel/openrouter | 3561.293 | 218.678 | 16.29× | 10.36–27.90 |
| http-stream/openrouter | 5187.512 | 252.240 | 20.57× | 5.85–35.48 |
| http-decision/typesafe/jev-latest | 1752.555 | 164.352 | 10.66× | 8.80–12.46 |
| http-decision/openrouter/typesafe/jev-1.14 | 1853.192 | 178.107 | 10.40× | 5.78–12.07 |
| http-decision/openai/gpt-6-luna | 1738.260 | 185.431 | 9.37× | 8.58–10.69 |
| http/openai/responses-text | 2401.209 | 205.383 | 11.69× | 11.00–14.26 |
| http-stream/openai/responses-text | 5933.477 | 289.930 | 20.47× | 6.70–34.29 |
| http/openai/responses-tool | 2448.695 | 183.191 | 13.37× | 1.97–16.88 |
| http-stream/openai/responses-tool | 5676.629 | 278.312 | 20.40× | 16.97–26.18 |
| http/openrouter/responses-text | 2408.595 | 175.995 | 13.69× | 4.70–16.65 |
| http-stream/openrouter/responses-text | 6104.044 | 234.715 | 26.01× | 10.64–30.38 |
| http/openrouter/responses-tool | 2396.587 | 164.918 | 14.53× | 8.59–17.08 |
| http-stream/openrouter/responses-tool | 6025.217 | 228.513 | 26.37× | 7.74–30.71 |
| http/mistral/responses-text | 2449.059 | 158.637 | 15.44× | 7.14–18.31 |
| http-stream/mistral/responses-text | 6044.908 | 272.301 | 22.20× | 17.88–31.57 |
| http/mistral/responses-tool | 2602.882 | 152.641 | 17.05× | 13.12–23.66 |
| http-stream/mistral/responses-tool | 6004.314 | 263.579 | 22.78× | 18.99–31.76 |

## Interpretation and limitations

The fixtures have fixed field types, short strings and predictable chunk boundaries. Partial streams retain all snapshots until the end of each iteration, so their cost grows with the number and size of intermediate models. First-yield measurements start with fragments already in memory. The synchronous and asynchronous cases isolate local work under a single consumer. Applications with different schemas, backpressure or concurrent requests can have different costs.

HTTP timings include the provider SDK on the Python side, the Rust HTTP client, the fixture server and OS scheduling. Both workers reuse their native client stacks. Provider requests can differ in optional metadata and schema representation. The server simulates immediate responses and the measurements exclude remote network latency, model inference and token generation delays. These ratios describe the measured client workflows on this machine. Python also uses native Rust components through Pydantic Core and jiter.

The report measures warmed wall-clock time. We paused the project’s other builds for the timed rounds. The shared cgroup reported 74.983 ms of CPU throttling during the rounds. CPU frequency, shared host activity, allocator state, compiler optimizations and interpreter versions affect the results. For `http/openai/responses-tool`, the six paired ratios ranged from 1.97× to 16.88×. A 10 ms calibration target can yield small batches for expensive operations. Memory use, tail latency and sustained concurrent throughput require separate measurements.

The three Responses providers share one loopback fixture and Python’s OpenAI-compatible transport. Their timings describe processing the same record through each Rust provider configuration. Production provider behavior requires independent validation. Google uses different SDK protocols across the two languages. Batch timings cover in-memory preparation and parsing. Isolating disk downloads or the cost of a particular feature requires a dedicated workload. To compare revisions, alternate them under the same conditions.
