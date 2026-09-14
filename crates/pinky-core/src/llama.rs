use std::{fmt, time::Duration};

use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERROR_BODY_CHARS: usize = 500;
const API_KEY_HEX_LENGTH: usize = 64;
pub const MIN_CHAT_CONTEXT: u64 = 2_048;

#[derive(Debug, Error)]
pub enum LlamaError {
    #[error("llama-server endpoint must be http://127.0.0.1:<port> with no path, credentials, query, or fragment")]
    InvalidEndpoint,
    #[error("llama-server API key must be a 256-bit hexadecimal token")]
    InvalidApiKey,
    #[error("llama-server request was cancelled")]
    Cancelled,
    #[error("llama-server request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("llama-server rejected a request with HTTP {status}: {body}")]
    Response { status: StatusCode, body: String },
    #[error("llama-server returned an invalid health response")]
    InvalidHealthResponse,
    #[error("llama-server returned invalid model properties")]
    InvalidProperties,
    #[error("llama-server is not ready: {0}")]
    NotReady(String),
    #[error("llama-server model context is {found} tokens; at least {minimum} are required")]
    ContextTooSmall { found: u64, minimum: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlamaHealth {
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlamaRuntimeInfo {
    pub model_path: String,
    pub context_size: u64,
    pub total_slots: u64,
}

#[derive(Clone)]
pub struct LlamaClient {
    client: Client,
    base_url: Url,
    api_key: Zeroizing<String>,
}

impl fmt::Debug for LlamaClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LlamaClient")
            .field("base_url", &self.base_url.as_str())
            .field("api_key", &"[redacted]")
            .finish()
    }
}

impl LlamaClient {
    pub fn connect(endpoint: &str, api_key: Zeroizing<String>) -> Result<Self, LlamaError> {
        let base_url = validate_endpoint(endpoint)?;
        validate_api_key(&api_key)?;
        let client = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .build()?;
        Ok(Self {
            client,
            base_url,
            api_key,
        })
    }

    pub async fn health(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<LlamaHealth, LlamaError> {
        let request = self
            .client
            .get(
                self.base_url
                    .join("health")
                    .map_err(|_| LlamaError::InvalidEndpoint)?,
            )
            .bearer_auth(self.api_key.as_str())
            .timeout(HEALTH_TIMEOUT);

        let response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(LlamaError::Cancelled),
            response = request.send() => response?,
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            return Err(LlamaError::Response {
                status,
                body: bounded(&body),
            });
        }

        let health = response
            .json::<HealthResponse>()
            .await
            .map_err(|_| LlamaError::InvalidHealthResponse)?;
        if health.status != "ok" {
            return Err(LlamaError::NotReady(bounded(&health.status)));
        }
        Ok(LlamaHealth {
            status: health.status,
        })
    }

    pub async fn probe(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<LlamaRuntimeInfo, LlamaError> {
        self.health(cancellation).await?;
        let request = self
            .client
            .get(
                self.base_url
                    .join("props")
                    .map_err(|_| LlamaError::InvalidEndpoint)?,
            )
            .bearer_auth(self.api_key.as_str())
            .timeout(HEALTH_TIMEOUT);

        let properties = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(LlamaError::Cancelled),
            properties = read_properties(request) => properties?,
        };
        if properties.model_path.trim().is_empty() || properties.total_slots == 0 {
            return Err(LlamaError::InvalidProperties);
        }
        let context_size = properties.default_generation_settings.n_ctx;
        if context_size < MIN_CHAT_CONTEXT {
            return Err(LlamaError::ContextTooSmall {
                found: context_size,
                minimum: MIN_CHAT_CONTEXT,
            });
        }
        Ok(LlamaRuntimeInfo {
            model_path: properties.model_path,
            context_size,
            total_slots: properties.total_slots,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HealthResponse {
    status: String,
}

#[derive(Deserialize)]
struct PropertiesResponse {
    model_path: String,
    total_slots: u64,
    default_generation_settings: GenerationSettings,
}

#[derive(Deserialize)]
struct GenerationSettings {
    n_ctx: u64,
}

async fn read_properties(
    request: reqwest::RequestBuilder,
) -> Result<PropertiesResponse, LlamaError> {
    let response = request.send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await?;
        return Err(LlamaError::Response {
            status,
            body: bounded(&body),
        });
    }
    response
        .json::<PropertiesResponse>()
        .await
        .map_err(|_| LlamaError::InvalidProperties)
}

fn validate_endpoint(endpoint: &str) -> Result<Url, LlamaError> {
    let url = Url::parse(endpoint).map_err(|_| LlamaError::InvalidEndpoint)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(LlamaError::InvalidEndpoint);
    }
    Ok(url)
}

fn validate_api_key(api_key: &str) -> Result<(), LlamaError> {
    if api_key.len() != API_KEY_HEX_LENGTH || !api_key.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(LlamaError::InvalidApiKey);
    }
    Ok(())
}

fn bounded(value: &str) -> String {
    value.trim().chars().take(MAX_ERROR_BODY_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    fn key() -> Zeroizing<String> {
        Zeroizing::new("ab".repeat(32))
    }

    #[test]
    fn rejects_endpoints_that_are_not_exact_ipv4_loopback_origins() {
        for endpoint in [
            "https://127.0.0.1:8080",
            "http://localhost:8080",
            "http://127.0.0.2:8080",
            "http://[::1]:8080",
            "http://127.0.0.1",
            "http://user@127.0.0.1:8080",
            "http://127.0.0.1:8080/v1",
            "http://127.0.0.1:8080?debug=true",
        ] {
            assert!(matches!(
                LlamaClient::connect(endpoint, key()),
                Err(LlamaError::InvalidEndpoint)
            ));
        }
    }

    #[test]
    fn requires_a_256_bit_hexadecimal_key_and_redacts_it() {
        assert!(matches!(
            LlamaClient::connect("http://127.0.0.1:8080", Zeroizing::new("too-short".into())),
            Err(LlamaError::InvalidApiKey)
        ));
        let key = key();
        let client = LlamaClient::connect("http://127.0.0.1:8080", key.clone()).unwrap();
        let debug = format!("{client:?}");
        assert!(!debug.contains(key.as_str()));
        assert!(debug.contains("[redacted]"));
    }

    #[tokio::test]
    async fn health_uses_the_bearer_token_and_accepts_only_ready_status() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected_key = key();
        let server_key = expected_key.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let length = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(request.starts_with("GET /health HTTP/1.1\r\n"));
            assert!(request
                .to_ascii_lowercase()
                .contains(&format!("authorization: bearer {}", server_key.as_str())));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}",
                )
                .unwrap();
        });

        let client =
            LlamaClient::connect(&format!("http://127.0.0.1:{port}"), expected_key).unwrap();
        assert_eq!(
            client.health(&CancellationToken::new()).await.unwrap(),
            LlamaHealth {
                status: "ok".into()
            }
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn a_pre_cancelled_health_request_never_connects() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let client = LlamaClient::connect("http://127.0.0.1:9", key()).unwrap();
        assert!(matches!(
            client.health(&cancellation).await,
            Err(LlamaError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn probe_authenticates_the_protected_properties_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected_key = key();
        let server_key = expected_key.clone();
        let server = thread::spawn(move || {
            respond(
                &listener,
                "/health",
                &server_key,
                "200 OK",
                r#"{"status":"ok"}"#,
            );
            respond(
                &listener,
                "/props",
                &server_key,
                "200 OK",
                r#"{"model_path":"/models/qwen.gguf","total_slots":1,"default_generation_settings":{"n_ctx":4096},"extra":"accepted"}"#,
            );
        });

        let client =
            LlamaClient::connect(&format!("http://127.0.0.1:{port}"), expected_key).unwrap();
        assert_eq!(
            client.probe(&CancellationToken::new()).await.unwrap(),
            LlamaRuntimeInfo {
                model_path: "/models/qwen.gguf".into(),
                context_size: 4_096,
                total_slots: 1,
            }
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn probe_rejects_a_key_refused_by_the_protected_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected_key = key();
        let client_key = Zeroizing::new("cd".repeat(32));
        let server = thread::spawn(move || {
            respond(
                &listener,
                "/health",
                &client_key,
                "200 OK",
                r#"{"status":"ok"}"#,
            );
            respond(
                &listener,
                "/props",
                &expected_key,
                "200 OK",
                r#"{"model_path":"/models/qwen.gguf","total_slots":1,"default_generation_settings":{"n_ctx":4096}}"#,
            );
        });

        let client = LlamaClient::connect(
            &format!("http://127.0.0.1:{port}"),
            Zeroizing::new("cd".repeat(32)),
        )
        .unwrap();
        assert!(matches!(
            client.probe(&CancellationToken::new()).await,
            Err(LlamaError::Response {
                status: StatusCode::UNAUTHORIZED,
                ..
            })
        ));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn probe_rejects_a_context_too_small_for_cited_answers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected_key = key();
        let server_key = expected_key.clone();
        let server = thread::spawn(move || {
            respond(
                &listener,
                "/health",
                &server_key,
                "200 OK",
                r#"{"status":"ok"}"#,
            );
            respond(
                &listener,
                "/props",
                &server_key,
                "200 OK",
                r#"{"model_path":"/models/tiny.gguf","total_slots":1,"default_generation_settings":{"n_ctx":1024}}"#,
            );
        });

        let client =
            LlamaClient::connect(&format!("http://127.0.0.1:{port}"), expected_key).unwrap();
        assert!(matches!(
            client.probe(&CancellationToken::new()).await,
            Err(LlamaError::ContextTooSmall {
                found: 1_024,
                minimum: MIN_CHAT_CONTEXT,
            })
        ));
        server.join().unwrap();
    }

    fn respond(
        listener: &TcpListener,
        path: &str,
        expected_key: &str,
        success_status: &str,
        success_body: &str,
    ) {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let length = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..length]);
        assert!(request.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
        let authorized = request
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {expected_key}"));
        let (status, body) = if authorized {
            (success_status, success_body)
        } else {
            (
                "401 Unauthorized",
                r#"{"error":{"message":"Invalid API Key"}}"#,
            )
        };
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }
}
