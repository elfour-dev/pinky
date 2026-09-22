use std::{future::Future, pin::Pin};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub const MAX_SYSTEM_BYTES: usize = 16 * 1024;
pub const MAX_PROMPT_BYTES: usize = 256 * 1024;
pub const MAX_SCHEMA_BYTES: usize = 64 * 1024;
pub const MAX_INFERENCE_REQUEST_BYTES: usize = 512 * 1024;
pub const MAX_INFERENCE_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
// Cited answers are intentionally concise. Keeping this bounded prevents a
// slow local model from spending minutes producing an unnecessarily large
// response while leaving room for several citation URIs.
pub const MAX_OUTPUT_TOKENS: u32 = 512;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredGenerationRequest {
    pub system: String,
    pub prompt: String,
    pub output_schema: Value,
    pub max_output_tokens: u32,
}

impl StructuredGenerationRequest {
    pub fn validate(&self) -> Result<(), InferenceError> {
        if self.system.trim().is_empty() || self.system.len() > MAX_SYSTEM_BYTES {
            return Err(InferenceError::InvalidRequest("invalid system prompt size"));
        }
        if self.prompt.trim().is_empty() || self.prompt.len() > MAX_PROMPT_BYTES {
            return Err(InferenceError::InvalidRequest("invalid prompt size"));
        }
        if !self.output_schema.is_object()
            || serde_json::to_vec(&self.output_schema)
                .map_err(|_| InferenceError::InvalidRequest("invalid output schema"))?
                .len()
                > MAX_SCHEMA_BYTES
        {
            return Err(InferenceError::InvalidRequest("invalid output schema"));
        }
        if self.max_output_tokens == 0 || self.max_output_tokens > MAX_OUTPUT_TOKENS {
            return Err(InferenceError::InvalidRequest(
                "invalid maximum output tokens",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceResponse {
    pub model: String,
    pub content: String,
    pub done_reason: Option<String>,
    pub metrics: InferenceMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceMetrics {
    pub total_duration_ns: Option<u64>,
    pub load_duration_ns: Option<u64>,
    pub prompt_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Error)]
pub enum InferenceError {
    #[error("invalid inference request: {0}")]
    InvalidRequest(&'static str),
    #[error("inference was cancelled")]
    Cancelled,
    #[error("inference server is unavailable: {0}")]
    Unavailable(String),
    #[error("inference timed out")]
    Timeout,
    #[error("inference server rejected the request with HTTP {status}: {body}")]
    Rejected { status: u16, body: String },
    #[error("inference response exceeded the {limit}-byte limit")]
    ResponseTooLarge { limit: usize },
    #[error("inference server returned malformed output")]
    MalformedResponse,
    #[error("inference response was incomplete")]
    IncompleteResponse,
    #[error("inference response model `{found}` did not match attached model `{expected}`")]
    ModelMismatch { expected: String, found: String },
    #[error("inference response referred to a remote or cloud model")]
    RemoteResponse,
}

pub type InferenceFuture<'a> =
    Pin<Box<dyn Future<Output = Result<InferenceResponse, InferenceError>> + Send + 'a>>;

pub trait InferenceProvider: Send + Sync {
    fn generate_structured<'a>(
        &'a self,
        request: &'a StructuredGenerationRequest,
        cancellation: &'a CancellationToken,
    ) -> InferenceFuture<'a>;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request() -> StructuredGenerationRequest {
        StructuredGenerationRequest {
            system: "Return grounded JSON.".into(),
            prompt: "Question and evidence".into(),
            output_schema: json!({"type": "object"}),
            max_output_tokens: 512,
        }
    }

    #[test]
    fn validates_all_request_bounds() {
        request().validate().unwrap();

        let mut invalid = request();
        invalid.prompt = " ".into();
        assert!(matches!(
            invalid.validate(),
            Err(InferenceError::InvalidRequest(_))
        ));

        let mut invalid = request();
        invalid.system = "x".repeat(MAX_SYSTEM_BYTES + 1);
        assert!(matches!(
            invalid.validate(),
            Err(InferenceError::InvalidRequest(_))
        ));

        let mut invalid = request();
        invalid.prompt = "x".repeat(MAX_PROMPT_BYTES + 1);
        assert!(matches!(
            invalid.validate(),
            Err(InferenceError::InvalidRequest(_))
        ));

        let mut invalid = request();
        invalid.output_schema = Value::String("object".into());
        assert!(matches!(
            invalid.validate(),
            Err(InferenceError::InvalidRequest(_))
        ));

        let mut invalid = request();
        invalid.output_schema =
            json!({"type": "object", "description": "x".repeat(MAX_SCHEMA_BYTES)});
        assert!(matches!(
            invalid.validate(),
            Err(InferenceError::InvalidRequest(_))
        ));

        let mut invalid = request();
        invalid.max_output_tokens = 0;
        assert!(matches!(
            invalid.validate(),
            Err(InferenceError::InvalidRequest(_))
        ));

        let mut invalid = request();
        invalid.max_output_tokens = MAX_OUTPUT_TOKENS + 1;
        assert!(matches!(
            invalid.validate(),
            Err(InferenceError::InvalidRequest(_))
        ));
    }
}
