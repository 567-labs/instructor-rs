# Define a typed response

Use the model attribute to derive JSON Schema, decoding and validation from one Rust declaration. Run the local example to inspect a schema and parse a meeting with a tagged enum, a list and an optional duration. [Response types](types.md) · [Validation](concepts.md)

```sh
cargo run -p instructor-ai --example types
```

The OpenAI SDK entry point accepts a derived struct as its response model. See the repository README for the request syntax.
