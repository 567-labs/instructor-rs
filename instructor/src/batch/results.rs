use super::*;

/// An individual provider or validation failure inside a finished job.
#[derive(Debug, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct BatchFailure {
    pub kind: String,
    pub message: String,
}

impl BatchFailure {
    fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
    fn from_provider(error: &Value, default_kind: &str) -> Self {
        let error = error.get("error").filter(|e| !e.is_null()).unwrap_or(error);
        let label = |key| {
            error
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        };
        let kind = label("type")
            .or_else(|| label("code"))
            .unwrap_or(default_kind);
        let message = error
            .as_str()
            .or_else(|| label("message"))
            .map(str::to_owned)
            .unwrap_or_else(|| error.to_string());
        Self::new(kind, message)
    }
}

impl From<Error> for BatchFailure {
    fn from(error: Error) -> Self {
        let kind = match error {
            Error::Incomplete(_) => "incomplete_output",
            Error::Refusal(_) => "refusal",
            Error::Parse(_) | Error::Validation(_) => "parsing_error",
            _ => "extraction_error",
        };
        Self::new(kind, error.to_string())
    }
}

/// A complete JSONL record plus its typed result or a record-local failure.
#[derive(Debug)]
pub struct BatchResult<T> {
    pub custom_id: Option<String>,
    pub raw: Value,
    pub result: Result<T, BatchFailure>,
}

impl<T: ResponseModel> Batch<T> {
    /// Retrieve a finished job and decode each record independently.
    pub async fn results(&self, id: &str) -> Result<Vec<BatchResult<T>>, Error> {
        Ok(self.parse(&self.results_raw(id).await?))
    }

    pub fn parse(&self, jsonl: &str) -> Vec<BatchResult<T>> {
        self.parse_with_context(jsonl, |_| Value::Null)
    }

    /// Look up each validation context by ID, regardless of output order.
    pub fn parse_with_context(
        &self,
        jsonl: &str,
        mut context: impl FnMut(&str) -> Value,
    ) -> Vec<BatchResult<T>> {
        jsonl
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let raw: Value = match serde_json::from_str(line) {
                    Ok(raw) => raw,
                    Err(e) => {
                        return BatchResult {
                            custom_id: None,
                            raw: json!({"raw_line":line}),
                            result: Err(BatchFailure::new("json_parse_error", e.to_string())),
                        }
                    }
                };
                let custom_id = raw["custom_id"].as_str().map(str::to_owned);
                let result = self.payload(&raw).and_then(|body| {
                    let ctx = custom_id.as_deref().map(&mut context).unwrap_or_default();
                    self.decode(body, &ctx).map_err(Into::into)
                });
                BatchResult {
                    custom_id,
                    raw,
                    result,
                }
            })
            .collect()
    }

    fn payload<'a>(&self, record: &'a Value) -> Result<&'a Value, BatchFailure> {
        if self.provider() == Provider::Anthropic {
            let result = &record["result"];
            return match result["type"].as_str() {
                Some("succeeded") => Ok(&result["message"]),
                Some("errored" | "error") => Err(BatchFailure::from_provider(
                    &result["error"],
                    "anthropic_error",
                )),
                Some(other) => Err(BatchFailure::new(other, format!("batch request {other}"))),
                None => Err(BatchFailure::new(
                    "extraction_error",
                    "missing Anthropic result",
                )),
            };
        }
        let response = &record["response"];
        let body = &response["body"];
        let kind = if self.provider() == Provider::OpenAI {
            "openai_error"
        } else {
            "mistral_error"
        };
        for error in [&record["error"], &body["error"]] {
            if !error.is_null() {
                return Err(BatchFailure::from_provider(error, kind));
            }
        }
        if let Some(status) = response["status_code"].as_i64() {
            if !(200..300).contains(&status) {
                return Err(BatchFailure::new(kind, format!("HTTP {status} error")));
            }
        }
        Ok(body)
    }

    fn decode(&self, body: &Value, context: &Value) -> Result<T, Error> {
        if self.provider() != Provider::Anthropic || self.client.config.mode() != Mode::Tools {
            return self.client.parse(body, &self.schema, context);
        }
        crate::providers::check_finish(&body["stop_reason"])?;
        let content = body["content"]
            .as_array()
            .ok_or_else(|| Error::Protocol("missing message content".into()))?;
        if let Some(tool) = content.iter().find(|c| c["type"] == "tool_use") {
            let input = tool
                .get("input")
                .ok_or_else(|| Error::Protocol("missing tool input".into()))?;
            return self.schema.parse(&input.to_string(), context);
        }
        // Some historical jobs contain text blocks instead of tool calls.
        for text in content
            .iter()
            .filter(|c| c["type"] == "text")
            .filter_map(|c| c["text"].as_str())
        {
            if serde_json::from_str::<Value>(text).is_ok() {
                return self.schema.parse(text, context);
            }
        }
        Err(Error::Protocol("missing tool input or JSON text".into()))
    }
}
