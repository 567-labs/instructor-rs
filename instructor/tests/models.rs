//! Public workflows corresponding to Python's Maybe and test_citation.py.
use instructor_ai::{
    model, parse_response, response_schema, Citations, Maybe, MaybePartial, PartialModel,
};
use serde_json::{json, Value};

#[model]
#[derive(Debug)]
struct Fact {
    statement: String,
    #[serde(flatten)]
    citations: Citations,
}

#[test]
fn maybe_distinguishes_missing_data_from_errors() {
    let empty: Maybe<Fact> = parse_response("{}", &Value::Null).unwrap();
    assert!(!empty.has_result());
    assert!(!empty.error);
    assert!(empty.message.is_none());

    let absent: Maybe<Fact> = parse_response(
        r#"{"result":null,"error":true,"message":"no source"}"#,
        &Value::Null,
    )
    .unwrap();
    assert!(!absent.has_result());
    assert!(absent.error);
    assert_eq!(absent.message.as_deref(), Some("no source"));
    let schema = response_schema::<Maybe<Fact>>();
    assert_eq!(schema["properties"]["error"]["default"], false);
    assert!(schema["properties"]["result"].is_object());
}

#[test]
fn maybe_with_flattened_citations_validates_and_has_typed_snapshots() {
    let input = r#"{"result":{"statement":"Margin","substring_quotes":["50% (aprox)","absent"]}}"#;
    let context = json!({"context":"The margin is 50% (approx) this quarter."});
    let fact: Maybe<Fact> = parse_response(input, &context).unwrap();
    assert!(fact.has_result());
    assert_eq!(
        fact.result.as_ref().unwrap().citations.substring_quotes,
        ["50% (approx)"]
    );
    let schema = response_schema::<Fact>();
    assert!(schema["properties"]["substring_quotes"].is_object());
    let partial: MaybePartial<Fact> = fact.into_partial();
    let fact: FactPartial = partial.result.unwrap();
    assert_eq!(
        fact.citations.unwrap().substring_quotes.unwrap(),
        ["50% (approx)"]
    );
    let raw: MaybePartial<Fact> = serde_json::from_str(input).unwrap();
    assert_eq!(
        raw.result
            .unwrap()
            .citations
            .unwrap()
            .substring_quotes
            .unwrap()
            .len(),
        2
    );
}

fn quotes(input: &[&str], context: Value) -> Vec<String> {
    let payload = json!({"substring_quotes":input});
    let result: Citations = parse_response(&payload.to_string(), &context).unwrap();
    result.substring_quotes
}

#[test]
fn no_context_preserves_proposals_and_empty_context_drops_them() {
    for absent in [Value::Null, json!({}), json!({"context":null})] {
        assert_eq!(quotes(&["unverified"], absent), ["unverified"]);
    }
    for empty in [json!({"context":[]}), json!({"context":""})] {
        assert!(quotes(&["unverified"], empty).is_empty());
    }
    assert!(quotes(&[""], json!({"context":"source"})).is_empty());
}

#[test]
fn nontext_sources_fail_without_mutating_quotes() {
    let mut cit = Citations {
        substring_quotes: vec!["quote".into()],
    };
    for bad in [json!(42), json!({"nested":true}), json!(["source", 42])] {
        let error = cit.validate_sources(&json!({"context":bad})).unwrap_err();
        assert!(error.contains("Citation context must be text"));
        assert_eq!(cit.substring_quotes, ["quote"]);
    }
}

#[test]
fn retrieves_from_independent_chunks_in_source_order() {
    let sources = json!({"context":["","abcdefghijklmnop","qrstuvwxyzabcdef"]});
    let input = [
        "qrstuvwxyz",
        "abcdefghijklmnop\nqrstuvwxyzabcdef",
        "ijklmnop",
    ];
    assert_eq!(quotes(&input, sources), ["ijklmnop", "qrstuvwxyz"]);
}

#[test]
fn metacharacters_match_literally() {
    for text in [
        "50% (approx",
        "cost [USD]",
        "a+b*c",
        r"path\to\file",
        "who? (maybe)",
    ] {
        let context = json!({"context":format!("prefix {text} suffix")});
        assert_eq!(quotes(&[text], context), [text]);
    }
}

#[test]
fn fuzzy_edits_are_bounded_by_quote_length_and_five_characters() {
    for (text, source, expected) in [
        ("at", "ax", None),
        ("cat", "cut", Some("cut")),
        ("cat", "cox", None),
        ("0123456ZZZ", "0123456789", Some("0123456789")),
        ("0123ZZZZZZ", "0123456789", None),
        ("abcdefghijklmnopZZZZZZ", "abcdefghijklmnopqrstuv", None),
        ("1999", "Revenue grew 12% in 2021.", None),
        ("Paris", "The capital of Germany is Berlin.", None),
        ("London!", "The capital of Germany is Berlin.", None),
        ("Margin", "Margin and Margin", Some("Margin")),
        ("東京", "東海", None),
        ("東京駅", "東京駅です", Some("東京駅")),
    ] {
        let result = quotes(&[text], json!({"context":source}));
        assert_eq!(result, expected.into_iter().collect::<Vec<_>>(), "{text:?}");
    }
}

#[test]
fn spans_index_utf8_text_and_prefer_exact_matches() {
    let source = "🌊 café，最初は東京。cafe.";
    let cit = Citations {
        substring_quotes: vec!["cafe".into(), "最初の東京".into()],
    };
    let spans: Vec<_> = cit.get_spans(source).map(|s| &source[s]).collect();
    assert_eq!(spans, ["cafe", "最初は東京"]);
}
