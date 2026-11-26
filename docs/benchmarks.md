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
| Batch | Prepare a complete in-memory JSONL buffer from 16 prompts or parse 16 successful result lines with request IDs and typed records. Cases cover OpenAI, Anthropic and Mistral. The Python worker redirects the processor's status messages to the null device. Rust retains the raw envelope in each parsed result. |
| Cache | Repeat a typed OpenAI request using an in-process LRU of capacity 10. The initial proof call populates the cache. The fixture server confirms that subsequent requests hit the cache. |
| HTTP creation | Send the same prompt and record schema, then return one typed record. OpenAI, OpenRouter, Anthropic and Mistral use tools. Google Gemini and Cohere use their native schema output. The retry case receives one invalid OpenAI tool result, reasks with feedback and accepts the corrected record. |
| HTTP parallel tools | Send two different tool definitions through OpenAI, Anthropic or OpenRouter, and consume both returned calls in order. The first returns one record and the second a batch of two records. Rust uses a typed enum and Python an iterable over a union. |
| HTTP streaming | Consume a complete OpenAI, OpenRouter or Anthropic tool call delivered in three argument fragments over server-sent events. The fixture writes the framed events together without delays. |
| HTTP decisions | Call the blocking Decisions client with a mixed ticket model against a fixed answer for direct Jev, Jev through OpenRouter or native OpenAI. The measured work includes preparation, HTTP and typed answer validation. |

`benchmarks/workloads.py` generates each fixture once and the driver passes it to both workers. Validation checks independent expected values, then compares the workers' results across languages and rounds. For a full stream it checks every interim snapshot or completed item in order. Rust must terminate partial streams with `PartialEvent::Complete`. Batch preparation compares decoded request records, allowing for provider schema dialect annotations and the field titles, formats and descriptions emitted by Pydantic and Schemars. Batch result comparison checks IDs, success and typed values. OpenAI decision evidence is compared as decoded JSON. The server counts total, streaming and invalid responses to verify that the clients perform the expected requests and retries.

## Timing method

Each round starts fresh Python and Rust worker processes sequentially. Their order alternates. The driver builds the Rust worker once in release mode. The workers create clients, prepare fragments, initialize model types and materialize a result for comparison outside the timed loop. HTTP clients reuse connections within a workload. The server runs in the driver process with HTTP/1.1 keep-alive and TCP_NODELAY.

Each worker warms up a case by doubling the batch length from one until a trial reaches the target duration. It times a new batch of that length with `time.perf_counter_ns` or `Instant`. Each iteration consumes and releases its result, with Rust using `black_box`. A stream iteration covers the entire indicated consumption pattern. Parsing a decision answer excludes question preparation. Serializing results for the harness, interpreter startup, imports, Cargo compilation and reading fixtures fall outside the timer. Python uses its default garbage collector.

For each language the table reports the median of the round-level values `elapsed_ns / iterations` in microseconds. Python / Rust divides the Python median by the Rust median. A ratio of 2 means Rust took half the Python time. A ratio of 0.5 means Rust took twice the Python time. The last column gives the smallest and largest Python / Rust ratio among individual paired rounds.

## Recorded results

The recorded machine used an Intel Xeon Platinum 8573C on Linux x86-64 with a four-CPU cgroup quota, CPython 3.12.14, uv 0.13.9 and rustc 1.100.0. The Python checkout was `b01fff4e` (Instructor 1.18.0). The Rust implementation and harness were at `c644289e941e9121343d55b787cddb1ddc689a51`. The run used six pairs, a 10 ms calibration target and the archived dependency locks. See `benchmarks/recorded/report.json` for the environment, source fingerprint and raw samples.

Rust had a lower median duration in all 49 workloads. For 16 records in 16-byte chunks, the full synchronous partial stream took 172 µs in Rust and 1,236 µs in Python. The same inputs streamed as individually completed records took 6.24 µs and 81.65 µs respectively. The Jev ratio varied by operation, from 148.55× for preparing the three questions to 11.85× for decoding their answers and 8.90× for a complete loopback request. The lowest median ratio was 2.97× for parsing an Anthropic batch.

| Workload | Python µs/op | Rust µs/op | Python / Rust | Round ratio range |
| --- | ---: | ---: | ---: | ---: |
| schema/batch | 238.632 | 2.503 | 95.36× | 93.07–100.03 |
| parse/1 | 1.602 | 0.156 | 10.25× | 10.14–10.43 |
| parse/16 | 8.886 | 1.461 | 6.08× | 5.97–6.13 |
| parse/128 | 61.520 | 9.104 | 6.76× | 6.50–6.79 |
| parse/fenced | 20.615 | 1.284 | 16.06× | 15.42–17.09 |
| parse/invalid | 2.369 | 0.449 | 5.28× | 5.14–5.42 |
| context/validate | 9.267 | 1.283 | 7.22× | 7.05–7.41 |
| context/validate_error | 16.104 | 1.346 | 11.97× | 11.82–12.18 |
| partial/1x8 | 81.432 | 3.579 | 22.76× | 22.12–23.48 |
| partial/16x16 | 1236.407 | 172.354 | 7.17× | 6.86–7.33 |
| partial/16x128 | 174.222 | 19.560 | 8.91× | 8.74–9.08 |
| partial/128x128 | 6291.633 | 1426.410 | 4.41× | 4.37–4.98 |
| iterable/1x8 | 8.539 | 0.743 | 11.50× | 11.14–12.20 |
| iterable/16x16 | 81.648 | 6.235 | 13.10× | 12.41–14.35 |
| iterable/16x128 | 68.520 | 5.943 | 11.53× | 11.22–42.65 |
| iterable/128x128 | 539.120 | 46.615 | 11.57× | 11.16–12.01 |
| partial_first/16x20 | 16.229 | 0.843 | 19.25× | 18.88–20.24 |
| iterable_first/16x20 | 7.126 | 0.544 | 13.10× | 12.96–14.03 |
| partial_async/16x16 | 1293.091 | 175.435 | 7.37× | 7.07–7.47 |
| iterable_async/16x16 | 107.460 | 6.655 | 16.15× | 15.60–16.91 |
| decision/jev-latest/questions | 812.359 | 5.469 | 148.55× | 146.61–164.38 |
| decision/jev-latest/answers | 4.361 | 0.368 | 11.85× | 11.01–12.21 |
| decision/jev-latest/reject | 1.515 | 0.451 | 3.36× | 3.21–3.56 |
| decision/gpt-6-luna/questions | 837.476 | 6.595 | 126.98× | 126.21–129.72 |
| decision/gpt-6-luna/answers | 5.564 | 0.391 | 14.22× | 14.08–14.86 |
| decision/gpt-6-luna/reject | 2.599 | 0.475 | 5.47× | 5.04–6.47 |
| batch-prepare/openai | 2539.673 | 192.468 | 13.20× | 12.85–13.88 |
| batch-parse/openai | 121.909 | 29.970 | 4.07× | 4.04–11.19 |
| batch-prepare/anthropic | 2191.004 | 221.913 | 9.87× | 9.51–10.11 |
| batch-parse/anthropic | 108.049 | 36.351 | 2.97× | 2.82–3.04 |
| batch-prepare/mistral | 2247.730 | 184.564 | 12.18× | 11.07–12.86 |
| batch-parse/mistral | 121.350 | 30.942 | 3.92× | 1.24–4.11 |
| cache/hit | 536.672 | 9.773 | 54.92× | 18.45–128.31 |
| http/openai | 1877.732 | 238.374 | 7.88× | 4.00–12.33 |
| http/anthropic | 1918.570 | 183.766 | 10.44× | 8.58–11.25 |
| http/openrouter | 1872.116 | 179.147 | 10.45× | 9.46–14.76 |
| http/google | 4369.309 | 191.717 | 22.79× | 20.33–29.32 |
| http/cohere | 1676.054 | 190.065 | 8.82× | 7.33–12.49 |
| http/mistral | 9236.158 | 218.543 | 42.26× | 38.02–54.27 |
| http/retry | 3917.325 | 378.779 | 10.34× | 8.69–13.47 |
| http-parallel/openai | 3532.058 | 229.037 | 15.42× | 13.27–18.35 |
| http-stream/openai | 4904.877 | 244.720 | 20.04× | 14.93–21.72 |
| http-parallel/anthropic | 2820.472 | 228.009 | 12.37× | 10.72–20.29 |
| http-stream/anthropic | 4280.113 | 242.104 | 17.68× | 12.99–22.34 |
| http-parallel/openrouter | 3679.020 | 201.062 | 18.30× | 13.21–26.41 |
| http-stream/openrouter | 5264.541 | 287.591 | 18.31× | 17.77–25.17 |
| http-decision/typesafe/jev-latest | 1784.746 | 200.442 | 8.90× | 8.67–10.31 |
| http-decision/openrouter/typesafe/jev-1.13 | 1836.201 | 219.836 | 8.35× | 7.22–19.99 |
| http-decision/openai/gpt-6-luna | 1934.934 | 213.493 | 9.06× | 2.58–11.78 |

## Interpretation and limitations

The fixtures have fixed field types, short strings and predictable chunk boundaries. Partial streams retain all snapshots until the end of each iteration, so their cost grows with the number and size of intermediate models. First-yield measurements start with fragments already in memory. The synchronous and asynchronous cases isolate local work under a single consumer. Applications with different schemas, backpressure or concurrent requests can have different costs.

HTTP timings include the provider SDK on the Python side, the Rust HTTP client, the fixture server and OS scheduling. Both workers reuse their native client stacks. Provider requests can differ in optional metadata and schema representation. The server simulates immediate responses and the measurements exclude remote network latency, model inference and token generation delays. These ratios describe the measured client workflows on this machine. Python also uses native Rust components through Pydantic Core and jiter.

The report measures warmed wall-clock time. Other project checks were paused during measurement, and the cgroup reported zero CPU throttling. CPU frequency, system load, interpreter version, allocator behavior and compiler optimizations affect the results. Some cases varied widely across the six pairs. The cache-hit ratio ranged from 18.45× to 128.31×. A 10 ms calibration target gives only a few iterations for the slowest operations. The range describes these six observations. Memory use, tail latency and sustained concurrent throughput need separate measurements.
