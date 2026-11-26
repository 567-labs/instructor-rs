//! Native Rust response values, both standalone and nested inside models.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::Hash;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde_json::Value;

use crate::{
    InstructMacro, InstructMacroResult, Parameter, ParameterInfo, PartialModel, SchemaType,
    StructInfo,
};

fn wrapped_info<T: InstructMacro + ?Sized>() -> InstructMacroResult {
    InstructMacroResult::Struct(StructInfo {
        name: T::model_name(),
        description: String::new(),
        parameters: vec![T::parameter("content".to_string(), String::new())],
        is_optional: false,
        is_list: false,
    })
}

macro_rules! metadata {
    ($name:expr, $schema:ty) => {
        fn model_name() -> String {
            $name.to_string()
        }

        fn get_info() -> InstructMacroResult {
            wrapped_info::<Self>()
        }

        fn analysis_schema() -> Value {
            crate::schema::root::<Self>(false)
        }

        fn describe(generator: &mut SchemaGenerator) -> Schema {
            <$schema as JsonSchema>::json_schema(generator)
        }
    };
}

macro_rules! field {
    () => {
        fn parameter(name: String, description: String) -> Parameter {
            Parameter::Field(ParameterInfo {
                name,
                r#type: Self::model_name(),
                comment: description,
                is_optional: false,
                is_list: false,
            })
        }
    };
}

macro_rules! scalars {
    ($($ty:ty),+) => {$(
        impl InstructMacro for $ty {
            metadata!(stringify!($ty), Self);
            field!();
        }

        impl PartialModel for $ty {
            type Partial = Self;
            type Field = Option<Self>;
            fn into_partial(self) -> Self { self }
        }
    )+};
}

scalars!(
    bool, char, String, i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64,
    Value
);

impl InstructMacro for str {
    metadata!("str", str);
    field!();
}

impl InstructMacro for () {
    metadata!("Null", ());
    field!();
}

impl PartialModel for () {
    type Partial = ();
    type Field = Option<()>;
    fn into_partial(self) -> Self::Partial {}
}

impl<T: InstructMacro> InstructMacro for Option<T> {
    fn prepare_partial(
        value: &mut Value,
        open_path: Option<&[String]>,
        context: &Value,
    ) -> Result<(), String> {
        T::prepare_partial(value, open_path, context)
    }
    metadata!(
        format!("Optional{}", T::model_name()),
        Option<SchemaType<T>>
    );

    fn parameter(name: String, description: String) -> Parameter {
        let mut param = T::parameter(name, description);
        match &mut param {
            Parameter::Struct(s) => s.is_optional = true,
            Parameter::Enum(e) => e.is_optional = true,
            Parameter::Field(f) => {
                f.r#type = format!("Option<{}>", f.r#type);
                f.is_optional = true;
            }
        }
        param
    }

    fn validate(&self) -> Result<(), String> {
        self.as_ref().map_or(Ok(()), T::validate)
    }

    fn validate_with_context(&mut self, context: &Value) -> Result<(), String> {
        self.as_mut()
            .map_or(Ok(()), |v| v.validate_with_context(context))
    }
}

impl<T: PartialModel> PartialModel for Option<T> {
    type Partial = Option<T::Partial>;
    type Field = Self::Partial;
    fn into_partial(self) -> Self::Partial {
        self.map(T::into_partial)
    }
}

impl<T: InstructMacro> InstructMacro for Box<T> {
    fn prepare_partial(
        value: &mut Value,
        open_path: Option<&[String]>,
        context: &Value,
    ) -> Result<(), String> {
        T::prepare_partial(value, open_path, context)
    }
    fn model_name() -> String {
        T::model_name()
    }
    fn get_info() -> InstructMacroResult {
        T::get_info()
    }
    fn describe(generator: &mut SchemaGenerator) -> Schema {
        T::describe(generator)
    }
    fn parameter(name: String, description: String) -> Parameter {
        T::parameter(name, description)
    }
    fn validate(&self) -> Result<(), String> {
        T::validate(self)
    }
    fn validate_with_context(&mut self, context: &Value) -> Result<(), String> {
        T::validate_with_context(self, context)
    }
}

impl<T: PartialModel> PartialModel for Box<T> {
    type Partial = Box<T::Partial>;
    type Field = Option<Self::Partial>;
    fn into_partial(self) -> Self::Partial {
        Box::new(T::into_partial(*self))
    }
}

fn list_param<T: InstructMacro>(name: String, description: String) -> Parameter {
    let mut param = T::parameter(name, description);
    match &mut param {
        Parameter::Struct(s) => s.is_list = true,
        Parameter::Enum(e) => e.is_list = true,
        Parameter::Field(f) => f.is_list = true,
    }
    param
}

macro_rules! list_methods {
    () => {
        fn prepare_partial(
            value: &mut Value,
            open_path: Option<&[String]>,
            context: &Value,
        ) -> Result<(), String> {
            if let Some(items) = value.as_array_mut() {
                for (index, item) in items.iter_mut().enumerate() {
                    if item.is_array() || item.is_object() {
                        crate::snapshots::child::<T>(&index.to_string(), item, open_path, context)?;
                    }
                }
            }
            Ok(())
        }

        fn parameter(name: String, description: String) -> Parameter {
            list_param::<T>(name, description)
        }

        fn validate(&self) -> Result<(), String> {
            for (index, item) in self.iter().enumerate() {
                item.validate().map_err(|e| format!("[{index}]: {e}"))?;
            }
            Ok(())
        }
    };
}

fn validate_slice<T: InstructMacro>(items: &mut [T], context: &Value) -> Result<(), String> {
    for (index, item) in items.iter_mut().enumerate() {
        item.validate_with_context(context)
            .map_err(|e| format!("[{index}]: {e}"))?;
    }
    Ok(())
}

impl<T: InstructMacro> InstructMacro for Vec<T> {
    metadata!(format!("List{}", T::model_name()), Vec<SchemaType<T>>);
    list_methods!();

    fn validate_with_context(&mut self, context: &Value) -> Result<(), String> {
        validate_slice(self, context)
    }
}

impl<T: PartialModel> PartialModel for Vec<T> {
    type Partial = Vec<T::Partial>;
    type Field = Option<Self::Partial>;
    fn into_partial(self) -> Self::Partial {
        self.into_iter().map(T::into_partial).collect()
    }
}

impl<T: InstructMacro, const N: usize> InstructMacro for [T; N] {
    list_methods!();

    fn get_info() -> InstructMacroResult {
        wrapped_info::<Self>()
    }
    fn model_name() -> String {
        format!("Array{N}{}", T::model_name())
    }
    fn describe(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = <Vec<T> as InstructMacro>::describe(generator);
        schema.insert("minItems".into(), N.into());
        schema.insert("maxItems".into(), N.into());
        schema
    }
    fn validate_with_context(&mut self, context: &Value) -> Result<(), String> {
        validate_slice(self, context)
    }
}

impl<T: PartialModel, const N: usize> PartialModel for [T; N] {
    type Partial = Vec<T::Partial>;
    type Field = Option<Self::Partial>;
    fn into_partial(self) -> Self::Partial {
        self.into_iter().map(T::into_partial).collect()
    }
}

macro_rules! map {
    ($map:ident) => {
        impl<T: InstructMacro> InstructMacro for $map<String, T> {
            fn prepare_partial(value: &mut Value, open_path: Option<&[String]>, context: &Value) -> Result<(), String> {
                if let Some(fields) = value.as_object_mut() {
                    for (key, field) in fields {
                        crate::snapshots::child::<T>(key,field,open_path,context)?;
                    }
                }
                Ok(())
            }

            metadata!(format!("Map{}", T::model_name()), $map<String, SchemaType<T>>);
            field!();
            fn validate(&self) -> Result<(), String> {
                for (key, item) in self {
                    item.validate().map_err(|e| format!("[{key:?}]: {e}"))?;
                }
                Ok(())
            }
            fn validate_with_context(&mut self, context: &Value) -> Result<(), String> {
                for (key, item) in self {
                    item.validate_with_context(context).map_err(|e| format!("[{key:?}]: {e}"))?;
                }
                Ok(())
            }
        }

        impl<T: PartialModel> PartialModel for $map<String, T> {
            type Partial = $map<String, T::Partial>;
            type Field = Option<Self::Partial>;
            fn into_partial(self) -> Self::Partial {
                self.into_iter().map(|(k,v)| (k,v.into_partial())).collect()
            }
        }
    };
}

map!(BTreeMap);
map!(HashMap);

macro_rules! set {
    ($set:ident, $($bounds:tt)+) => {
        impl<T: InstructMacro + $($bounds)+> InstructMacro for $set<T> {
            metadata!(format!("Set{}", T::model_name()), $set<SchemaType<T>>);
            list_methods!();

            fn validate_with_context(&mut self, context: &Value) -> Result<(), String> {
                // A validator can change the ordering/hash of an element.
                let mut values: Vec<_> = std::mem::take(self).into_iter().collect();
                let result = validate_slice(&mut values, context);
                self.extend(values);
                result
            }
        }

        impl<T: PartialModel> PartialModel for $set<T> {
            type Partial = Vec<T::Partial>;
            type Field = Option<Self::Partial>;
            fn into_partial(self) -> Self::Partial {
                self.into_iter().map(T::into_partial).collect()
            }
        }
    };
}

set!(BTreeSet, Ord);
set!(HashSet, Eq + Hash);

macro_rules! tuples {
    ($($ty:ident $index:tt),+) => {
        impl<$($ty: InstructMacro),+> InstructMacro for ($($ty,)+) {
            fn prepare_partial(value: &mut Value, open_path: Option<&[String]>, context: &Value) -> Result<(), String> {
                $(if let Some(field) = value.get_mut($index) {
                    crate::snapshots::child::<$ty>(stringify!($index), field,open_path,context)?;
                })+
                Ok(())
            }

            metadata!("Tuple", ($(SchemaType<$ty>,)+));
            field!();

            fn validate(&self) -> Result<(), String> {
                $(self.$index.validate().map_err(|e| format!("[{}]: {e}", $index))?;)+
                Ok(())
            }
            fn validate_with_context(&mut self, context: &Value) -> Result<(), String> {
                $(self.$index.validate_with_context(context).map_err(|e| format!("[{}]: {e}", $index))?;)+
                Ok(())
            }
        }

        impl<$($ty: PartialModel),+> PartialModel for ($($ty,)+) {
            type Partial = ($($ty::Partial,)+);
            type Field = Option<Self::Partial>;
            fn into_partial(self) -> Self::Partial {
                ($(self.$index.into_partial(),)+)
            }
        }
    };
}
tuples!(A 0);
tuples!(A 0, B 1);
tuples!(A 0, B 1, C 2);
tuples!(A 0, B 1, C 2, D 3);
tuples!(A 0, B 1, C 2, D 3, E 4);
tuples!(A 0, B 1, C 2, D 3, E 4, F 5);
tuples!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
tuples!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);
tuples!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8);
tuples!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9);
tuples!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10);
tuples!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11);
