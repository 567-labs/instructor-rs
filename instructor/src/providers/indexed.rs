use super::response::{arguments, entries};
use crate::streaming::{Boundaries, MAX_BUFFER};
use crate::Error;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct IndexedCalls {
    calls: BTreeMap<u64, Call>,
    deferred_bytes: usize,
}

#[derive(Default)]
struct Call {
    name: String,
    text: String,
    // Only the first call needs incremental boundary tracking.
    boundaries: Boundaries,
    complete: bool,
}

impl IndexedCalls {
    pub fn has_calls(&self) -> bool {
        !self.calls.is_empty()
    }

    // Call zero streams immediately. Later indices can arrive in any order;
    // hold their arguments until the provider signals a successful finish.
    pub fn feed(&mut self, calls: &Value, expected: &str) -> Result<String, Error> {
        let mut out = String::new();
        for (position, delta) in entries(calls).enumerate() {
            let index = match &delta["index"] {
                Value::Null => position as u64,
                v => v
                    .as_u64()
                    .ok_or_else(|| Error::Protocol("invalid streamed tool index".into()))?,
            };
            if self.calls.len() >= 1024 && !self.calls.contains_key(&index) {
                return Err(Error::Protocol("too many streamed tool calls".into()));
            }
            let call = self.calls.entry(index).or_default();
            let function = &delta["function"];
            if let Some(name) = function["name"].as_str() {
                if name != expected || call.name != expected {
                    call.name.push_str(name);
                }
                if !expected.starts_with(&call.name) {
                    return Err(Error::Parse(format!("unexpected tool {}", call.name)));
                }
            }
            if function["arguments"].is_null() {
                continue;
            }
            let part = arguments(&function["arguments"])?;
            if index == 0 {
                for ch in part.chars() {
                    let closed = call.boundaries.read(ch)?;
                    if !ch.is_ascii_whitespace() {
                        call.complete = closed;
                    }
                }
                out.push_str(&part);
            } else {
                self.deferred_bytes += part.len();
                if self.deferred_bytes > MAX_BUFFER {
                    return Err(Error::Protocol(
                        "deferred tool arguments exceed 16 MiB".into(),
                    ));
                }
                call.text.push_str(&part);
            }
        }
        Ok(out)
    }

    pub fn finish(self, expected: &str) -> impl Iterator<Item = Result<String, Error>> + '_ {
        self.calls
            .into_iter()
            .flat_map(move |(index, call)| {
                if !call.name.is_empty() && call.name != expected {
                    return [
                        Some(Err(Error::Parse(format!("unexpected tool {}", call.name)))),
                        None,
                    ];
                }
                let complete = if index == 0 {
                    call.complete
                } else {
                    serde_json::from_str::<serde::de::IgnoredAny>(&call.text).is_ok()
                };
                // Preserve finished records within a truncated array, but never
                // let one call finish the open JSON document of an earlier call.
                [
                    (!call.text.is_empty()).then_some(Ok(call.text)),
                    (!complete).then(|| {
                        Err(Error::Incomplete(format!(
                            "unfinished tool call at index {index}"
                        )))
                    }),
                ]
            })
            .flatten()
    }
}
