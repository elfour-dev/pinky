use pinky_core::{InferenceProvider, OllamaClient, StructuredGenerationRequest};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "requires an explicitly configured local Ollama endpoint and model"]
async fn probes_an_explicit_target_host_ollama_model() {
    let (endpoint, model) = configured_runtime();

    let client = OllamaClient::connect(&endpoint, &model).unwrap();
    let info = client.probe(&CancellationToken::new()).await.unwrap();

    assert_eq!(info.model_name, model);
    assert!(info.context_size >= pinky_core::MIN_CHAT_CONTEXT);
    assert!(!info.version.trim().is_empty());
}

#[tokio::test]
#[ignore = "requires an explicitly configured local Ollama endpoint and model"]
async fn generates_a_structured_target_host_response() {
    let (endpoint, model) = configured_runtime();
    let client = OllamaClient::connect(&endpoint, &model).unwrap();
    let response = client
        .generate_structured(
            &StructuredGenerationRequest {
                system: "Return only JSON matching the supplied schema.".into(),
                prompt: "Set the ping field to the single word pong.".into(),
                output_schema: json!({
                    "type": "object",
                    "properties": {"ping": {"type": "string"}},
                    "required": ["ping"],
                    "additionalProperties": false
                }),
                max_output_tokens: 64,
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let content: serde_json::Value = serde_json::from_str(&response.content).unwrap();
    assert!(content["ping"].is_string());
    assert_eq!(content.as_object().unwrap().len(), 1);
}

fn configured_runtime() -> (String, String) {
    let endpoint = std::env::var("PINKY_OLLAMA_ENDPOINT")
        .expect("set PINKY_OLLAMA_ENDPOINT to an explicit IPv4-loopback origin");
    let model = std::env::var("PINKY_OLLAMA_MODEL")
        .expect("set PINKY_OLLAMA_MODEL to an installed local model name");
    (endpoint, model)
}
