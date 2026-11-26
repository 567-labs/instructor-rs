//! Inspect a proposed fact and highlight its evidence without a provider call.
use instructor_ai::{model, parse_response, Citations, Error, Maybe};
use serde_json::json;

#[model]
struct Fact {
    statement: String,
    #[serde(flatten)]
    citations: Citations,
}

fn main() -> Result<(), Error> {
    let source = "The margin is 50% (approx) this quarter.";
    let context = json!({"context": source});
    let input = json!({
        "result": {
            "statement": "The margin is about half.",
            "substring_quotes": ["50% (aprox)", "an unverified figure"]
        }
    });
    let response: Maybe<Fact> = parse_response(&input.to_string(), &context)?;
    if let Some(fact) = response.result {
        println!("{}", fact.statement);
        for span in fact.citations.get_spans(source) {
            println!("{}", &source[span]);
        }
    }
    let missing: Maybe<Fact> = parse_response(r#"{"message":"No margin provided."}"#, &context)?;
    assert!(!missing.has_result());
    Ok(())
}
