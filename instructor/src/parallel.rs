use std::{collections::BTreeMap, future::ready, sync::Arc};

use crate::{
    schema::{schema, Schema},
    BlockingClient, Client, Error, Request, ResponseModel,
};
use serde_json::Value;

type Parse<T> = Box<dyn Fn(&str, &Value) -> Result<T, Error> + Send + Sync>;

/// Associates a typed tool schema with its constructor in an output enum.
pub struct ParallelTool<T> {
    schema: Arc<Schema>,
    parse: Parse<T>,
}

impl<T: 'static> ParallelTool<T> {
    pub fn new<M: ResponseModel>(constructor: fn(M) -> T) -> Self {
        let schema = schema::<M>();
        Self {
            schema: schema.clone(),
            parse: Box::new(move |input, context| schema.parse(input, context).map(constructor)),
        }
    }
}

/// Result type for parallel tool calls. A single response model is already
/// usable. For heterogeneous results, derive this on an enum with one
/// response model inside each variant.
pub trait ParallelTools: Sized + 'static {
    fn tools() -> Vec<ParallelTool<Self>>;
}

impl<T: ResponseModel> ParallelTools for T {
    fn tools() -> Vec<ParallelTool<Self>> {
        vec![ParallelTool::new(std::convert::identity)]
    }
}

impl Client {
    /// Validate every returned tool call, preserving provider order.
    /// A failed call rejects the whole attempt.
    pub async fn create_parallel<T: ParallelTools>(
        &self,
        request: impl Into<Request>,
    ) -> Result<Vec<T>, Error> {
        self.create_parallel_with_completion(request)
            .await
            .map(|(items, _)| items)
    }

    /// Return the typed calls alongside their successful provider envelope.
    pub async fn create_parallel_with_completion<T: ParallelTools>(
        &self,
        request: impl Into<Request>,
    ) -> Result<(Vec<T>, Value), Error> {
        let req = request.into().prepare(true)?;
        let tools = T::tools();
        let mut registry = BTreeMap::new();
        for tool in &tools {
            let name = tool.schema.value["title"].as_str().ok_or_else(|| {
                Error::Configuration("parallel tool schema is missing its title".into())
            })?;
            if registry.insert(name, &tool.parse).is_some() {
                return Err(Error::Configuration(format!(
                    "duplicate parallel tool name {name:?}"
                )));
            }
        }
        let wire = self
            .config
            .parallel_request(&req, tools.iter().map(|tool| &tool.schema.value))?;
        self.complete_with(
            &req,
            wire,
            |raw| {
                self.config
                    .parallel_calls(raw)?
                    .into_iter()
                    .map(|(name, arguments)| {
                        let parser = registry.get(name).ok_or_else(|| {
                            let expected = registry.keys().copied().collect::<Vec<_>>().join(", ");
                            Error::Parse(format!(
                                "Unknown tool call {name:?}. Expected one of: {expected}"
                            ))
                        })?;
                        parser(&arguments, &req.context)
                    })
                    .collect()
            },
            |values| ready(Ok(values)),
        )
        .await
        .map(crate::Completion::into_pair)
    }
}

impl BlockingClient {
    pub fn create_parallel<T: ParallelTools>(
        &self,
        request: impl Into<Request>,
    ) -> Result<Vec<T>, Error> {
        self.runtime.block_on(self.client.create_parallel(request))
    }

    pub fn create_parallel_with_completion<T: ParallelTools>(
        &self,
        request: impl Into<Request>,
    ) -> Result<(Vec<T>, Value), Error> {
        self.runtime
            .block_on(self.client.create_parallel_with_completion(request))
    }
}
