# Design a response type

Apply `#[model]` to your output type and each nested type you own. It derives Serde serialization and deserialization, JSON Schema through Schemars, Instructor validation, and a partial representation. Ordinary Rust field access, methods, and pattern matching work on the returned value. Add `#[derive(Debug)]` if you want to print the entire value.

## Port a Python type annotation

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

`meetingTitle` appears on the wire and `meeting.title` in Rust. The `kind` tag determines which fields deserialize into `Venue`. Check the target model's schema capabilities before sending a union or open-ended map in a constrained mode.

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
