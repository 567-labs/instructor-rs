use std::marker::PhantomData;

use super::{
    validated, Boundaries, Decoder, DeserializeOwned, Error, InstructMacro, Value, MAX_BUFFER,
};

/// Only the current item and optional wrapper metadata are retained.
/// The wrapper is checked as JSON with an empty array substituted for the
/// records already emitted, so other keys cannot become spurious items.
pub(super) struct Items<T> {
    stage: Stage,
    object: Token,
    item: Token,
    seen_document: bool,
    context: Value,
    model: PhantomData<fn() -> T>,
}

#[derive(Clone, Copy)]
enum Stage {
    Root,
    Object,
    FirstItem,
    NextItem,
    Item,
    Separator,
    WrapperSuffix,
}

#[derive(Default)]
struct Token {
    json: String,
    boundaries: Boundaries,
    last_quote: usize,
}

impl Token {
    fn push(&mut self, ch: char) -> Result<bool, Error> {
        if self.json.len() + ch.len_utf8() > MAX_BUFFER {
            return Err(Error::Protocol(
                "iterable item or metadata exceeds 16 MiB".into(),
            ));
        }
        if ch == '"' && !self.boundaries.quoted {
            self.last_quote = self.json.len();
        }
        self.json.push(ch);
        self.boundaries.read(ch)
    }

    fn begins_tasks(&self, ch: char) -> bool {
        if ch != '[' || self.boundaries.depth != 1 || self.boundaries.quoted {
            return false;
        }
        let suffix = self.json[self.last_quote..].trim_end();
        let Some(key) = suffix.strip_suffix(':') else {
            return false;
        };
        serde_json::from_str::<String>(key).is_ok_and(|k| k == "tasks")
    }

    fn reset(&mut self) {
        self.json.clear();
        self.boundaries = Boundaries::default();
        self.last_quote = 0;
    }
}

impl<T> Items<T> {
    pub fn new(context: Value) -> Self {
        Self {
            stage: Stage::Root,
            object: Token::default(),
            item: Token::default(),
            seen_document: false,
            context,
            model: PhantomData,
        }
    }

    fn close_array(&mut self) -> Result<(), Error> {
        if self.object.json.is_empty() {
            self.end_document();
        } else {
            self.object.push(']')?;
            self.stage = Stage::WrapperSuffix;
        }
        Ok(())
    }

    fn end_document(&mut self) {
        self.object.reset();
        self.stage = Stage::Root;
        self.seen_document = true;
    }
}

fn whitespace(ch: char) -> bool {
    matches!(ch, ' ' | '\r' | '\n' | '\t')
}

impl<T: InstructMacro + DeserializeOwned> Decoder for Items<T> {
    type Item = T;

    fn read(&mut self, fragment: &mut &str) -> Result<Option<T>, Error> {
        let input = std::mem::take(fragment);
        for (offset, ch) in input.char_indices() {
            let rest = &input[offset + ch.len_utf8()..];
            match self.stage {
                Stage::Root => match ch {
                    '[' => self.stage = Stage::FirstItem,
                    '{' => {
                        self.object.push(ch)?;
                        self.stage = Stage::Object;
                    }
                    ch if whitespace(ch) => (),
                    _ => return Err(Error::Protocol("expected a JSON array or object".into())),
                },
                Stage::Object => {
                    if self.object.begins_tasks(ch) {
                        self.object.push(ch)?;
                        self.stage = Stage::FirstItem;
                    } else if self.object.push(ch)? {
                        let value = validated(&self.object.json, &self.context)?;
                        self.end_document();
                        *fragment = rest;
                        return Ok(Some(value));
                    }
                }
                Stage::WrapperSuffix => {
                    if self.object.begins_tasks(ch) {
                        return Err(Error::Protocol("duplicate tasks array".into()));
                    }
                    if self.object.push(ch)? {
                        serde_json::from_str::<Value>(&self.object.json)?;
                        self.end_document();
                    }
                }
                Stage::FirstItem if ch == ']' => self.close_array()?,
                Stage::FirstItem | Stage::NextItem => {
                    if !whitespace(ch) {
                        self.item.reset();
                        self.item.push(ch)?;
                        self.stage = Stage::Item;
                    }
                }
                Stage::Item => {
                    // A number can span fragments: 1 followed by 23 is 123.
                    // Consume its separator on the next call.
                    let scalar = self.item.boundaries.depth == 0 && !self.item.boundaries.quoted;
                    if scalar && (whitespace(ch) || matches!(ch, ',' | ']')) {
                        *fragment = &input[offset..];
                    } else if self.item.push(ch)? {
                        *fragment = rest;
                    } else {
                        continue;
                    }
                    let value = validated(&self.item.json, &self.context)?;
                    self.stage = Stage::Separator;
                    return Ok(Some(value));
                }
                Stage::Separator => match ch {
                    ',' => self.stage = Stage::NextItem,
                    ']' => self.close_array()?,
                    ch if whitespace(ch) => (),
                    _ => return Err(Error::Protocol("expected ',' or ']' after item".into())),
                },
            }
        }
        Ok(None)
    }

    fn finish(self) -> Result<Option<T>, Error> {
        if self.seen_document && matches!(self.stage, Stage::Root) {
            Ok(None)
        } else {
            Err(Error::Incomplete(
                "stream ended before the iterable was complete".into(),
            ))
        }
    }
}
