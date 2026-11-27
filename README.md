# Instructor for Rust

Get a validated Rust value from an LLM. Define the fields in a struct, use enums for the choices your application accepts, and call `client.create(prompt)`. Instructor derives the schema from your type and can ask the model to correct an invalid reply.

## Route your first ticket

Turn a customer message into a `Ticket` and route it with a Rust `match`. With a recent stable Rust toolchain, create a project and add Instructor. In an existing project, run just the `cargo add` command. Use the Git revision below while these APIs are awaiting release on crates.io.

```sh
cargo new ticket-router
cd ticket-router
cargo add instructor-ai --git https://github.com/567-labs/instructor-rs \
  --rev 278dcced36a2b5b076d7b1fd20606eaabc7a75de
```

If you have an `instructor-rs` checkout beside `ticket-router`, you can add it with `cargo add instructor-ai --path ../instructor-rs/instructor`.

Replace `src/main.rs` with this program. The `Ticket` annotation tells `create` what to ask for. You can try it with OpenAI or use the saved reply below.

```rust
use instructor_ai::{model, Error};

#[model]
#[serde(rename_all = "snake_case")]
enum Queue {
    Billing,
    Shipping,
    Other,
}

#[model]
struct Ticket {
    queue: Queue,
    order_id: String,
}

fn main() -> Result<(), Error> {
    let client = instructor_ai::BlockingClient::from_provider("openai/gpt-6-luna")?;
    let ticket: Ticket = client.create("Route this ticket: I was charged twice for order A17.")?;

    let action = match ticket.queue {
        Queue::Billing => "Review charges",
        Queue::Shipping => "Check tracking",
        Queue::Other => "Read the ticket",
    };
    println!("{action} for order {}", ticket.order_id);
    Ok(())
}
```

Export `OPENAI_API_KEY` in your shell and run the program. For this message, you should see `Review charges for order A17`.

```sh
cargo run
```

## Try a saved reply without a key

Replace the first two lines inside `main` (the `client` and `ticket` bindings) with these lines.

```rust
let saved_reply = r#"{"queue":"billing","order_id":"A17"}"#;
let ticket: Ticket = instructor_ai::parse_response(saved_reply, &Default::default())?;
```

Run `cargo run` to route the saved ticket locally. Change `billing` to `shipping` to print `Check tracking for order A17`, or to `sales` to see the parser reject an unknown queue. If you saved the entire provider response with `create_with_completion`, use `parse_completion` to read it. [Parse a saved completion](docs/types.md#parse-a-saved-completion)

## Choose your provider

In the original program, change the model argument to `BlockingClient::from_provider` and export the corresponding environment variable with your API key. Choose a model your account can access.

| Provider | Model argument | Environment variable |
| --- | --- | --- |
| OpenAI | `"openai/gpt-6-luna"` | `OPENAI_API_KEY` |
| Anthropic | `"anthropic/claude-haiku-5-5"` | `ANTHROPIC_API_KEY` |
| OpenRouter | `"openrouter/openai/gpt-6.2-sol"` | `OPENROUTER_API_KEY` |

You can also use Google Gemini, Cohere or Mistral. [Provider setup and supported modes](docs/providers.md)

## Build on the example

- **In a Tokio service**, use `Client::from_provider` and await `client.create::<Ticket>(message)`. [Async quickstart](docs/index.md#call-from-tokio)
- **Check an order ID against your records** with a validator. Instructor can send your error message back to the model and retry within your budget. [Response types](docs/types.md) · [Validation and retries](docs/concepts.md)
- **Show a ticket as it fills in** with `create_partial::<Ticket>`. For a response containing several tickets, use `create_iterable::<Ticket>` to process each completed ticket as it arrives. [Streaming examples](docs/streaming.md)
- **Choose a queue and score urgency** with the Decisions API. You can also ask how likely it is that the customer wants a refund. Use Jev through TypeSafe or OpenRouter, or use OpenAI. [Typed decisions](docs/decisions.md)
- **Read a receipt**, check a quote against its source, or process a backlog with a batch job. [Media input](docs/media.md) · [Source citations](docs/grounded-extraction.md) · [Batch jobs](docs/batch.md)

Coming from Python Instructor? Check the supported workflows and measured performance for your use case. [Python comparison](docs/parity.md) · [Benchmarks](docs/benchmarks.md)
