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
        let tail = path
            .and_then(|p| p.split_first())
            .and_then(|(head, tail)| (head == key).then_some(tail));
        T::prepare_partial(value, tail, context).map_err(|e| format!("{key}: {e}"))?;
    }
    Ok(())
}
