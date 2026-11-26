//! Generation checks called with real provider credentials by live.rs and
//! deterministic HTTP fixtures by providers/complexity.rs. Case names and
//! inputs follow tests/llm/test_core_providers in Python Instructor.
use futures_util::{StreamExt, TryStreamExt};
use instructor_ai::{model, Client, Error, Mode, PartialEvent, Provider, ProviderConfig, Request};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

#[model]
struct ExtractedNumber {
    value: i32,
}

#[model]
#[derive(Debug, PartialEq)]
pub(super) struct User {
    name: String,
    age: i32,
}

#[model]
struct Address {
    street: String,
    city: String,
    country: String,
}

#[model]
struct UserWithAddress {
    name: String,
    age: i32,
    address: Address,
}

#[model]
struct RetriedUser {
    #[validate(context = "normalize_name")]
    name: String,
    #[validate(custom = "check_age")]
    #[schemars(range(min = 0, max = 120))]
    age: i32,
}

fn normalize_name(name: &mut String, _: &Value) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Name must not be empty".into());
    }
    *name = name.trim().into();
    Ok(())
}

fn check_age(age: &i32) -> Result<(), String> {
    if (0..=120).contains(age) {
        Ok(())
    } else {
        Err("Age must be between 0 and 120".into())
    }
}

// --8<-- [start:shipment-models]
#[model]
#[derive(Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ShipmentState {
    Packed,
    InTransit,
    Delivered,
}

#[model]
struct Package {
    tracking_id: String,
    state: ShipmentState,
    carrier: Option<String>,
}

#[model]
struct Manifest {
    depot: String,
    packages: Vec<Package>,
}
// --8<-- [end:shipment-models]

fn check_users(users: Vec<User>, expected: &[(&str, i32)]) {
    let mut pairs = users
        .iter()
        .map(|u| (u.name.as_str(), u.age))
        .collect::<Vec<_>>();
    pairs.sort_unstable();
    assert_eq!(pairs, expected);
}

pub(super) struct Generation {
    client: Client,
    provider: Provider,
    mode: Mode,
    disable_reasoning: bool,
}

impl Generation {
    pub(super) fn new(config: ProviderConfig) -> Result<Self, Error> {
        Ok(Self {
            provider: config.provider(),
            mode: config.mode(),
            disable_reasoning: config.provider() == Provider::OpenAI
                && config.model() == "gpt-6-luna",
            client: Client::new(config)?,
        })
    }

    fn request(&self, prompt: &str) -> Request {
        let req = Request::new(prompt);
        let req = match (self.provider, self.mode) {
            (_, Mode::ResponsesTools) => req.option("max_output_tokens", 2048),
            (Provider::OpenAI, _) => req.option("max_completion_tokens", 2048),
            (Provider::Google, _) => {
                req.option("generationConfig", json!({"maxOutputTokens":2048}))
            }
            _ => req.option("max_tokens", 2048),
        };
        if self.disable_reasoning {
            if self.mode == Mode::ResponsesTools {
                req.option("reasoning", json!({"effort":"none"}))
            } else {
                req.option("reasoning_effort", "none")
            }
        } else {
            req
        }
    }

    pub(super) async fn test_current_documented_model(&self) -> Result<(), Error> {
        let req = self.request("Extract the integer 42.").max_retries(1);
        let result: ExtractedNumber = self.client.create(req).await?;
        assert_eq!(result.value, 42);
        Ok(())
    }

    pub(super) async fn test_int(&self) -> Result<(), Error> {
        let req = self.request("Return the number 42");
        let result: i32 = self.client.create(req).await?;
        assert_eq!(result, 42);
        Ok(())
    }

    pub(super) async fn test_simple_extraction(&self) -> Result<(), Error> {
        let req = self.request("Extract: Jason is 25 years old");
        let (user, raw) = self.client.create_with_completion::<User>(req).await?;
        assert_eq!(user.name, "Jason");
        assert_eq!(user.age, 25);
        assert!(raw.is_object());
        Ok(())
    }

    pub(super) async fn test_enum_extraction(&self) -> Result<(), Error> {
        let req = self.request("The package is in transit. Extract its shipment state.");
        let state: ShipmentState = self.client.create(req).await?;
        assert_eq!(state, ShipmentState::InTransit);
        Ok(())
    }

    pub(super) async fn test_list_extraction(&self) -> Result<(), Error> {
        let req = self.request("Extract: Alice is 30, Bob is 25, Charlie is 35");
        let users: Vec<User> = self.client.create(req).await?;
        check_users(users, &[("Alice", 30), ("Bob", 25), ("Charlie", 35)]);
        Ok(())
    }

    pub(super) async fn test_nested_model_extraction(&self) -> Result<(), Error> {
        let req =
            self.request("Extract: John Doe, 28 years old, lives at 123 Main St, Springfield, USA");
        let user: UserWithAddress = self.client.create(req).await?;
        assert_eq!(user.name, "John Doe");
        assert_eq!(user.age, 28);
        assert_eq!(user.address.street, "123 Main St");
        assert_eq!(user.address.city, "Springfield");
        assert_eq!(user.address.country, "USA");
        Ok(())
    }

    pub(super) async fn test_nested_enum_collection(&self) -> Result<(), Error> {
        let req = self.request(
            "Extract the manifest for the North depot in the given order. \
            Package A1 is packed, carrier unknown. Package B2 is in transit \
            with carrier Swift. Use null for an unknown carrier.",
        );
        let manifest: Manifest = self.client.create(req).await?;
        assert_eq!(manifest.depot, "North");
        assert_eq!(manifest.packages.len(), 2);
        let first = &manifest.packages[0];
        assert_eq!(first.tracking_id, "A1");
        assert_eq!(first.state, ShipmentState::Packed);
        assert!(first.carrier.is_none());
        let second = &manifest.packages[1];
        assert_eq!(second.tracking_id, "B2");
        assert_eq!(second.state, ShipmentState::InTransit);
        assert_eq!(second.carrier.as_deref(), Some("Swift"));
        Ok(())
    }

    pub(super) async fn test_validation_with_retries(&self) -> Result<(), Error> {
        let req = self
            .request("Extract: Sarah Johnson is 25 years old")
            .max_retries(2);
        let attempts = AtomicUsize::new(0);
        let result: RetriedUser = self
            .client
            .create_validated(req, |value| {
                let answer = if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                    Err("Check both fields against the original text and submit them again.".into())
                } else {
                    Ok(value)
                };
                std::future::ready(answer)
            })
            .await?;
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
        assert_eq!(result.name, "Sarah Johnson");
        assert_eq!(result.age, 25);
        Ok(())
    }

    pub(super) async fn test_partial_streaming(&self) -> Result<(), Error> {
        let req = self.request("Jason Liu is 30 years old");
        let mut updates = self.client.create_partial::<User>(req).await?;
        let mut completed = None;
        while let Some(update) = updates.next().await {
            assert!(
                completed.is_none(),
                "received another event after completion"
            );
            if let PartialEvent::Complete(user) = update? {
                completed = Some(user);
            }
        }
        assert_eq!(
            completed,
            Some(User {
                name: "Jason Liu".into(),
                age: 30
            })
        );
        Ok(())
    }

    pub(super) async fn test_iterable_streaming(&self) -> Result<(), Error> {
        let req = self.request("Create 3 users: Alice (25), Bob (30), Carol (35)");
        let users = self
            .client
            .create_iterable::<User>(req)
            .await?
            .try_collect()
            .await?;
        check_users(users, &[("Alice", 25), ("Bob", 30), ("Carol", 35)]);
        Ok(())
    }
}
