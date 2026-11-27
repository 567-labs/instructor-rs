//! Opt-in provider contracts. See docs/parity.md for credentials and invocation.
use instructor_ai::{
    decisions::{Choices, DecisionError, DecisionModel, DecisionsConfig, Noul, Score},
    Mode, ProviderConfig,
};
use serde_json::{json, Value};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const REPLAY_KEY: &str = "synthetic-replay";

// Enabled only in the binary built by scripts/paired_replay.py. It requires an
// explicit port and constructs clients without reading credential variables.
fn replay_host() -> Option<String> {
    option_env!("INSTRUCTOR_BUILD_REPLAY").map(|_| {
        let port = std::env::var("INSTRUCTOR_REPLAY_PORT")
            .expect("replay port required")
            .parse::<std::num::NonZeroU16>()
            .expect("invalid replay port");
        format!("http://127.0.0.1:{port}")
    })
}

fn generation_config(route: &str) -> Result<ProviderConfig, instructor_ai::Error> {
    if let Some(host) = replay_host() {
        let (provider, model) = route.split_once('/').unwrap();
        let provider: instructor_ai::Provider = provider.parse()?;
        let base = reqwest::Url::parse(provider.default_base_url()).unwrap();
        Ok(ProviderConfig::new(provider, model, REPLAY_KEY)
            .with_base_url(format!("{host}{}", base.path())))
    } else {
        ProviderConfig::from_env(route)
    }
}

fn route(provider: &str, variable: &str, default_model: &str) -> String {
    let model = std::env::var(variable).unwrap_or_else(|_| default_model.to_owned());
    format!("{provider}/{model}")
}

fn generation_route(
    provider: &str,
    default: &str,
    read: impl Fn(&str) -> Option<String>,
) -> String {
    let variable = format!("INSTRUCTOR_LIVE_{}_MODEL", provider.to_ascii_uppercase());
    let nonempty = |key: &str| read(key).filter(|s| !s.trim().is_empty());
    let configured = nonempty(&variable).or_else(|| {
        if provider == "google" {
            nonempty("GOOGLE_GENAI_MODEL")
        } else {
            None
        }
    });
    let model = configured.as_deref().unwrap_or(default).trim();
    let model = if provider == "google" {
        model
            .strip_prefix("google/")
            .or_else(|| model.strip_prefix("gemini/"))
            .unwrap_or(model)
    } else {
        model
    };
    format!("{provider}/{model}")
}

#[path = "support/generation.rs"]
mod generation;
use generation::Generation;

macro_rules! cases {
    ($($name:ident),+ $(,)?) => {
        $(
            #[tokio::test]
            #[ignore = "live provider API, see docs/parity.md for credentials"]
            async fn $name()->TestResult {
                core()?.$name().await?;
                Ok(())
            }
        )+
    }
}

macro_rules! provider_suite {
    ($name:ident, $provider:literal, $model:literal $(=> $mode:expr)? $(, $documented:ident)?) => {
        mod $name {
            use super::*;

            pub(super) fn route(read: impl Fn(&str) -> Option<String>) -> String {
                generation_route($provider, $model, read)
            }

            fn core()->Result<Generation,instructor_ai::Error>{
                let route = route(|key| std::env::var(key).ok());
                Generation::new(generation_config(&route)?$(.with_mode($mode))?)
            }

            cases!(
                test_int,
                test_simple_extraction,
                test_enum_extraction,
                test_list_extraction,
                test_nested_model_extraction,
                test_nested_enum_collection,
                test_validation_with_retries,
                test_partial_streaming,
                test_iterable_streaming,
            );
            $(
                #[tokio::test]
                #[ignore = "live documented model, see docs/parity.md for credentials"]
                async fn $documented()->TestResult {
                    core()?.test_current_documented_model().await?;
                    Ok(())
                }
            )?
        }
    }
}

provider_suite!(
    openai_structured,
    "openai",
    "gpt-6-luna",
    test_current_documented_model
);
provider_suite!(
    openai_responses,
    "openai",
    "gpt-6-luna" => Mode::ResponsesTools
);
provider_suite!(
    anthropic_messages,
    "anthropic",
    "claude-haiku-5-5",
    test_current_documented_model
);
provider_suite!(
    openrouter_responses,
    "openrouter",
    "openai/gpt-6.2-sol",
    test_current_documented_model
);
provider_suite!(
    google_interactions,
    "google",
    "gemini-4-hydrogen",
    test_current_documented_model
);
provider_suite!(
    cohere_chat,
    "cohere",
    "command-a-plus-05-2026",
    test_current_documented_model
);
provider_suite!(
    mistral_responses,
    "mistral",
    "mistral-large-4",
    test_current_documented_model
);

#[test]
fn standalone_generation_defaults_follow_the_guide_and_allow_overrides() {
    assert_eq!(
        openrouter_responses::route(|_| None),
        "openrouter/openai/gpt-6.2-sol"
    );
    assert_eq!(
        google_interactions::route(|_| None),
        "google/gemini-4-hydrogen"
    );
    assert_eq!(
        cohere_chat::route(|_| None),
        "cohere/command-a-plus-05-2026"
    );
    assert_eq!(
        mistral_responses::route(|_| Some(" ".into())),
        "mistral/mistral-large-4"
    );
    for model in ["google/custom", "gemini/custom", "custom"] {
        let route =
            google_interactions::route(|key| (key == "GOOGLE_GENAI_MODEL").then(|| model.into()));
        assert_eq!(route, "google/custom");
    }
    let route = google_interactions::route(|key| match key {
        "INSTRUCTOR_LIVE_GOOGLE_MODEL" => Some("google/override".into()),
        _ => Some("ignored".into()),
    });
    assert_eq!(route, "google/override");
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

// Mirrors the route matrix, ticket and assertions in Python Instructor's
// tests/llm/test_decisions/{conftest,test_live_decisions}.py.
struct DecisionRoute {
    provider: &'static str,
    default_model: &'static str,
    variable: &'static str,
    legacy_variable: &'static str,
}

const OPENAI_DECISIONS: DecisionRoute = DecisionRoute {
    provider: "openai",
    default_model: "gpt-6-luna",
    variable: "OPENAI_DECISIONS_MODEL",
    legacy_variable: "INSTRUCTOR_LIVE_OPENAI_DECISIONS_MODEL",
};

const OPENROUTER_JEV: DecisionRoute = DecisionRoute {
    provider: "openrouter",
    default_model: "typesafe/jev-1.14",
    variable: "OPENROUTER_DECISIONS_MODEL",
    legacy_variable: "INSTRUCTOR_LIVE_OPENROUTER_JEV_MODEL",
};

const TYPESAFE_JEV: DecisionRoute = DecisionRoute {
    provider: "typesafe",
    default_model: "jev-latest",
    variable: "TYPESAFE_DECISIONS_MODEL",
    legacy_variable: "INSTRUCTOR_LIVE_TYPESAFE_JEV_MODEL",
};

impl DecisionRoute {
    fn config(&self) -> Result<DecisionsConfig, DecisionError> {
        let config = DecisionsConfig::new(&self.spec())?;
        let Some(host) = replay_host() else {
            return Ok(config);
        };
        let path = match self.provider {
            "openai" => "/v1/decisions",
            "openrouter" => "/api/alpha/decisions",
            "typesafe" => "/v1/systemone",
            _ => unreachable!("unknown replay provider"),
        };
        Ok(config.api_key(REPLAY_KEY).endpoint(format!("{host}{path}")))
    }

    fn spec(&self) -> String {
        let legacy = std::env::var(self.legacy_variable)
            .ok()
            .filter(|value| !value.trim().is_empty());
        route(
            self.provider,
            self.variable,
            legacy.as_deref().unwrap_or(self.default_model),
        )
    }
}

fn decision_context() -> Value {
    json!({"ticket": "Please refund the duplicate charge on my invoice. There is no rush."})
}

fn finite_number(value: &Value) -> f64 {
    value
        .as_f64()
        .filter(|n| n.is_finite())
        .unwrap_or_else(|| panic!("expected a finite JSON number, received {value}"))
}

fn probability(value: &Value) -> f64 {
    let number = finite_number(value);
    assert!(
        (0.0..=1.0).contains(&number),
        "out-of-range probability {number}"
    );
    number
}

fn assert_distribution(answer: &Value, labels: &[&str], openai: bool) {
    let values = &answer["probabilities"];
    let entries: Vec<(String, f64)> = if openai {
        values
            .as_array()
            .expect("OpenAI probabilities must be an array")
            .iter()
            .map(|item| {
                let label = match &item["value"] {
                    Value::String(label) => label.clone(),
                    value => value
                        .as_u64()
                        .expect("expected a string or integer label")
                        .to_string(),
                };
                (label, probability(&item["probability"]))
            })
            .collect()
    } else {
        values
            .as_object()
            .expect("Jev probabilities must be keyed by label")
            .iter()
            .map(|(label, value)| (label.clone(), probability(value)))
            .collect()
    };
    assert_eq!(entries.len(), labels.len());
    for label in labels {
        assert!(
            entries.iter().any(|(key, _)| key == label),
            "missing probability for {label}"
        );
    }
    let sum: f64 = entries.iter().map(|(_, value)| value).sum();
    assert!((sum - 1.0).abs() <= 1e-5, "probabilities sum to {sum}");
    probability(&answer["confidence"]);
}

fn assert_decision(provider: &str, ticket: &Ticket, raw: &Value) {
    assert_eq!(ticket.department, Department::Billing);
    assert!(ticket.refund > 0.5);
    assert!((0.0..=10.0).contains(&ticket.urgency));
    assert!(raw["model"].as_str().is_some_and(|model| !model.is_empty()));

    let answers = &raw["answers"];
    let openai = provider == "openai";
    let (choice, refund, score) = if openai {
        let answers = answers.as_array().expect("OpenAI answers must be an array");
        assert_eq!(answers.len(), 3);
        assert_eq!(answers[0]["name"], "department");
        assert_eq!(answers[1]["name"], "refund");
        assert_eq!(answers[2]["name"], "urgency");
        (&answers[0], &answers[1], &answers[2])
    } else {
        assert!(
            answers.is_object(),
            "Jev answers must be keyed by field name"
        );
        (
            &answers["department"],
            &answers["refund"],
            &answers["urgency"],
        )
    };
    assert_eq!(choice["type"], "choice");
    assert_eq!(choice["choice"], "billing");
    let (kind, key) = if openai {
        ("predicate", "probability")
    } else {
        ("noul", "noul")
    };
    assert_eq!(refund["type"], kind);
    assert_eq!(probability(&refund[key]), ticket.refund);
    assert_eq!(score["type"], "score");
    let expected = finite_number(&score["score"]) * 5.0;
    let tolerance = 1e-12_f64.max(expected.abs() * 1e-6);
    assert!((ticket.urgency - expected).abs() <= tolerance);

    let levels = ["0", "1", "2"];
    assert_distribution(choice, &["billing", "technical"], openai);
    assert_distribution(score, &levels, openai);
    if !openai {
        let legend = score["legend"]
            .as_object()
            .expect("expected a Jev score legend");
        assert_eq!(legend.len(), levels.len());
        assert!(levels.iter().all(|level| legend.contains_key(*level)));
    }

    let usage = &raw["usage"];
    let input = usage["input_tokens"]
        .as_u64()
        .expect("expected integer input_tokens");
    let output = usage["output_tokens"]
        .as_u64()
        .expect("expected integer output_tokens");
    assert!(input > 0);
    if openai {
        assert_eq!(output, 0);
    } else {
        assert!(output > 0);
    }
    if provider == "openrouter" {
        assert!(finite_number(&usage["cost"]) >= 0.0);
    }
}

async fn decisions(route: DecisionRoute) -> TestResult {
    let client = route.config()?.build()?;
    let (ticket, raw) = client
        .create_with_completion::<Ticket>(decision_context())
        .await?;
    assert_decision(route.provider, &ticket, &raw);
    Ok(())
}

fn blocking_decisions(route: DecisionRoute) -> TestResult {
    let client = route.config()?.build_blocking()?;
    let (ticket, raw) = client.create_with_completion::<Ticket>(decision_context())?;
    assert_decision(route.provider, &ticket, &raw);
    Ok(())
}

#[tokio::test]
#[ignore = "live Decisions API, requires OPENAI_API_KEY and model access"]
async fn openai_decisions() -> TestResult {
    decisions(OPENAI_DECISIONS).await
}

#[test]
#[ignore = "live Decisions API, requires OPENAI_API_KEY and model access"]
fn openai_decisions_blocking() -> TestResult {
    blocking_decisions(OPENAI_DECISIONS)
}

#[tokio::test]
#[ignore = "live Jev API, requires OPENROUTER_API_KEY"]
async fn openrouter_jev() -> TestResult {
    decisions(OPENROUTER_JEV).await
}

#[test]
#[ignore = "live Jev API, requires OPENROUTER_API_KEY"]
fn openrouter_jev_blocking() -> TestResult {
    blocking_decisions(OPENROUTER_JEV)
}

#[tokio::test]
#[ignore = "live Jev API, requires TYPESAFE_API_KEY"]
async fn typesafe_jev() -> TestResult {
    decisions(TYPESAFE_JEV).await
}

#[test]
#[ignore = "live Jev API, requires TYPESAFE_API_KEY"]
fn typesafe_jev_blocking() -> TestResult {
    blocking_decisions(TYPESAFE_JEV)
}
