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

pub(crate) fn tool_description(schema: &Value) -> &str {
    schema["description"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Return the extracted structured data.")
}

pub(crate) fn tool_parameters(schema: &Value) -> Value {
    let mut params = schema.clone();
    if let Some(root) = params.as_object_mut() {
        root.remove("title");
        root.remove("description");
    }
    params
}

// Wrapping relocates the model from the schema root to a property. Rebase
// its local JSON Pointers, except for definitions that stay at the root.
fn wrap(key: &str, mut inner: Value) -> Value {
    use instruct_macros_types::schemars::{transform::transform_subschemas, Schema};

    fn rebase(schema: &mut Schema, key: &str) {
        let Some(object) = schema.as_object_mut() else {
            return;
        };
        if object.contains_key("$id") {
            return; // Local references resolve within this separate resource.
        }
        if let Some(Value::String(path)) = object.get_mut("$ref") {
            if (path == "#" || path.starts_with("#/")) && !path.starts_with("#/$defs/") {
                *path = format!("#/properties/{key}{}", &path[1..]);
            }
        }
        transform_subschemas(&mut |s: &mut Schema| rebase(s, key), schema);
    }

    let mut result = json!({"type": "object", "properties": {}, "required": [key]});
    if let Ok(schema) = <&mut Schema>::try_from(&mut inner) {
        rebase(schema, key);
        if schema.get("$id").is_none() {
            if let Some(defs) = schema.remove("$defs") {
                result["$defs"] = defs;
            }
        }
    }
    result["properties"][key] = inner;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[crate::model]
    enum Expr {
        Int(i64),
        Neg(Box<Self>),
    }

    #[test]
    fn wrapped_recursive_enum_refers_to_typed_payload() {
        let schema = response_schema::<Expr>();
        let expr = &schema["properties"]["content"];
        assert_eq!(
            expr["oneOf"][1]["properties"]["Neg"]["$ref"],
            "#/properties/content"
        );
        parse_response::<Expr>(r#"{"content":{"Neg":{"Int":1}}}"#, &Value::Null).unwrap();
    }
}
