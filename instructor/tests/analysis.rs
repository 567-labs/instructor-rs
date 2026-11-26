//! Advisory mode and analysis contracts from Python's test_auto_mode and test_schema_analyzer.
use std::collections::BTreeMap;

use instructor_ai::{analyze_schema, model, select_mode, Mode, Provider, SchemaAnalysis, Severity};
use serde_json::{json, Map, Value};

#[model]
struct Leaf {
    name: String,
}

#[model]
struct Record {
    by_id: BTreeMap<String, Leaf>,
    pair: Option<(Leaf, Leaf)>,
}

#[test]
fn counts_shared_models_once_across_tuple_map_and_union() {
    let analysis = analyze_schema::<Record>();
    assert_eq!(analysis.total_fields, 3);
    assert_eq!(analysis.max_depth, 3);
    assert_eq!(analysis.num_required, 2);
    assert_eq!(analysis.num_optional, 1);
    assert_eq!(analysis.estimated_token_overhead, 45);
    assert_eq!(analysis.complexity_score(), 30);
    assert!(!analysis.has_warnings());
    assert!(!analysis.has_errors());
    assert_eq!(analysis.recommended_mode(), None);
}

#[test]
fn detects_recursion_and_selects_available_modes() {
    #[model]
    struct Node {
        label: String,
        children: Vec<Node>,
    }
    let analysis = analyze_schema::<Node>();
    assert!(analysis.has_recursion);
    assert_eq!(analysis.total_fields, 2);
    assert_eq!(analysis.recommended_mode(), Some(Mode::MarkdownJson));
    assert!(analysis.findings.iter().any(|f| f.code == "RECURSIVE_REF"));
    for provider in [
        Provider::OpenAI,
        Provider::OpenRouter,
        Provider::Mistral,
        Provider::Cohere,
        Provider::Google,
        Provider::Anthropic,
    ] {
        assert_eq!(select_mode::<Leaf>(provider), Mode::Tools);
        let preferred = match provider {
            Provider::Google | Provider::Anthropic => Mode::Json,
            _ => Mode::MarkdownJson,
        };
        assert_eq!(select_mode::<Node>(provider), preferred);
        assert_eq!(analysis.select_mode(provider, false), preferred);
    }
}

#[test]
fn shared_dag_is_counted_once_and_keeps_deepest_use() {
    let mut defs = json!({"Leaf":{"properties":{"value":{"type":"string"}}}});
    let leaf = json!({"$ref":"#/$defs/Leaf"});
    let mut branch = leaf.clone();
    for i in 0..12 {
        let name = format!("Branch{i}");
        defs[&name] = json!({"properties":{"left":branch,"right":branch}});
        branch = json!({"$ref":format!("#/$defs/{name}")});
    }
    let root = json!({"$defs":defs, "properties":{"short":leaf, "long":branch}});
    let report = SchemaAnalysis::from_json_schema(&root);
    assert_eq!(report.total_fields, 27);
    assert_eq!(report.max_depth, 14);
    assert!(!report.has_recursion);
    assert!(report.has_errors());
    assert!(report.has_warnings());
    assert_eq!(report.select_mode(Provider::OpenAI, true), Mode::JsonSchema);
    assert_eq!(report.select_mode(Provider::OpenAI, false), Mode::Tools);
}

#[test]
fn respects_score_boundary_and_strict_preference() {
    let mut report = SchemaAnalysis {
        total_fields: 15,
        max_depth: 3,
        estimated_token_overhead: 1600,
        ..Default::default()
    };
    assert_eq!(report.complexity_score(), 70);
    assert_eq!(report.select_mode(Provider::OpenAI, true), Mode::Tools);
    report.estimated_token_overhead = 1700;
    assert_eq!(report.complexity_score(), 71);
    assert_eq!(report.select_mode(Provider::OpenAI, true), Mode::JsonSchema);
    assert_eq!(
        report.select_mode(Provider::OpenAI, false),
        Mode::MarkdownJson
    );
    assert_eq!(
        report.select_mode(Provider::Mistral, true),
        Mode::JsonSchema
    );
    assert_eq!(report.select_mode(Provider::Google, true), Mode::Tools);
    report.has_recursion = true;
    report.max_depth = 100;
    report.estimated_token_overhead = 10000;
    assert_eq!(report.complexity_score(), 100);
}

#[test]
fn escapes_definition_paths_and_handles_missing_references_and_boolean_nodes() {
    let schema = json!({"$defs":{"a/b~":{"properties":{"label":{"enum":(0..16).collect::<Vec<_>>()}}}},
        "prefixItems":[true,{"$ref":"#/$defs/missing"},{"$ref":"#/$defs/a~1b~0"}]});
    let report = SchemaAnalysis::from_json_schema(&schema);
    assert_eq!(report.total_fields, 1);
    assert_eq!(report.max_depth, 2);
    assert_eq!(report.findings[0].code, "ENUM_LARGE");
    assert_eq!(report.findings[0].path, "$[2].$a/b~.label");
    assert_eq!(report.findings[0].severity, Severity::Warning);
}

#[test]
fn global_complexity_thresholds_match_python() {
    for n in [10, 11, 12, 13, 20, 21, 50, 51] {
        let properties: Map<String, Value> = (0..n)
            .map(|i| (i.to_string(), json!({"type":"string"})))
            .collect();
        let names: Vec<_> = properties.keys().collect();
        let report =
            SchemaAnalysis::from_json_schema(&json!({"properties":properties,"required":names}));
        let found = |code| report.findings.iter().any(|f| f.code == code);
        assert_eq!(found("WIDE_OBJECT"), n > 10);
        assert_eq!(found("MANY_REQUIRED"), n > 12);
        assert_eq!(found("MANY_FIELDS"), (21..=50).contains(&n));
        assert_eq!(found("TOO_MANY_FIELDS"), n > 50);
    }
    for n in [4, 5, 7, 8] {
        let mut value = json!({});
        for _ in 0..n {
            value = json!({"items":value});
        }
        let report = SchemaAnalysis::from_json_schema(&value);
        assert_eq!(report.max_depth, n);
        assert_eq!(report.has_warnings(), (5..=7).contains(&n));
        assert_eq!(report.has_errors(), n > 7);
    }
    let choices = json!({"enum":(0..51).collect::<Vec<_>>()});
    assert_eq!(
        SchemaAnalysis::from_json_schema(&choices).findings[0].code,
        "ENUM_TOO_LARGE"
    );
}
