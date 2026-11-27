use super::*;
use crate::providers::send;
use reqwest::{
    multipart::{Form, Part},
    Method, RequestBuilder,
};
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};

const BLOCK_SIZE: usize = 64 * 1024;

fn file_path(id: &str) -> Result<String, Error> {
    Ok(format!("/files/{}/content", segment(id)?))
}

fn invalid_utf8(error: std::str::Utf8Error) -> Error {
    Error::Protocol(format!("invalid UTF-8 in batch results: {error}"))
}

async fn write_block(
    output: &mut (impl AsyncWrite + Unpin),
    bytes: &mut Vec<u8>,
    eof: bool,
) -> Result<(), Error> {
    let len = match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        // At most three bytes of a character can span two blocks.
        Err(err) if !eof && err.error_len().is_none() => err.valid_up_to(),
        Err(err) => return Err(invalid_utf8(err)),
    };
    output.write_all(&bytes[..len]).await?;
    bytes.drain(..len);
    Ok(())
}

fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, Error> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Protocol(format!("missing {key} in batch response")))
}

fn segment(id: &str) -> Result<&str, Error> {
    if id.is_empty()
        || matches!(id, "." | "..")
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
    {
        return Err(Error::Configuration("invalid batch or file ID".into()));
    }
    Ok(id)
}

async fn json_response(request: RequestBuilder) -> Result<Value, Error> {
    Ok(send(request).await?.json().await?)
}

impl<T: ResponseModel> Batch<T> {
    fn api(&self, method: Method, path: &str) -> Result<RequestBuilder, Error> {
        self.client
            .config
            .authenticated(&self.client.http, method, path)
    }

    fn root(&self) -> &'static str {
        match self.provider() {
            Provider::Anthropic => "/messages/batches",
            Provider::Mistral => "/batch/jobs",
            _ => "/batches",
        }
    }

    fn path(&self, id: &str) -> Result<String, Error> {
        Ok(format!("{}/{}", self.root(), segment(id)?))
    }

    fn validate_lines(&self, jsonl: &str) -> Result<Vec<Value>, Error> {
        let mut seen = HashSet::new();
        let mut records = Vec::new();
        for (index, line) in jsonl.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(line)
                .map_err(|e| Error::Configuration(format!("batch line {}: {e}", index + 1)))?;
            let id = value["custom_id"].as_str().ok_or_else(|| {
                Error::Configuration(format!("missing custom ID on line {}", index + 1))
            })?;
            self.validate_id(&mut seen, id)?;
            let anthropic = self.provider() == Provider::Anthropic;
            let body = &value[if anthropic { "params" } else { "body" }];
            if !body.is_object() {
                return Err(Error::Configuration(format!(
                    "invalid request body for {id}"
                )));
            }
            if self.provider() == Provider::OpenAI
                && (value["method"] != "POST" || value["url"] != self.endpoint())
            {
                return Err(Error::Configuration(format!(
                    "wrong batch endpoint for {id}"
                )));
            }
            // Only Anthropic embeds the parsed records directly in its POST.
            if anthropic {
                records.push(value);
            }
        }
        if seen.is_empty() {
            return Err(Error::Configuration("empty batch payload".into()));
        }
        Ok(records)
    }

    /// Submit a prepared JSONL payload. Returns the provider's job ID.
    pub async fn submit(&self, jsonl: &str) -> Result<String, Error> {
        self.submit_with_options(jsonl, BatchOptions::default())
            .await
    }

    pub async fn submit_with_options(
        &self,
        jsonl: &str,
        options: BatchOptions,
    ) -> Result<String, Error> {
        let requests = self.validate_lines(jsonl)?;
        if options.completion_window.is_some() && self.provider() != Provider::OpenAI {
            return Err(Error::Configuration(
                "completion_window requires OpenAI".into(),
            ));
        }
        if options
            .metadata
            .as_ref()
            .is_some_and(|meta| !meta.is_object())
        {
            return Err(Error::Configuration(
                "batch metadata must be an object".into(),
            ));
        }
        let body = if self.provider() == Provider::Anthropic {
            if options.metadata.is_some() {
                return Err(Error::Configuration(
                    "Anthropic batches do not accept metadata".into(),
                ));
            }
            let mut body = json!({});
            body["requests"] = Value::Array(requests);
            body
        } else {
            let file = Part::bytes(jsonl.as_bytes().to_vec())
                .file_name("batch.jsonl")
                .mime_str("application/jsonl")?;
            let form = Form::new().text("purpose", "batch").part("file", file);
            let uploaded = json_response(self.api(Method::POST, "/files")?.multipart(form)).await?;
            let id = text(&uploaded, "id")?;
            if self.provider() == Provider::Mistral {
                json!({"input_files":[id],"model":self.client.config.model(),"endpoint":self.endpoint(),
                    "metadata":options.metadata.unwrap_or_else(||json!({}))})
            } else {
                json!({"input_file_id":id,"endpoint":self.endpoint(),
                    "completion_window":options.completion_window.as_deref().unwrap_or("24h"),
                    "metadata":options.metadata.unwrap_or_else(||json!({"description":"Instructor batch job"}))})
            }
        };
        let job = json_response(self.api(Method::POST, self.root())?.json(&body)).await?;
        Ok(text(&job, "id")?.to_owned())
    }

    fn job(&self, raw: Value) -> Result<BatchJob, Error> {
        let id = text(&raw, "id")?.to_owned();
        let status_key = if self.provider() == Provider::Anthropic {
            "processing_status"
        } else {
            "status"
        };
        let status = text(&raw, status_key)?.to_owned();
        let request_counts = if self.provider() == Provider::Mistral {
            json!({"total":raw["total_requests"],"succeeded":raw["succeeded_requests"],"failed":raw["failed_requests"]})
        } else {
            raw["request_counts"].clone()
        };
        Ok(BatchJob {
            id,
            status,
            request_counts,
            raw,
        })
    }

    pub async fn status(&self, id: &str) -> Result<BatchJob, Error> {
        let job = self.job(json_response(self.api(Method::GET, &self.path(id)?)?).await?)?;
        if job.id != id {
            return Err(Error::Protocol("mismatched batch job ID".into()));
        }
        Ok(job)
    }

    pub async fn list(&self, limit: u32) -> Result<Vec<BatchJob>, Error> {
        let key = if self.provider() == Provider::Mistral {
            "page_size"
        } else {
            "limit"
        };
        let mut response =
            json_response(self.api(Method::GET, self.root())?.query(&[(key, limit)])).await?;
        let items = response
            .get_mut("data")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| Error::Protocol("expected batch list data".into()))?;
        items
            .drain(..)
            .take(limit as usize)
            .map(|job| self.job(job))
            .collect()
    }

    pub async fn cancel(&self, id: &str) -> Result<Value, Error> {
        json_response(self.api(Method::POST, &format!("{}/cancel", self.path(id)?))?).await
    }

    /// Delete a Mistral batch job.
    pub async fn delete(&self, id: &str) -> Result<Value, Error> {
        if self.provider() != Provider::Mistral {
            return Err(Error::Configuration(
                "batch deletion requires Mistral".into(),
            ));
        }
        let bytes = send(self.api(Method::DELETE, &self.path(id)?)?)
            .await?
            .bytes()
            .await?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn result_paths(&self, id: &str) -> Result<Vec<String>, Error> {
        let job = self.status(id).await?;
        match self.provider() {
            Provider::OpenAI => self.openai_result_paths(job).await,
            Provider::Anthropic if job.status == "ended" => {
                Ok(vec![format!("{}/results", self.path(id)?)])
            }
            Provider::Mistral if job.status == "SUCCESS" => {
                if job.request_counts["succeeded"] == 0
                    && job.request_counts["failed"].as_u64().unwrap_or(0) > 0
                {
                    return Err(Error::Provider("all Mistral requests failed".into()));
                }
                Ok(vec![file_path(text(&job.raw, "output_file")?)?])
            }
            _ => Err(unavailable(&job)),
        }
    }

    async fn openai_result_paths(&self, mut job: BatchJob) -> Result<Vec<String>, Error> {
        for attempt in 0..=10 {
            if !matches!(job.status.as_str(), "completed" | "expired" | "cancelled") {
                return Err(unavailable(&job));
            }
            if let Some(files) = openai_files(&job) {
                return files.into_iter().map(file_path).collect();
            }
            if attempt < 10 {
                tokio::time::sleep(Duration::from_secs(5 + attempt)).await;
                job = self.status(&job.id).await?;
            }
        }
        Err(Error::Protocol(format!(
            "batch {} result files unavailable after 10 polls",
            job.id
        )))
    }

    async fn copy_results(
        &self,
        paths: Vec<String>,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<(), Error> {
        let mut block = Vec::new();
        let mut separator = false;
        for path in paths {
            let mut response = send(self.api(Method::GET, &path)?).await?;
            if separator {
                block.push(b'\n');
                separator = false;
            }
            while let Some(chunk) = response.chunk().await? {
                let mut rest = chunk.as_ref();
                if let Some(last) = rest.last() {
                    separator = *last != b'\n';
                }
                while !rest.is_empty() {
                    let n = rest.len().min(BLOCK_SIZE - block.len());
                    block.extend_from_slice(&rest[..n]);
                    rest = &rest[n..];
                    if block.len() == BLOCK_SIZE {
                        write_block(output, &mut block, false).await?;
                    }
                }
            }
            write_block(output, &mut block, true).await?;
        }
        output.flush().await?;
        Ok(())
    }

    /// Retrieve every available record, including OpenAI's separate errors file.
    pub async fn results_raw(&self, id: &str) -> Result<String, Error> {
        let mut bytes = Vec::new();
        self.copy_results(self.result_paths(id).await?, &mut bytes)
            .await?;
        Ok(String::from_utf8(bytes).expect("write_block validates UTF-8"))
    }

    /// Stream results to a temporary file and replace the destination on success.
    /// Dropping the future during the transfer discards the partial file.
    pub async fn download(&self, id: &str, path: impl AsRef<Path>) -> Result<(), Error> {
        let paths = self.result_paths(id).await?;
        let path = path.as_ref();
        let dest = path.to_owned();
        let temporary = tokio::task::spawn_blocking(move || temporary_file(&dest))
            .await
            .map_err(|e| Error::Protocol(e.to_string()))??;
        let (file, guard) = temporary.into_parts();
        let mut output = tokio::fs::File::from_std(file);
        self.copy_results(paths, &mut output)
            .await
            .map_err(|err| match err {
                Error::Io(e) => file_error(path, e),
                other => other,
            })?;
        let ready = tempfile::NamedTempFile::from_parts(output.into_std().await, guard);
        // Keep the rename on this task. A detached writer could publish after cancellation.
        replace_file(ready, path)
    }
}

fn unavailable(job: &BatchJob) -> Error {
    Error::Provider(format!(
        "batch {} results unavailable with status {}",
        job.id, job.status
    ))
}

fn openai_files(job: &BatchJob) -> Option<Vec<&str>> {
    let mut files = Vec::new();
    for (file_key, count_key) in [("output_file_id", "completed"), ("error_file_id", "failed")] {
        match job.raw[file_key].as_str().filter(|s| !s.is_empty()) {
            Some(id) if !files.contains(&id) => files.push(id),
            None if job.request_counts[count_key].as_u64().unwrap_or(0) > 0 => return None,
            _ => (),
        }
    }
    let empty_job = matches!(job.status.as_str(), "expired" | "cancelled")
        && job.request_counts["completed"] == 0
        && job.request_counts["failed"] == 0;
    (!files.is_empty() || empty_job).then_some(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_waits_for_expected_files_and_deduplicates() {
        let mut job = BatchJob {
            id: "1".into(),
            status: "completed".into(),
            request_counts: json!({"completed":1,"failed":1}),
            raw: json!({"output_file_id":"both"}),
        };
        assert!(openai_files(&job).is_none());
        job.raw["error_file_id"] = "both".into();
        assert_eq!(openai_files(&job).unwrap(), vec!["both"]);
        job.raw = json!({});
        job.request_counts = json!({"completed":0,"failed":0});
        assert!(openai_files(&job).is_none());
        job.status = "cancelled".into();
        assert!(openai_files(&job).unwrap().is_empty());
    }
}
