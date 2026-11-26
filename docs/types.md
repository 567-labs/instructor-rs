# Design a response type

Apply `#[model]` to your output type and each nested type you own. It derives Serde serialization and deserialization, JSON Schema through Schemars, Instructor validation, and a partial representation. Ordinary Rust field access, methods, and pattern matching work on the returned value. Add `#[derive(Debug)]` if you want to print the entire value.

## Port a Python type annotation

The Python call `client.create(response_model=Meeting, messages=...)` corresponds to `let meeting: Meeting = client.create(request)?;` or `client.create::<Meeting>(request)?` on a blocking client. On an async client, append `.await?` to the call. Rust also infers the output type from a function's return type.

| Python output | Rust output | Behavior |
| --- | --- | --- |
| `str`, `bool`, `int`, `float` | `String`, `bool`, an integer type, `f64` | The chosen integer type sets the accepted range. |
| `list[Person]` | `Vec<Person>` | One result with the full list. |
| `Person \| None` | `Option<Person>` | A nullable result or field. |
| `dict[str, T]` | `BTreeMap<String, T>` or `HashMap<String, T>` | String keys with typed values. |
| `BaseModel` or `TypedDict` | `#[model] struct` | Named fields with compile-time access. |
| `Literal` or string enum | Unit variants of a `#[model] enum` | Each variant represents an allowed value. |
| Discriminated union | Data-carrying enum with `#[serde(tag = "kind")]` | Match on the variant to access its fields. |
| `Field(description=...)` | `///` on the field or `#[description("…")]` | Description appears in the schema. |
| `Field(alias=...)` | `#[serde(rename = "wireName")]` | Rust uses the field name and the provider uses the wire name. |
| `Partial[Meeting]` | `Partial<Meeting>` | Alias for the generated `MeetingPartial` snapshot type. |

Use `Option<T>` on a struct field when the fact may be missing. Serde reads an absent field or JSON `null` as `None`. By default a required field with no value fails deserialization. `#[serde(default)]` opts a field into a default when the key is missing. Values must fit the Rust field type. For example, a string containing digits fails deserialization into `u16`, as does an integer outside the range 0 through 65535.

## Inspect the schema and parse a sample locally

This example models an in-person or remote venue, a list of attendees, a renamed field, and an optional duration. `response_schema` exposes the provider-neutral request schema. `parse_response` uses the same typed parser and contextual validation as extraction. The example uses `serde_json = "1"` to construct an empty validation context.

```sh
cargo run -p instructor-ai --example types
```

```rust
--8<-- "instructor/examples/types.rs"
```

`meetingTitle` appears on the wire and `meeting.title` in Rust. The `kind` tag determines which fields deserialize into `Venue`. Check the target model's schema capabilities before sending a union or open-ended map in a constrained mode. [Provider modes](providers.md)

## Parse a saved completion

Save the raw JSON value returned by `create_with_completion` with its provider and mode. Pass all three to `parse_completion` to run extraction and validation again locally. Supply a new validation context or use `&Value::Null` for an empty one. For a file, load the envelope with `serde_json::from_reader(std::fs::File::open(path)?)?`. This example checks quotes from a saved Anthropic tool call against a source passage. [Python `ResponseSchema.from_response`](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/instructor/v2/core/function_calls.py#L168)

```rust
use instructor_ai::{parse_completion, Citations, Error, Mode, Provider};
use serde_json::json;

fn main() -> Result<(), Error> {
    let raw = json!({
        "stop_reason": "tool_use",
        "content": [{
            "type": "tool_use", "id": "toolu_1", "name": "Citations",
            "input": {"substring_quotes": ["opened in 1901", "A grant funded it."]}
        }]
    });
    let source = json!({"context": "The library opened in 1901."});
    let citations: Citations = parse_completion(Provider::Anthropic, Mode::Tools, &raw, &source)?;
    assert_eq!(citations.substring_quotes, ["opened in 1901"]);
    Ok(())
}
```

## Return a scalar or collection directly

Providers often require an object at the root of a tool schema. Instructor wraps a scalar in a `content` field and an array in a `tasks` field, then unwraps the result for the caller. Inspect the native schema using `T::json_schema()` with the `InstructMacro` trait in scope.

```rust
use instructor_ai::{parse_response, response_schema, Error, InstructMacro};
use serde_json::Value;

fn main() -> Result<(), Error> {
    let count: u16 = parse_response(r#"{"content": 42}"#, &Value::Null)?;
    let names: Vec<String> = parse_response(r#"{"tasks": ["Ali", "Bo"]}"#, &Value::Null)?;
    assert_eq!(count, 42);
    assert_eq!(names, ["Ali", "Bo"]);
    assert_eq!(u16::json_schema()["type"], "integer");
    assert_eq!(response_schema::<u16>()["properties"]["content"]["type"], "integer");
    Ok(())
}
```

Use `ResponseModel` as the bound in helpers that extract arbitrary supported types. It includes the Instructor schema and validation trait and owned Serde deserialization. The caller retains the concrete return type.

```rust
use instructor_ai::{BlockingClient, Error, ResponseModel};

fn extract<T: ResponseModel>(client: &BlockingClient, text: &str) -> Result<T, Error> {
    client.create(text)
}

fn main() -> Result<(), Error> {
    let client = BlockingClient::from_provider("anthropic/claude-haiku-5-5")?;
    let names: Vec<String> = extract(&client, "List the names. Ali and Bo attended.")?;
    println!("{names:?}");
    Ok(())
}
```

For a sequence processed item by item, pass the element type to `create_iterable::<T>`. For a preview of a single object, pass the complete type to `create_partial::<T>`. [Streaming and partial types](streaming.md) · [Contextual validators](concepts.md)

## Test increasingly complex response types

Start with a scalar to verify that the client unwraps the generated value. Add fields, enum labels, and nested records to exercise the parts of the schema your application needs. Each row names a Python test under `tests/llm/test_core_providers/` and the corresponding Rust test. Rust runs the same assertion in `instructor/tests/support/generation.rs` against loopback fixtures and live endpoints. [Python extraction cases](https://github.com/567-labs/instructor/blob/0ab894f0661a6968b17e67cf5b7657f1fcb3b9f7/tests/llm/test_core_providers/test_basic_extraction.py) · [Python enum cases](https://github.com/567-labs/instructor/blob/0ab894f0661a6968b17e67cf5b7657f1fcb3b9f7/tests/llm/test_core_providers/test_model_complexity.py)

| Response shape | Python → Rust | Test name in both suites | Expected output |
| --- | --- | --- | --- |
| Scalar | `int` → `i32` | `test_int` | The integer `42` |
| Flat record | `User(BaseModel)` → `struct User` | `test_simple_extraction` | Jason, age 25 |
| Finite choice | `ShipmentState(str, Enum)` → `enum ShipmentState` | `test_enum_extraction` | Wire label `in_transit`, decoded as `ShipmentState::InTransit` |
| Collection | `list[User]` → `Vec<User>` | `test_list_extraction` | Alice, Bob, and Charlie with their ages |
| Nested record | `UserWithAddress` → `struct UserWithAddress` | `test_nested_model_extraction` | John Doe and the street, city, and country in his address |
| Collection of records with enum and nullable fields | `Manifest.packages: list[Package]` → `Vec<Package>` in a `Manifest` | `test_nested_enum_collection` | Two packages in order, each with a typed state. The first has an unknown carrier. |

The manifest test uses `#[serde(rename_all = "snake_case")]` to keep the JSON labels identical to the Python enum values. Each package holds its own state and optional carrier. This example uses the model declarations from the test suite and parses a response locally.

```rust
use instructor_ai::{model, parse_response, Error};
use serde_json::Value;

--8<-- "instructor/tests/support/generation.rs:shipment-models"

fn main() -> Result<(), Error> {
    let text = r#"{
        "depot": "North",
        "packages": [
            {"tracking_id": "A1", "state": "packed", "carrier": null},
            {"tracking_id": "B2", "state": "in_transit", "carrier": "Swift"}
        ]
    }"#;
    let manifest: Manifest = parse_response(text, &Value::Null)?;
    assert_eq!(manifest.depot, "North");
    assert!(manifest.packages[0].carrier.is_none());
    let second = &manifest.packages[1];
    assert_eq!(second.state, ShipmentState::InTransit);
    assert_eq!(second.carrier.as_deref(), Some("Swift"));
    Ok(())
}
```

Run the offline checks from the workspace root. The first command runs all the cases against mock HTTP servers for six providers, including OpenAI's two generation APIs. The second isolates recovery from a bad enum label. Each mock sends `lost` for the second package, checks that the correction mentions the rejected label, and then sends `in_transit`. [Select a live counterpart](parity.md#opt-in-live-checks)

```sh
cargo test -p instructor-ai --test providers complexity::
cargo test -p instructor-ai --test providers complexity::invalid_enum_in_nested_record_causes_a_reask -- --exact
```

## Analyze a schema before selecting a mode

`analyze_schema::<T>()` reports structural findings, complexity, and an optional mode recommendation. To get a provider-specific choice, use `select_mode::<T>(provider)`, which prefers strict output, or reuse the report with `report.select_mode(provider, prefer_strict)`. Apply the returned mode to the provider configuration to use it. This local analysis uses heuristics, including a token estimate of 15 per declared field. Check the chosen model's capabilities before relying on its recommendation.

```rust
use instructor_ai::{analyze_schema, model, select_mode, Provider};

#[model]
struct Label {
    name: String,
}

fn main() {
    let report = analyze_schema::<Label>();
    let mode = select_mode::<Label>(Provider::OpenAI);
    println!("Complexity {}, mode {mode:?}", report.complexity_score());
    assert_eq!(mode, report.select_mode(Provider::OpenAI, true));
}
```

For an existing schema value, call `SchemaAnalysis::from_json_schema(&schema)`. Use `has_warnings()` and `has_errors()` to inspect findings. [Provider modes](providers.md)
