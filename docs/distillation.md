# Record a teacher and dispatch a student

`Instructions<I, O>` connects an input type, a teacher function and its response model. Supply a function name and definition once. The definition can include a signature, a description and any source you want the student to see. Export calls to JSONL, then ask a configured model to predict the same output type using the same prompt.

## Generate records locally

This example writes three multiplication records to standard output. The teacher computes each product in Rust. Its `Product` response schema becomes the model's function schema.

```sh
cargo run -p instructor-ai --example distillation > products.jsonl
```

```rust
--8<-- "instructor/examples/distillation.rs"
```

`distil` invokes the teacher once, checks the output's synchronous validators, writes one record, then returns the typed output. The sink can be any `std::io::Write`, including a buffered file or an in-memory buffer. If the teacher or validation fails, the sink receives no bytes for that call. A write failure returns `Error::Io` and can leave an incomplete line. Flush buffered writers before reading the dataset.

For an output you already have, use `task.track(&input, &output, &mut sink)?`. To inspect or transform an individual record before writing, use `task.record(&input, &output)?` to get a `serde_json::Value`.

## Choose a record format

| Format | Contents |
| --- | --- |
| `FinetuneFormat::Messages` (default) | System definition, user call, assistant `function_call` with serialized arguments and the matching `functions` declaration. These are the legacy function fields used by Python Instructor's export. |
| `FinetuneFormat::Raw` | `fn_name`, `fn_repr`, positional `args`, named `kwargs`, the typed result as `resp` and its `schema`. |

Set `.format(FinetuneFormat::Raw)` on `Instructions` to keep a provider-independent record. A tuple or list becomes positional arguments, a struct or map becomes named arguments, unit means a call with zero arguments, and a scalar becomes one positional argument. The prompt renders each argument as JSON. Use `.with_arguments(...)` to map a typed input to an `Arguments` value when the call mixes positional and named arguments or accepts a collection as one argument.

## Predict with a model

Configure a client with the student model identifier. Within an async function using the `task` and `Product` definitions above, dispatch the same input as follows.

```rust
let client = instructor_ai::Client::from_provider(&std::env::var("STUDENT_MODEL")?)?;
let prediction: Product = task.dispatch(&client, &(123, 456)).await?;
```

Use `task.dispatch_blocking(&client, &input)?` with `BlockingClient`. `task.request(&input)?` returns the prompt if you need to customize request options before calling `create::<Product>`. Dispatch uses the client's output mode, validators and retry policy.

Python's decorator reads the teacher's signature or source and uses logging handlers as sinks. Rust provides the description and the sink at the call site. Both workflows export examples and support inference with a selected student. Training is a separate operation governed by the chosen service's dataset format. [Python distillation guide](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/distillation.md) · [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/coverage/test_legacy_distil_coverage.py)
