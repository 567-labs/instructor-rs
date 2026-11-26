use crate::Error;
use serde::de::IgnoredAny;

const MAX_CHARS: usize = 1024 * 1024;

fn check_size(count: usize) -> Result<(), Error> {
    if count > MAX_CHARS {
        Err(Error::Protocol(
            "Markdown JSON exceeds 1 Mi characters".into(),
        ))
    } else {
        Ok(())
    }
}

/// For one model, Python selects the last syntactically valid outer container,
/// regardless of fences. On malformed openers it retries inside the candidate.
pub(crate) fn extract_markdown_json(text: &str) -> Result<&str, Error> {
    check_size(text.chars().count())?;
    let mut last = text;
    let mut remaining = text;
    let mut budget = 16 * text.len();
    while let Some(start) = remaining.find(['{', '[']) {
        remaining = &remaining[start..];
        if remaining.len() > budget {
            return Err(Error::Protocol(
                "Markdown JSON extraction work limit exceeded".into(),
            ));
        }
        let mut scanner = serde_json::Deserializer::from_str(remaining).into_iter::<IgnoredAny>();
        if matches!(scanner.next(), Some(Ok(_))) {
            let size = scanner.byte_offset();
            let span = &remaining[..size];
            let mut boundaries = Boundaries::default();
            for ch in span.chars() {
                boundaries.read(ch)?;
            }
            last = span;
            remaining = &remaining[size..];
            budget -= size;
        } else {
            budget -= remaining.len();
            remaining = &remaining[1..];
        }
    }
    Ok(last)
}

const MAX_DEPTH: usize = 128;

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

#[cfg(test)]
mod tests {
    use super::*;

    const FENCE: &str = "```";

    #[test]
    fn single_model_chooses_last_valid_span_inside_or_after_fence() {
        let text = format!("example {{}} {FENCE}json [1] {FENCE} final [2] {{\"unfinished\":");
        assert_eq!(extract_markdown_json(&text).unwrap(), "[2]");
        assert_eq!(extract_markdown_json("prefix { [1]").unwrap(), "[1]");
        assert_eq!(extract_markdown_json("oops").unwrap(), "oops");
        let nested = r#"{"text":"a \" } } {", "obj":{}}"#;
        assert_eq!(
            extract_markdown_json(&format!("{{}} {nested}")).unwrap(),
            nested
        );
    }

    #[test]
    fn limits_count_characters_and_bound_repeated_decoding() {
        let text = format!("[\"{}\"]", "雪".repeat(MAX_CHARS - 4));
        assert_eq!(extract_markdown_json(&text).unwrap(), text);
        assert!(extract_markdown_json(&(text + " ")).is_err());
        for depth in [128, 129] {
            let text = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            assert_eq!(extract_markdown_json(&text).is_ok(), depth == 128);
        }
        assert!(extract_markdown_json(&"[".repeat(200)).is_err());
    }
}
