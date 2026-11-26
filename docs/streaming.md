# Stream typed output

Use `create_partial::<Meeting>` to redraw a preview as fields arrive. Use `create_iterable::<Assignment>` to process assignments one at a time. The first call describes one object. The second describes **one item** of a collection. Both methods select streaming when building the provider request.

## Watch a partial model take shape

Apply `#[model]` to the complete model. It generates the corresponding `MeetingPartial` type. Import `instructor_ai::Partial` to write `Partial<Meeting>`, the equivalent of Python's `Partial[Meeting]`. Both Rust names refer to `<Meeting as PartialModel>::Partial`. The compiler knows which fields can be missing when you match on the event.

| Field on `Meeting` | Field on `MeetingPartial` |
| --- | --- |
| `title: String` | `title: Option<String>` |
| `venue: Venue` | `venue: Option<VenuePartial>` |
| `organizer: Option<Person>` | `organizer: Option<PersonPartial>` |
| `attendees: Vec<Person>` | `attendees: Option<Vec<PersonPartial>>` |

For a partial venue, `city: Option<String>` can hold an unfinished string. A field with a Serde default can take that default before the value appears. Use `PartialEvent::Partial(snapshot)` for display. On successful completion, `PartialEvent::Complete(meeting)` holds an ordinary `Meeting` whose required fields are present and whose validators have passed.

Run the async example with `ANTHROPIC_API_KEY` set. For an external crate, add `tokio = { version = "1", features = ["macros", "rt"] }` and `futures-util = "0.3"` to your Cargo dependencies.

```sh
cargo run -p instructor-ai --example stream_partial -- anthropic/claude-haiku-5-5
```

```rust
--8<-- "instructor/examples/stream_partial.rs"
```

Python's `client.create_partial(response_model=Meeting, ...)` exposes successive instances with fields filled in. Rust makes the transition to a validated result explicit. A successful stream yields zero or more previews followed by one `Complete`. Chunk boundaries determine preview frequency. A prefix can repeat a value or lack a representable enum variant. Partially received values may change as more tokens arrive.

For a scalar, array, or union at the root, pass that type to `create_partial` directly. The client supplies the object envelope required by the provider and unwraps it in the stream. For example, a `Vec<Meeting>` produces snapshots of the accumulating list and one validated list on completion. Use `create_iterable::<Meeting>` to consume each meeting independently.

## When nested validators run

The decoder tracks the closing delimiter of each model, including models inside lists, maps and options. A preview can contain a normalized child while its parent still lacks required fields. If validation of a closed child fails, the stream yields an error and stops. Each preview starts from the accumulated source text. A validator can run again on later previews and on the final result, so keep it deterministic. The root validates at finalization.

For a tagged enum, Rust uses the completed Serde discriminator to select which child models to validate. This works with internal tags (`{"kind":"counted",...}`), adjacent tags (`{"kind":"counted","payload":...}`) and external tags (`{"counted":...}`). Tags can follow the payload, and Serde aliases can identify the variant. If a prefix such as `"counted` could become `"counted_extra"`, the closing quote determines which validator applies. The enum's own validator waits for its enclosing braces to close.

For example, a batch contains a list of entries. Each `counted` entry has a child model whose validator multiplies `n` by a context factor of 3. The three fragments below show when that transformation becomes visible.

| Next fragment | Observed event |
| --- | --- |
| `{"items":[{"kind":"counted","child":{"n":2` | `Partial` with `n = Some(2)`. The child is open. |
| `}` | `Partial` with `n = Some(6)`. The child has closed inside the open entry. |
| `}]}` | `Complete` with `n = 6`. Final validation starts from the input value 2. |

A field marked `#[serde(flatten)]` shares the enclosing object's braces. With one flattened struct or map, Rust follows its nested fields and excludes the parent's named fields and aliases. The flattened model's own checks run with its parent. Multiple flattened fields, untagged alternatives, and fields using a custom deserializer can require the complete enclosing model to resolve their input.

Python's tracker infers that an item is complete when it sees a subsequent sibling or completes the root. In a replay with `list[Counted | Raw]` at `b01fff4`, the entry remained a dictionary with `n = 2` during previews and became a `Counted` with `n = 6` at root validation. Rust's explicit tag and delimiter tracking make the normalized child available in the second fragment above. [Python tracker](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/dsl/json_tracker.py) · [Python partial construction](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/dsl/partial.py)

Replay every split point of the tagged and flattened payloads, including an escaped tag and a tag arriving after the child, with the local tests.

```sh
cargo test -p instructor-ai --test partial_completeness
```

## Replay fragments locally

Use the same decoder with saved JSON fragments to test display logic. The strings are deltas in order, with the surrounding provider event envelopes already removed.

```sh
cargo run -p instructor-ai --example replay
```

```rust
--8<-- "instructor/examples/replay.rs"
```

For these three fragments, the first preview has `name = Some("Ad")` and an absent age. The second has `name = Some("Ada")`. The completed model contains `"Ada"` and `37`. The local variants are `streaming::partial` and `streaming::iterable` for iterators, with `partial_async` and `iterable_async` for async streams. Each input is a `Result`, allowing a source error to propagate at its position in the stream.

## Process complete records

Python's `client.create_iterable(response_model=Assignment, ...)` corresponds to `create_iterable::<Assignment>(request)`. Each yielded `Ok(assignment)` has passed validation, including any contextual validator. The parser can emit the first record while the model is still writing the next one. Run the blocking example with `OPENROUTER_API_KEY` set.

```sh
cargo run -p instructor-ai --example stream_iterable -- openrouter/google/gemini-3.8-flash
```

```rust
--8<-- "instructor/examples/stream_iterable.rs"
```

On `BlockingClient`, consume either streaming method with a `for` loop. On `Client`, await the method, then consume items with `StreamExt::next().await`. To commit all records together, collect them into a `Result<Vec<_>, _>` on a blocking iterator, or use `TryStreamExt::try_collect` on an async stream. A later error can follow earlier validated records. For a single non-streaming request returning the entire list, use `create::<Vec<Assignment>>`.

## Handle errors and cancellation

- Check the result when opening the stream and each result during iteration. A provider rejection can surface at either stage. A parse or validation error before the first value is eligible for a correction under the request's retry policy.
- An interrupted object, missing required field, failed validator, or terminal provider error ends the stream with `Err`. Save a partial model only when the `Complete` event arrives. The client returns errors during consumption directly to the caller, so already delivered records are processed once per stream.
- Drop the iterator or stream to release its upstream response early. A loop that takes ownership of the iterator drops it on exit. For a stream stored in a variable, use `drop(updates)` or let it leave scope.
- Native modes can yield fields incrementally. In `MarkdownJson` mode, the single-model decoder waits for the end to select the last complete JSON candidate. For collections it prefers fenced values, emitting each when its enclosing object or array closes, and uses bare candidates at the end when no fence appears.

The decoder accepts JSON split within strings, escapes, and nested values. A collection can be a bare array or a `{"tasks": [...]}` envelope. For tool-mode iterable streams on OpenAI Chat, OpenRouter, and Mistral, the client tracks interleaved argument fragments by call index. For a typed enum of distinct named tools, use `create_parallel`. [Parallel tools](parallel.md) · [Providers](providers.md) · [Response types](types.md) · [Python comparison](parity.md)
