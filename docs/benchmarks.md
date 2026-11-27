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

The archived run covers the original 49 cases. The current harness adds 12 Responses cases and updates the Rust Google protocol.

The run started on 2026-11-26 at 21:39 UTC on an Intel Xeon Platinum 8573C with Linux x86-64, a four-CPU cgroup quota, CPython 3.12.14 and rustc 1.100.0. Python was at `b01fff4e` (Instructor 1.18.0). The measured Rust checkout was `494da5964429713003ee85890d67eeb5590df7e3`. The review revision `61718fce4b81d4d48055d432d175a375174b2fca` differs only in `docs/parity.md` and has the same Rust and harness source fingerprint, `05074292da031574e2a76a0ee7271731bc88ec6767e35e983c90b84fdff10ded`. Use these review and Python revisions to reproduce the measurements. [Rust benchmark checkout](https://github.com/567-labs/instructor-rs/tree/61718fce4b81d4d48055d432d175a375174b2fca) · [Python benchmark checkout](https://github.com/567-labs/instructor/tree/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d)

The smoke run verified all 49 workloads. Six alternating timed pairs used a 10 ms calibration target and the archived dependency locks. The workers ran with synthetic credentials inside a Linux network namespace exposing only loopback. The server verified 21,375 requests across calibration, output checks and timing, including 3,252 streaming responses and 636 invalid responses for retries. See `benchmarks/recorded/report.json` for the raw samples and environment. The previous run in PR #39 used the same workloads, dependency locks and runtimes. [Previous raw measurements](https://github.com/567-labs/instructor-rs/blob/def3761d0fe33db821c4ae2d52d10339ef2617d8/benchmarks/recorded/report.json) · [Original benchmark PR](https://github.com/567-labs/instructor-rs/pull/39)

Rust took less time in all 49 workload medians and all 294 individual pairs. The median ratios ranged from 2.94× for parsing an Anthropic batch to 162.48× for preparing OpenAI decision questions. For 16 records in 16-byte chunks, a full synchronous partial stream took 187.20 µs in Rust and 1,261.20 µs in Python. Consuming individually completed records took 6.14 µs and 83.13 µs respectively. The Jev ratios were 157.38× for preparing three questions, 13.87× for decoding their answers and 11.09× for a complete loopback request.

| Workload | Python µs/op | Rust µs/op | Python / Rust | Round ratio range |
| --- | ---: | ---: | ---: | ---: |
| schema/batch | 240.743 | 2.614 | 92.09× | 81.98–94.92 |
| parse/1 | 1.618 | 0.156 | 10.37× | 9.28–13.01 |
| parse/16 | 9.048 | 1.480 | 6.11× | 5.40–6.26 |
| parse/128 | 61.455 | 9.176 | 6.70× | 5.64–6.96 |
| parse/fenced | 20.955 | 1.282 | 16.34× | 11.33–16.57 |
| parse/invalid | 2.399 | 0.462 | 5.20× | 4.74–5.54 |
| context/validate | 9.472 | 1.353 | 7.00× | 6.60–12.69 |
| context/validate_error | 16.054 | 1.395 | 11.51× | 10.40–12.37 |
| partial/1x8 | 81.705 | 3.849 | 21.23× | 20.22–34.84 |
| partial/16x16 | 1261.202 | 187.199 | 6.74× | 6.43–68.15 |
| partial/16x128 | 177.168 | 20.627 | 8.59× | 8.21–14.46 |
| partial/128x128 | 6227.383 | 1502.910 | 4.14× | 2.66–4.30 |
| iterable/1x8 | 8.424 | 0.742 | 11.35× | 9.93–12.17 |
| iterable/16x16 | 83.133 | 6.138 | 13.54× | 12.17–14.01 |
| iterable/16x128 | 70.995 | 5.893 | 12.05× | 10.49–16.61 |
| iterable/128x128 | 543.232 | 45.672 | 11.89× | 11.68–15.18 |
| partial_first/16x20 | 16.294 | 0.899 | 18.13× | 17.19–20.48 |
| iterable_first/16x20 | 7.222 | 0.548 | 13.17× | 12.59–42.07 |
| partial_async/16x16 | 1357.140 | 182.667 | 7.43× | 7.16–12.73 |
| iterable_async/16x16 | 108.766 | 6.661 | 16.33× | 15.12–28.01 |
| decision/jev-latest/questions | 842.866 | 5.356 | 157.38× | 149.25–226.46 |
| decision/jev-latest/answers | 5.197 | 0.375 | 13.87× | 11.65–18.89 |
| decision/jev-latest/reject | 1.965 | 0.491 | 4.00× | 3.09–6.02 |
| decision/gpt-6-luna/questions | 1063.243 | 6.544 | 162.48× | 90.70–533.56 |
| decision/gpt-6-luna/answers | 6.599 | 0.402 | 16.40× | 12.97–30.70 |
| decision/gpt-6-luna/reject | 2.712 | 0.459 | 5.90× | 2.71–21.02 |
| batch-prepare/openai | 2607.417 | 192.220 | 13.56× | 12.93–14.18 |
| batch-parse/openai | 126.439 | 28.928 | 4.37× | 3.53–4.84 |
| batch-prepare/anthropic | 2301.429 | 221.979 | 10.37× | 10.13–12.37 |
| batch-parse/anthropic | 108.221 | 36.781 | 2.94× | 2.92–3.04 |
| batch-prepare/mistral | 2413.935 | 188.380 | 12.81× | 11.03–26.44 |
| batch-parse/mistral | 122.433 | 31.819 | 3.85× | 3.72–9.59 |
| cache/hit | 554.356 | 9.893 | 56.03× | 50.17–58.67 |
| http/openai | 2031.863 | 173.805 | 11.69× | 6.91–14.08 |
| http/anthropic | 1926.411 | 152.488 | 12.63× | 10.98–17.54 |
| http/openrouter | 1968.755 | 196.208 | 10.03× | 8.08–10.73 |
| http/google | 4245.566 | 174.939 | 24.27× | 21.75–29.36 |
| http/cohere | 1654.083 | 189.488 | 8.73× | 7.34–12.03 |
| http/mistral | 9515.038 | 185.411 | 51.32× | 35.76–74.39 |
| http/retry | 4075.313 | 380.991 | 10.70× | 9.10–18.91 |
| http-parallel/openai | 3782.058 | 221.989 | 17.04× | 5.10–21.00 |
| http-stream/openai | 5192.447 | 289.451 | 17.94× | 4.67–47.93 |
| http-parallel/anthropic | 3134.349 | 220.176 | 14.24× | 4.62–20.76 |
| http-stream/anthropic | 4825.126 | 259.459 | 18.60× | 5.37–92.83 |
| http-parallel/openrouter | 3589.642 | 202.503 | 17.73× | 16.98–21.35 |
| http-stream/openrouter | 5536.789 | 232.675 | 23.80× | 15.58–29.64 |
| http-decision/typesafe/jev-latest | 2018.766 | 182.057 | 11.09× | 5.31–13.37 |
| http-decision/openrouter/typesafe/jev-1.13 | 1777.979 | 165.085 | 10.77× | 7.95–13.82 |
| http-decision/openai/gpt-6-luna | 1875.088 | 186.422 | 10.06× | 8.24–12.55 |

## Interpretation and limitations

The fixtures have fixed field types, short strings and predictable chunk boundaries. Partial streams retain all snapshots until the end of each iteration, so their cost grows with the number and size of intermediate models. First-yield measurements start with fragments already in memory. The synchronous and asynchronous cases isolate local work under a single consumer. Applications with different schemas, backpressure or concurrent requests can have different costs.

HTTP timings include the provider SDK on the Python side, the Rust HTTP client, the fixture server and OS scheduling. Both workers reuse their native client stacks. Provider requests can differ in optional metadata and schema representation. The server simulates immediate responses and the measurements exclude remote network latency, model inference and token generation delays. These ratios describe the measured client workflows on this machine. Python also uses native Rust components through Pydantic Core and jiter.

The report measures warmed wall-clock time. Other CPU-intensive project checks paused during collection. The shared cgroup reported 94.629 ms of CPU throttling. CPU frequency, system load, interpreter version, allocator behavior and compiler optimizations affect the results. For `partial/16x16`, the six paired ratios ranged from 6.43× to 68.15×. A 10 ms calibration target gives only a few iterations for the slowest operations. Memory use, tail latency and sustained concurrent throughput need separate measurements.

Each report applies to the source fingerprint and dependency versions it records. This run includes the changes to nested partial validation, token accounting, cache backends and batch readers. Operations such as using an application-provided cache or reading a batch from a file require dedicated workloads to isolate their costs. Attributing timing changes between revisions to a specific feature would require alternating the two Rust revisions under the same conditions.
