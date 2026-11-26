use instructor_ai::{model, BlockingClient, Request};
use serde_json::{json, Value};

#[model]
#[validate(context = "check_quote")]
struct Evidence {
    /// A verbatim excerpt answering the question.
    quote: String,
}

fn check_quote(evidence: &mut Evidence, context: &Value) -> Result<(), String> {
    let source = context["source"].as_str().ok_or("source is required")?;
    if evidence.quote.trim().is_empty() || !source.contains(&evidence.quote) {
        return Err("Copy a nonempty quote exactly as it appears in the source.".into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args().nth(1).ok_or("pass provider/model")?;
    let client = BlockingClient::from_provider(&model)?;
    let request = Request::new("When did the library open? Quote from this passage. {{ source }}")
        .context(json!({"source": "Work began in 1894. The library opened in 1901."}))
        .max_retries(1);
    let (evidence, completion) = client.create_with_completion::<Evidence>(request)?;
    println!("{}", evidence.quote);
    if let Some(id) = completion.get("id") {
        println!("Response ID {id}");
    }
    Ok(())
}
