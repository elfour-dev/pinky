use std::{collections::BTreeMap, fmt, time::Duration};

use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::MIN_CHAT_CONTEXT;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERROR_BODY_CHARS: usize = 500;

#[derive(Debug, Error)]
pub enum OllamaError {
    #[error("Ollama endpoint must be http://127.0.0.1:<port> with no path, credentials, query, or fragment")]
    InvalidEndpoint,
    #[error("select an installed Ollama model")]
    InvalidModel,
    #[error("Ollama request was cancelled")]
    Cancelled,
    #[error("Ollama request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Ollama rejected a request with HTTP {status}: {body}")]
    Response { status: StatusCode, body: String },
    #[error("Ollama returned an invalid {0} response")]
    InvalidResponse(&'static str),
    #[error("Ollama model `{requested}` is not installed; available models: {available}")]
    ModelNotInstalled {
        requested: String,
        available: String,
    },
    #[error("Ollama model `{0}` is not a local GGUF completion model")]
    UnsupportedModel(String),
    #[error("Ollama model context is {found} tokens; at least {minimum} are required")]
    ContextTooSmall { found: u64, minimum: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaRuntimeInfo {
    pub model_name: String,
    pub context_size: u64,
    pub version: String,
}

#[derive(Clone)]
pub struct OllamaClient {
    client: Client,
    base_url: Url,
    model_name: String,
}

impl fmt::Debug for OllamaClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OllamaClient")
            .field("base_url", &self.base_url.as_str())
            .field("model_name", &self.model_name)
            .finish()
    }
}

impl OllamaClient {
    pub fn connect(endpoint: &str, model_name: &str) -> Result<Self, OllamaError> {
        let base_url = validate_endpoint(endpoint)?;
        let model_name = model_name.trim();
        if model_name.is_empty()
            || model_name.len() > 255
            || model_name.chars().any(char::is_control)
        {
            return Err(OllamaError::InvalidModel);
        }
        let client = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .build()?;
        Ok(Self {
            client,
            base_url,
            model_name: model_name.to_owned(),
        })
    }

    pub async fn probe(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<OllamaRuntimeInfo, OllamaError> {
        let version: VersionResponse = self
            .get_json("api/version", "version", cancellation)
            .await?;
        if version.version.trim().is_empty() {
            return Err(OllamaError::InvalidResponse("version"));
        }

        let tags: TagsResponse = self.get_json("api/tags", "models", cancellation).await?;
        let installed = tags.models.iter().find(|model| {
            model.name == self.model_name || model.model.as_deref() == Some(&self.model_name)
        });
        let Some(installed) = installed else {
            let available = if tags.models.is_empty() {
                "none".to_owned()
            } else {
                tags.models
                    .iter()
                    .take(10)
                    .map(|model| model.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return Err(OllamaError::ModelNotInstalled {
                requested: self.model_name.clone(),
                available,
            });
        };
        if installed.remote_model.as_deref().is_some_and(not_empty)
            || installed.remote_host.as_deref().is_some_and(not_empty)
        {
            return Err(OllamaError::UnsupportedModel(self.model_name.clone()));
        }

        let request = self
            .client
            .post(
                self.base_url
                    .join("api/show")
                    .map_err(|_| OllamaError::InvalidEndpoint)?,
            )
            .json(&ShowRequest {
                model: &self.model_name,
                verbose: false,
            })
            .timeout(PROBE_TIMEOUT);
        let details: ShowResponse = read_json(request, "model details", cancellation).await?;
        if details.remote_model.as_deref().is_some_and(not_empty)
            || details.remote_host.as_deref().is_some_and(not_empty)
            || !details.details.format.eq_ignore_ascii_case("gguf")
            || !details
                .capabilities
                .iter()
                .any(|capability| capability == "completion")
        {
            return Err(OllamaError::UnsupportedModel(self.model_name.clone()));
        }
        let context_size = details
            .details
            .context_length
            .or_else(|| {
                details
                    .model_info
                    .iter()
                    .filter(|(key, _)| key.ends_with(".context_length"))
                    .filter_map(|(_, value)| value.as_u64())
                    .max()
            })
            .ok_or(OllamaError::InvalidResponse("model context"))?;
        if context_size < MIN_CHAT_CONTEXT {
            return Err(OllamaError::ContextTooSmall {
                found: context_size,
                minimum: MIN_CHAT_CONTEXT,
            });
        }

        Ok(OllamaRuntimeInfo {
            model_name: self.model_name.clone(),
            context_size,
            version: version.version,
        })
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        response_name: &'static str,
        cancellation: &CancellationToken,
    ) -> Result<T, OllamaError> {
        let request = self
            .client
            .get(
                self.base_url
                    .join(path)
                    .map_err(|_| OllamaError::InvalidEndpoint)?,
            )
            .timeout(PROBE_TIMEOUT);
        read_json(request, response_name, cancellation).await
    }
}

#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<TaggedModel>,
}

#[derive(Deserialize)]
struct TaggedModel {
    name: String,
    model: Option<String>,
    remote_model: Option<String>,
    remote_host: Option<String>,
}

#[derive(Serialize)]
struct ShowRequest<'a> {
    model: &'a str,
    verbose: bool,
}

#[derive(Deserialize)]
struct ShowResponse {
    details: ModelDetails,
    model_info: BTreeMap<String, Value>,
    #[serde(default)]
    capabilities: Vec<String>,
    remote_model: Option<String>,
    remote_host: Option<String>,
}

#[derive(Deserialize)]
struct ModelDetails {
    format: String,
    context_length: Option<u64>,
}

async fn read_json<T: for<'de> Deserialize<'de>>(
    request: reqwest::RequestBuilder,
    response_name: &'static str,
    cancellation: &CancellationToken,
) -> Result<T, OllamaError> {
    let response = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(OllamaError::Cancelled),
        response = request.send() => response?,
    };
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await?;
        return Err(OllamaError::Response {
            status,
            body: bounded(&body),
        });
    }
    response
        .json::<T>()
        .await
        .map_err(|_| OllamaError::InvalidResponse(response_name))
}

fn validate_endpoint(endpoint: &str) -> Result<Url, OllamaError> {
    let url = Url::parse(endpoint).map_err(|_| OllamaError::InvalidEndpoint)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(OllamaError::InvalidEndpoint);
    }
    Ok(url)
}

fn bounded(value: &str) -> String {
    value.trim().chars().take(MAX_ERROR_BODY_CHARS).collect()
}

fn not_empty(value: &str) -> bool {
    !value.is_empty()
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    #[test]
    fn accepts_only_exact_ipv4_loopback_origins_and_a_model_name() {
        for endpoint in [
            "https://127.0.0.1:11434",
            "http://localhost:11434",
            "http://127.0.0.2:11434",
            "http://[::1]:11434",
            "http://127.0.0.1",
            "http://user@127.0.0.1:11434",
            "http://127.0.0.1:11434/api",
        ] {
            assert!(matches!(
                OllamaClient::connect(endpoint, "qwen3:8b"),
                Err(OllamaError::InvalidEndpoint)
            ));
        }
        assert!(matches!(
            OllamaClient::connect("http://127.0.0.1:11434", "  "),
            Err(OllamaError::InvalidModel)
        ));
    }

    #[tokio::test]
    async fn probes_an_installed_model_without_an_authorization_header() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            respond(&listener, "GET", "/api/version", r#"{"version":"0.12.6"}"#);
            respond(
                &listener,
                "GET",
                "/api/tags",
                r#"{"models":[{"name":"qwen3:8b","model":"qwen3:8b"}]}"#,
            );
            respond(
                &listener,
                "POST",
                "/api/show",
                r#"{"details":{"format":"gguf"},"model_info":{"qwen3.context_length":32768},"capabilities":["completion","tools"]}"#,
            );
        });

        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "qwen3:8b").unwrap();
        assert_eq!(
            client.probe(&CancellationToken::new()).await.unwrap(),
            OllamaRuntimeInfo {
                model_name: "qwen3:8b".into(),
                context_size: 32_768,
                version: "0.12.6".into(),
            }
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn rejects_models_that_are_not_installed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            respond(&listener, "GET", "/api/version", r#"{"version":"0.12.6"}"#);
            respond(
                &listener,
                "GET",
                "/api/tags",
                r#"{"models":[{"name":"gemma3:4b","model":"gemma3:4b"}]}"#,
            );
        });
        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "qwen3:8b").unwrap();
        assert!(matches!(
            client.probe(&CancellationToken::new()).await,
            Err(OllamaError::ModelNotInstalled { .. })
        ));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn rejects_non_gguf_models() {
        let result = probe_with_details(
            r#"{"details":{"format":"safetensors"},"model_info":{"model.context_length":4096},"capabilities":["completion"]}"#,
        )
        .await;
        assert!(matches!(result, Err(OllamaError::UnsupportedModel(_))));
    }

    #[tokio::test]
    async fn rejects_models_with_too_little_context() {
        let result = probe_with_details(
            r#"{"details":{"format":"gguf"},"model_info":{"tiny.context_length":1024},"capabilities":["completion"]}"#,
        )
        .await;
        assert!(matches!(
            result,
            Err(OllamaError::ContextTooSmall {
                found: 1_024,
                minimum: MIN_CHAT_CONTEXT,
            })
        ));
    }

    #[tokio::test]
    async fn rejects_remote_ollama_models() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            respond(&listener, "GET", "/api/version", r#"{"version":"0.12.6"}"#);
            respond(
                &listener,
                "GET",
                "/api/tags",
                r#"{"models":[{"name":"large-cloud","model":"large-cloud","remote_model":"large","remote_host":"https://ollama.com"}]}"#,
            );
        });
        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "large-cloud").unwrap();
        assert!(matches!(
            client.probe(&CancellationToken::new()).await,
            Err(OllamaError::UnsupportedModel(_))
        ));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn cancellation_prevents_a_request() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let client = OllamaClient::connect("http://127.0.0.1:9", "qwen3:8b").unwrap();
        assert!(matches!(
            client.probe(&cancellation).await,
            Err(OllamaError::Cancelled)
        ));
    }

    async fn probe_with_details(details: &'static str) -> Result<OllamaRuntimeInfo, OllamaError> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            respond(&listener, "GET", "/api/version", r#"{"version":"0.12.6"}"#);
            respond(
                &listener,
                "GET",
                "/api/tags",
                r#"{"models":[{"name":"tiny:latest","model":"tiny:latest"}]}"#,
            );
            respond(&listener, "POST", "/api/show", details);
        });
        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "tiny:latest").unwrap();
        let result = client.probe(&CancellationToken::new()).await;
        server.join().unwrap();
        result
    }

    fn respond(listener: &TcpListener, method: &str, path: &str, body: &str) {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 8192];
        let length = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..length]);
        assert!(request.starts_with(&format!("{method} {path} HTTP/1.1\r\n")));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        if method == "POST" {
            assert!(request.contains(r#""model":""#));
            assert!(request.contains(r#""verbose":false"#));
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }
}
