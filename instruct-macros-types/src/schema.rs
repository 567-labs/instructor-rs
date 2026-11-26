use std::{borrow::Cow, marker::PhantomData};

use schemars::{generate::SchemaSettings, JsonSchema, Schema, SchemaGenerator};
use serde_json::{json, Map, Value};

use crate::{InstructMacro, Parameter};

/// Adapts Instructor types, including hand-written legacy implementations,
/// to Schemars's generator without requiring a JsonSchema implementation.
#[doc(hidden)]
pub struct SchemaType<T: ?Sized>(PhantomData<T>);

impl<T: InstructMacro + ?Sized> JsonSchema for SchemaType<T> {
    fn schema_name() -> Cow<'static, str> {
        T::model_name().into()
    }

    fn schema_id() -> Cow<'static, str> {
        std::any::type_name::<T>().into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        T::describe(generator)
    }
}

pub(crate) fn root<T: InstructMacro + ?Sized>(inline: bool) -> Value {
    let mut settings = SchemaSettings::draft2020_12();
    settings.inline_subschemas = inline;
    settings.meta_schema = None;
    settings
        .into_generator()
        .into_root_schema_for::<SchemaType<T>>()
        .to_value()
}

// Preserve schema access for existing hand-written InstructMacro impls.
pub(crate) fn legacy(param: Parameter) -> Schema {
    let (mut value, description, list, optional) = match param {
        Parameter::Field(info) => {
            let ty = info
                .r#type
                .trim_start_matches("Option<")
                .trim_end_matches('>');
            let kind = match ty {
                "bool" => "boolean",
                "f32" | "f64" => "number",
                "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
                | "u128" | "usize" => "integer",
                _ => "string",
            };
            (
                json!({"type":kind}),
                info.comment,
                info.is_list,
                info.is_optional,
            )
        }
        Parameter::Enum(info) => (
            json!({"type":"string","enum":info.r#enum}),
            info.description,
            info.is_list,
            info.is_optional,
        ),
        Parameter::Struct(info) => {
            let mut properties = Map::new();
            let mut required = Vec::new();
            for param in info.parameters {
                let (name, optional) = match &param {
                    Parameter::Field(p) => (&p.name, p.is_optional),
                    Parameter::Enum(p) => (&p.title, p.is_optional),
                    Parameter::Struct(p) => (&p.name, p.is_optional),
                };
                if !optional {
                    required.push(name.clone());
                }
                properties.insert(name.clone(), legacy(param).to_value());
            }
            (
                json!({"type":"object","properties":properties,"required":required}),
                info.description,
                info.is_list,
                info.is_optional,
            )
        }
    };
    if list {
        value = json!({"type":"array", "items":value});
    }
    if optional {
        value = json!({"anyOf":[value,{"type":"null"}]});
    }
    if !description.is_empty() {
        value["description"] = description.into();
    }
    Schema::try_from(value).expect("legacy schemas always construct objects")
}

/// The old metadata tree has no representation for references. Stop
/// expanding properties at a recursive edge. Native schemas retain refs.
#[doc(hidden)]
pub fn legacy_parameters<T: ?Sized>(get: impl FnOnce() -> Vec<Parameter>) -> Vec<Parameter> {
    use std::cell::RefCell;
    thread_local! {
        static ACTIVE: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }
    let name = std::any::type_name::<T>();
    if ACTIVE.with_borrow(|names| names.contains(&name)) {
        return Vec::new();
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            ACTIVE.with_borrow_mut(|names| names.pop());
        }
    }
    ACTIVE.with_borrow_mut(|names| names.push(name));
    let _guard = Guard;
    get()
}
