# Validate a response and ask for a correction

The requested type gives the provider a schema and gives the local parser a Rust target. After deserialization, Instructor runs nested and field validators, then the whole-model validators. A failed parse or validation can trigger another request that includes feedback. This is useful when a schema checks the shape of a value and your application also needs to check its meaning.

## Check a field

Use `#[validate(custom = "function_name")]` on a field or a model. The function accepts a reference to the value and returns an error with a message the provider can use to correct it. Run the same check locally with `parse_response` and an empty context.

```rust
use instructor_ai::{model, parse_response, Error};
use serde_json::Value;

#[model]
struct Reservation {
    #[validate(custom = "check_count")]
    guests: u16,
}

fn check_count(guests: &u16) -> Result<(), String> {
    if *guests == 0 {
        return Err("Supply a positive number of guests.".into());
    }
    Ok(())
}

fn main() {
    let result = parse_response::<Reservation>(r#"{"guests": 0}"#, &Value::Null);
    assert!(matches!(result, Err(Error::Validation(_))));
}
```
