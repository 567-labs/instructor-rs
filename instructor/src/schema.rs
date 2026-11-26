use crate::{Error, InstructMacro};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::{
    any::TypeId,
    collections::HashMap,
    sync::{Arc, LazyLock, RwLock},
};

/// Types accepted by the structured creation methods.
pub trait ResponseModel: InstructMacro + DeserializeOwned + 'static {}
impl<T: InstructMacro + DeserializeOwned + 'static> ResponseModel for T {}

pub(crate) struct Schema {
    pub value: Value,
    pub wrapper: Option<&'static str>,
}

// JSON Schema definitions referenced by "#/$defs/..." must stay at the root
// when a scalar, union or collection is wrapped for a provider.
fn wrap(key: &str, mut inner: Value) -> Value {
    let defs = inner.as_object_mut().and_then(|map| map.remove("$defs"));
    let mut result = json!({"type": "object", "properties": {(key): inner}, "required": [key]});
    if let Some(defs) = defs {
        result["$defs"] = defs;
    }
    result
}

fn build<T: InstructMacro>() -> Schema {
    let mut value = T::json_schema();
    let wrapper = match value["type"].as_str() {
        Some("object") if value.get("properties").is_some() => None,
        Some("array") => Some("tasks"),
        _ => Some("content"),
    };
    if let Some(key) = wrapper {
        value = wrap(key, value);
    }
    let title = value["title"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(T::model_name);
    let name: String = title
        .chars()
        .take(64)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    value["title"] = Value::String(if name.is_empty() {
        "Response".into()
    } else {
        name
    });
    Schema { value, wrapper }
}

pub(crate) fn schema<T: InstructMacro + 'static>() -> Arc<Schema> {
    static CACHE: LazyLock<RwLock<HashMap<TypeId, Arc<Schema>>>> = LazyLock::new(Default::default);
    let id = TypeId::of::<T>();
    if let Some(entry) = CACHE.read().unwrap().get(&id) {
        return entry.clone();
    }
    let entry = Arc::new(build::<T>());
    CACHE.write().unwrap().entry(id).or_insert(entry).clone()
}

/// Provider-neutral object schema used by the client. The returned copy is editable.
pub fn response_schema<T: InstructMacro + 'static>() -> Value {
    schema::<T>().value.clone()
}

/// Decode provider JSON, unwrap native values and run contextual validation.
pub fn parse_response<T: ResponseModel>(input: &str, context: &Value) -> Result<T, Error> {
    schema::<T>().parse(input, context)
}

impl Schema {
    pub fn parse<T: ResponseModel>(&self, input: &str, context: &Value) -> Result<T, Error> {
        let input = input.trim();
        let input = input
            .strip_prefix("```json")
            .or_else(|| input.strip_prefix("```"))
            .and_then(|rest| rest.strip_suffix("```"))
            .unwrap_or(input);
        let mut typed: T = if let Some(field) = self.wrapper {
            let mut obj: Value = serde_json::from_str(input)?;
            let value = obj
                .get_mut(field)
                .ok_or_else(|| Error::Parse(format!("missing field {field}")))?;
            serde_json::from_value(value.take())?
        } else {
            serde_json::from_str(input)?
        };
        typed
            .validate_with_context(context)
            .map_err(Error::Validation)?;
        Ok(typed)
    }
}
