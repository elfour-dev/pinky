use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use chrono::Utc;
use pinky_core::{
    answer_question, ConversationService, Database, InferenceProvider, LocalIngestor, MessageDraft,
    MountVerifier, ObjectStore, OllamaClient, RetrievalService, StructuredGenerationRequest,
    TaskEvent, TaskJournal, TaskPhase, TaskState, Vault,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroizing;

struct Mounted;

impl MountVerifier for Mounted {
    fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
        Ok(true)
    }
}

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

#[tokio::test]
#[ignore = "requires an explicitly configured local Ollama endpoint and model"]
async fn completes_project_alder_cited_conversation_without_web_fetches() {
    let (endpoint, model) = configured_runtime();
    let client = OllamaClient::connect(&endpoint, &model).unwrap();
    client.probe(&CancellationToken::new()).await.unwrap();

    let vault_root = tempfile::tempdir().unwrap();
    let vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
    let database = Arc::new(Mutex::new(
        Database::open(&vault, Zeroizing::new(vec![0x6a; 32])).unwrap(),
    ));
    let objects = ObjectStore::new(vault.clone());
    let approved_root = tempfile::tempdir().unwrap();
    let source_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/pinky-tutorial-sources");
    for name in [
        "01-project-brief.md",
        "02-decision-log.md",
        "03-alert-runbook.md",
        "06-sample-incident.log",
    ] {
        fs::copy(source_root.join(name), approved_root.path().join(name)).unwrap();
    }

    let ingestor = LocalIngestor::new(database.clone(), objects.clone());
    for name in [
        "01-project-brief.md",
        "02-decision-log.md",
        "03-alert-runbook.md",
        "06-sample-incident.log",
    ] {
        ingestor
            .ingest(approved_root.path(), approved_root.path().join(name))
            .unwrap();
    }
    let retrieval = RetrievalService::new(database.clone(), objects.clone());
    let hits = retrieval
        .search("IRIS417 threshold duration operator runbook", 50)
        .unwrap();
    assert!(
        !hits.is_empty(),
        "Project Alder evidence must be searchable"
    );

    let task_id = Uuid::new_v4();
    let answer = answer_question(
        &client,
        task_id,
        "Ollama",
        &model,
        "Why was IRIS417 raised and what should the operator do?",
        hits,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!answer.summary_citations.is_empty());
    assert!(!answer.claims.is_empty());
    for citation in answer.summary_citations.iter().chain(
        answer
            .claims
            .iter()
            .flat_map(|claim| claim.citations.iter()),
    ) {
        assert!(retrieval.open_citation(citation).is_ok());
    }

    let conversations = ConversationService::new(database.clone(), objects.clone());
    let conversation = conversations.create("Project Alder acceptance").unwrap();
    TaskJournal::new(database.clone(), objects.clone())
        .append(&TaskEvent {
            schema_major: 1,
            schema_minor: 0,
            sequence: 0,
            task_id,
            parent_id: None,
            timestamp: Utc::now(),
            state: TaskState::Completed,
            phase: TaskPhase {
                name: "cited answer".into(),
                progress: Some(1.0),
                activity: "Validated Project Alder answer".into(),
            },
            tool_name: None,
            resource_uri: None,
            permission_state: "vault_only".into(),
            budget_state: "within_limits".into(),
            cancellable: false,
            error: None,
        })
        .unwrap();
    conversations
        .append_message(&MessageDraft {
            conversation_id: conversation.id,
            role: "user",
            content: "Why was IRIS417 raised and what should the operator do?",
            model: None,
            citations: &[],
            task_id: None,
            replaces_message_id: None,
        })
        .unwrap();
    conversations
        .append_message(&MessageDraft {
            conversation_id: conversation.id,
            role: "assistant",
            content: &answer.summary,
            model: Some(&model),
            citations: &answer.summary_citations,
            task_id: Some(task_id),
            replaces_message_id: None,
        })
        .unwrap();
    assert_eq!(
        conversations.get(conversation.id).unwrap().messages.len(),
        2
    );

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled = answer_question(
        &client,
        Uuid::new_v4(),
        "Ollama",
        &model,
        "Why was IRIS417 raised?",
        retrieval.search("IRIS417", 12).unwrap(),
        &cancellation,
    )
    .await;
    assert!(matches!(
        cancelled,
        Err(pinky_core::QaError::Inference(
            pinky_core::InferenceError::Cancelled
        ))
    ));
}

fn configured_runtime() -> (String, String) {
    let endpoint = std::env::var("PINKY_OLLAMA_ENDPOINT")
        .expect("set PINKY_OLLAMA_ENDPOINT to an explicit IPv4-loopback origin");
    let model = std::env::var("PINKY_OLLAMA_MODEL")
        .expect("set PINKY_OLLAMA_MODEL to an installed local model name");
    (endpoint, model)
}
