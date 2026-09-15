use std::{collections::BTreeMap, fmt, time::Duration};

use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    EmbeddingError, EmbeddingFuture, EmbeddingProvider, EmbeddingResponse, InferenceError,
    InferenceFuture, InferenceMetrics, InferenceProvider, InferenceResponse,
    StructuredGenerationRequest, MAX_EMBEDDING_ERROR_BYTES, MAX_EMBEDDING_INPUTS,
    MAX_EMBEDDING_INPUT_BYTES, MAX_EMBEDDING_RESPONSE_BYTES, MAX_INFERENCE_REQUEST_BYTES,
    MAX_INFERENCE_RESPONSE_BYTES, MIN_CHAT_CONTEXT,
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const INFERENCE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_ERROR_BODY_CHARS: usize = 500;
const KEEP_ALIVE: &str = "5m";

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
    #[error("Ollama model `{0}` is not a local GGUF embedding model")]
    UnsupportedEmbeddingModel(String),
    #[error("Ollama embedding smoke test failed: {0}")]
    Embedding(#[from] EmbeddingError),
    #[error("Ollama model context is {found} tokens; at least {minimum} are required")]
    ContextTooSmall { found: u64, minimum: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaRuntimeInfo {
    pub model_name: String,
    pub context_size: u64,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaEmbeddingRuntimeInfo {
    pub model_name: String,
    pub dimensions: usize,
    pub version: String,
}

#[derive(Clone)]
pub struct OllamaClient {
    client: Client,
    base_url: Url,
    model_name: String,
    inference_timeout: Duration,
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
            inference_timeout: INFERENCE_TIMEOUT,
        })
    }

    pub async fn probe(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<OllamaRuntimeInfo, OllamaError> {
        let (version, details) = self.probe_metadata(cancellation).await?;
        if !details.details.format.eq_ignore_ascii_case("gguf")
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

    pub async fn probe_embedding(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<OllamaEmbeddingRuntimeInfo, OllamaError> {
        let (version, details) = self.probe_metadata(cancellation).await?;
        if !details.details.format.eq_ignore_ascii_case("gguf")
            || !details
                .capabilities
                .iter()
                .any(|capability| capability == "embedding")
        {
            return Err(OllamaError::UnsupportedEmbeddingModel(
                self.model_name.clone(),
            ));
        }
        let smoke_test = self
            .embed_inner(
                &["Pinky embedding capability check".to_owned()],
                cancellation,
            )
            .await?;
        let dimensions = smoke_test
            .vectors
            .first()
            .map(Vec::len)
            .ok_or(EmbeddingError::InvalidVectors)?;
        Ok(OllamaEmbeddingRuntimeInfo {
            model_name: self.model_name.clone(),
            dimensions,
            version: version.version,
        })
    }

    async fn probe_metadata(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<(VersionResponse, ShowResponse), OllamaError> {
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
        {
            return Err(OllamaError::UnsupportedModel(self.model_name.clone()));
        }
        Ok((version, details))
    }

    pub async fn embed(
        &self,
        inputs: &[String],
        cancellation: &CancellationToken,
    ) -> Result<EmbeddingResponse, EmbeddingError> {
        self.embed_inner(inputs, cancellation).await
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

    async fn generate_structured_inner(
        &self,
        request: &StructuredGenerationRequest,
        cancellation: &CancellationToken,
    ) -> Result<InferenceResponse, InferenceError> {
        request.validate()?;
        let body = serde_json::to_vec(&ChatRequest {
            model: &self.model_name,
            messages: [
                ChatMessageRequest {
                    role: "system",
                    content: &request.system,
                },
                ChatMessageRequest {
                    role: "user",
                    content: &request.prompt,
                },
            ],
            format: &request.output_schema,
            stream: false,
            think: false,
            keep_alive: KEEP_ALIVE,
            options: ChatOptions {
                temperature: 0.0,
                num_predict: request.max_output_tokens,
            },
        })
        .map_err(|_| InferenceError::InvalidRequest("request is not serializable"))?;
        if body.len() > MAX_INFERENCE_REQUEST_BYTES {
            return Err(InferenceError::InvalidRequest(
                "serialized request exceeds size limit",
            ));
        }
        let request = self
            .client
            .post(
                self.base_url
                    .join("api/chat")
                    .map_err(|_| InferenceError::InvalidRequest("invalid provider endpoint"))?,
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .timeout(self.inference_timeout);

        let response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(InferenceError::Cancelled),
            response = request.send() => response.map_err(map_inference_request_error)?,
        };
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = read_bounded_error(response, cancellation).await?;
            return Err(InferenceError::Rejected { status, body });
        }
        let body = read_bounded_inference_body(response, cancellation).await?;
        let response: ChatResponse =
            serde_json::from_slice(&body).map_err(|_| InferenceError::MalformedResponse)?;
        if response.remote_model.as_deref().is_some_and(not_empty)
            || response.remote_host.as_deref().is_some_and(not_empty)
        {
            return Err(InferenceError::RemoteResponse);
        }
        if response.model != self.model_name {
            return Err(InferenceError::ModelMismatch {
                expected: self.model_name.clone(),
                found: response.model,
            });
        }
        if !response.done {
            return Err(InferenceError::IncompleteResponse);
        }
        if response.message.role != "assistant" || response.message.content.trim().is_empty() {
            return Err(InferenceError::MalformedResponse);
        }
        Ok(InferenceResponse {
            model: self.model_name.clone(),
            content: response.message.content,
            done_reason: response.done_reason,
            metrics: InferenceMetrics {
                total_duration_ns: response.total_duration,
                load_duration_ns: response.load_duration,
                prompt_tokens: response.prompt_eval_count,
                output_tokens: response.eval_count,
            },
        })
    }

    async fn embed_inner(
        &self,
        inputs: &[String],
        cancellation: &CancellationToken,
    ) -> Result<EmbeddingResponse, EmbeddingError> {
        if inputs.is_empty() || inputs.len() > MAX_EMBEDDING_INPUTS {
            return Err(EmbeddingError::InvalidInputCount);
        }
        if inputs
            .iter()
            .any(|input| input.trim().is_empty() || input.len() > MAX_EMBEDDING_INPUT_BYTES)
        {
            return Err(EmbeddingError::InvalidInput);
        }
        let body = serde_json::to_vec(&EmbedRequest {
            model: &self.model_name,
            input: inputs,
        })
        .map_err(|_| EmbeddingError::MalformedResponse)?;
        if body.len() > MAX_EMBEDDING_INPUT_BYTES {
            return Err(EmbeddingError::InvalidInput);
        }
        let request = self
            .client
            .post(
                self.base_url
                    .join("api/embed")
                    .map_err(|_| EmbeddingError::MalformedResponse)?,
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .timeout(PROBE_TIMEOUT);
        let response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(EmbeddingError::Cancelled),
            response = request.send() => response?,
        };
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = read_bounded_embedding_error(response, cancellation).await?;
            return Err(EmbeddingError::Rejected { status, body });
        }
        let body = read_bounded_embedding_body(response, cancellation).await?;
        let response: OllamaEmbedResponse =
            serde_json::from_slice(&body).map_err(|_| EmbeddingError::MalformedResponse)?;
        if response.remote_model.as_deref().is_some_and(not_empty)
            || response.remote_host.as_deref().is_some_and(not_empty)
        {
            return Err(EmbeddingError::RemoteResponse);
        }
        if response.model != self.model_name {
            return Err(EmbeddingError::ModelMismatch {
                expected: self.model_name.clone(),
                found: response.model,
            });
        }
        if response.embeddings.len() != inputs.len()
            || response.embeddings.is_empty()
            || response.embeddings.iter().any(|vector| {
                vector.is_empty()
                    || vector.iter().any(|value| !value.is_finite())
                    || vector.iter().map(|value| value * value).sum::<f32>() <= f32::EPSILON
            })
        {
            return Err(EmbeddingError::InvalidVectors);
        }
        let dimension = response.embeddings[0].len();
        if response
            .embeddings
            .iter()
            .any(|vector| vector.len() != dimension)
        {
            return Err(EmbeddingError::InvalidVectors);
        }
        Ok(EmbeddingResponse {
            model: self.model_name.clone(),
            vectors: response.embeddings,
        })
    }
}

impl InferenceProvider for OllamaClient {
    fn generate_structured<'a>(
        &'a self,
        request: &'a StructuredGenerationRequest,
        cancellation: &'a CancellationToken,
    ) -> InferenceFuture<'a> {
        Box::pin(self.generate_structured_inner(request, cancellation))
    }
}

impl EmbeddingProvider for OllamaClient {
    fn embed<'a>(
        &'a self,
        inputs: &'a [String],
        cancellation: &'a CancellationToken,
    ) -> EmbeddingFuture<'a> {
        Box::pin(self.embed_inner(inputs, cancellation))
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

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: [ChatMessageRequest<'a>; 2],
    format: &'a Value,
    stream: bool,
    think: bool,
    keep_alive: &'static str,
    options: ChatOptions,
}

#[derive(Serialize)]
struct ChatMessageRequest<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatOptions {
    temperature: f32,
    num_predict: u32,
}

#[derive(Deserialize)]
struct ChatResponse {
    model: String,
    message: ChatMessageResponse,
    done: bool,
    done_reason: Option<String>,
    remote_model: Option<String>,
    remote_host: Option<String>,
    total_duration: Option<u64>,
    load_duration: Option<u64>,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
}

#[derive(Deserialize)]
struct ChatMessageResponse {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct OllamaEmbedResponse {
    model: String,
    embeddings: Vec<Vec<f32>>,
    remote_model: Option<String>,
    remote_host: Option<String>,
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

async fn read_bounded_inference_body(
    mut response: reqwest::Response,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, InferenceError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_INFERENCE_RESPONSE_BYTES as u64)
    {
        return Err(InferenceError::ResponseTooLarge {
            limit: MAX_INFERENCE_RESPONSE_BYTES,
        });
    }
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(InferenceError::Cancelled),
            chunk = response.chunk() => chunk.map_err(map_inference_request_error)?,
        };
        let Some(chunk) = chunk else {
            return Ok(body);
        };
        if body.len().saturating_add(chunk.len()) > MAX_INFERENCE_RESPONSE_BYTES {
            return Err(InferenceError::ResponseTooLarge {
                limit: MAX_INFERENCE_RESPONSE_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
}

async fn read_bounded_error(
    mut response: reqwest::Response,
    cancellation: &CancellationToken,
) -> Result<String, InferenceError> {
    let mut body = Vec::new();
    let byte_limit = MAX_ERROR_BODY_CHARS * 4;
    while body.len() < byte_limit {
        let chunk = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(InferenceError::Cancelled),
            chunk = response.chunk() => chunk.map_err(map_inference_request_error)?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let remaining = byte_limit - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    Ok(bounded(&String::from_utf8_lossy(&body)))
}

async fn read_bounded_embedding_body(
    mut response: reqwest::Response,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, EmbeddingError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_EMBEDDING_RESPONSE_BYTES as u64)
    {
        return Err(EmbeddingError::ResponseTooLarge {
            limit: MAX_EMBEDDING_RESPONSE_BYTES,
        });
    }
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(EmbeddingError::Cancelled),
            chunk = response.chunk() => chunk?,
        };
        let Some(chunk) = chunk else {
            return Ok(body);
        };
        if body.len().saturating_add(chunk.len()) > MAX_EMBEDDING_RESPONSE_BYTES {
            return Err(EmbeddingError::ResponseTooLarge {
                limit: MAX_EMBEDDING_RESPONSE_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
}

async fn read_bounded_embedding_error(
    mut response: reqwest::Response,
    cancellation: &CancellationToken,
) -> Result<String, EmbeddingError> {
    let mut body = Vec::new();
    while body.len() < MAX_EMBEDDING_ERROR_BYTES {
        let chunk = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(EmbeddingError::Cancelled),
            chunk = response.chunk() => chunk?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let remaining = MAX_EMBEDDING_ERROR_BYTES - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    Ok(String::from_utf8_lossy(&body).trim().to_owned())
}

fn map_inference_request_error(error: reqwest::Error) -> InferenceError {
    if error.is_timeout() {
        InferenceError::Timeout
    } else {
        InferenceError::Unavailable(error.to_string())
    }
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
        sync::mpsc,
        thread,
    };

    use serde_json::json;

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
    async fn probes_embedding_model_and_runs_a_bounded_smoke_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            respond(&listener, "GET", "/api/version", r#"{"version":"0.12.6"}"#);
            respond(
                &listener,
                "GET",
                "/api/tags",
                r#"{"models":[{"name":"nomic-embed-text","model":"nomic-embed-text"}]}"#,
            );
            respond(
                &listener,
                "POST",
                "/api/show",
                r#"{"details":{"format":"gguf"},"model_info":{},"capabilities":["embedding"]}"#,
            );
            let (request, stream) = accept_http_request(&listener);
            assert!(request.starts_with("POST /api/embed HTTP/1.1\r\n"));
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            let body: Value = serde_json::from_str(http_body(&request)).unwrap();
            assert_eq!(body["model"], "nomic-embed-text");
            assert_eq!(body["input"], json!(["Pinky embedding capability check"]));
            write_http_response(
                stream,
                "200 OK",
                r#"{"model":"nomic-embed-text","embeddings":[[1.0,0.0,0.0]]}"#,
            )
            .unwrap();
        });

        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "nomic-embed-text").unwrap();
        assert_eq!(
            client
                .probe_embedding(&CancellationToken::new())
                .await
                .unwrap(),
            OllamaEmbeddingRuntimeInfo {
                model_name: "nomic-embed-text".into(),
                dimensions: 3,
                version: "0.12.6".into(),
            }
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn rejects_a_completion_model_as_an_embedding_model() {
        let result = probe_embedding_with_details(
            r#"{"details":{"format":"gguf"},"model_info":{},"capabilities":["completion"]}"#,
        )
        .await;
        assert!(matches!(
            result,
            Err(OllamaError::UnsupportedEmbeddingModel(_))
        ));
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

    #[tokio::test]
    async fn sends_a_bounded_structured_chat_request_without_authentication() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (request, stream) = accept_http_request(&listener);
            assert!(request.starts_with("POST /api/chat HTTP/1.1\r\n"));
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            let body: Value = serde_json::from_str(http_body(&request)).unwrap();
            assert_eq!(body["model"], "qwen3:8b");
            assert_eq!(body["stream"], false);
            assert_eq!(body["think"], false);
            assert_eq!(body["keep_alive"], "5m");
            assert_eq!(body["options"]["temperature"], 0.0);
            assert_eq!(body["options"]["num_predict"], 512);
            assert_eq!(body["messages"][0]["role"], "system");
            assert_eq!(body["messages"][1]["role"], "user");
            assert_eq!(body["format"]["type"], "object");
            write_http_response(
                stream,
                "200 OK",
                r#"{"model":"qwen3:8b","message":{"role":"assistant","content":"{\"answer\":\"grounded\"}"},"done":true,"done_reason":"stop","total_duration":42,"load_duration":5,"prompt_eval_count":12,"eval_count":7}"#,
            )
            .unwrap();
        });

        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "qwen3:8b").unwrap();
        let response = client
            .generate_structured(&inference_request(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(response.content, r#"{"answer":"grounded"}"#);
        assert_eq!(response.metrics.prompt_tokens, Some(12));
        assert_eq!(response.metrics.output_tokens, Some(7));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn sends_bounded_embedding_request_without_authentication() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (request, stream) = accept_http_request(&listener);
            assert!(request.starts_with("POST /api/embed HTTP/1.1\r\n"));
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            let body: Value = serde_json::from_str(http_body(&request)).unwrap();
            assert_eq!(body["model"], "nomic-embed-text");
            assert_eq!(body["input"], json!(["first passage", "second passage"]));
            write_http_response(
                stream,
                "200 OK",
                r#"{"model":"nomic-embed-text","embeddings":[[1.0,0.0],[0.5,0.5]]}"#,
            )
            .unwrap();
        });
        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "nomic-embed-text").unwrap();
        let inputs = vec!["first passage".to_owned(), "second passage".to_owned()];
        let response = client
            .embed(&inputs, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(response.model, "nomic-embed-text");
        assert_eq!(response.vectors.len(), 2);
        assert_eq!(response.vectors[1], vec![0.5, 0.5]);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn classifies_rejection_malformed_incomplete_mismatch_and_remote_responses() {
        let cases = [
            (
                "404 Not Found",
                r#"{"error":"model disappeared"}"#,
                "rejected",
            ),
            ("200 OK", "not-json", "malformed"),
            (
                "200 OK",
                r#"{"model":"qwen3:8b","message":{"role":"assistant","content":"{}"},"done":false}"#,
                "incomplete",
            ),
            (
                "200 OK",
                r#"{"model":"other:latest","message":{"role":"assistant","content":"{}"},"done":true}"#,
                "mismatch",
            ),
            (
                "200 OK",
                r#"{"model":"qwen3:8b","message":{"role":"assistant","content":"{}"},"done":true,"remote_host":"https://ollama.com"}"#,
                "remote",
            ),
        ];
        for (status, body, expected) in cases {
            let result = generate_with_response(status, body).await;
            assert!(
                matches!(
                    (&result, expected),
                    (Err(InferenceError::Rejected { .. }), "rejected")
                        | (Err(InferenceError::MalformedResponse), "malformed")
                        | (Err(InferenceError::IncompleteResponse), "incomplete")
                        | (Err(InferenceError::ModelMismatch { .. }), "mismatch")
                        | (Err(InferenceError::RemoteResponse), "remote")
                ),
                "unexpected result for {expected}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn rejects_oversized_response_from_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (_request, mut stream) = accept_http_request(&listener);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_INFERENCE_RESPONSE_BYTES + 1
            )
            .unwrap();
        });
        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "qwen3:8b").unwrap();
        assert!(matches!(
            client
                .generate_structured(&inference_request(), &CancellationToken::new())
                .await,
            Err(InferenceError::ResponseTooLarge { .. })
        ));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_response_without_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (_request, mut stream) = accept_http_request(&listener);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let oversized = vec![b'x'; MAX_INFERENCE_RESPONSE_BYTES + 1];
            let _ = stream.write_all(&oversized);
        });
        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "qwen3:8b").unwrap();
        assert!(matches!(
            client
                .generate_structured(&inference_request(), &CancellationToken::new())
                .await,
            Err(InferenceError::ResponseTooLarge { .. })
        ));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn bounds_serialized_requests_and_rejected_error_bodies() {
        let client = OllamaClient::connect("http://127.0.0.1:9", "qwen3:8b").unwrap();
        let mut oversized = inference_request();
        oversized.prompt = "\0".repeat(crate::MAX_PROMPT_BYTES);
        assert!(matches!(
            client
                .generate_structured(&oversized, &CancellationToken::new())
                .await,
            Err(InferenceError::InvalidRequest(
                "serialized request exceeds size limit"
            ))
        ));

        let long_error = "x".repeat(MAX_ERROR_BODY_CHARS * 10);
        let result = generate_with_owned_response("500 Internal Server Error", long_error).await;
        let Err(InferenceError::Rejected { body, .. }) = result else {
            panic!("expected bounded rejection");
        };
        assert_eq!(body.chars().count(), MAX_ERROR_BODY_CHARS);
    }

    #[tokio::test]
    async fn cancellation_discards_a_late_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (accepted_sender, accepted_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let (_request, stream) = accept_http_request(&listener);
            accepted_sender.send(()).unwrap();
            thread::sleep(Duration::from_millis(100));
            let _ = write_http_response(
                stream,
                "200 OK",
                r#"{"model":"qwen3:8b","message":{"role":"assistant","content":"{}"},"done":true}"#,
            );
        });
        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "qwen3:8b").unwrap();
        let cancellation = CancellationToken::new();
        let cancellation_signal = cancellation.clone();
        let cancel = thread::spawn(move || {
            accepted_receiver.recv().unwrap();
            cancellation_signal.cancel();
        });
        assert!(matches!(
            client
                .generate_structured(&inference_request(), &cancellation)
                .await,
            Err(InferenceError::Cancelled)
        ));
        cancel.join().unwrap();
        server.join().unwrap();
    }

    #[tokio::test]
    async fn classifies_timeout_and_unavailable_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let _accepted = listener.accept().unwrap();
            thread::sleep(Duration::from_millis(100));
        });
        let mut client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "qwen3:8b").unwrap();
        client.inference_timeout = Duration::from_millis(20);
        assert!(matches!(
            client
                .generate_structured(&inference_request(), &CancellationToken::new())
                .await,
            Err(InferenceError::Timeout)
        ));
        server.join().unwrap();

        let unavailable = OllamaClient::connect("http://127.0.0.1:9", "qwen3:8b").unwrap();
        assert!(matches!(
            unavailable
                .generate_structured(&inference_request(), &CancellationToken::new())
                .await,
            Err(InferenceError::Unavailable(_))
        ));
    }

    fn inference_request() -> StructuredGenerationRequest {
        StructuredGenerationRequest {
            system: "Use only supplied evidence.".into(),
            prompt: "Question with evidence".into(),
            output_schema: json!({
                "type": "object",
                "properties": {"answer": {"type": "string"}},
                "required": ["answer"]
            }),
            max_output_tokens: 512,
        }
    }

    async fn generate_with_response(
        status: &'static str,
        body: &'static str,
    ) -> Result<InferenceResponse, InferenceError> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (_request, stream) = accept_http_request(&listener);
            write_http_response(stream, status, body).unwrap();
        });
        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "qwen3:8b").unwrap();
        let result = client
            .generate_structured(&inference_request(), &CancellationToken::new())
            .await;
        server.join().unwrap();
        result
    }

    async fn generate_with_owned_response(
        status: &'static str,
        body: String,
    ) -> Result<InferenceResponse, InferenceError> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (_request, stream) = accept_http_request(&listener);
            write_http_response(stream, status, &body).unwrap();
        });
        let client =
            OllamaClient::connect(&format!("http://127.0.0.1:{port}"), "qwen3:8b").unwrap();
        let result = client
            .generate_structured(&inference_request(), &CancellationToken::new())
            .await;
        server.join().unwrap();
        result
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

    async fn probe_embedding_with_details(
        details: &'static str,
    ) -> Result<OllamaEmbeddingRuntimeInfo, OllamaError> {
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
        let result = client.probe_embedding(&CancellationToken::new()).await;
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

    fn accept_http_request(listener: &TcpListener) -> (String, std::net::TcpStream) {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let length = stream.read(&mut buffer).unwrap();
            if length == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..length]);
            let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
        (String::from_utf8(request).unwrap(), stream)
    }

    fn http_body(request: &str) -> &str {
        request.split_once("\r\n\r\n").unwrap().1
    }

    fn write_http_response(
        mut stream: std::net::TcpStream,
        status: &str,
        body: &str,
    ) -> std::io::Result<()> {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }
}
