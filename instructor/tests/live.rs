//! Opt-in provider contracts. See docs/parity.md for credentials and invocation.
use futures_util::{StreamExt, TryStreamExt};
use instructor_ai::{
    decisions::{Choices, DecisionModel, DecisionsClient, Noul, Score},
    model, Client, Mode, PartialEvent, ProviderConfig, Request,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn route(provider: &str, variable: &str, default_model: &str) -> String {
    let model = std::env::var(variable).unwrap_or_else(|_| default_model.to_owned());
    format!("{provider}/{model}")
}

#[model]
#[derive(Debug, PartialEq)]
struct Person {
    name: String,
    age: u8,
}

async fn generation(route: &str, mode: Mode) -> TestResult {
    let config = ProviderConfig::from_env(route)?.with_mode(mode);
    let client = Client::new(config)?;
    let request = |prompt: &str| {
        let req = Request::new(prompt).max_retries(0);
        match mode {
            Mode::ResponsesTools => req.option("max_output_tokens", 2048),
            _ if route.starts_with("openai/") => req.option("max_completion_tokens", 2048),
            _ => req.option("max_tokens", 2048),
        }
    };
    let ali = Person {
        name: "Ali".into(),
        age: 28,
    };
    let bo = Person {
        name: "Bo".into(),
        age: 35,
    };
    let person = request("Extract the person. Ali is 28 years old.");
    let attempts = AtomicUsize::new(0);
    let result: Person = client
        .create_validated(person.clone().max_retries(1), |value| {
            // Force one reask even when the model's first answer is correct.
            let answer = if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                Err("Check both fields against the original text and submit them again.".into())
            } else {
                Ok(value)
            };
            std::future::ready(answer)
        })
        .await?;
    assert_eq!(result, ali);
    assert_eq!(attempts.load(Ordering::Relaxed), 2);

    let mut updates = client.create_partial::<Person>(person).await?;
    let mut completed = None;
    while let Some(update) = updates.next().await {
        assert!(
            completed.is_none(),
            "received another event after completion"
        );
        if let PartialEvent::Complete(person) = update? {
            completed = Some(person);
        }
    }
    assert_eq!(completed.as_ref(), Some(&ali));

    let list = request("List both people in order. Ali is 28 years old. Bo is 35 years old.");
    let people: Vec<Person> = client
        .create_iterable::<Person>(list)
        .await?
        .try_collect()
        .await?;
    assert_eq!(people, [ali, bo]);
    Ok(())
}

#[tokio::test]
#[ignore = "live API, requires OPENAI_API_KEY"]
async fn openai_chat() -> TestResult {
    let route = route("openai", "INSTRUCTOR_LIVE_OPENAI_MODEL", "gpt-6-luna");
    generation(&route, Mode::Tools).await
}

#[tokio::test]
#[ignore = "live API, requires OPENAI_API_KEY"]
async fn openai_responses() -> TestResult {
    let route = route("openai", "INSTRUCTOR_LIVE_OPENAI_MODEL", "gpt-6-luna");
    generation(&route, Mode::ResponsesTools).await
}

#[tokio::test]
#[ignore = "live API, requires ANTHROPIC_API_KEY"]
async fn anthropic_messages() -> TestResult {
    let route = route(
        "anthropic",
        "INSTRUCTOR_LIVE_ANTHROPIC_MODEL",
        "claude-haiku-5-5",
    );
    generation(&route, Mode::Tools).await
}

#[tokio::test]
#[ignore = "live API, requires OPENROUTER_API_KEY"]
async fn openrouter_chat() -> TestResult {
    let route = route(
        "openrouter",
        "INSTRUCTOR_LIVE_OPENROUTER_MODEL",
        "google/gemini-3.8-flash",
    );
    generation(&route, Mode::Tools).await
}

/// Which team handles the customer's request?
#[derive(Choices, Debug, PartialEq)]
enum Department {
    Billing,
    Technical,
}

#[derive(DecisionModel)]
struct Ticket {
    department: Department,
    #[decision(Noul::new("Is the customer requesting a refund?"))]
    refund: f64,
    #[decision(Score::new(
        "How urgently does the customer need help?",
        ["Not urgent", "Somewhat urgent", "Emergency"]
    ).scale(0.0, 10.0))]
    urgency: f64,
}

async fn decisions(route: &str) -> TestResult {
    let client = DecisionsClient::from_provider(route)?;
    let context =
        json!({"ticket": "Please refund the duplicate charge on my invoice. There is no rush."});
    let (ticket, raw) = client.create_with_completion::<Ticket>(context).await?;
    assert_eq!(ticket.department, Department::Billing);
    assert!(ticket.refund > 0.5 && ticket.refund <= 1.0);
    assert!((0.0..=10.0).contains(&ticket.urgency));
    let answers = &raw["answers"];
    let (choice, probability, score) = if route.starts_with("openai/") {
        (&answers[0], &answers[1]["probability"], &answers[2])
    } else {
        (
            &answers["department"],
            &answers["refund"]["noul"],
            &answers["urgency"],
        )
    };
    assert_eq!(choice["choice"], "billing");
    assert!(choice.get("probabilities").is_some());
    assert_eq!(probability.as_f64(), Some(ticket.refund));
    let native_score = score["score"].as_f64().ok_or("expected a numeric score")?;
    assert!((ticket.urgency - 5.0 * native_score).abs() < 1e-10);
    Ok(())
}

#[tokio::test]
#[ignore = "live Decisions API, requires OPENAI_API_KEY and model access"]
async fn openai_decisions() -> TestResult {
    let route = route(
        "openai",
        "INSTRUCTOR_LIVE_OPENAI_DECISIONS_MODEL",
        "gpt-6-luna",
    );
    decisions(&route).await
}

#[tokio::test]
#[ignore = "live Jev API, requires OPENROUTER_API_KEY"]
async fn openrouter_jev() -> TestResult {
    let route = route(
        "openrouter",
        "INSTRUCTOR_LIVE_OPENROUTER_JEV_MODEL",
        "typesafe/jev-1.13",
    );
    decisions(&route).await
}

#[tokio::test]
#[ignore = "live Jev API, requires TYPESAFE_API_KEY"]
async fn typesafe_jev() -> TestResult {
    let route = route(
        "typesafe",
        "INSTRUCTOR_LIVE_TYPESAFE_JEV_MODEL",
        "jev-latest",
    );
    decisions(&route).await
}
