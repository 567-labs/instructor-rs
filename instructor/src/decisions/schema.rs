use super::questions::Spec;
use super::{Answers, DecisionError, DecisionModel, DecisionRequest, Noul, Provider, Route, Value};
use serde_json::{json, Map};
use std::{collections::HashSet, marker::PhantomData};

enum Expected {
    Choice(Vec<Value>),
    Noul,
    Score { max: f64, scale: Option<(f64, f64)> },
}

impl Expected {
    fn kind(&self, provider: Provider) -> &'static str {
        match self {
            Self::Choice(_) => "choice",
            Self::Noul if provider == Provider::OpenAi => "predicate",
            Self::Noul => "noul",
            Self::Score { .. } => "score",
        }
    }

    fn parse(&self, provider: Provider, answer: &Value) -> Result<Value, String> {
        let kind = self.kind(provider);
        if answer["type"] == "refusal" {
            return Err("provider refused the decision question".into());
        }
        if answer["type"] != kind {
            return Err(format!("missing or mismatched answer, expected {kind}"));
        }
        let (key, max, scale) = match self {
            Self::Choice(labels) => {
                let value = &answer["choice"];
                if !labels.contains(value) {
                    return Err("expected a declared choice label".into());
                }
                return Ok(value.clone());
            }
            Self::Noul if provider == Provider::OpenAi => ("probability", 1.0, None),
            Self::Noul => ("noul", 1.0, None),
            Self::Score { max, scale } => ("score", *max, *scale),
        };
        let value = answer[key]
            .as_f64()
            .filter(|x| (0.0..=max).contains(x))
            .ok_or_else(|| format!("{key} must be a number in [0, {max}]"))?;
        let value = if let Some((low, high)) = scale {
            // Avoid overflowing high - low for intervals spanning zero.
            let weight = value / max;
            low * (1.0 - weight) + high * weight
        } else {
            value
        };
        Ok(Value::from(value))
    }
}

fn with_examples(meaning: Value, examples: Vec<String>) -> Result<Value, String> {
    if !matches!(
        meaning,
        Value::String(_) | Value::Object(_) | Value::Array(_) | Value::Null
    ) {
        return Err("criteria must be strings, objects, or arrays".into());
    }
    if examples.is_empty() {
        Ok(meaning)
    } else {
        Ok(json!({"meaning": meaning, "examples": examples}))
    }
}

fn predicate_criteria(noul: &mut Noul) -> Result<Map<String, Value>, String> {
    if let Some(criteria) = noul.criteria.take() {
        if !noul.when_true.is_null() || !noul.when_false.is_null() {
            return Err("criteria cannot be combined with when_true or when_false".into());
        }
        let Value::Object(mut criteria) = criteria else {
            return Err("Noul criteria must be an object".into());
        };
        noul.when_true = criteria.remove("true").unwrap_or_default();
        noul.when_false = criteria.remove("false").unwrap_or_default();
        if !criteria.is_empty() {
            return Err("Noul criteria may contain only true and false keys".into());
        }
    }
    let mut criteria = Map::new();
    for (key, meaning, examples) in [
        ("true", &mut noul.when_true, &mut noul.positive),
        ("false", &mut noul.when_false, &mut noul.negative),
    ] {
        let value = with_examples(meaning.take(), std::mem::take(examples))?;
        if !value.is_null() {
            criteria.insert(key.into(), value);
        }
    }
    Ok(criteria)
}

fn build_question(
    provider: Provider,
    name: &str,
    spec: Spec,
    context: &Value,
) -> Result<(Value, Expected), String> {
    let openai = provider == Provider::OpenAi;
    let mut question = Map::new();
    let (mut instructions, expected) = match spec {
        Spec::Choice {
            instructions,
            options,
        } => {
            let min = if openai { 2 } else { 1 };
            if !(min..=255).contains(&options.len()) {
                return Err(format!("a choice requires {min} to 255 options"));
            }
            let mut labels = Vec::with_capacity(options.len());
            let mut choices = Vec::new();
            let mut criteria = Map::new();
            for option in options {
                if option.label == "" || labels.contains(&option.label) {
                    return Err("choice labels must be unique and nonempty".into());
                }
                let meaning = with_examples(option.meaning, option.examples)?;
                if openai {
                    let mut item = json!({"value": option.label});
                    if !meaning.is_null() {
                        item["description"] = text(meaning).into();
                    }
                    choices.push(item);
                } else {
                    let key = option
                        .label
                        .as_str()
                        .ok_or("boolean choice values require OpenAI")?;
                    criteria.insert(key.to_owned(), meaning);
                }
                labels.push(option.label);
            }
            if openai {
                question.insert("choices".into(), Value::Array(choices));
            } else {
                question.insert("criteria".into(), Value::Object(criteria));
            }
            (instructions, Expected::Choice(labels))
        }
        Spec::Noul(mut noul) => {
            let criteria = predicate_criteria(&mut noul)?;
            if !criteria.is_empty() {
                question.insert("criteria".into(), Value::Object(criteria));
            }
            (noul.instructions, Expected::Noul)
        }
        Spec::Score(mut score) => {
            if !(2..=10).contains(&score.levels.len()) {
                return Err("a score requires 2 to 10 levels".into());
            }
            if let Some((low, high)) = score.scale {
                if !low.is_finite() || !high.is_finite() || low >= high {
                    return Err("scale bounds must be finite and increasing".into());
                }
            }
            for (example, index) in score.examples {
                let level = score
                    .levels
                    .get_mut(index)
                    .ok_or_else(|| format!("invalid score example level index {index}"))?;
                level.examples.push(example);
            }
            let max = (score.levels.len() - 1) as f64;
            let mut levels = Vec::with_capacity(score.levels.len());
            for (i, level) in score.levels.into_iter().enumerate() {
                if level.meaning.is_null() {
                    return Err("score levels require a meaning".into());
                }
                let meaning = with_examples(level.meaning, level.examples)?;
                levels.push(if openai {
                    let label = level.label.unwrap_or_else(|| i.to_string());
                    json!({"label": label, "description": text(meaning)})
                } else {
                    meaning
                });
            }
            let key = if openai { "levels" } else { "criteria" };
            question.insert(key.into(), Value::Array(levels));
            (
                score.instructions,
                Expected::Score {
                    max,
                    scale: score.scale,
                },
            )
        }
    };
    let valid = match &instructions {
        Value::String(s) => !s.is_empty(),
        Value::Object(map) => !map.is_empty(),
        Value::Array(array) => !array.is_empty(),
        _ => false,
    };
    if !valid {
        return Err("instructions require a nonempty string, object, or array".into());
    }
    render(&mut instructions, context)?;
    if openai {
        question.insert("name".into(), name.into());
        instructions = openai_instructions(instructions, question.remove("criteria"))?;
    }
    question.insert("type".into(), expected.kind(provider).into());
    question.insert("instructions".into(), instructions);
    Ok((Value::Object(question), expected))
}

fn openai_instructions(mut instructions: Value, criteria: Option<Value>) -> Result<Value, String> {
    let criteria = criteria.map(|value| format!("Criteria: {}", text(value)));
    if let Some(parts) = instructions.as_array_mut() {
        let native = parts.iter().any(|part| {
            matches!(
                part["type"].as_str(),
                Some("input_text" | "input_image" | "input_json")
            )
        });
        if native {
            if let Some(text) = criteria {
                parts.push(json!({"type":"input_text", "text":text}));
            }
            for part in parts {
                if !part["prompt_cache_breakpoint"].is_null() {
                    return Err("place prompt cache breakpoints in shared input".into());
                }
                let valid = match part["type"].as_str() {
                    Some("input_text") => part["text"].is_string(),
                    Some("input_image") => part["image_url"].as_str().is_some_and(|s| {
                        s.starts_with("https://")
                            || s.starts_with("http://")
                            || s.starts_with("data:")
                    }),
                    Some("input_json") => {
                        part["data"].is_object()
                            || part["data"]
                                .as_array()
                                .is_some_and(|a| a.iter().all(Value::is_object))
                    }
                    _ => false,
                };
                if !valid {
                    return Err(
                        "expected input_text, input_image or input_json instruction parts".into(),
                    );
                }
            }
            return Ok(instructions);
        }
    }
    let mut description = text(instructions);
    if let Some(criteria) = criteria {
        description.push('\n');
        description.push_str(&criteria);
    }
    Ok(Value::String(description))
}

fn text(value: Value) -> String {
    match value {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

fn render(value: &mut Value, context: &Value) -> Result<(), String> {
    match value {
        Value::String(s) => {
            crate::render(s, context).map_err(|e| format!("invalid instruction template: {e}"))?;
        }
        Value::Array(items) => {
            for item in items {
                render(item, context)?;
            }
        }
        Value::Object(items) => {
            for item in items.values_mut() {
                render(item, context)?;
            }
        }
        _ => (),
    }
    Ok(())
}

/// A rendered body and its corresponding answer contract. Use this to
/// inspect a request offline or integrate a custom transport.
pub struct PreparedDecision<T> {
    body: Value,
    provider: Provider,
    fields: Vec<(String, Expected)>,
    // Jev's context is already present under body["state"].
    openai_context: Option<Value>,
    strict: bool,
    result: PhantomData<fn() -> T>,
}

impl<T: DecisionModel> PreparedDecision<T> {
    pub fn new(route: &str, request: impl Into<DecisionRequest>) -> Result<Self, DecisionError> {
        Self::prepare(&Route::new(route)?, request.into())
    }

    pub(super) fn prepare(route: &Route, request: DecisionRequest) -> Result<Self, DecisionError> {
        let openai = route.provider == Provider::OpenAi;
        if !request.context.is_object() {
            return Err(DecisionError::Request(
                "context must be a JSON object".into(),
            ));
        }
        if !request.openai_options.is_empty() {
            if !openai {
                return Err(DecisionError::Request(
                    "prompt cache and safety controls require OpenAI".into(),
                ));
            }
            for (key, limit) in [("prompt_cache_key", 64), ("safety_identifier", 128)] {
                if request
                    .openai_options
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.chars().nth(limit).is_some())
                {
                    return Err(DecisionError::Request(format!(
                        "{key} exceeds {limit} characters"
                    )));
                }
            }
        }
        if let Some(input) = &request.input {
            if !openai {
                return Err(DecisionError::Request(
                    "native input requires OpenAI, use context for Jev".into(),
                ));
            }
            if !input.is_string()
                && !input
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_object))
            {
                return Err(DecisionError::Request(
                    "input must be text or an array of native input objects".into(),
                ));
            }
        }

        let questions = T::questions();
        if questions.is_empty() {
            return Err(DecisionError::Request(
                "expected at least one decision question".into(),
            ));
        }
        if openai && questions.len() > 400 {
            return Err(DecisionError::Request(
                "OpenAI accepts up to 400 questions".into(),
            ));
        }
        let mut names = HashSet::new();
        let mut ordered = Vec::new();
        let mut named = Map::new();
        let mut fields = Vec::with_capacity(questions.len());
        for field in questions {
            let name = field.name;
            if name.is_empty() || !names.insert(name.clone()) {
                return Err(DecisionError::Request(
                    "question names must be unique and nonempty".into(),
                ));
            }
            let (question, expected) =
                build_question(route.provider, &name, field.spec, &request.context)
                    .map_err(|e| DecisionError::Request(format!("{name}: {e}")))?;
            if openai {
                ordered.push(question);
            } else {
                named.insert(name.clone(), question);
            }
            fields.push((name, expected));
        }

        let mut body = Value::Object(request.openai_options);
        body["model"] = route.model.clone().into();
        let openai_context = if openai {
            body["input"] = request
                .input
                .unwrap_or_else(|| Value::String(request.context.to_string()));
            body["questions"] = Value::Array(ordered);
            Some(request.context)
        } else {
            body["state"] = request.context;
            body["questions"] = Value::Object(named);
            None
        };
        Ok(Self {
            body,
            provider: route.provider,
            fields,
            openai_context,
            strict: request.strict,
            result: PhantomData,
        })
    }

    /// The complete JSON body used by the built-in clients.
    pub fn body(&self) -> &Value {
        &self.body
    }

    /// Validate answers against the exact questions sent, then run model
    /// validation against the original context. The raw value is borrowed.
    pub fn parse(&self, raw: &Value) -> Result<T, DecisionError> {
        let answers = &raw["answers"];
        let openai = self.provider == Provider::OpenAi;
        if openai {
            let array = answers
                .as_array()
                .ok_or_else(|| DecisionError::Answer("expected an answers array".into()))?;
            if array.len() != self.fields.len() {
                return Err(DecisionError::Answer(
                    "answer count differs from question count".into(),
                ));
            }
        } else if !answers.is_object() {
            return Err(DecisionError::Answer("expected an answers object".into()));
        }

        let mut values = Map::new();
        for (i, (name, expected)) in self.fields.iter().enumerate() {
            let answer = if openai {
                let answer = &answers[i];
                if answer["name"].as_str() != Some(name) {
                    return Err(DecisionError::Answer(format!(
                        "{name}: missing or mismatched answer name"
                    )));
                }
                answer
            } else {
                &answers[name]
            };
            let value = expected
                .parse(self.provider, answer)
                .map_err(|e| DecisionError::Answer(format!("{name}: {e}")))?;
            values.insert(name.clone(), value);
        }
        let mut result = T::from_answers(Answers(values))?;
        let context = self.openai_context.as_ref().unwrap_or(&self.body["state"]);
        result
            .validate_decision(context, self.strict)
            .map_err(DecisionError::Validation)?;
        Ok(result)
    }
}
