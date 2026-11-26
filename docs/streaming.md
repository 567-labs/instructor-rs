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

A closed nested struct can already have validated and normalized fields, including inside a list, map, or option. The root model validates at finalization. Checks for enum variants, flattened fields, and custom deserialization may wait until the enclosing model is complete. A nested validator can run again on a later preview or the final value, so use deterministic checks.

For a scalar, array, or union at the root, pass that type to `create_partial` directly. The client supplies the object envelope required by the provider and unwraps it in the stream. For example, a `Vec<Meeting>` produces snapshots of the accumulating list and one validated list on completion. Use `create_iterable::<Meeting>` to consume each meeting independently.

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

The decoder accepts JSON split within strings, escapes, and nested values. A collection can be a bare array or a `{"tasks": [...]}` envelope. For tool-mode iterable streams on OpenAI Chat, OpenRouter, and Mistral, the client tracks interleaved argument fragments by call index. [Providers](providers.md) · [Response types](types.md)
