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

After the smoke run verified all 61 results, timing began on 2026-11-27 at 02:55 UTC. Each workload ran in six alternating pairs with a 10 ms calibration target. The run used an Intel Xeon Platinum 8573C, Linux x86-64, a four-CPU quota, CPython 3.12.14, Instructor 1.18.0 and rustc 1.100.0. Both checkouts were clean. Rust was at `67aae030c4165d31b4161a334952f2a57dcaa93f` with default features, Python at `b01fff4e01ecab2bdcea6ad24e795bc3f549b89d`. The Rust and harness source fingerprint was `5ec2be24154508535082421675ca5f1d90dd91351d95ad188e362c009149187b`. The raw samples, versions and lockfile hashes are in `benchmarks/recorded/report.json`. [Rust checkout](https://github.com/567-labs/instructor-rs/tree/67aae030c4165d31b4161a334952f2a57dcaa93f) · [Python checkout](https://github.com/567-labs/instructor/tree/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d)

The workers used synthetic credentials inside a Linux network namespace exposing only loopback. The server verified 37,701 requests across output checks, calibration and timing, including 15,609 to Responses, 9,786 streaming responses and 666 invalid responses for retries. The previous 49-case suite used Chat Completions for OpenAI, OpenRouter and Mistral and the earlier Gemini protocol in both languages. The current suite adds 12 Responses cases and exercises Rust's Interactions integration. [Previous 49-case run](https://github.com/567-labs/instructor-rs/blob/4328ec17dd5905d25210fb5b39f707768a88c527/benchmarks/recorded/report.json) · [First recorded run](https://github.com/567-labs/instructor-rs/blob/def3761d0fe33db821c4ae2d52d10339ef2617d8/benchmarks/recorded/report.json)

Rust took less time in all 61 workload medians and all 366 individual pairs. Ratios of medians ranged from 2.93× for `batch-parse/anthropic` to 234.11× for `decision/jev-latest/questions`. OpenAI Responses text extraction took 176.22 µs in Rust and 2,654.23 µs in Python. Streaming the same record as three partials took 286.12 µs and 5,890.96 µs respectively. For a Jev decision, the Python/Rust median ratio was 10.84× with direct routing and 11.15× through OpenRouter.

| Workload | Python µs/op | Rust µs/op | Python / Rust | Round ratio range |
| --- | ---: | ---: | ---: | ---: |
| schema/batch | 239.005 | 2.594 | 92.14× | 90.16–94.21 |
| parse/1 | 1.603 | 0.158 | 10.17× | 9.31–10.87 |
| parse/16 | 8.970 | 1.489 | 6.02× | 5.81–6.27 |
| parse/128 | 61.261 | 9.292 | 6.59× | 6.46–6.84 |
| parse/fenced | 20.779 | 1.319 | 15.76× | 11.83–16.29 |
| parse/invalid | 2.389 | 0.476 | 5.02× | 3.32–5.32 |
| context/validate | 9.205 | 1.362 | 6.76× | 4.58–7.45 |
| context/validate_error | 15.953 | 1.547 | 10.31× | 7.30–18.55 |
| partial/1x8 | 80.238 | 4.040 | 19.86× | 10.95–60.18 |
| partial/16x16 | 1257.630 | 185.897 | 6.77× | 6.11–12.77 |
| partial/16x128 | 174.008 | 21.594 | 8.06× | 7.93–15.13 |
| partial/128x128 | 6189.042 | 1493.026 | 4.15× | 4.08–7.93 |
| iterable/1x8 | 8.357 | 0.716 | 11.67× | 11.44–20.24 |
| iterable/16x16 | 81.946 | 5.850 | 14.01× | 12.37–24.14 |
| iterable/16x128 | 69.511 | 5.587 | 12.44× | 11.66–21.10 |
| iterable/128x128 | 533.631 | 44.496 | 11.99× | 8.35–20.91 |
| partial_first/16x20 | 16.282 | 0.946 | 17.21× | 7.12–27.79 |
| iterable_first/16x20 | 7.125 | 0.530 | 13.45× | 9.62–21.65 |
| partial_async/16x16 | 1345.604 | 191.528 | 7.03× | 3.67–13.63 |
| iterable_async/16x16 | 106.249 | 6.377 | 16.66× | 9.07–52.06 |
| decision/jev-latest/questions | 812.823 | 3.472 | 234.11× | 199.48–429.81 |
| decision/jev-latest/answers | 4.388 | 0.371 | 11.82× | 11.41–20.94 |
| decision/jev-latest/reject | 1.502 | 0.430 | 3.49× | 3.03–6.66 |
| decision/gpt-6-luna/questions | 889.562 | 4.579 | 194.29× | 179.65–329.91 |
| decision/gpt-6-luna/answers | 5.687 | 0.407 | 13.98× | 11.22–23.74 |
| decision/gpt-6-luna/reject | 2.635 | 0.448 | 5.88× | 5.67–10.92 |
| batch-prepare/openai | 2622.425 | 88.051 | 29.78× | 24.90–57.10 |
| batch-parse/openai | 120.511 | 27.284 | 4.42× | 4.16–7.40 |
| batch-prepare/anthropic | 2302.275 | 84.686 | 27.19× | 23.79–28.99 |
| batch-parse/anthropic | 107.296 | 36.650 | 2.93× | 1.75–3.02 |
| batch-prepare/mistral | 2309.142 | 86.374 | 26.73× | 22.79–36.83 |
| batch-parse/mistral | 122.016 | 30.160 | 4.05× | 3.56–4.33 |
| cache/hit | 546.469 | 6.795 | 80.42× | 55.82–425.84 |
| http/openai | 2046.729 | 166.020 | 12.33× | 8.44–72.92 |
| http/anthropic | 1959.740 | 155.166 | 12.63× | 11.49–13.23 |
| http/openrouter | 1856.322 | 182.433 | 10.18× | 8.79–30.82 |
| http/google | 4185.249 | 180.446 | 23.19× | 19.81–30.80 |
| http/cohere | 1572.724 | 177.346 | 8.87× | 8.64–13.38 |
| http/mistral | 9877.829 | 172.811 | 57.16× | 37.06–87.99 |
| http/retry | 4012.229 | 340.210 | 11.79× | 3.62–16.42 |
| http-parallel/openai | 3659.798 | 192.387 | 19.02× | 5.69–24.97 |
| http-stream/openai | 5129.831 | 217.350 | 23.60× | 5.69–38.51 |
| http-parallel/anthropic | 3092.841 | 199.724 | 15.49× | 13.69–20.07 |
| http-stream/anthropic | 4884.166 | 214.902 | 22.73× | 19.96–30.66 |
| http-parallel/openrouter | 3547.278 | 186.227 | 19.05× | 15.91–27.16 |
| http-stream/openrouter | 5128.964 | 202.370 | 25.34× | 21.74–40.46 |
| http-decision/typesafe/jev-latest | 1810.583 | 167.081 | 10.84× | 8.69–15.43 |
| http-decision/openrouter/typesafe/jev-1.14 | 1855.756 | 166.426 | 11.15× | 8.63–37.68 |
| http-decision/openai/gpt-6-luna | 1832.155 | 189.428 | 9.67× | 4.85–15.10 |
| http/openai/responses-text | 2654.232 | 176.224 | 15.06× | 7.25–21.05 |
| http-stream/openai/responses-text | 5890.956 | 286.115 | 20.59× | 9.42–44.53 |
| http/openai/responses-tool | 2499.685 | 203.083 | 12.31× | 7.17–66.31 |
| http-stream/openai/responses-tool | 5839.635 | 248.194 | 23.53× | 18.76–30.61 |
| http/openrouter/responses-text | 2593.470 | 162.969 | 15.91× | 12.99–42.87 |
| http-stream/openrouter/responses-text | 6440.809 | 235.911 | 27.30× | 19.77–49.14 |
| http/openrouter/responses-tool | 2498.788 | 167.442 | 14.92× | 12.58–17.64 |
| http-stream/openrouter/responses-tool | 5808.207 | 261.187 | 22.24× | 17.10–50.06 |
| http/mistral/responses-text | 2608.015 | 190.003 | 13.73× | 8.03–17.27 |
| http-stream/mistral/responses-text | 5898.858 | 294.707 | 20.02× | 12.86–26.76 |
| http/mistral/responses-tool | 2691.778 | 173.332 | 15.53× | 13.63–26.47 |
| http-stream/mistral/responses-tool | 6062.483 | 233.216 | 26.00× | 8.33–42.09 |

## Interpretation and limitations

The fixtures have fixed field types, short strings and predictable chunk boundaries. Partial streams retain all snapshots until the end of each iteration, so their cost grows with the number and size of intermediate models. First-yield measurements start with fragments already in memory. The synchronous and asynchronous cases isolate local work under a single consumer. Applications with different schemas, backpressure or concurrent requests can have different costs.

HTTP timings include the provider SDK on the Python side, the Rust HTTP client, the fixture server and OS scheduling. Both workers reuse their native client stacks. Provider requests can differ in optional metadata and schema representation. The server simulates immediate responses and the measurements exclude remote network latency, model inference and token generation delays. These ratios describe the measured client workflows on this machine. Python also uses native Rust components through Pydantic Core and jiter.

The report measures warmed wall-clock time. We paused the project’s other builds for the timed rounds. The shared cgroup reported 53.927 ms of CPU throttling during the rounds. CPU frequency, shared host activity, allocator state, compiler optimizations and interpreter versions affect the results. For `http/openai/responses-tool`, the six paired ratios ranged from 7.17× to 66.31×. A 10 ms calibration target can yield small batches for expensive operations. Memory use, tail latency and sustained concurrent throughput require separate measurements.

The three Responses providers share one loopback fixture and Python’s OpenAI-compatible transport. Their timings describe processing the same record through each Rust provider configuration. Production provider behavior requires independent validation. Google uses different SDK protocols across the two languages. Batch timings cover in-memory preparation and parsing. Isolating disk downloads or the cost of a particular feature requires a dedicated workload. To compare revisions, alternate them under the same conditions.
