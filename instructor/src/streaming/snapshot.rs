use crate::{
    schema::{schema, Schema},
    ResponseModel,
};
use std::{marker::PhantomData, sync::Arc};

use jiter::{JsonValue, PartialMode};

use super::{Boundaries, Decoder, Error, PartialEvent, PartialModel, Value, MAX_BUFFER};

pub(super) struct Snapshots<T> {
    json: String,
    boundaries: Boundaries,
    closed: bool,
    string_value: bool,
    context: Value,
    schema: Arc<Schema>,
    model: PhantomData<fn() -> T>,
}

impl<T: ResponseModel> Snapshots<T> {
    pub fn new(context: Value) -> Self {
        Self {
            json: String::new(),
            boundaries: Boundaries::default(),
            closed: false,
            string_value: false,
            context,
            schema: schema::<T>(),
            model: PhantomData,
        }
    }

    fn append(&mut self, text: &str) -> Result<(), Error> {
        if text.len() + self.json.len() > MAX_BUFFER {
            return Err(Error::Protocol("partial model exceeds 16 MiB".into()));
        }
        for ch in text.chars() {
            // Match Instructor's partial JSON sanitizer, before escape tracking.
            // Escaped controls survive, as do DEL and C1 within strings.
            if ch < ' ' || (!self.boundaries.quoted && ch.is_control()) {
                continue;
            }
            if self.json.is_empty() {
                if ch == ' ' {
                    continue;
                }
                if ch != '{' {
                    return Err(Error::Protocol(
                        "expected an object in the partial stream".into(),
                    ));
                }
            }
            if self.closed {
                if ch != ' ' {
                    return Err(Error::Protocol("trailing data after partial object".into()));
                }
                continue;
            }
            if ch == '"' && !self.boundaries.quoted {
                self.string_value = self.json.trim_end().ends_with(':');
            }
            self.json.push(ch);
            self.closed = self.boundaries.read(ch)?;
        }
        Ok(())
    }
}

impl<T: ResponseModel + PartialModel> Decoder for Snapshots<T> {
    type Item = PartialEvent<T>;

    fn read(&mut self, fragment: &mut &str) -> Result<Option<Self::Item>, Error> {
        if fragment.is_empty() {
            return Ok(None);
        }
        self.append(std::mem::take(fragment))?;
        if self.closed || self.json.is_empty() {
            return Ok(None);
        }
        // Jiter is also the prefix parser used by Python Instructor. When an
        // unfinished string cannot yet inhabit its Rust type (e.g. an enum),
        // retry without that string to keep the earlier fields available.
        for mode in [PartialMode::TrailingStrings, PartialMode::On] {
            let parsed = JsonValue::parse_with_config(self.json.trim().as_bytes(), false, mode)
                .map_err(|e| Error::Protocol(format!("invalid partial JSON: {e}")))?;
            if let Some(mut value) = serde_value(&parsed) {
                // In trailing-strings mode Jiter includes an unfinished value
                // (but not a field name) as the object's last entry. Mark it
                // open so an enum tag cannot select a validator prematurely.
                let unfinished_value = self.boundaries.quoted
                    && self.string_value
                    && matches!(mode, PartialMode::TrailingStrings);
                let path = open_path(
                    &parsed,
                    self.boundaries.depth + usize::from(unfinished_value),
                );
                let mut path = path.as_slice();
                if let Some(key) = self.schema.wrapper {
                    let Some(content) = value.get_mut(key) else {
                        continue;
                    };
                    value = content.take();
                    path = path
                        .split_first()
                        .filter(|(head, _)| head.as_str() == key)
                        .map(|(_, rest)| rest)
                        .unwrap_or_default();
                }
                T::prepare_partial(&mut value, Some(path), &self.context)
                    .map_err(Error::Validation)?;
                if let Ok(snapshot) = serde_json::from_value(value) {
                    return Ok(Some(PartialEvent::Partial(snapshot)));
                }
            }
        }
        Ok(None)
    }

    fn finish(self) -> Result<Option<Self::Item>, Error> {
        if !self.closed {
            return Err(Error::Incomplete(
                "stream ended before the object was complete".into(),
            ));
        }
        let model = self.schema.parse(&self.json, &self.context)?;
        Ok(Some(PartialEvent::Complete(model)))
    }
}

// Only the final child of a container can still be open. Jiter preserves
// source key order, so the delimiter depth identifies the open branch in
// the parsed prefix, even when an unfinished field name was omitted.
fn open_path(mut value: &JsonValue<'_>, depth: usize) -> Vec<String> {
    let mut path = Vec::new();
    for _ in 1..depth {
        let child = match value {
            JsonValue::Object(object) => object.last().map(|(key, v)| (key.to_string(), v)),
            JsonValue::Array(items) => items.last().map(|v| ((items.len() - 1).to_string(), v)),
            _ => None,
        };
        let Some((key, value_at_key)) = child else {
            break;
        };
        path.push(key);
        value = value_at_key;
    }
    path
}

// Jiter's partial parser returns its own borrowed JSON tree. Only provisional
// snapshots take this path. Final deserialization always uses the original
// text, preserving explicit nulls, numeric precision and duplicate-key checks.
fn serde_value(value: &JsonValue<'_>) -> Option<Value> {
    Some(match value {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(*b),
        JsonValue::Int(n) => (*n).into(),
        JsonValue::BigInt(n) => Value::Number(n.to_string().parse().ok()?),
        JsonValue::Float(f) => Value::Number(serde_json::Number::from_f64(*f)?),
        JsonValue::Str(s) => Value::String(s.to_string()),
        JsonValue::Array(a) => Value::Array(a.iter().map(serde_value).collect::<Option<_>>()?),
        JsonValue::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| Some((k.to_string(), serde_value(v)?)))
                .collect::<Option<_>>()?,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_path_follows_input_order_and_ignores_braces_in_strings() {
        let cases: &[(&str, &[&str])] = &[
            (r#"{"z":{},"a":{"nested":[],"unfinished"#, &["a"]),
            (r#"{"items":[{"name":"hi"},"#, &["items"]),
            (
                r#"{"items":[{},{"label":{"text":"{[\""#,
                &["items", "1", "label"],
            ),
            (r#"{"items":[{},{"label":{"text":"{[\""}}],"note":"h"#, &[]),
            (r#"{"a.b":[{},[],{"\u0041":{"#, &["a.b", "2", "A"]),
        ];
        for (raw, want) in cases {
            let mut boundaries = Boundaries::default();
            for ch in raw.chars() {
                boundaries.read(ch).unwrap();
            }
            let parsed =
                JsonValue::parse_with_config(raw.as_bytes(), false, PartialMode::TrailingStrings)
                    .unwrap();
            let path = open_path(&parsed, boundaries.depth);
            assert_eq!(&path, want);
        }
    }
}
