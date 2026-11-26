//! Mirrors tests/typing/test_public_surface.py, tests/v2/test_simple_type_fixes.py
//! and tests/dsl/test_partial_{optional_lists,default_factories}.py.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use instruct_macros::{model, InstructMacro, PartialModel};
use instruct_macros_types::{InstructMacro, PartialModel};
use serde_json::{json, Value};

mod child {
    use super::*;

    /// A person identified in the text.
    #[model]
    #[serde(rename_all = "camelCase")]
    pub struct Person {
        #[validate(context = "normalize_name")]
        #[description("Name as written.")]
        pub first_name: String,
        /// Age in completed years.
        #[validate(custom = "age_range")]
        pub age: u32,
    }

    fn normalize_name(name: &mut String, context: &Value) -> Result<(), String> {
        if context["uppercase"] == true {
            *name = name.to_uppercase();
        }
        Ok(())
    }

    fn age_range(age: &u32) -> Result<(), String> {
        if *age <= 125 {
            Ok(())
        } else {
            Err("invalid age".into())
        }
    }
}

type People = Option<Vec<Option<child::Person>>>;

#[model]
#[serde(deny_unknown_fields)]
struct Report {
    #[serde(rename = "people", alias = "records")]
    entries: People,
    title: String,
}

#[model]
struct Envelope<T> {
    value: T,
}

#[model]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
enum Event<T> {
    RecordCreated { record: T },
    FinalCode(i64),
    Done,
}

#[test]
fn schema_contains_serde_wire_names_descriptions_and_required_fields() {
    let schema = child::Person::json_schema();
    assert_eq!(schema["description"], "A person identified in the text.");
    assert_eq!(
        schema["properties"]["firstName"]["description"],
        "Name as written."
    );
    assert_eq!(
        schema["properties"]["age"]["description"],
        "Age in completed years."
    );
    assert_eq!(schema["properties"]["age"]["type"], "integer");
    assert_eq!(schema["required"].as_array().unwrap().len(), 2);
    let report = Report::json_schema();
    assert_eq!(report["additionalProperties"], false);
    assert!(report["properties"]["people"].is_object());
    assert_eq!(report["required"], json!(["title"]));
    assert_eq!(Report::model_name(), "Report");
}

#[test]
fn stable_legacy_derive_needs_no_auxiliary_imports() {
    mod legacy {
        #[derive(instruct_macros::InstructMacro)]
        pub struct Legacy<T> {
            pub rows: Option<std::vec::Vec<T>>,
        }
    }
    let schema = legacy::Legacy::<i64>::json_schema();
    assert_eq!(schema["properties"]["rows"]["anyOf"][0]["type"], "array");
    assert_eq!(
        schema["properties"]["rows"]["anyOf"][0]["items"]["type"],
        "integer"
    );
}

#[test]
fn scalar_and_collection_schemas_describe_native_values() {
    assert_eq!(bool::json_schema()["type"], "boolean");
    assert_eq!(String::json_schema()["type"], "string");
    assert_eq!(f64::json_schema()["type"], "number");
    assert_eq!(Vec::<i64>::json_schema()["items"]["type"], "integer");
    assert_eq!(HashSet::<i64>::json_schema()["uniqueItems"], true);
    assert_eq!(BTreeSet::<i64>::json_schema()["uniqueItems"], true);
    assert_eq!(
        HashMap::<String, i64>::json_schema()["additionalProperties"]["type"],
        "integer"
    );
    let schema = Vec::<BTreeMap<String, (i64, String)>>::json_schema();
    let pair = &schema["items"]["additionalProperties"];
    assert_eq!(pair["prefixItems"][0]["type"], "integer");
    assert_eq!(pair["prefixItems"][1]["type"], "string");
    assert_eq!(<[bool; 4]>::json_schema()["minItems"], 4);
    assert_eq!(<[bool; 4]>::json_schema()["maxItems"], 4);
}

#[test]
fn tagged_enum_and_generic_models_validate_nested_records() {
    type Model = Envelope<Event<child::Person>>;
    let wire = json!({"value":{"kind":"record_created","data":{
        "record":{"firstName":"ada","age":42}
    }}});
    let mut model: Model = serde_json::from_value(wire).unwrap();
    model
        .validate_with_context(&json!({"uppercase":true}))
        .unwrap();
    if let Event::RecordCreated { record } = &mut model.value {
        assert_eq!(record.first_name, "ADA");
        record.age = 200;
    } else {
        panic!("expected record");
    }
    assert!(model.validate().unwrap_err().contains("invalid age"));
    let snapshot: EnvelopePartial<Event<child::Person>> = model.into_partial();
    match snapshot.value.unwrap() {
        EventPartial::RecordCreated { record } => {
            let person: child::PersonPartial = record.unwrap();
            assert_eq!(person.first_name.as_deref(), Some("ADA"));
        }
        _ => panic!("expected record"),
    }
    let schema = Model::json_schema();
    assert!(schema.to_string().contains("record_created"));
    assert!(schema["properties"]["value"]["oneOf"].is_array());
}

#[test]
fn nullable_lists_keep_typed_optional_elements_and_serde_aliases() {
    let payload = json!({"records":[null,{"firstName":"Ad"}]});
    let snapshot: instruct_macros_types::Partial<Report> =
        serde_json::from_value(payload.clone()).unwrap();
    let entries: Vec<Option<child::PersonPartial>> = snapshot.entries.unwrap();
    assert!(entries[0].is_none());
    assert_eq!(
        entries[1].as_ref().unwrap().first_name.as_deref(),
        Some("Ad")
    );
    assert_eq!(entries[1].as_ref().unwrap().age, None);
    assert_eq!(snapshot.title, None);
    assert!(serde_json::from_value::<Report>(payload).is_err());
    for payload in [json!({}), json!({"people":null})] {
        assert!(serde_json::from_value::<ReportPartial>(payload)
            .unwrap()
            .entries
            .is_none());
    }
    assert!(
        serde_json::from_value::<ReportPartial>(json!({"people":[]}))
            .unwrap()
            .entries
            .unwrap()
            .is_empty()
    );
    assert!(serde_json::from_value::<ReportPartial>(json!({"unknown":1})).is_err());
}

#[test]
fn partial_defaults_are_independent_and_explicit_values_win() {
    fn initial() -> child::Person {
        child::Person {
            first_name: "default".into(),
            age: 18,
        }
    }

    #[model]
    struct Defaults {
        #[serde(default)]
        labels: Vec<String>,
        #[serde(default = "initial")]
        owner: child::Person,
    }
    let full: Defaults = serde_json::from_value(json!({})).unwrap();
    assert_eq!(full.owner.first_name, "default");
    let mut a: DefaultsPartial = serde_json::from_value(json!({})).unwrap();
    let b: DefaultsPartial = serde_json::from_value(json!({"owner":{"age":25}})).unwrap();
    assert_eq!(a.owner.unwrap().first_name.as_deref(), Some("default"));
    assert_eq!(b.owner.unwrap().first_name, None);
    a.labels.as_mut().unwrap().push("first".into());
    assert!(b.labels.unwrap().is_empty());
}

#[test]
fn context_flows_through_maps_lists_boxes_to_fields_before_model_check() {
    #[model]
    #[validate(custom = "approved")]
    struct Directory {
        entries: BTreeMap<String, Vec<Option<Box<child::Person>>>>,
    }

    fn approved(d: &Directory) -> Result<(), String> {
        if d.entries["eng"][0].as_ref().unwrap().first_name == "ADA" {
            Ok(())
        } else {
            Err("approval missing".into())
        }
    }
    let wire = json!({"entries":{"eng":[{"firstName":"Ada","age":42}]}});
    let mut d: Directory = serde_json::from_value(wire).unwrap();
    assert!(d.validate().unwrap_err().contains("approval missing"));
    d.validate_with_context(&json!({"uppercase":true})).unwrap();
    d.entries.get_mut("eng").unwrap()[0].as_mut().unwrap().age = 200;
    let error = d.validate().unwrap_err();
    assert!(error.starts_with("entries: [\"eng\"]: [0]:"));
    assert!(error.contains("age"));
}

#[test]
fn recursive_models_generate_finite_schema_and_typed_snapshots() {
    #[model]
    struct Node {
        value: String,
        next: Option<Box<Self>>,
    }
    // The legacy metadata tree has no reference node, but inspecting it
    // must still terminate for a recursive model.
    let _ = Node::get_info();
    let schema = Node::json_schema();
    assert!(schema.to_string().contains("$ref"));
    let snapshot: NodePartial = serde_json::from_value(json!({"next":{"value":"leaf"}})).unwrap();
    assert!(snapshot.value.is_none());
    assert_eq!(snapshot.next.unwrap().value.as_deref(), Some("leaf"));
}

#[test]
fn derive_partial_model_for_preexisting_struct() {
    #[derive(InstructMacro, PartialModel)]
    struct Prior {
        complete: bool,
    }
    let snapshot = Prior { complete: true }.into_partial();
    assert_eq!(snapshot.complete, Some(true));
}

#[test]
fn validates_completed_items_before_the_root_but_keeps_open_item_provisional() {
    let ctx = json!({"uppercase":true});
    let path = ["records".to_string(), "1".to_string()];
    let mut object = json!({"records":[{"firstName":"ada","age":42},{"firstName":"Bo"}]});
    Report::prepare_partial(&mut object, Some(&path), &ctx).unwrap();
    assert_eq!(object["records"][0]["firstName"], "ADA");
    assert_eq!(object["records"][1]["firstName"], "Bo");
    let error = Report::prepare_partial(&mut object, Some(&[]), &ctx).unwrap_err();
    assert!(error.contains("age"));
}

#[test]
fn normalizes_completed_nested_objects_under_a_renamed_map() {
    #[model]
    struct Directory {
        #[serde(rename = "group")]
        persons: BTreeMap<String, Vec<child::Person>>,
    }
    let ctx = json!({"uppercase":true});
    let mut object = json!({"group":{"north":[{"firstName":"ada","age":42}]}});
    Directory::prepare_partial(&mut object, Some(&[]), &ctx).unwrap();
    assert_eq!(object["group"]["north"][0]["firstName"], "ADA");
}

#[test]
fn model_and_parameter_names_can_shadow_prelude_types() {
    #[model]
    struct Result<String, Option> {
        summary: String,
        selected: Option,
    }

    let wire = json!({"summary":"ready","selected":true});
    let result: Result<String, bool> = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(result.into_partial()).unwrap(), wire);
}
