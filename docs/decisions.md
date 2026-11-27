# Typed decisions with OpenAI and Jev

A decision model asks several questions about the same evidence in one HTTP request. Each field returns a label, a probability, or a weighted score. Use it to route a support ticket, assess whether it asks for a refund, and estimate its urgency.

## Choose a route

| Provider and model | Credential | Endpoint |
| --- | --- | --- |
| `openrouter/typesafe/jev-1.13` | `OPENROUTER_API_KEY` | `https://openrouter.ai/api/alpha/decisions` |
| `typesafe/jev-latest` | `TYPESAFE_API_KEY` | `https://api.typesafe.ai/v1/systemone` |
| `openai/gpt-6-luna` | `OPENAI_API_KEY` | `https://api.openai.com/v1/decisions` |

The first path segment selects the provider. OpenRouter receives the full model identifier `typesafe/jev-1.13`. Set the credential for your route and run the example from the workspace root.

```sh
cargo run -p instructor-ai --example decisions -- openrouter/typesafe/jev-1.13
```

## Classify a ticket

```rust
use instructor_ai::decisions::{Choices, DecisionModel, DecisionsClient, Level, Noul, Score};
use serde_json::json;

/// Which team handles this {{ item.kind }}?
#[derive(Choices, Debug)]
enum Team {
    #[choice(description = "Subscription charges", examples = ["I was charged twice."])]
    Billing,
    /// Sign-in problems
    Access,
    #[choice(value = "general")]
    Other,
}

#[derive(DecisionModel)]
struct Ticket {
    team: Team,
    #[decision(Noul::new("Is a refund requested?")
        .when_true("Requests money back")
        .when_false("Only asks for information")
        .example("Please reverse the charge", true))]
    refund: f64,
    #[decision(Score::new("How urgent?", [
        Level::new("Routine"),
        Level::new("Action needed").example("Duplicate charge"),
        Level::new("Blocking"),
    ]).scale(0.0, 10.0))]
    urgency: f64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = DecisionsClient::from_provider("openrouter/typesafe/jev-1.13")?;
    let context = json!({"item": {"kind": "ticket", "body": "Please refund my duplicate charge."}});
    let (ticket, raw) = client.create_with_completion::<Ticket>(context).await?;
    println!("{:?} {} {}", ticket.team, ticket.refund, ticket.urgency);
    println!("Team distribution {}", raw["answers"]["team"]["probabilities"]);
    Ok(())
}
```

The type parameter selects the model, just as Python's `response_model=Ticket` does. For the typed result alone, use `let ticket: Ticket = client.create(context).await?;`. Match on `ticket.team` as an enum. The derives construct the result directly, so Serde attributes on your application types can independently control storage and serialization. Field names identify questions on the wire.

`create_with_completion` also returns the full provider JSON as a `serde_json::Value`. Jev indexes its answers by field name. OpenAI returns an array in question order. The raw answers retain provider units, probability distributions, confidence, legends, and any additional fields returned by the route. Usage and cost metadata depend on the provider.

## Define the questions

| Python annotation | Rust field |
| --- | --- |
| `Choices` or a string enum | A unit enum deriving `Choices`. Each label defaults to its lowercase variant name. Use `#[choice(value = "label")]` to set it explicitly. |
| `Question` or `Field(description=...)` | Use `#[decision(Question::new(...))]` on an enum field. Instructions default to the field description, then the enum description. Set descriptions with `#[description("...")]` or doc comments. |
| `Literal` or an annotated union | Use `Question::choices` for inline labels on a `String`, or a derived enum for exhaustive matching. |
| `Annotated[float, Noul(...)]` | Use `#[decision(Noul::new(...))]` on `f64` for the probability of true in the interval from 0 to 1. |
| `Annotated[float, Score(...)]` | Use `#[decision(Score::new(...))]` on `f64` for a weighted score over 2–10 ordered levels. |

```rust
use instructor_ai::decisions::{Choice, DecisionModel, Noul, Question};
use serde_json::json;

#[derive(DecisionModel)]
struct Message {
    #[decision(Question::choices("Which channel?", ["email", "chat"]))]
    channel: String,
    #[decision(Question::choices(json!({"question": "What does {{ customer }} need?"}), [
        Choice::new("refund").description("Money back").example("Reverse the charge"),
        Choice::new("access").description(json!(["Password reset", "Account recovery"])),
    ]))]
    intent: String,
    #[decision(Noul::new("Does the customer need a reply?")
        .criteria(json!({"true": ["Asks a question"], "false": "Acknowledgment"})))]
    needs_reply: f64,
}
```

A choice requires 1–255 unique, nonempty labels. Instructions accept a nonempty string, object, or array. Nested instruction strings render against the context using MiniJinja with strict undefined values. Criteria are literal. A variant's documentation supplies its description when the `choice` attribute omits one. For a predicate, supply either `when_true` and `when_false` meanings or a `criteria` object keyed by `"true"` and `"false"`.

Use `.example(value)` on a choice or level, `.example(value, bool)` on a predicate, or `.example(value, index)` on a score. The library groups examples with their meanings and formats inputs with Rust's `ToString`. Use strings when their exact spelling matters.

With three levels, a score spans 0–2. Applying `.scale(0.0, 10.0)` maps a raw 1.4 to 7.0. Bounds must be finite and increasing. Scaling applies before application validation.

## Use native OpenAI input

Jev sends the entire context object as provider state. OpenAI sends the context as JSON text when it is the only argument. To send text or native content parts, use `DecisionRequest::input`. The following continues the ticket example.

```rust
use instructor_ai::decisions::DecisionRequest;

let client = DecisionsClient::from_provider("openai/gpt-6-luna")?;
let request = DecisionRequest::input(json!([{
    "role": "user",
    "content": [
        {"type": "input_json", "data": {"issue": "duplicate charge"}},
        {"type": "input_image", "image_url": "https://example.com/invoice.png"}
    ]
}])).with_context(json!({"item": {"kind": "ticket"}}));
let ticket: Ticket = client.create(request).await?;
```

Replace the example URL with an accessible image or a base64 data URL. For text, use `DecisionRequest::input("My invoice shows two charges.")`. With native input, context drives templates and application validation. Interpolated values reach the provider in the rendered question. OpenAI receives predicates for `Noul`, an ordered list of choices, and labeled score levels. Structured question metadata is encoded as text.

## Validate answers and inspect failures

The same `#[validate(custom = "check")]` and `#[validate(context = "check")]` attributes used for structured generation work with decision models. Each answer passes through its choice enum's checks and its field checks. Model checks run after field validation. Scores reach these functions in application units after scaling. Add `#[model]` alongside `#[derive(DecisionModel)]` when the type also needs a structured generation schema and Serde support. Apply `#[model]` to its choice enums as well.

Add `#[decision(validate = check_ticket)]` to the `Ticket` struct when the final model check should also receive the request's strict flag.

```rust
fn check_ticket(ticket: &mut Ticket, context: &serde_json::Value, strict: bool) -> Result<(), String> {
    if strict && context["requires_team"] == true && matches!(ticket.team, Team::Other) {
        return Err("This ticket requires a named team".into());
    }
    Ok(())
}
```

`DecisionRequest::context(context).strict(false)` passes `false` to this callback. Rust field types, label membership, and numeric ranges are enforced for every request. Application validators receive the original context. The result can be transformed in place.

`DecisionError` identifies the failing stage. Invalid question specifications and templates fail before sending. HTTP errors retain the status, request ID header and response bytes. Use `error.http_details()` for the status, request ID and provider code in an incident report. Missing answers, mismatched types, unknown labels, refusals, and out-of-range values are answer errors. OpenAI responses also require matching names, count, and order. Application failures produce `DecisionError::Validation`. [HTTP diagnostics](concepts.md#diagnose-a-failed-http-request)

For an offline check, construct `PreparedDecision::<Ticket>::new(route, context)?`. Inspect its `.body()` and call `.parse(&raw)?` against a saved JSON response. The clients use the same preparation and parsing code.

## Configure the transport

Call `.decisions()?` on a configured OpenAI or OpenRouter `Client` or `BlockingClient` to reuse its model identifier, credentials, timeout, and connection pool. OpenRouter's versioned base ending in `/v1` maps to the corresponding `/alpha/decisions` endpoint. Other bases use the `/decisions` suffix. The following continues the async ticket example.

```rust
let client = instructor_ai::from_provider("openrouter/typesafe/jev-1.13")?;
let decisions = client.decisions()?;
let ticket: Ticket = decisions.create(json!({
    "item": {"kind": "ticket", "body": "Please refund this duplicate charge."}
})).await?;
```

Use `BlockingDecisionsClient::from_provider(route)?` from synchronous code and call the same methods without `.await`. Built-in clients use a 60-second timeout. Customize them with `DecisionsConfig::new(route)?`, `.api_key(key)`, `.endpoint(full_url)`, and `.timeout(Some(duration))`, then `.build()?` or `.build_blocking()?`. Explicit keys take precedence over provider environment variables. `.timeout(None)` disables the built-in deadline.

To reuse a transport, finish the configuration with `.with_client(http.clone())?` or `.with_blocking_client(http.clone())?`. A supplied Reqwest transport keeps its timeout unless you set a duration for the decision request. To disable its timeout, configure the transport itself. Cloned handles share the underlying connection pool. Rust releases the pool when its last handle drops.

Like Python, Decisions performs a single request and returns the complete answer set. Streaming, reasks, caching, and completion hooks belong to the structured generation client. [Python Decisions guide](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/decisions.md)
