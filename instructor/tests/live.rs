//! Opt-in provider contracts. See docs/providers.md for generation credentials.
use futures_util::{StreamExt, TryStreamExt};
use instructor_ai::{model, Client, Mode, PartialEvent, ProviderConfig, Request};
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
