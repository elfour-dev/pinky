use pinky_core::OllamaClient;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "requires an explicitly configured local Ollama endpoint and model"]
async fn probes_an_explicit_target_host_ollama_model() {
    let endpoint = std::env::var("PINKY_OLLAMA_ENDPOINT")
        .expect("set PINKY_OLLAMA_ENDPOINT to an explicit IPv4-loopback origin");
    let model = std::env::var("PINKY_OLLAMA_MODEL")
        .expect("set PINKY_OLLAMA_MODEL to an installed local model name");

    let client = OllamaClient::connect(&endpoint, &model).unwrap();
    let info = client.probe(&CancellationToken::new()).await.unwrap();

    assert_eq!(info.model_name, model);
    assert!(info.context_size >= pinky_core::MIN_CHAT_CONTEXT);
    assert!(!info.version.trim().is_empty());
}
