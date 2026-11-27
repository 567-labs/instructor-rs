//! Media is explicitly supplied by the caller. URL sources are forwarded to
//! the model provider. The client never opens local paths or fetches URLs.
use crate::{Error, Provider, ProviderConfig};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// An attachment for a user message. Use a MIME type appropriate to its bytes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Media {
    mime: String,
    source: Source,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Source {
    Base64(String),
    Url(String),
}

impl Media {
    pub fn bytes(mime: impl Into<String>, data: impl AsRef<[u8]>) -> Self {
        Self {
            mime: mime.into(),
            source: Source::Base64(STANDARD.encode(data)),
        }
    }

    pub fn url(mime: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            mime: mime.into(),
            source: Source::Url(url.into()),
        }
    }

    fn url_or_data_uri(&self) -> String {
        match &self.source {
            Source::Base64(data) => format!("data:{};base64,{data}", self.mime),
            Source::Url(url) => url.clone(),
        }
    }

    fn block(&self, config: &ProviderConfig) -> Result<Value, Error> {
        let provider = config.provider();
        let mode = config.mode();
        let unsupported = || {
            Error::Configuration(format!(
                "unsupported {} attachment or source for {provider} in {mode:?}",
                self.mime
            ))
        };
        let kind = match self.mime.as_str() {
            "image/jpeg" | "image/png" | "image/webp" | "image/gif" => "image",
            "application/pdf" => "document",
            "audio/wav" | "audio/mpeg" => "audio",
            _ => return Err(unsupported()),
        };
        match &self.source {
            Source::Url(url) => {
                let parsed = reqwest::Url::parse(url)
                    .map_err(|_| Error::Configuration("invalid media URL".into()))?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                {
                    return Err(Error::Configuration(
                        "expected an HTTP(S) media URL without user credentials".into(),
                    ));
                }
            }
            Source::Base64(data) if data.is_empty() => {
                return Err(Error::Configuration("empty attachment".into()))
            }
            _ => (),
        }
        let source = &self.source;
        match provider {
            Provider::Google => Ok(match source {
                Source::Base64(data) => json!({"inlineData":{"mimeType":self.mime,"data":data}}),
                Source::Url(url) => json!({"fileData":{"mimeType":self.mime,"fileUri":url}}),
            }),
            Provider::Anthropic if kind != "audio" => {
                let value = match source {
                    Source::Base64(data) => {
                        json!({"type":"base64","media_type":self.mime,"data":data})
                    }
                    Source::Url(url) => json!({"type":"url","url":url}),
                };
                Ok(json!({"type":kind,"source":value}))
            }
            _ if config.uses_responses()
                && !(provider == Provider::OpenRouter && kind == "audio") =>
            {
                match (kind, source) {
                    ("image", _) => {
                        Ok(json!({"type":"input_image","image_url":self.url_or_data_uri()}))
                    }
                    ("document", Source::Url(url)) => {
                        Ok(json!({"type":"input_file","file_url":url}))
                    }
                    ("document", Source::Base64(_)) => Ok(
                        json!({"type":"input_file","filename":"document.pdf","file_data":self.url_or_data_uri()}),
                    ),
                    _ => Err(unsupported()),
                }
            }
            Provider::OpenAI | Provider::OpenRouter => match (kind, source) {
                ("image", _) => {
                    Ok(json!({"type":"image_url","image_url":{"url":self.url_or_data_uri()}}))
                }
                ("document", Source::Base64(_)) => Ok(
                    json!({"type":"file","file":{"filename":"document.pdf","file_data":self.url_or_data_uri()}}),
                ),
                ("document", Source::Url(url)) if provider == Provider::OpenRouter => {
                    Ok(json!({"type":"file","file":{"filename":"document.pdf","file_data":url}}))
                }
                ("audio", Source::Base64(data)) => {
                    let format = if self.mime == "audio/mpeg" {
                        "mp3"
                    } else {
                        "wav"
                    };
                    Ok(json!({"type":"input_audio","input_audio":{"data":data,"format":format}}))
                }
                _ => Err(unsupported()),
            },
            Provider::Mistral if kind == "image" => {
                Ok(json!({"type":"image_url","image_url":self.url_or_data_uri()}))
            }
            Provider::Cohere if kind == "image" => {
                Ok(json!({"type":"image_url","image_url":{"url":self.url_or_data_uri()}}))
            }
            _ => Err(unsupported()),
        }
    }
}

pub(crate) fn content(
    config: &ProviderConfig,
    text: &str,
    attachments: &[Media],
) -> Result<Value, Error> {
    let provider = config.provider();
    if attachments.is_empty() && provider != Provider::Google {
        return Ok(json!(text));
    }
    let mut parts = Vec::new();
    if !text.is_empty() || attachments.is_empty() {
        parts.push(match provider {
            Provider::Google => json!({"text":text}),
            _ if config.uses_responses() => {
                json!({"type":"input_text","text":text})
            }
            _ => json!({"type":"text","text":text}),
        });
    }
    for media in attachments {
        parts.push(media.block(config)?);
    }
    Ok(parts.into())
}
