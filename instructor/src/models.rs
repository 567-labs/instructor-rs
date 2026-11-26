//! Optional extractions and quotations grounded in request context.
use std::ops::Range;

use serde_json::Value;

use crate::model;

/// A successful request can report that there is too little evidence to
/// extract a value. Request failures still use the client's Result.
#[model]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maybe<T> {
    /// The extracted value, if one was found.
    pub result: Option<T>,
    /// Whether the model encountered an extraction problem.
    #[serde(default)]
    pub error: bool,
    /// A short explanation if no value was found.
    pub message: Option<String>,
}

impl<T> Maybe<T> {
    /// Corresponds to the truth value of Python's Maybe instance.
    pub fn has_result(&self) -> bool {
        self.result.is_some()
    }
}

/// Quotes describing the evidence for a response. Embed with
/// `#[serde(flatten)]` for Python CitationMixin's flat wire format.
#[model]
#[derive(Debug, Clone, PartialEq, Eq)]
#[validate(context = "Citations::validate_sources")]
pub struct Citations {
    /// List of unique, specific substrings from the source used to answer the question.
    pub substring_quotes: Vec<String>,
}

impl Citations {
    /// Find one best matching range for each quote in the source. The
    /// returned half-open UTF-8 byte ranges can index the original string.
    pub fn get_spans<'a>(&'a self, source: &'a str) -> impl Iterator<Item = Range<usize>> + 'a {
        self.substring_quotes
            .iter()
            .filter_map(move |quote| span(quote, source))
    }

    /// Search each source separately and replace proposed quotes with exact
    /// source substrings. Without a context entry, keep the proposed text.
    pub fn validate_sources(&mut self, context: &Value) -> Result<(), String> {
        const INVALID: &str = "Citation context must be text or a list of text chunks";
        let sources = match context.get("context") {
            None | Some(Value::Null) => return Ok(()),
            Some(Value::String(s)) => vec![s.as_str()],
            Some(Value::Array(chunks)) => chunks
                .iter()
                .map(|s| s.as_str().ok_or(INVALID))
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err(INVALID.to_owned()),
        };
        self.substring_quotes = sources
            .into_iter()
            .flat_map(|source| self.get_spans(source).map(move |i| source[i].to_owned()))
            .collect();
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Alignment {
    edits: usize,
    end: usize,
}

impl Alignment {
    fn cost(self, edits: usize) -> Self {
        Self {
            edits: self.edits + edits,
            ..self
        }
    }

    fn prefer(self, other: Self) -> Self {
        if self.edits <= other.edits {
            self
        } else {
            other
        }
    }
}

fn span(quote: &str, source: &str) -> Option<Range<usize>> {
    if quote.is_empty() || source.is_empty() {
        return None;
    }
    if let Some(start) = source.find(quote) {
        return Some(start..start + quote.len());
    }
    let pattern: Vec<_> = quote.chars().collect();
    let mut limit = 5.min(pattern.len() / 3);
    if limit == 0 || pattern.len() > source.len().saturating_add(limit) {
        return None;
    }
    // Semi-global Levenshtein alignment in Unicode scalar values. A cell
    // stores the edits and end offset to align a pattern suffix at the
    // current source offset. Walk back to prefer the leftmost best match.
    let mut row: Vec<_> = (0..=pattern.len())
        .map(|i| Alignment {
            edits: pattern.len() - i,
            end: source.len(),
        })
        .collect();
    let mut best = None;
    for (start, ch) in source.char_indices().rev() {
        let mut diagonal = row[pattern.len()];
        row[pattern.len()] = Alignment {
            edits: 0,
            end: start,
        };
        for i in (0..pattern.len()).rev() {
            let insert = row[i];
            let delete = row[i + 1];
            row[i] = diagonal
                .cost(usize::from(pattern[i] != ch))
                .prefer(insert.cost(1))
                .prefer(delete.cost(1));
            diagonal = insert;
        }
        if row[0].edits <= limit {
            best = Some(start..row[0].end);
            limit = row[0].edits;
        }
    }
    best
}
