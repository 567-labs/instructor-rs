# Extract data from media

Attach media to a user message with `Message::user(prompt).attach(media)`. Provide the bytes or a provider-readable HTTP(S) URL and the MIME type. Instructor encodes the attachment in the selected provider's content format. The output model, validators, and retry policy apply to the extracted data.

## Read a receipt

Select a vision-capable model, set its credential, and pass a PNG receipt as the second argument. The example reads the supplied file and sends its bytes with the prompt.

```sh
cargo run -p instructor-ai --example receipt -- anthropic/claude-haiku-5-5 receipt.png
```

```rust
--8<-- "instructor/examples/receipt.rs"
```

To send a link, construct `Media::url("image/png", url)`. Instructor forwards the URL to the provider for retrieval. The URL must be accessible to the selected API. Use `Media::bytes("application/pdf", pdf)` for an in-memory PDF. Repeated calls to `attach` append attachments in order after the text.

## Accepted inputs

The table describes the content blocks Rust can build. Model capabilities, size limits, and accepted URI sources depend on the provider. In the table, **both** means bytes and HTTP(S) URL. An unsupported combination returns a configuration error.

| API | Image | PDF | Audio |
| --- | --- | --- | --- |
| OpenAI Responses | Both | Both | Unsupported |
| Anthropic Messages | Both | Both | Unsupported |
| OpenRouter Responses | Both | Both | Bytes |
| Gemini | Both | Both | Both |
| Cohere | Both | Unsupported | Unsupported |
| Mistral Responses | Both | Both | Unsupported |

For an OpenAI model that accepts audio through Chat Completions, select `.with_chat_completions()` on the provider config and attach audio bytes. For PDFs on Mistral, use Responses and supply bytes or an HTTPS URL.

Supported image MIME types are `image/jpeg`, `image/png`, `image/webp`, and `image/gif`. Use `application/pdf` for documents, and `audio/wav` or `audio/mpeg` for audio. The caller supplies a MIME type matching the data. Gemini sends content blocks with `type` and `mime_type`, using `data` for inline bytes or `uri` for a URL. Use a URI its API accepts.

Python provides helpers to detect and load media from files and other sources. In Rust, the application opens the file or obtains the bytes explicitly. The declared MIME type and the selected source determine the provider encoding. [Python multimodal guide](https://github.com/567-labs/instructor/blob/b01fff4e01ecab2bdcea6ad24e795bc3f549b89d/docs/concepts/multimodal.md) · [Provider credentials and modes](providers.md)
