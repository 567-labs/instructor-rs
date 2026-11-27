//! Classify a ticket with the same model through OpenRouter, TypeSafe or OpenAI.
use instructor_ai::decisions::{
    BlockingDecisionsClient, Choices, DecisionModel, Level, Noul, Score,
};
use serde_json::json;

/// Which team handles this {{ item.kind }}?
#[derive(Choices, Debug)]
enum Team {
    #[choice(examples = ["I was charged twice."])]
    Billing,
    /// Sign-in problems
    Access,
    Other,
}

#[derive(DecisionModel)]
struct Ticket {
    team: Team,
    #[decision(Noul::new("Is a refund requested?")
        .when_true("Requests money back")
        .when_false("Only asks for information"))]
    refund: f64,
    #[decision(Score::new("How urgent?", [
        Level::new("Routine"),
        Level::new("Action needed").example("Duplicate charge"),
        Level::new("Blocking"),
    ]).scale(0.0, 10.0))]
    urgency: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let route = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "openrouter/typesafe/jev-1.14".into());
    let client = BlockingDecisionsClient::from_provider(&route)?;
    let context = json!({"item": {"kind": "ticket", "body": "My invoice shows two charges. Please refund one."}});
    let (ticket, raw) = client.create_with_completion::<Ticket>(context)?;
    println!(
        "Team {:?}, refund probability {}, urgency {}",
        ticket.team, ticket.refund, ticket.urgency
    );
    println!("Usage {}", raw["usage"]);
    Ok(())
}
