use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use pinky_core::{
    answer_question, AnswerClaimV1, AnswerEnvelopeV1, ClaimSupportV1, ConversationService,
    Database, ImageOcrWorker, InferenceFuture, InferenceMetrics, InferenceProvider,
    InferenceResponse, LocalIngestor, MessageDraft, MountVerifier, ObjectStore, QuestionRequestV1,
    RetrievalService, StructuredGenerationRequest, Vault, QA_SCHEMA_VERSION,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroizing;

struct Mounted;

impl MountVerifier for Mounted {
    fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
        Ok(true)
    }
}

struct FixtureProvider {
    calls: AtomicUsize,
}

impl FixtureProvider {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl InferenceProvider for FixtureProvider {
    fn generate_structured<'a>(
        &'a self,
        request: &'a StructuredGenerationRequest,
        _: &'a CancellationToken,
    ) -> InferenceFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let marker = request.prompt.find("BEGIN-").expect("evidence marker");
            let start = marker
                + request.prompt[marker..]
                    .find('\n')
                    .expect("evidence marker line")
                + 1;
            let end = request
                .prompt
                .rfind("\nEND-")
                .expect("evidence marker terminator");
            let question: QuestionRequestV1 =
                serde_json::from_str(&request.prompt[start..end]).expect("bounded QA request");
            let citation = question
                .evidence
                .first()
                .expect("offline fixture supplies evidence")
                .citation_uri
                .clone();
            let answer = AnswerEnvelopeV1 {
                schema_version: QA_SCHEMA_VERSION,
                summary: "IRIS417 was raised by sustained unexpected flow; the retained runbook describes the operator response.".into(),
                summary_citations: vec![citation.clone()],
                claims: vec![AnswerClaimV1 {
                    statement: "The retained runbook says IRIS417 indicates sustained unexpected flow and directs the operator to inspect the zone before escalation.".into(),
                    support: ClaimSupportV1::Direct,
                    citations: vec![citation],
                }],
                warnings: vec![],
                unresolved_gaps: vec![],
            };
            Ok(InferenceResponse {
                model: "offline-fixture".into(),
                content: serde_json::to_string(&answer).expect("fixture answer JSON"),
                done_reason: Some("stop".into()),
                metrics: InferenceMetrics::default(),
            })
        })
    }
}

#[tokio::test]
async fn offline_cited_qa_round_trip_is_restart_safe() {
    let vault_root = tempfile::tempdir().unwrap();
    let vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
    let database = Arc::new(Mutex::new(
        Database::open(&vault, Zeroizing::new(vec![0x4d; 32])).unwrap(),
    ));
    let objects = ObjectStore::new(vault.clone());
    let approved_root = tempfile::tempdir().unwrap();
    let source_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/pinky-tutorial-sources");
    for name in [
        "02-decision-log.md",
        "03-alert-runbook.md",
        "06-sample-incident.log",
    ] {
        fs::copy(source_root.join(name), approved_root.path().join(name)).unwrap();
    }

    let ingestor = LocalIngestor::new(database.clone(), objects.clone());
    for name in [
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
        .search("IRIS417 sustained flow operator", 12)
        .unwrap();
    assert!(
        !hits.is_empty(),
        "tutorial evidence must be searchable offline"
    );

    let provider = FixtureProvider::new();
    let answer = answer_question(
        &provider,
        Uuid::new_v4(),
        "offline-fixture",
        "offline-fixture",
        "Why was IRIS417 raised and what should the operator do?",
        hits,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(provider.call_count(), 1);
    assert_eq!(answer.claims.len(), 1);
    assert!(answer.summary.contains("IRIS417"));
    let citation = answer.summary_citations.first().unwrap();
    let passage = retrieval.open_citation(citation).unwrap();
    assert!(passage.passage.contains("IRIS417"));
    assert_eq!(passage.citation_uri, *citation);

    let conversations = ConversationService::new(database.clone(), objects.clone());
    let conversation = conversations.create("Offline acceptance").unwrap();
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
            model: Some("offline-fixture"),
            citations: &answer.summary_citations,
            task_id: None,
            replaces_message_id: None,
        })
        .unwrap();

    let calls_before_gap = provider.call_count();
    let gap = answer_question(
        &provider,
        Uuid::new_v4(),
        "offline-fixture",
        "offline-fixture",
        "What is the operating temperature of a device absent from the sources?",
        Vec::new(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(gap.claims.is_empty());
    assert!(!gap.unresolved_gaps.is_empty());
    assert_eq!(provider.call_count(), calls_before_gap);

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled = answer_question(
        &provider,
        Uuid::new_v4(),
        "offline-fixture",
        "offline-fixture",
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

    drop(conversations);
    drop(database);
    let restarted_vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
    let restarted_database = Arc::new(Mutex::new(
        Database::open(&restarted_vault, Zeroizing::new(vec![0x4d; 32])).unwrap(),
    ));
    let restarted = ConversationService::new(restarted_database, ObjectStore::new(restarted_vault));
    let restored = restarted.get(conversation.id).unwrap();
    assert_eq!(restored.messages.len(), 2);
    assert_eq!(restored.messages[1].content, answer.summary);
    assert_eq!(restored.messages[1].citations, answer.summary_citations);
}

#[cfg(unix)]
#[tokio::test]
async fn image_ocr_attachment_survives_vault_reopen_and_remains_citable() {
    use std::os::unix::fs::PermissionsExt;

    let vault_root = tempfile::tempdir().unwrap();
    let vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
    let database = Arc::new(Mutex::new(
        Database::open(&vault, Zeroizing::new(vec![0x5a; 32])).unwrap(),
    ));
    let objects = ObjectStore::new(vault.clone());
    let approved_root = tempfile::tempdir().unwrap();
    let image = approved_root.path().join("restart.png");
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend_from_slice(&[0, 0, 1, 0, 0, 0, 1, 0, 8, 6, 0, 0, 0, 0]);
    fs::write(&image, bytes).unwrap();

    let ingestor = LocalIngestor::new(database.clone(), objects.clone());
    let retained = ingestor.ingest(approved_root.path(), &image).unwrap();
    let executable = vault_root.path().join("fake-ocr.sh");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf 'restart safe OCR text\\n' > \"$2.txt\"\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let worker = ImageOcrWorker::new(&executable, "eng").unwrap();
    ingestor
        .ocr_current_image(
            retained.source_id,
            retained.version_id,
            &worker,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let retrieval = RetrievalService::new(database.clone(), objects.clone());
    let before_restart = retrieval.search("restart", 5).unwrap();
    let before_ocr = before_restart
        .iter()
        .find(|hit| hit.coordinates["ocr"].as_bool().unwrap_or(false))
        .expect("OCR chunk must be searchable before restart");
    assert_eq!(before_ocr.version_id, retained.version_id);

    drop(retrieval);
    drop(ingestor);
    drop(database);
    drop(objects);

    let restarted_vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
    let restarted_database = Arc::new(Mutex::new(
        Database::open(&restarted_vault, Zeroizing::new(vec![0x5a; 32])).unwrap(),
    ));
    let restarted_objects = ObjectStore::new(restarted_vault);
    let restarted = RetrievalService::new(restarted_database, restarted_objects);
    let after_restart = restarted.search("restart", 5).unwrap();
    let after_ocr = after_restart
        .iter()
        .find(|hit| hit.coordinates["ocr"].as_bool().unwrap_or(false))
        .expect("OCR chunk must remain searchable after restart");
    assert_eq!(after_ocr.version_id, retained.version_id);
    let citation = restarted.open_citation(&after_ocr.citation_uri).unwrap();
    assert_eq!(citation.passage, "restart safe OCR text");
}
