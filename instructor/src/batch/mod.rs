//! Provider-managed jobs over batches of typed requests.
//!
//! ```no_run
//! use instructor_ai::{model, Client};
//!
//! #[model]
//! struct Ticket { topic: String }
//!
//! # async fn run() -> Result<(), instructor_ai::Error> {
//! let client = Client::from_provider("openai/gpt-6-luna")?;
//! let batch = client.batch::<Ticket>()?;
//! let requests = batch.prepare(["Double charged", "Cannot log in"])?;
//! let id = batch.submit(&requests).await?;
//! // Store the ID and check again later.
//! if batch.status(&id).await?.status == "completed" {
//!     for record in batch.results(&id).await? {
//!         match record.result {
//!             Ok(ticket) => println!("{:?}: {}", record.custom_id, ticket.topic),
//!             Err(error) => eprintln!("{:?}: {}", record.custom_id, error),
//!         }
//!     }
//! }
//! # Ok(())
//! # }
//! ```

mod blocking;
mod jobs;
mod results;

pub use blocking::BlockingBatch;
pub use results::{BatchFailure, BatchResult};

use crate::{
    schema::{schema, Schema},
    Client, Error, Mode, Provider, Request, ResponseModel,
};
use serde_json::{json, Value};
use std::{collections::HashSet, io::Write, marker::PhantomData, path::Path, sync::Arc};

/// Provider-specific options for job creation.
#[derive(Debug, Clone, Default)]
pub struct BatchOptions {
    /// Job metadata for OpenAI or Mistral.
    pub metadata: Option<Value>,
    /// OpenAI completion window, default 24h.
    pub completion_window: Option<String>,
}

/// Job metadata including the original provider object.
#[derive(Debug, Clone)]
pub struct BatchJob {
    pub id: String,
    pub status: String,
    pub request_counts: Value,
    pub raw: Value,
}

/// One conversation and its optional caller-assigned identifier.
pub struct BatchItem {
    request: Request,
    id: Option<String>,
}

impl BatchItem {
    pub fn new(request: impl Into<Request>) -> Self {
        Self {
            request: request.into(),
            id: None,
        }
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

impl<R: Into<Request>> From<R> for BatchItem {
    fn from(request: R) -> Self {
        Self::new(request)
    }
}

/// Async batch processor bound to one provider, model type, and output mode.
pub struct Batch<T> {
    client: Client,
    schema: Arc<Schema>,
    strict: bool,
    model: PhantomData<fn() -> T>,
}

impl Client {
    /// OpenAI and Mistral use native JSON schemas. Anthropic uses a named tool.
    pub fn batch<T: ResponseModel>(&self) -> Result<Batch<T>, Error> {
        let mode = if self.config.provider() == Provider::Anthropic {
            Mode::Tools
        } else {
            Mode::JsonSchema
        };
        self.batch_with_mode(mode)
    }

    pub fn batch_with_mode<T: ResponseModel>(&self, mode: Mode) -> Result<Batch<T>, Error> {
        if !matches!(
            self.config.provider(),
            Provider::OpenAI | Provider::Anthropic | Provider::Mistral
        ) {
            return Err(Error::Configuration(format!(
                "{} has no batch processor",
                self.config.provider()
            )));
        }
        let mut client = self.clone();
        client.config = client.config.with_mode(mode);
        client.config.validate()?;
        Ok(Batch {
            client,
            schema: schema::<T>(),
            strict: true,
            model: PhantomData,
        })
    }
}

impl<T: ResponseModel> Batch<T> {
    fn provider(&self) -> Provider {
        self.client.config.provider()
    }

    /// Set schema strictness for the OpenAI or Mistral JSON schema mode.
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    fn validate_id(&self, seen: &mut HashSet<String>, id: &str) -> Result<(), Error> {
        if id.trim().is_empty() || !seen.insert(id.into()) {
            return Err(Error::Configuration(format!(
                "empty or duplicate batch custom ID: {id:?}"
            )));
        }
        if self.provider() == Provider::Anthropic
            && (id.len() > 64
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
        {
            return Err(Error::Configuration(format!(
                "invalid Anthropic custom ID {id:?}"
            )));
        }
        Ok(())
    }

    fn endpoint(&self) -> &'static str {
        if self.client.config.mode() == Mode::ResponsesTools {
            "/v1/responses"
        } else {
            "/v1/chat/completions"
        }
    }

    fn request(&self, req: Request, id: String) -> Result<Value, Error> {
        let config = &self.client.config;
        let mut req = req.prepare(false)?;
        if !req.hooks.is_empty() {
            return Err(Error::Configuration(
                "batch jobs cannot run per-request hooks".into(),
            ));
        }
        let opts = &mut req.options;
        if !opts.contains_key("max_completion_tokens") && !opts.contains_key("max_output_tokens") {
            opts.entry("max_tokens").or_insert(json!(1000));
        }
        if self.provider() == Provider::OpenAI
            && config.model() == "gpt-6-luna"
            && config.mode() != Mode::ResponsesTools
        {
            if let Some(limit) = opts.remove("max_tokens") {
                opts.entry("max_completion_tokens").or_insert(limit);
            }
            opts.entry("reasoning_effort").or_insert(json!("none"));
        }
        if self.provider() != Provider::Anthropic {
            opts.entry("temperature").or_insert(json!(0.1));
        }
        if opts.get("temperature") == Some(&Value::Null) {
            opts.remove("temperature");
        }
        let batch_schema = config.mode() == Mode::JsonSchema
            && matches!(self.provider(), Provider::OpenAI | Provider::Mistral);
        let strict = config
            .schema_strictness(&req)?
            .or(batch_schema.then_some(self.strict));
        let mut body = config
            .request_with_strictness(&req, Some(&self.schema.value), false, strict)?
            .body;
        if batch_schema && strict == Some(false) {
            body["response_format"]["json_schema"]["strict"] = false.into();
        }
        let fields = body.as_object_mut().expect("request body is an object");
        fields.remove("stream");
        let mut record = json!({});
        record["custom_id"] = id.into();
        match self.provider() {
            Provider::Anthropic => {
                if config.mode() == Mode::Tools {
                    let tool = &mut body["tools"][0];
                    tool["name"] = "extract_data".into();
                    tool["description"] =
                        format!("Extract data matching the {} schema", T::model_name()).into();
                    // Restore the root metadata omitted from ordinary tool parameters.
                    let schema = &mut tool["input_schema"];
                    for key in ["title", "description"] {
                        if let Some(value) = self.schema.value.get(key) {
                            schema[key] = value.clone();
                        }
                    }
                    if schema.get("additionalProperties").is_none() {
                        schema["additionalProperties"] = false.into();
                    }
                    if !req.options.contains_key("tool_choice")
                        && body["tool_choice"]["type"] == "tool"
                    {
                        body["tool_choice"] = json!({"type":"tool","name":"extract_data"});
                    }
                }
                record["params"] = body;
            }
            Provider::Mistral => {
                fields.remove("model");
                record["body"] = body;
            }
            _ => {
                record["method"] = "POST".into();
                record["url"] = self.endpoint().into();
                record["body"] = body;
            }
        }
        Ok(record)
    }

    /// Build a complete JSONL payload in memory, including the final newline.
    pub fn prepare<R: Into<BatchItem>>(
        &self,
        requests: impl IntoIterator<Item = R>,
    ) -> Result<String, Error> {
        let mut seen = HashSet::new();
        let mut lines = Vec::new();
        for (index, item) in requests.into_iter().enumerate() {
            let item = item.into();
            let id = item.id.unwrap_or_else(|| format!("request-{index}"));
            self.validate_id(&mut seen, &id)?;
            serde_json::to_writer(&mut lines, &self.request(item.request, id)?)?;
            lines.push(b'\n');
        }
        if seen.is_empty() {
            return Err(Error::Configuration(
                "batch requires at least one item".into(),
            ));
        }
        Ok(String::from_utf8(lines).expect("serde_json writes UTF-8"))
    }

    /// Atomically replace a request file after preparing and validating all items.
    pub fn prepare_file<R: Into<BatchItem>>(
        &self,
        requests: impl IntoIterator<Item = R>,
        path: impl AsRef<Path>,
    ) -> Result<(), Error> {
        let text = self.prepare(requests)?;
        let path = path.as_ref();
        let mut file = temporary_file(path)?;
        file.write_all(text.as_bytes())
            .map_err(|e| file_error(path, e))?;
        replace_file(file, path)
    }
}

fn file_error(path: &Path, error: std::io::Error) -> Error {
    Error::Configuration(format!("writing {}: {error}", path.display()))
}

fn temporary_file(path: &Path) -> Result<tempfile::NamedTempFile, Error> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    tempfile::NamedTempFile::new_in(parent).map_err(|e| file_error(path, e))
}

fn replace_file(file: tempfile::NamedTempFile, path: &Path) -> Result<(), Error> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.is_file() {
            file.as_file()
                .set_permissions(metadata.permissions())
                .map_err(|e| file_error(path, e))?;
        }
    }
    file.persist(path).map_err(|e| file_error(path, e.error))?;
    Ok(())
}
