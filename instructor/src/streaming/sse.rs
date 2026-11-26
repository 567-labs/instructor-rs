use super::MAX_BUFFER;
use crate::Error;

#[derive(Default)]
#[cfg_attr(test, derive(Clone, Debug, PartialEq))]
pub(crate) struct SseEvent {
    pub name: String,
    pub data: String,
}

/// Network reads can end in the middle of a UTF-8 sequence or CRLF.
#[derive(Default)]
pub(crate) struct SseDecoder {
    line: Vec<u8>,
    event: SseEvent,
    emitted: bool,
    after_cr: bool,
    started: bool,
}

impl SseDecoder {
    /// Consume through one event, borrowing its fields until the next read.
    /// None as input indicates EOF, which can terminate the final event.
    pub fn read(&mut self, chunk: &mut Option<&[u8]>) -> Result<Option<&SseEvent>, Error> {
        if std::mem::take(&mut self.emitted) {
            self.event.name.clear();
            self.event.data.clear();
        }
        let Some(bytes) = chunk else {
            if !self.line.is_empty() {
                self.end_line()?;
            }
            return Ok(self.end_event().then_some(&self.event));
        };
        while let Some((&byte, rest)) = bytes.split_first() {
            *bytes = rest;
            if std::mem::take(&mut self.after_cr) && byte == b'\n' {
                continue;
            }
            if byte == b'\r' || byte == b'\n' {
                self.after_cr = byte == b'\r';
                if self.end_line()? {
                    return Ok(Some(&self.event));
                }
            } else if self.line.len() + self.event.data.len() >= MAX_BUFFER {
                return Err(Error::Protocol("SSE event exceeds 16 MiB".into()));
            } else {
                self.line.push(byte);
            }
        }
        Ok(None)
    }

    fn end_event(&mut self) -> bool {
        // Each data line contributes a trailing newline, including empty lines.
        self.emitted = self.event.data.pop().is_some();
        if !self.emitted {
            self.event.name.clear();
        }
        self.emitted
    }

    fn end_line(&mut self) -> Result<bool, Error> {
        let mut line = std::str::from_utf8(&self.line)
            .map_err(|err| Error::Protocol(format!("invalid UTF-8 in SSE line: {err}")))?;
        if !self.started {
            self.started = true;
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
        }
        if line.is_empty() {
            self.line.clear();
            return Ok(self.end_event());
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => {
                self.event.name.clear();
                self.event.name.push_str(value);
            }
            "data" => {
                self.event.data.push_str(value);
                self.event.data.push('\n');
            }
            _ => (),
        }
        self.line.clear();
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_byte_splits_and_mixed_line_endings() {
        let raw = concat!(
            "\u{feff}: keepalive\r\n",
            "id: 123\revent: ignored\r\r",
            "event: delta\r\ndata: 雪🌷\ndata:  second:line\r\n\r\n",
            "data\n\n",
            "data:[DONE]"
        )
        .as_bytes();
        for size in 1..=raw.len() {
            let mut decoder = SseDecoder::default();
            let mut events = Vec::new();
            for mut chunk in raw.chunks(size).map(Some).chain([None]) {
                while let Some(event) = decoder.read(&mut chunk).unwrap() {
                    events.push(event.clone());
                }
            }
            assert_eq!(
                events,
                vec![
                    SseEvent {
                        name: "delta".into(),
                        data: "雪🌷\n second:line".into()
                    },
                    SseEvent {
                        name: String::new(),
                        data: "".into()
                    },
                    SseEvent {
                        name: String::new(),
                        data: "[DONE]".into()
                    },
                ]
            );
        }
    }

    #[test]
    fn only_consumes_through_next_event() {
        let mut decoder = SseDecoder::default();
        let mut bytes = Some(&b"data:first\n\ndata:\xff\n\n"[..]);
        assert_eq!(decoder.read(&mut bytes).unwrap().unwrap().data, "first");
        assert_eq!(bytes.unwrap(), b"data:\xff\n\n");
        assert!(matches!(decoder.read(&mut bytes), Err(Error::Protocol(_))));
    }

    #[test]
    fn incomplete_utf8_at_eof() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.read(&mut Some(b"data:\xe9")).unwrap().is_none());
        assert!(matches!(decoder.read(&mut None), Err(Error::Protocol(_))));
    }
}
