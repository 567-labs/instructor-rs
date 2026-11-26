use super::{Boundaries, Error};
use futures_util::{stream::BoxStream, StreamExt};
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

type Fragments = BoxStream<'static, Result<String, Error>>;

pub(crate) fn markdown_fragments(mut input: Fragments, iterable: bool) -> Fragments {
    async_stream::try_stream! {
        let mut size = 0;
        if iterable {
            let mut selection = Candidates::default();
            while let Some(chunk) = input.next().await {
                let chunk = chunk?;
                for ch in chunk.chars() {
                    size += 1;
                    check_size(size)?;
                    if let Some(candidate) = selection.push(ch)? {
                        yield candidate;
                    }
                }
            }
            for candidate in selection.finish() {
                yield candidate;
            }
        } else {
            let mut text = String::new();
            let mut ends = Vec::new();
            while let Some(chunk) = input.next().await {
                let chunk = chunk?;
                size += chunk.chars().count();
                check_size(size)?;
                if !chunk.is_empty() {
                    text.push_str(&chunk);
                    ends.push(text.len());
                }
            }
            let selected = extract_markdown_json(&text)?;
            if selected == text.trim() && matches!(selected.chars().next(), Some('{' | '[')) {
                // Preserve the original snapshots of a pure JSON response.
                let mut start = 0;
                for end in ends {
                    yield text[start..end].to_owned();
                    start = end;
                }
            } else if selected == text {
                // No complete candidate. Keep an unfinished object so callers
                // can see its last preview before the incomplete-output error.
                let mut selection = Candidates::default();
                for ch in text.chars() {
                    if let Some(candidate) = selection.push(ch)? {
                        yield candidate;
                    }
                }
                for candidate in selection.finish() {
                    yield candidate;
                }
            } else {
                yield selected.to_owned();
            }
        }
    }
    .boxed()
}

/// For several models, fenced values take precedence over prose examples.
/// Bare candidates are held until EOF. Fenced candidates can be delivered
/// before the closing fence, once their outer container closes.
#[derive(Default)]
struct Candidates {
    boundaries: Boundaries,
    current: String,
    ticks: u8,
    fenced: bool,
    saw_fence: bool,
    deferred: Vec<String>,
}

impl Candidates {
    fn push(&mut self, ch: char) -> Result<Option<String>, Error> {
        if ch == '`' && !self.boundaries.quoted {
            self.ticks += 1;
            if self.ticks == 3 {
                self.ticks = 0;
                self.saw_fence = true;
                self.fenced = !self.fenced;
                self.deferred.clear();
                self.boundaries = Boundaries::default();
                let unfinished = std::mem::take(&mut self.current);
                if !self.fenced && !unfinished.is_empty() {
                    return Ok(Some(unfinished));
                }
            }
            return Ok(None);
        }
        self.ticks = 0;
        if self.saw_fence && !self.fenced {
            return Ok(None);
        }
        if self.current.is_empty() && !matches!(ch, '{' | '[') {
            return Ok(None);
        }
        self.current.push(ch);
        if self.boundaries.read(ch)? {
            let candidate = std::mem::take(&mut self.current);
            if serde_json::from_str::<IgnoredAny>(&candidate).is_ok() {
                if self.fenced {
                    return Ok(Some(candidate));
                }
                self.deferred.push(candidate);
            }
        }
        Ok(None)
    }

    fn finish(mut self) -> Vec<String> {
        if !self.current.is_empty() {
            self.deferred.push(self.current);
        }
        self.deferred
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

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

    async fn selected(input: &str, iterable: bool) -> Vec<String> {
        let chars: Vec<_> = input.chars().map(|c| Ok(c.to_string())).collect();
        markdown_fragments(stream::iter(chars).boxed(), iterable)
            .map(Result::unwrap)
            .collect()
            .await
    }

    #[tokio::test]
    async fn bare_values_and_split_fenced_input() {
        assert_eq!(selected(" []", false).await, [" ", "[", "]"]);
        assert_eq!(selected("[] {}", true).await, ["[]", "{}"]);
        let embedded = format!("[\"{FENCE}\"]");
        let text = format!("example [1] {FENCE}json {embedded} [2]{FENCE}after [3]");
        assert_eq!(selected(&text, true).await, [embedded.as_str(), "[2]"]);
        assert_eq!(selected(&text, false).await, ["[3]"]);
        assert_eq!(
            selected(&format!("prefix {FENCE}json {{\"v\":1"), false).await,
            ["{\"v\":1"]
        );
    }

    #[tokio::test]
    async fn fenced_values_are_available_before_later_reads() {
        let first = Ok(format!("example [9] {FENCE}json[1]"));
        let input = stream::iter([first]).chain(stream::poll_fn(|_| panic!("read ahead")));
        assert_eq!(
            markdown_fragments(input.boxed(), true)
                .next()
                .await
                .unwrap()
                .unwrap(),
            "[1]"
        );
    }
}
