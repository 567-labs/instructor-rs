use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

#[doc(hidden)]
pub use {schemars, serde, serde_json};

mod builtins;
mod schema;
#[doc(hidden)]
pub use schema::{legacy_parameters, SchemaType};

pub trait InstructMacro {
    fn get_info() -> InstructMacroResult;
    /// Check this value and its nested models.
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }

    /// Check and optionally normalize a completed value with per-request data.
    fn validate_with_context(&mut self, _context: &Value) -> Result<(), String> {
        self.validate()
    }

    /// Validate completed child objects in an in-progress JSON document.
    /// The path locates the innermost open container or unfinished object string value.
    /// None means the value is closed.
    #[doc(hidden)]
    fn prepare_partial(
        _value: &mut Value,
        _open_path: Option<&[String]>,
        _context: &Value,
    ) -> Result<(), String> {
        Ok(())
    }

    /// The name used to identify this output type in a tool call.
    fn model_name() -> String {
        match Self::get_info() {
            InstructMacroResult::Struct(info) => info.name,
            InstructMacroResult::Enum(info) => info.title,
        }
    }

    /// JSON Schema for the native value, before the provider's wire envelope.
    fn json_schema() -> Value {
        schema::root::<Self>(true)
    }

    /// Schema preserving named definitions to count reused models once.
    #[doc(hidden)]
    fn analysis_schema() -> Value {
        Self::json_schema()
    }

    #[doc(hidden)]
    fn describe(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schema::legacy(Self::get_info().wrap_info(String::new()))
    }

    #[doc(hidden)]
    fn parameter(name: String, description: String) -> Parameter {
        Self::get_info()
            .override_description(description)
            .wrap_info(name)
    }
}

/// The typed snapshot of a model at a particular stage of generation.
pub type Partial<T> = <T as PartialModel>::Partial;

/// Relates a full output type to the type of an in-progress snapshot.
///
/// A nested field can be absent. For an already nullable field there is still
/// a single layer of optionality. Collection elements use `Partial`, so a
/// nullable element and a missing collection remain distinct.
pub trait PartialModel: Sized {
    type Partial: DeserializeOwned + Serialize + std::fmt::Debug;
    #[doc(hidden)]
    type Field: DeserializeOwned + Serialize + std::fmt::Debug + Default + From<Self::Partial>;

    fn into_partial(self) -> Self::Partial;

    #[doc(hidden)]
    fn into_field(self) -> Self::Field {
        self.into_partial().into()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum InstructMacroResult {
    Struct(StructInfo),
    Enum(EnumInfo),
}

impl InstructMacroResult {
    pub fn wrap_info(self, new_name: String) -> Parameter {
        match self {
            InstructMacroResult::Struct(struct_info) => struct_info.wrap_info(new_name),
            InstructMacroResult::Enum(enum_info) => enum_info.wrap_info(new_name),
        }
    }

    pub fn override_description(self, new_description: String) -> InstructMacroResult {
        match self {
            InstructMacroResult::Struct(struct_info) => {
                InstructMacroResult::Struct(struct_info.override_description(new_description))
            }
            InstructMacroResult::Enum(enum_info) => {
                InstructMacroResult::Enum(enum_info.override_description(new_description))
            }
        }
    }

    pub fn set_optional(self, is_optional: bool) -> InstructMacroResult {
        match self {
            InstructMacroResult::Struct(struct_info) => {
                InstructMacroResult::Struct(struct_info.set_optional(is_optional))
            }
            InstructMacroResult::Enum(enum_info) => {
                InstructMacroResult::Enum(enum_info.set_optional(is_optional))
            }
        }
    }

    pub fn set_list(self, is_list: bool) -> InstructMacroResult {
        match self {
            InstructMacroResult::Struct(struct_info) => {
                InstructMacroResult::Struct(struct_info.set_list(is_list))
            }
            InstructMacroResult::Enum(enum_info) => {
                InstructMacroResult::Enum(enum_info.set_list(is_list))
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct StructInfo {
    pub name: String,
    pub description: String,
    pub parameters: Vec<Parameter>,
    pub is_optional: bool,
    pub is_list: bool,
}

impl StructInfo {
    pub fn wrap_info(mut self, new_name: String) -> Parameter {
        self.name = new_name;
        Parameter::Struct(self)
    }

    pub fn override_description(mut self, new_description: String) -> StructInfo {
        if !new_description.is_empty() {
            self.description = new_description;
        }
        self
    }

    pub fn set_optional(mut self, is_optional: bool) -> StructInfo {
        self.is_optional = is_optional;
        self
    }

    pub fn set_list(mut self, is_list: bool) -> StructInfo {
        self.is_list = is_list;
        self
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Parameter {
    Struct(StructInfo),
    Field(ParameterInfo),
    Enum(EnumInfo),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ParameterInfo {
    pub name: String,
    pub r#type: String,
    pub comment: String,
    pub is_optional: bool,
    pub is_list: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct EnumInfo {
    pub title: String,
    pub r#enum: Vec<String>,
    pub r#type: String,
    pub description: String,
    pub is_optional: bool,
    pub is_list: bool,
}

impl EnumInfo {
    pub fn wrap_info(mut self, new_name: String) -> Parameter {
        self.title = new_name;
        Parameter::Enum(self)
    }

    pub fn override_description(mut self, new_description: String) -> EnumInfo {
        if !new_description.is_empty() {
            self.description = new_description;
        }
        self
    }

    pub fn set_optional(mut self, is_optional: bool) -> EnumInfo {
        self.is_optional = is_optional;
        self
    }

    pub fn set_list(mut self, is_list: bool) -> EnumInfo {
        self.is_list = is_list;
        self
    }
}

pub struct FieldInfo {
    pub name: String,
    pub description: String,
    pub r#type: String,
    pub is_complex: bool,
    pub is_optional: bool,
    pub is_list: bool,
}

#[doc(hidden)]
pub mod snapshots;
