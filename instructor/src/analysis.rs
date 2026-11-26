//! Local preflight analysis and advisory output mode selection.
use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::{InstructMacro, Mode, Provider};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SchemaFinding {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub path: String,
    pub suggestion: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SchemaAnalysis {
    pub findings: Vec<SchemaFinding>,
    pub total_fields: usize,
    pub max_depth: usize,
    pub has_recursion: bool,
    pub num_required: usize,
    pub num_optional: usize,
    pub estimated_token_overhead: usize,
}

/// Inspect the native schema of a response type prior to provider conversion.
pub fn analyze_schema<T: InstructMacro>() -> SchemaAnalysis {
    SchemaAnalysis::from_json_schema(&T::analysis_schema())
}

/// Suggest a mode using Python Instructor's strict-preference heuristic.
pub fn select_mode<T: InstructMacro>(provider: Provider) -> Mode {
    analyze_schema::<T>().select_mode(provider, true)
}

impl SchemaAnalysis {
    pub fn from_json_schema(schema: &Value) -> Self {
        let mut walk = Walk {
            report: Self::default(),
            root: schema,
            heights: HashMap::from([("#", None)]),
        };
        walk.report.max_depth = walk.depth(schema, "$");
        let mut report = walk.report;
        report.estimated_token_overhead = report.total_fields * 15;
        report.globals();
        report
    }

    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    pub fn has_warnings(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity == Severity::Warning)
    }

    /// Structural heuristic in the range 0 through 100.
    pub fn complexity_score(&self) -> usize {
        self.total_fields.min(15) * 2
            + self.max_depth.min(4) * 8
            + usize::from(self.has_recursion) * 20
            + (self.estimated_token_overhead / 100).min(18)
    }

    /// Provider-independent hint when the schema is recursive, deep or complex.
    pub fn recommended_mode(&self) -> Option<Mode> {
        if self.has_recursion {
            Some(Mode::MarkdownJson)
        } else if self.complexity_score() > 70 || self.max_depth > 4 {
            Some(Mode::JsonSchema)
        } else {
            None
        }
    }

    /// Suggest a mode using a precomputed report. The candidate set follows
    /// the Python provider declarations. This is advisory and independent of
    /// modes that can be requested explicitly at a particular deployment.
    pub fn select_mode(&self, provider: Provider, prefer_strict: bool) -> Mode {
        use Mode::{Json, JsonSchema, MarkdownJson, Tools};
        let preferences = if self.has_recursion {
            [MarkdownJson, Json, JsonSchema, Tools]
        } else if prefer_strict && self.recommended_mode().is_some() {
            [JsonSchema, Tools, MarkdownJson, Json]
        } else if !prefer_strict && self.complexity_score() > 70 {
            [MarkdownJson, JsonSchema, Tools, Json]
        } else {
            return Tools;
        };
        let candidates: &[Mode] = match provider {
            Provider::OpenAI => &[Tools, JsonSchema, MarkdownJson, Json],
            Provider::Anthropic => &[Tools, Json, JsonSchema],
            Provider::Google => &[Tools, Json],
            Provider::Cohere | Provider::Mistral | Provider::OpenRouter => {
                &[Tools, JsonSchema, MarkdownJson]
            }
        };
        preferences
            .into_iter()
            .find(|m| candidates.contains(m))
            .unwrap_or(Tools)
    }

    fn finding(
        &mut self,
        severity: Severity,
        code: &'static str,
        message: String,
        path: &str,
        suggestion: &'static str,
    ) {
        self.findings.push(SchemaFinding {
            severity,
            code,
            message,
            path: path.to_string(),
            suggestion,
        });
    }

    fn globals(&mut self) {
        use Severity::{Error, Warning};
        let depth = self.max_depth;
        if depth > 4 {
            let (severity, code) = if depth > 7 {
                (Error, "DEPTH_EXCESSIVE")
            } else {
                (Warning, "DEPTH_HIGH")
            };
            self.finding(
                severity,
                code,
                format!("Schema depth is {depth}."),
                "",
                "Flatten nested structures or extract sub-models in separate calls.",
            );
        }
        let fields = self.total_fields;
        if fields > 20 {
            let (severity, code) = if fields > 50 {
                (Error, "TOO_MANY_FIELDS")
            } else {
                (Warning, "MANY_FIELDS")
            };
            self.finding(
                severity,
                code,
                format!("Schema has {fields} total fields."),
                "",
                "Review which fields are needed in a single extraction.",
            );
        }
        if self.num_required > 12 {
            self.finding(
                Warning,
                "MANY_REQUIRED",
                format!("Schema has {} required fields.", self.num_required),
                "",
                "Consider whether some fields are optional.",
            );
        }
    }
}

struct Walk<'a> {
    report: SchemaAnalysis,
    root: &'a Value,
    // None marks a definition on the active stack. Some(height) reuses a
    // completed definition without recounting fields or revisiting its tree.
    heights: HashMap<&'a str, Option<usize>>,
}

impl<'a> Walk<'a> {
    fn reference(&mut self, reference: &'a str, path: &str) -> usize {
        let name = reference
            .rsplit('/')
            .next()
            .unwrap_or(reference)
            .replace("~1", "/")
            .replace("~0", "~");
        if let Some(height) = self.heights.get(reference) {
            if let Some(h) = height {
                return *h;
            }
            self.report.has_recursion = true;
            self.report.finding(
                Severity::Warning,
                "RECURSIVE_REF",
                format!("Recursive reference to {name}."),
                path,
                "Check whether the selected deployment accepts recursive schemas.",
            );
            return 0;
        }
        let Some(target) = self.root.pointer(&reference[1..]) else {
            return 0;
        };
        self.heights.insert(reference, None);
        let height = self.depth(target, &format!("{path}.${name}"));
        self.heights.insert(reference, Some(height));
        height
    }

    fn depth(&mut self, node: &'a Value, path: &str) -> usize {
        if let Some(reference) = node["$ref"].as_str() {
            if reference == "#" || reference.starts_with("#/$defs/") {
                return self.reference(reference, path);
            }
        }
        let mut height = 0;
        for key in ["anyOf", "oneOf", "allOf"] {
            if let Some(values) = node[key].as_array() {
                for (index, item) in values.iter().enumerate() {
                    height = height.max(self.depth(item, &format!("{path}.{key}[{index}]")));
                }
            }
        }
        if let Some(values) = node["enum"].as_array() {
            let count = values.len();
            if count > 15 {
                let (severity, code) = if count > 50 {
                    (Severity::Error, "ENUM_TOO_LARGE")
                } else {
                    (Severity::Warning, "ENUM_LARGE")
                };
                self.report.finding(
                    severity,
                    code,
                    format!("Enum at {path} has {count} values."),
                    path,
                    "Review the size of the enumerated choice set.",
                );
            }
        }
        if let Some(properties) = node["properties"].as_object() {
            let count = properties.len();
            self.report.total_fields += count;
            if count > 10 {
                self.report.finding(
                    Severity::Warning,
                    "WIDE_OBJECT",
                    format!("Object at {path} has {count} properties."),
                    path,
                    "Consider grouping related properties.",
                );
            }
            for (key, item) in properties {
                if node["required"]
                    .as_array()
                    .is_some_and(|list| list.iter().any(|s| s == key))
                {
                    self.report.num_required += 1;
                } else {
                    self.report.num_optional += 1;
                }
                height = height.max(1 + self.depth(item, &format!("{path}.{key}")));
            }
        }
        if node["additionalProperties"].is_object() {
            height =
                height.max(1 + self.depth(&node["additionalProperties"], &format!("{path}.*")));
        }
        if let Some(items) = node.get("items") {
            height = height.max(1 + self.depth(items, &format!("{path}[]")));
        }
        if let Some(values) = node["prefixItems"].as_array() {
            for (index, item) in values.iter().enumerate() {
                height = height.max(1 + self.depth(item, &format!("{path}[{index}]")));
            }
        }
        height
    }
}
