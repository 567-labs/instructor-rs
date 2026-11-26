//! Export calls to a typed teacher as JSONL and reuse its prompt for inference.

use std::{io::Write, marker::PhantomData};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::{schema::schema, BlockingClient, Client, Error, Message, Request, ResponseModel};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FinetuneFormat {
    /// Legacy Instructor training format with messages and function schemas.
    #[default]
    Messages,
    /// Function definition, original inputs, teacher result and response schema.
    Raw,
}

/// Mapping from the typed input to the displayed call.
#[derive(Default)]
pub struct Arguments {
    pub args: Vec<Value>,
    pub kwargs: Map<String, Value>,
}

impl From<Value> for Arguments {
    fn from(value: Value) -> Self {
        let mut call = Self::default();
        match value {
            Value::Null => (),
            Value::Array(values) => call.args = values,
            Value::Object(obj) => call.kwargs = obj,
            single => call.args.push(single),
        }
        call
    }
}

/// Describe the function in the system prompt, and use the same typed
/// input and output for recording and prediction.
pub struct Instructions<I, O> {
    name: String,
    definition: String,
    format: FinetuneFormat,
    arguments: fn(&I) -> Result<Arguments, Error>,
    output: PhantomData<fn() -> O>,
}

impl<I: Serialize, O: ResponseModel + Serialize> Instructions<I, O> {
    pub fn new(name: impl Into<String>, definition: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            definition: definition.into(),
            format: FinetuneFormat::default(),
            arguments: |input| Ok(serde_json::to_value(input)?.into()),
            output: PhantomData,
        }
    }

    pub fn format(mut self, format: FinetuneFormat) -> Self {
        self.format = format;
        self
    }

    /// Override how positional and named arguments are derived from the input.
    pub fn with_arguments(mut self, map: fn(&I) -> Result<Arguments, Error>) -> Self {
        self.arguments = map;
        self
    }

    fn messages(&self, call: &Arguments) -> Vec<Message> {
        let args = call
            .args
            .iter()
            .map(Value::to_string)
            .chain(
                call.kwargs
                    .iter()
                    .map(|(key, value)| format!("{key}={value}")),
            )
            .collect::<Vec<_>>()
            .join(", ");
        vec![
            Message::system(format!(
                "Predict the results of this function:\n\n{}",
                self.definition
            )),
            Message::user(format!("Return `{}({args})`", self.name)),
        ]
    }

    /// Build the inference prompt, which can be customized before creation.
    pub fn request(&self, input: &I) -> Result<Request, Error> {
        let call = (self.arguments)(input)?;
        Ok(Request::messages(self.messages(&call)))
    }

    /// Build one training record without writing it.
    pub fn record(&self, input: &I, result: &O) -> Result<Value, Error> {
        result.validate().map_err(Error::Validation)?;
        let call = (self.arguments)(input)?;
        let result = serde_json::to_value(result)?;
        if self.format == FinetuneFormat::Raw {
            return Ok(json!({
                "fn_name": self.name,
                "fn_repr": self.definition,
                "args": call.args,
                "kwargs": call.kwargs,
                "resp": result,
                "schema": O::json_schema(),
            }));
        }
        let schema = schema::<O>();
        let arguments = match schema.wrapper {
            Some(key) => json!({(key): result}),
            None => result,
        };
        let mut messages = self
            .messages(&call)
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        messages.push(json!({
            "role":"assistant",
            "function_call":{
                "name":schema.value["title"],
                "arguments":serde_json::to_string_pretty(&arguments)?,
            }
        }));
        let params = crate::schema::tool_parameters(&schema.value);
        let name = &schema.value["title"];
        let description = crate::schema::tool_description(&schema.value);
        Ok(json!({
            "messages": messages,
            "functions": [{"name": name, "description": description, "parameters": params}],
        }))
    }

    /// Serialize a complete record before writing its JSON line.
    pub fn track(&self, input: &I, result: &O, mut sink: impl Write) -> Result<(), Error> {
        let mut line = serde_json::to_vec(&self.record(input, result)?)?;
        line.push(b'\n');
        sink.write_all(&line)?;
        Ok(())
    }

    /// Run a teacher once and append its successful result to a JSONL sink.
    pub fn distil(
        &self,
        input: &I,
        sink: impl Write,
        teacher: impl FnOnce(&I) -> Result<O, Error>,
    ) -> Result<O, Error> {
        let output = teacher(input)?;
        self.track(input, &output, sink)?;
        Ok(output)
    }

    /// Ask the client's model to predict the teacher's output.
    pub async fn dispatch(&self, client: &Client, input: &I) -> Result<O, Error> {
        client.create(self.request(input)?).await
    }

    /// Synchronous prediction using the same function definition and arguments.
    pub fn dispatch_blocking(&self, client: &BlockingClient, input: &I) -> Result<O, Error> {
        client.create(self.request(input)?)
    }
}
