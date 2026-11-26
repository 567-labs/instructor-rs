use super::DecisionError;
use serde_json::{Map, Value};
use std::marker::PhantomData;

/// A closed set of answers. Derive this on a unit enum to generate both
/// provider metadata and the mapping from wire labels to enum variants.
pub trait Choices: Sized {
    fn validate_choice(&mut self, _context: &Value) -> Result<(), String> {
        Ok(())
    }
    fn instructions() -> &'static str;
    fn options() -> Vec<Choice>;
    fn from_choice(label: &str) -> Option<Self>;
}

/// One named answer with optional criteria and examples.
#[derive(Debug, Clone)]
pub struct Choice {
    pub(super) label: String,
    pub(super) meaning: Value,
    pub(super) examples: Vec<String>,
}

impl Choice {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            meaning: Value::Null,
            examples: Vec::new(),
        }
    }

    pub fn description(mut self, meaning: impl Into<Value>) -> Self {
        self.meaning = meaning.into();
        self
    }

    pub fn example(mut self, example: impl ToString) -> Self {
        self.examples.push(example.to_string());
        self
    }
}

impl From<&str> for Choice {
    fn from(label: &str) -> Self {
        Self::new(label)
    }
}

impl From<String> for Choice {
    fn from(label: String) -> Self {
        Self::new(label)
    }
}

/// A binary question whose answer is the probability of true.
#[derive(Debug, Clone)]
pub struct Noul {
    pub(super) instructions: Value,
    pub(super) when_true: Value,
    pub(super) when_false: Value,
    pub(super) criteria: Option<Value>,
    pub(super) positive: Vec<String>,
    pub(super) negative: Vec<String>,
}

impl Noul {
    pub fn new(instructions: impl Into<Value>) -> Self {
        Self {
            instructions: instructions.into(),
            when_true: Value::Null,
            when_false: Value::Null,
            criteria: None,
            positive: Vec::new(),
            negative: Vec::new(),
        }
    }

    pub fn when_true(mut self, meaning: impl Into<Value>) -> Self {
        self.when_true = meaning.into();
        self
    }

    pub fn when_false(mut self, meaning: impl Into<Value>) -> Self {
        self.when_false = meaning.into();
        self
    }

    /// Supply native true/false criteria as a JSON object.
    pub fn criteria(mut self, criteria: Value) -> Self {
        self.criteria = Some(criteria);
        self
    }

    pub fn example(mut self, example: impl ToString, label: bool) -> Self {
        if label {
            self.positive.push(example.to_string());
        } else {
            self.negative.push(example.to_string());
        }
        self
    }
}

/// One position in an ordered score rubric.
#[derive(Debug, Clone)]
pub struct Level {
    pub(super) meaning: Value,
    pub(super) examples: Vec<String>,
}

impl Level {
    pub fn new(meaning: impl Into<Value>) -> Self {
        Self {
            meaning: meaning.into(),
            examples: Vec::new(),
        }
    }

    pub fn example(mut self, example: impl ToString) -> Self {
        self.examples.push(example.to_string());
        self
    }
}

impl<T: Into<Value>> From<T> for Level {
    fn from(meaning: T) -> Self {
        Self::new(meaning)
    }
}

/// An ordered rubric with 2 to 10 levels. Scale applies to the typed
/// result after the provider returns its score in level-index units.
#[derive(Debug, Clone)]
pub struct Score {
    pub(super) instructions: Value,
    pub(super) levels: Vec<Level>,
    pub(super) scale: Option<(f64, f64)>,
    pub(super) examples: Vec<(String, usize)>,
}

impl Score {
    pub fn new<L: Into<Level>>(
        instructions: impl Into<Value>,
        levels: impl IntoIterator<Item = L>,
    ) -> Self {
        Self {
            instructions: instructions.into(),
            levels: levels.into_iter().map(Into::into).collect(),
            scale: None,
            examples: Vec::new(),
        }
    }

    pub fn scale(mut self, low: f64, high: f64) -> Self {
        self.scale = Some((low, high));
        self
    }

    pub fn example(mut self, example: impl ToString, level: usize) -> Self {
        self.examples.push((example.to_string(), level));
        self
    }
}

pub(super) enum Spec {
    Choice {
        instructions: Value,
        options: Vec<Choice>,
    },
    Noul(Noul),
    Score(Score),
}

/// A question bound to its output type. The model derive checks this
/// association at compile time.
pub struct Question<T> {
    spec: Spec,
    result: PhantomData<fn() -> T>,
}

impl<T> Question<T> {
    fn from_spec(spec: Spec) -> Self {
        Self {
            spec,
            result: PhantomData,
        }
    }

    pub fn named(self, name: impl Into<String>) -> NamedQuestion {
        NamedQuestion {
            name: name.into(),
            spec: self.spec,
        }
    }
}

impl<T: Choices> Question<T> {
    /// Ask a question using this enum's options.
    pub fn new(instructions: impl Into<Value>) -> Self {
        Self::from_spec(Spec::Choice {
            instructions: instructions.into(),
            options: T::options(),
        })
    }

    pub fn from_enum() -> Self {
        Self::new(T::instructions())
    }
}

impl<T: Choices> From<&str> for Question<T> {
    fn from(instructions: &str) -> Self {
        Self::new(instructions)
    }
}

impl Question<String> {
    /// Use inline labels or rich choices and return a validated string.
    pub fn choices<C: Into<Choice>>(
        instructions: impl Into<Value>,
        options: impl IntoIterator<Item = C>,
    ) -> Self {
        Self::from_spec(Spec::Choice {
            instructions: instructions.into(),
            options: options.into_iter().map(Into::into).collect(),
        })
    }
}

impl From<Noul> for Question<f64> {
    fn from(noul: Noul) -> Self {
        Self::from_spec(Spec::Noul(noul))
    }
}

impl From<Score> for Question<f64> {
    fn from(score: Score) -> Self {
        Self::from_spec(Spec::Score(score))
    }
}

/// A question associated with its model field name.
pub struct NamedQuestion {
    pub(super) name: String,
    pub(super) spec: Spec,
}

/// Derive on a named struct to generate questions and construct a typed
/// result from validated answers.
pub trait DecisionModel: Sized {
    fn questions() -> Vec<NamedQuestion>;
    fn from_answers(answers: Answers) -> Result<Self, DecisionError>;

    /// Runs after wire validation and scaling. The original context and
    /// request's strict flag are available to application validation.
    fn validate_decision(&mut self, _context: &Value, _strict: bool) -> Result<(), String> {
        Ok(())
    }
}

/// Conversion from a validated decision answer to a Rust field.
pub trait DecisionValue: Sized {
    fn validate_value(&mut self, _context: &Value) -> Result<(), String> {
        Ok(())
    }
    fn decode(answer: Value) -> Option<Self>;
}

impl<T: Choices> DecisionValue for T {
    fn validate_value(&mut self, context: &Value) -> Result<(), String> {
        self.validate_choice(context)
    }
    fn decode(answer: Value) -> Option<Self> {
        T::from_choice(answer.as_str()?)
    }
}

impl DecisionValue for String {
    fn decode(answer: Value) -> Option<Self> {
        match answer {
            Value::String(label) => Some(label),
            _ => None,
        }
    }
}

impl DecisionValue for f64 {
    fn decode(answer: Value) -> Option<Self> {
        answer.as_f64()
    }
}

/// Validated answers keyed by Rust field name.
pub struct Answers(pub(super) Map<String, Value>);

impl Answers {
    pub fn take<T: DecisionValue>(&mut self, name: &str) -> Result<T, DecisionError> {
        self.0
            .remove(name)
            .and_then(T::decode)
            .ok_or_else(|| DecisionError::Answer(format!("{name}: answer cannot be decoded")))
    }
}
