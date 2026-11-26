use super::MAX_BUFFER;
use crate::Error;

#[derive(Debug, PartialEq)]
pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Network reads can end in the middle of a UTF-8 sequence or CRLF.
#[derive(Default)]
pub(crate) struct SseDecoder {
    line: Vec<u8>,
    name: Option<String>,
    data: String,
    after_cr: bool,
    started: bool,
}

impl SseDecoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Result<SseEvent, Error>> {
        let mut events = Vec::new();
        for &byte in bytes {
            if std::mem::take(&mut self.after_cr) && byte == b'\n' {
                continue;
            }
            if byte == b'\r' || byte == b'\n' {
                self.after_cr = byte == b'\r';
                match self.end_line() {
                    Ok(Some(event)) => events.push(Ok(event)),
                    Ok(None) => (),
                    Err(error) => {
                        events.push(Err(error));
                        break;
                    }
                }
            } else if self.line.len() + self.data.len() >= MAX_BUFFER {
                events.push(Err(Error::Protocol("SSE event exceeds 16 MiB".into())));
                break;
            } else {
                self.line.push(byte);
            }
        }
        events
    }

    /// Some endpoints omit a blank line after the final event.
    pub fn finish(&mut self) -> Vec<Result<SseEvent, Error>> {
        if !self.line.is_empty() {
            if let Err(error) = self.end_line() {
                return vec![Err(error)];
            }
        }
        self.take_event().map(Ok).into_iter().collect()
    }

    fn take_event(&mut self) -> Option<SseEvent> {
        let event = self.name.take();
        // Each data line contributes a trailing newline, including empty lines.
        self.data.pop()?;
        Some(SseEvent {
            event,
            data: std::mem::take(&mut self.data),
        })
    }

    fn end_line(&mut self) -> Result<Option<SseEvent>, Error> {
        let mut line = std::str::from_utf8(&self.line)
            .map_err(|err| Error::Protocol(format!("invalid UTF-8 in SSE line: {err}")))?;
        if !self.started {
            self.started = true;
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
        }
        if line.is_empty() {
            self.line.clear();
            return Ok(self.take_event());
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.name = Some(value.to_owned()),
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
            }
            _ => (),
        }
        self.line.clear();
        Ok(None)
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
            for chunk in raw.chunks(size) {
                events.extend(decoder.feed(chunk));
            }
            events.extend(decoder.finish());
            assert!(decoder.finish().is_empty());
            let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(
                events,
                vec![
                    SseEvent {
                        event: Some("delta".into()),
                        data: "雪🌷\n second:line".into()
                    },
                    SseEvent {
                        event: None,
                        data: "".into()
                    },
                    SseEvent {
                        event: None,
                        data: "[DONE]".into()
                    },
                ]
            );
        }
    }

    #[test]
    fn utf8_error_keeps_prior_events_in_same_read() {
        let mut decoder = SseDecoder::default();
        let mut events = decoder.feed(b"data:first\n\ndata:\xff\n\n").into_iter();
        assert_eq!(events.next().unwrap().unwrap().data, "first");
        assert!(matches!(events.next(), Some(Err(Error::Protocol(_)))));
    }

    #[test]
    fn incomplete_utf8_at_eof() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.feed(b"data:\xe9").is_empty());
        assert!(matches!(
            decoder.finish().as_slice(),
            [Err(Error::Protocol(_))]
        ));
    }
}
