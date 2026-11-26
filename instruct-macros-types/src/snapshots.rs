//! Runtime operations used by model derives on parsed JSON snapshots.

use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

use crate::InstructMacro;

/// Validate a closed model using the same route as final output.
pub fn normalize<T>(value: &mut Value, context: &Value) -> Result<(), String>
where
    T: InstructMacro + DeserializeOwned + Serialize,
{
    let mut model = T::deserialize(&*value).map_err(|e| e.to_string())?;
    model.validate_with_context(context)?;
    *value = serde_json::to_value(model).map_err(|e| e.to_string())?;
    Ok(())
}

/// Descend into a child container, shortening the path only if that child
/// contains the opening delimiter still being filled by the stream.
pub fn child<T: InstructMacro>(
    key: &str,
    value: &mut Value,
    path: Option<&[String]>,
    context: &Value,
) -> Result<(), String> {
    if value.is_object() || value.is_array() {
        let tail = child_path(key, path);
        T::prepare_partial(value, tail, context).map_err(|e| format!("{key}: {e}"))?;
    }
    Ok(())
}

pub fn child_path<'a>(key: &str, path: Option<&'a [String]>) -> Option<&'a [String]> {
    let (head, tail) = path?.split_first()?;
    (head == key).then_some(tail)
}

/// A string prefix may name a different variant from the finished tag.
pub fn discriminator<'a>(value: &'a Value, key: &str, path: Option<&[String]>) -> Option<&'a str> {
    if child_path(key, path).is_some() {
        return None;
    }
    value.get(key)?.as_str()
}

/// Temporarily set aside named fields, so a flattened model or map only
/// visits the fields Serde will pass to it. Restore siblings on error too.
pub fn flattened<T: InstructMacro>(
    value: &mut Value,
    claimed: &[&str],
    path: Option<&[String]>,
    context: &Value,
) -> Result<(), String> {
    let Some(map) = value.as_object_mut() else {
        return Ok(());
    };
    let siblings = claimed
        .iter()
        .filter_map(|k| map.remove_entry(*k))
        .collect::<Vec<_>>();
    // The flattened model has no separate closing delimiter.
    let result = T::prepare_partial(value, Some(path.unwrap_or_default()), context);
    if let Some(map) = value.as_object_mut() {
        map.extend(siblings);
    }
    result
}
