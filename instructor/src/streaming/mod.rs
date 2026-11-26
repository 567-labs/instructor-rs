//! Decode a stream of JSON fragments into typed snapshots or individual records.
//!
//! The parsers also work directly with local strings. Each source item is a
//! result so transport errors can propagate in order. Dropping an iterator or
//! stream releases its source. Parsing and validation errors are terminal.

mod items;
mod markdown;
mod snapshot;
mod sse;

use futures_util::{pin_mut, Stream, StreamExt};
use instruct_macros_types::{InstructMacro, PartialModel};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{Error, ResponseModel};
use items::Items;
pub(crate) use markdown::{extract_markdown_json, markdown_fragments};
use snapshot::Snapshots;
pub(crate) use sse::{SseDecoder, SseEvent};

pub(crate) const MAX_BUFFER: usize = 16 * 1024 * 1024;
const MAX_DEPTH: usize = 128;

/// A view of a streamed model at a particular stage of generation.
#[derive(Debug)]
pub enum PartialEvent<T: PartialModel> {
    /// Provisional, typed fields. Model validation is deferred.
    Partial(T::Partial),
    /// A full, validated model yielded once after a successful stream.
    Complete(T),
}

/// Emit typed previews of an object, followed by the validated model.
/// Fragments may split strings, escapes and nested values. A prefix that
/// cannot yet be represented by the partial type may produce no event.
pub fn partial<T>(
    fragments: impl IntoIterator<Item = Result<impl AsRef<str>, Error>>,
) -> impl Iterator<Item = Result<PartialEvent<T>, Error>>
where
    T: PartialModel + ResponseModel,
{
    decode(Snapshots::<T>::new(Value::Null), fragments)
}

/// Validate and emit each item in a bare array or a top-level "tasks" array.
/// Consecutive object documents are also supported.
pub fn iterable<T>(
    fragments: impl IntoIterator<Item = Result<impl AsRef<str>, Error>>,
) -> impl Iterator<Item = Result<T, Error>>
where
    T: InstructMacro + DeserializeOwned,
{
    decode(Items::<T>::new(Value::Null), fragments)
}

/// Asynchronous equivalent of [partial].
pub fn partial_async<T>(
    fragments: impl Stream<Item = Result<impl AsRef<str>, Error>>,
) -> impl Stream<Item = Result<PartialEvent<T>, Error>>
where
    T: PartialModel + ResponseModel,
{
    partial_async_with_context(fragments, Value::Null)
}

/// Pass a request-specific context to final model validation.
pub fn partial_async_with_context<T>(
    fragments: impl Stream<Item = Result<impl AsRef<str>, Error>>,
    context: Value,
) -> impl Stream<Item = Result<PartialEvent<T>, Error>>
where
    T: PartialModel + ResponseModel,
{
    decode_async(Snapshots::<T>::new(context), fragments)
}

/// Asynchronous equivalent of [iterable].
pub fn iterable_async<T>(
    fragments: impl Stream<Item = Result<impl AsRef<str>, Error>>,
) -> impl Stream<Item = Result<T, Error>>
where
    T: InstructMacro + DeserializeOwned,
{
    iterable_async_with_context(fragments, Value::Null)
}

/// Pass a request-specific context to validation for every item.
pub fn iterable_async_with_context<T>(
    fragments: impl Stream<Item = Result<impl AsRef<str>, Error>>,
    context: Value,
) -> impl Stream<Item = Result<T, Error>>
where
    T: InstructMacro + DeserializeOwned,
{
    decode_async(Items::<T>::new(context), fragments)
}

// Both drivers read only until the next model is ready, even when a single
// input fragment contains several records. Validation stays lazy and ordered.
trait Decoder {
    type Item;
    fn read(&mut self, fragment: &mut &str) -> Result<Option<Self::Item>, Error>;
    fn finish(self) -> Result<Option<Self::Item>, Error>;
}

fn decode<D: Decoder, F: AsRef<str>>(
    decoder: D,
    fragments: impl IntoIterator<Item = Result<F, Error>>,
) -> impl Iterator<Item = Result<D::Item, Error>> {
    let mut state = Some((decoder, fragments.into_iter(), None::<F>, 0));
    std::iter::from_fn(move || {
        let (decoder, source, fragment, offset) = state.as_mut()?;
        loop {
            if let Some(input) = fragment {
                let text = input.as_ref();
                let mut remaining = &text[*offset..];
                match decoder.read(&mut remaining) {
                    Ok(Some(event)) => {
                        *offset = text.len() - remaining.len();
                        return Some(Ok(event));
                    }
                    Ok(None) => (),
                    Err(error) => {
                        state.take();
                        return Some(Err(error));
                    }
                }
            }
            match source.next() {
                Some(Ok(next)) => {
                    *fragment = Some(next);
                    *offset = 0;
                }
                Some(Err(error)) => {
                    state.take();
                    return Some(Err(error));
                }
                None => return state.take()?.0.finish().transpose(),
            }
        }
    })
}

fn decode_async<D: Decoder, F: AsRef<str>>(
    mut decoder: D,
    fragments: impl Stream<Item = Result<F, Error>>,
) -> impl Stream<Item = Result<D::Item, Error>> {
    async_stream::stream! {
        // Release the source before yielding the terminal event.
        let terminal = {
            pin_mut!(fragments);
            'input: loop {
                let fragment = match fragments.next().await {
                    Some(Ok(fragment)) => fragment,
                    Some(Err(error)) => break Err(error),
                    None => break decoder.finish(),
                };
                let mut rest = fragment.as_ref();
                while !rest.is_empty() {
                    match decoder.read(&mut rest) {
                        Ok(Some(event)) => yield Ok(event),
                        Ok(None) => break,
                        Err(error) => break 'input Err(error),
                    }
                }
            }
        };
        if let Some(event) = terminal.transpose() {
            yield event;
        }
    }
}

fn validated<T: InstructMacro + DeserializeOwned>(text: &str, context: &Value) -> Result<T, Error> {
    let mut model: T = serde_json::from_str(text)?;
    model
        .validate_with_context(context)
        .map_err(Error::Validation)?;
    Ok(model)
}

/// Container and quote boundaries shared by the incremental parsers.
/// Complete candidates are deserialized with Serde for syntax checking.
#[derive(Default)]
pub(crate) struct Boundaries {
    depth: usize,
    quoted: bool,
    escaped: bool,
}

impl Boundaries {
    /// True when the root container or string closed on this character.
    pub(crate) fn read(&mut self, ch: char) -> Result<bool, Error> {
        if self.quoted {
            if self.escaped {
                self.escaped = false;
            } else if ch == '\\' {
                self.escaped = true;
            } else if ch == '"' {
                self.quoted = false;
                return Ok(self.depth == 0);
            }
        } else {
            match ch {
                '"' => self.quoted = true,
                '{' | '[' => {
                    self.depth += 1;
                    if self.depth > MAX_DEPTH {
                        return Err(Error::Protocol("JSON exceeds 128 levels".into()));
                    }
                }
                '}' | ']' => {
                    self.depth = self.depth.checked_sub(1).ok_or_else(|| {
                        Error::Protocol("unexpected JSON closing delimiter".into())
                    })?;
                    return Ok(self.depth == 0);
                }
                _ => (),
            }
        }
        Ok(false)
    }
}
