use super::*;
use crate::BlockingClient;
use tokio::runtime::Runtime;

/// Synchronous processor sharing its parent client's runtime.
pub struct BlockingBatch<T> {
    runtime: Arc<Runtime>,
    batch: Batch<T>,
}

impl BlockingClient {
    pub fn batch<T: ResponseModel>(&self) -> Result<BlockingBatch<T>, Error> {
        Ok(BlockingBatch {
            runtime: self.runtime.clone(),
            batch: self.client.batch()?,
        })
    }

    pub fn batch_with_mode<T: ResponseModel>(&self, mode: Mode) -> Result<BlockingBatch<T>, Error> {
        Ok(BlockingBatch {
            runtime: self.runtime.clone(),
            batch: self.client.batch_with_mode(mode)?,
        })
    }
}

impl<T: ResponseModel> BlockingBatch<T> {
    pub fn strict(mut self, strict: bool) -> Self {
        self.batch = self.batch.strict(strict);
        self
    }
    pub fn prepare<R: Into<BatchItem>>(
        &self,
        requests: impl IntoIterator<Item = R>,
    ) -> Result<String, Error> {
        self.batch.prepare(requests)
    }
    pub fn prepare_file<R: Into<BatchItem>>(
        &self,
        requests: impl IntoIterator<Item = R>,
        path: impl AsRef<Path>,
    ) -> Result<(), Error> {
        self.batch.prepare_file(requests, path)
    }
    pub fn submit(&self, jsonl: &str) -> Result<String, Error> {
        self.runtime.block_on(self.batch.submit(jsonl))
    }
    pub fn submit_with_options(&self, jsonl: &str, options: BatchOptions) -> Result<String, Error> {
        self.runtime
            .block_on(self.batch.submit_with_options(jsonl, options))
    }
    pub fn status(&self, id: &str) -> Result<BatchJob, Error> {
        self.runtime.block_on(self.batch.status(id))
    }
    pub fn list(&self, limit: u32) -> Result<Vec<BatchJob>, Error> {
        self.runtime.block_on(self.batch.list(limit))
    }
    pub fn cancel(&self, id: &str) -> Result<Value, Error> {
        self.runtime.block_on(self.batch.cancel(id))
    }
    pub fn delete(&self, id: &str) -> Result<Value, Error> {
        self.runtime.block_on(self.batch.delete(id))
    }
    pub fn results(&self, id: &str) -> Result<Vec<BatchResult<T>>, Error> {
        self.runtime.block_on(self.batch.results(id))
    }
    pub fn results_raw(&self, id: &str) -> Result<String, Error> {
        self.runtime.block_on(self.batch.results_raw(id))
    }
    pub fn parse(&self, jsonl: &str) -> Vec<BatchResult<T>> {
        self.batch.parse(jsonl)
    }
    pub fn parse_with_context(
        &self,
        jsonl: &str,
        ctx: impl FnMut(&str) -> Value,
    ) -> Vec<BatchResult<T>> {
        self.batch.parse_with_context(jsonl, ctx)
    }
    pub fn download(&self, id: &str, path: impl AsRef<Path>) -> Result<(), Error> {
        self.runtime.block_on(self.batch.download(id, path))
    }
}
