# Handle missing facts and verify quoted evidence

Python's `Maybe(Fact)` and Rust's `Maybe<Fact>` let a model report a missing fact in a successful response. The wire fields are `result`, `error`, and `message`. An omitted `result` or `message` becomes `None`, and `error` defaults to `false`. Use `if let Some(fact) = response.result` to access the typed value. `has_result()` corresponds to Python's truth check and depends only on `result`. A transport or validation failure is returned in the outer `Result`. [Python Maybe](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/maybe.md)

Embed `Citations` in a model with `#[serde(flatten)]` to use Python `CitationMixin`'s `substring_quotes` field. When validation receives a source, it removes unresolved proposals and replaces matched quotes with the literal source text. Nested facts use the same context as the enclosing response. Verification establishes that the text appears in the supplied sources. Whether that text supports the statement requires a separate judgment. [Python citations](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/citation.md)

## Inspect a result locally

The example repairs the spelling of `50% (aprox)`, drops an unrelated quote, and prints the exact evidence. It also handles a response with just an explanation. Run it without credentials.

```sh
cargo run -p instructor-ai --example grounded_extraction
```

```rust
--8<-- "instructor/examples/grounded_extraction.rs"
```

## Send sources to the model and validator

With the `Fact` type from the example, send the text in the prompt and provide the same source to the validator. For several passages, set the `context` entry to an array of strings. The validator searches each passage independently and collects matches in source order. A quote can occur in multiple passages. An empty source list yields no quotes, an absent or null entry preserves the proposed quotes, and a non-text entry raises a validation error.

```rust
use instructor_ai::{model, Citations, Client, Error, Maybe, Request};
use serde_json::json;

#[model]
struct Fact {
    statement: String,
    #[serde(flatten)]
    citations: Citations,
}

async fn margin(client: &Client, passages: &[String]) -> Result<Maybe<Fact>, Error> {
    let prompt = format!(
        "Find the margin and quote the evidence. If it is missing, explain why.\n{}",
        passages.join("\n")
    );
    let request = Request::new(prompt).context(json!({"context": passages}));
    client.create(request).await
}
```

## Highlight a matching span

`citations.get_spans(source)` yields a half-open UTF-8 byte range for each matched quote. Index the source with `&source[span]` to highlight it. Python returns string indices measured in Unicode code points. Both implementations prefer exact matches and treat punctuation literally. If an exact match is missing, a quote of `n` Unicode characters allows up to `min(5, n / 3)` insertions, deletions or substitutions, with integer division. Empty quotes yield no spans.

Rust uses Levenshtein alignment and prefers the leftmost substring among those requiring the fewest edits. Tied alignments at the same start can produce different end offsets from Python's fuzzy regex engine. Fuzzy matching requires time proportional to the product of quote and source lengths and memory proportional to the quote length. [Python tests](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/tests/v2/test_citation.py)
