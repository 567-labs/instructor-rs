//! Provider token accounting for typed, non-streaming requests.

use crate::{Error, Provider};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Token counts for one response or for every attempt of an operation.
///
/// Counts are inclusive. Cache reads and writes are part of input, reasoning
/// is part of output, and the lifetime-specific counts are part of writes.
/// Each counter is available independently. A missing count on any attempt
/// makes the corresponding cumulative count unknown. Sums saturate at
/// `u64::MAX`. `Default` represents unknown usage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Provider total when supplied, otherwise derived from input and output.
    pub total_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub cache_write_5m_tokens: Option<u64>,
    pub cache_write_1h_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

fn sum(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    a.zip(b).map(|(a, b)| a.saturating_add(b))
}

// Anthropic omits optional cache counters for uncached prompts. Google
// may omit thought counts when thinking is disabled. For constructing
// inclusive counts, only add these components when the provider sends them.
fn optional_component(value: &Value) -> Option<u64> {
    if value.is_null() {
        Some(0)
    } else {
        value.as_u64()
    }
}

impl TokenUsage {
    /// Zero cost of serving a validated response from a local cache.
    pub const ZERO: Self = Self {
        input_tokens: Some(0),
        output_tokens: Some(0),
        total_tokens: Some(0),
        cache_read_tokens: Some(0),
        cache_write_tokens: Some(0),
        cache_write_5m_tokens: Some(0),
        cache_write_1h_tokens: Some(0),
        reasoning_tokens: Some(0),
    };

    /// Normalize the usage in a single native response envelope.
    pub fn from_response(provider: Provider, raw: &Value) -> Self {
        let usage = &raw["usage"];
        let mut counts = match provider {
            Provider::OpenAI | Provider::OpenRouter | Provider::Mistral => {
                let input_details = usage
                    .get("prompt_tokens_details")
                    .unwrap_or(&usage["input_tokens_details"]);
                let output_details = usage
                    .get("completion_tokens_details")
                    .unwrap_or(&usage["output_tokens_details"]);
                Self {
                    input_tokens: usage["prompt_tokens"]
                        .as_u64()
                        .or_else(|| usage["input_tokens"].as_u64()),
                    output_tokens: usage["completion_tokens"]
                        .as_u64()
                        .or_else(|| usage["output_tokens"].as_u64()),
                    total_tokens: usage["total_tokens"].as_u64(),
                    cache_read_tokens: input_details["cached_tokens"].as_u64(),
                    cache_write_tokens: input_details["cache_write_tokens"].as_u64(),
                    reasoning_tokens: output_details["reasoning_tokens"].as_u64(),
                    ..Self::default()
                }
            }
            Provider::Anthropic => {
                let cache = &usage["cache_creation"];
                let five_minutes = cache["ephemeral_5m_input_tokens"].as_u64();
                let one_hour = cache["ephemeral_1h_input_tokens"].as_u64();
                let read = &usage["cache_read_input_tokens"];
                let write = &usage["cache_creation_input_tokens"];
                let cache_write_tokens = write.as_u64().or_else(|| sum(five_minutes, one_hour));
                let writes = if write.is_null() && cache.is_null() {
                    Some(0)
                } else {
                    cache_write_tokens
                };
                Self {
                    input_tokens: sum(
                        usage["input_tokens"].as_u64(),
                        sum(optional_component(read), writes),
                    ),
                    output_tokens: usage["output_tokens"].as_u64(),
                    cache_read_tokens: read.as_u64(),
                    cache_write_tokens,
                    cache_write_5m_tokens: five_minutes,
                    cache_write_1h_tokens: one_hour,
                    reasoning_tokens: usage["output_tokens_details"]["thinking_tokens"].as_u64(),
                    ..Self::default()
                }
            }
            Provider::Google => {
                let usage = &raw["usageMetadata"];
                let thoughts = &usage["thoughtsTokenCount"];
                Self {
                    input_tokens: usage["promptTokenCount"].as_u64(),
                    output_tokens: sum(
                        usage["candidatesTokenCount"].as_u64(),
                        optional_component(thoughts),
                    ),
                    total_tokens: usage["totalTokenCount"].as_u64(),
                    cache_read_tokens: usage["cachedContentTokenCount"].as_u64(),
                    reasoning_tokens: thoughts.as_u64(),
                    ..Self::default()
                }
            }
            Provider::Cohere => Self {
                input_tokens: usage["tokens"]["input_tokens"].as_u64(),
                output_tokens: usage["tokens"]["output_tokens"].as_u64(),
                ..Self::default()
            },
        };
        counts.total_tokens = counts
            .total_tokens
            .or_else(|| sum(counts.input_tokens, counts.output_tokens));
        counts
    }

    pub(crate) fn combine(self, other: Self) -> Self {
        Self {
            input_tokens: sum(self.input_tokens, other.input_tokens),
            output_tokens: sum(self.output_tokens, other.output_tokens),
            total_tokens: sum(self.total_tokens, other.total_tokens),
            cache_read_tokens: sum(self.cache_read_tokens, other.cache_read_tokens),
            cache_write_tokens: sum(self.cache_write_tokens, other.cache_write_tokens),
            cache_write_5m_tokens: sum(self.cache_write_5m_tokens, other.cache_write_5m_tokens),
            cache_write_1h_tokens: sum(self.cache_write_1h_tokens, other.cache_write_1h_tokens),
            reasoning_tokens: sum(self.reasoning_tokens, other.reasoning_tokens),
        }
    }
}

impl Error {
    /// Recover cumulative counts from an [`Error::Retry`] history, using the
    /// provider that made the request. Returns `None` for errors without history.
    pub fn retry_usage(&self, provider: Provider) -> Option<TokenUsage> {
        let Error::Retry { attempts, .. } = self else {
            return None;
        };
        Some(attempts.iter().fold(TokenUsage::ZERO, |counts, failed| {
            counts.combine(TokenUsage::from_response(provider, &failed.completion))
        }))
    }
}
