//! Deterministic, source-grounded claim assessment for R10.
//!
//! A caller may propose structure, but it cannot create a claim without exact
//! retained citations. Contradiction is preserved as evidence rather than
//! collapsed into a single supported conclusion.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    Database, InferenceError, InferenceProvider, RetrievalError, RetrievalService, SearchHit,
    StructuredGenerationRequest,
};
use tokio_util::sync::CancellationToken;

pub const CLAIM_SCHEMA_VERSION: u16 = 1;
pub const MAX_CLAIM_EVIDENCE: usize = 12;
pub const MAX_CLAIM_FIELD_BYTES: usize = 2 * 1024;
pub const MAX_ENTITY_ALIASES: usize = 16;
pub const MAX_DOSSIER_REFRESHES: usize = 32;
pub const STALE_AFTER_DAYS: i64 = 90;
pub const MAX_EXTRACTED_CLAIMS: usize = 12;
pub const MAX_UNRESOLVED_QUESTIONS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRelationshipV1 {
    Supporting,
    Contradicting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimEvidenceV1 {
    pub citation_uri: String,
    pub relationship: EvidenceRelationshipV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimDraftV1 {
    pub schema_version: u16,
    pub topic: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    #[serde(default)]
    pub subject_aliases: Vec<String>,
    #[serde(default)]
    pub object_aliases: Vec<String>,
    #[serde(default)]
    pub inferred: bool,
    pub evidence: Vec<ClaimEvidenceV1>,
}

/// The only extraction shape accepted from a model. It has no citation or
/// chunk-ID fields: references are indexes into Pinky's fixed evidence set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexedClaimProposalV1 {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    #[serde(default)]
    pub subject_aliases: Vec<String>,
    #[serde(default)]
    pub object_aliases: Vec<String>,
    #[serde(default)]
    pub inferred: bool,
    pub supporting_evidence: Vec<usize>,
    #[serde(default)]
    pub contradicting_evidence: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexedClaimExtractionResponseV1 {
    pub claims: Vec<IndexedClaimProposalV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatusV1 {
    Supported,
    Disputed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefreshCandidateV1 {
    pub source_id: Uuid,
    pub refresh_policy: String,
    pub last_checked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshScheduleV1 {
    pub due_source_ids: Vec<Uuid>,
    pub skipped_over_budget: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopicDossierV1 {
    pub topic_id: Uuid,
    pub label: String,
    pub metrics: DossierMetricsV1,
    pub unresolved_questions: Vec<String>,
    pub unresolved_question_score: f32,
    pub warnings: Vec<String>,
    pub claims: Vec<ClaimReviewV1>,
}

/// A reviewable claim keeps both sides of a conflict visible.  It deliberately
/// contains canonical citations rather than model-supplied identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimReviewV1 {
    pub claim_id: Uuid,
    pub statement: String,
    pub status: ClaimStatusV1,
    pub inferred: bool,
    pub warnings: Vec<String>,
    pub evidence: Vec<ClaimEvidenceV1>,
}

#[derive(Debug, Error)]
pub enum ClaimError {
    #[error("unsupported claim schema version")]
    UnsupportedSchema,
    #[error("claim {field} must contain between 1 and {MAX_CLAIM_FIELD_BYTES} bytes")]
    InvalidField { field: &'static str },
    #[error("a claim requires between 1 and {MAX_CLAIM_EVIDENCE} evidence entries")]
    InvalidEvidenceCount,
    #[error("claim evidence must use an exact retained citation URI")]
    InvalidCitation,
    #[error("claim evidence contains a duplicate citation relationship")]
    DuplicateEvidence,
    #[error("claim has no supporting evidence")]
    Unsupported,
    #[error("a direct claim does not match any supporting retained passage")]
    UnsupportedEvidence,
    #[error("entity aliases are invalid")]
    InvalidAliases,
    #[error("extraction proposal is invalid")]
    InvalidExtraction,
    #[error("claim database lock is poisoned")]
    DatabaseLock,
    #[error("claim database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("claim citation resolution failed: {0}")]
    Retrieval(#[from] RetrievalError),
    #[error("claim extraction inference failed: {0}")]
    Inference(#[from] InferenceError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedClaim {
    pub claim_id: Uuid,
    pub topic_id: Uuid,
    pub status: ClaimStatusV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DossierMetricsV1 {
    pub coverage: f32,
    pub authority: f32,
    pub independence: f32,
    pub freshness: f32,
    pub disputed: bool,
}

/// Deterministic, bounded scoring over already trusted source evidence.
pub fn score_dossier(evidence: &[(u8, bool, bool)], disputed: bool) -> DossierMetricsV1 {
    if evidence.is_empty() {
        return DossierMetricsV1 {
            coverage: 0.0,
            authority: 0.0,
            independence: 0.0,
            freshness: 0.0,
            disputed,
        };
    }
    let sources = evidence
        .iter()
        .filter(|(_, _, duplicate)| !duplicate)
        .count() as f32;
    let total = evidence.len() as f32;
    let authority = evidence
        .iter()
        .map(|(tier, _, _)| (5_u8.saturating_sub(*tier)) as f32 / 4.0)
        .sum::<f32>()
        / total;
    let freshness = evidence.iter().filter(|(_, fresh, _)| *fresh).count() as f32 / total;
    DossierMetricsV1 {
        coverage: (total / 5.0).min(1.0),
        authority,
        independence: (sources / total).min(1.0),
        freshness,
        disputed,
    }
}

/// Bounded, deterministic refresh selection. Local filesystem sources are
/// event-driven and must never be put on a network refresh queue.
pub fn schedule_refreshes(
    candidates: &[RefreshCandidateV1],
    now: DateTime<Utc>,
    limit: usize,
) -> RefreshScheduleV1 {
    let limit = limit.min(MAX_DOSSIER_REFRESHES);
    let stale_before = now - Duration::days(STALE_AFTER_DAYS);
    let mut due: Vec<_> = candidates
        .iter()
        .filter(|source| source.refresh_policy != "filesystem_event")
        .filter(|source| {
            source
                .last_checked_at
                .is_none_or(|checked| checked < stale_before)
        })
        .collect();
    due.sort_by_key(|source| (source.last_checked_at, source.source_id));
    let skipped_over_budget = due.len().saturating_sub(limit);
    RefreshScheduleV1 {
        due_source_ids: due
            .into_iter()
            .take(limit)
            .map(|source| source.source_id)
            .collect(),
        skipped_over_budget,
    }
}

/// Convert index-only model proposals to canonical retained citations. This is
/// the extraction trust boundary: an out-of-range or duplicate index fails
/// before a draft can reach persistence.
pub fn drafts_from_indexed_proposals(
    topic: &str,
    evidence: &[SearchHit],
    proposals: &[IndexedClaimProposalV1],
) -> Result<Vec<ClaimDraftV1>, ClaimError> {
    if topic.trim().is_empty() || proposals.len() > MAX_EXTRACTED_CLAIMS {
        return Err(ClaimError::InvalidExtraction);
    }
    proposals
        .iter()
        .map(|proposal| {
            // An index identifies one retained passage.  It may have exactly
            // one role in a proposal; allowing it in both lists lets a model
            // manufacture a dispute from one sentence.
            let mut used = HashSet::new();
            let mut items = Vec::new();
            for (indexes, relationship) in [
                (
                    &proposal.supporting_evidence,
                    EvidenceRelationshipV1::Supporting,
                ),
                (
                    &proposal.contradicting_evidence,
                    EvidenceRelationshipV1::Contradicting,
                ),
            ] {
                for index in indexes {
                    let hit = evidence.get(*index).ok_or(ClaimError::InvalidExtraction)?;
                    if !used.insert(*index) {
                        return Err(ClaimError::InvalidExtraction);
                    }
                    items.push(ClaimEvidenceV1 {
                        citation_uri: hit.citation_uri.clone(),
                        relationship,
                    });
                }
            }
            // Automatic direct claims are deliberately conservative.  A
            // proposal outside the small grammar, or one that does not occur
            // verbatim in its supporting evidence, is retained only as an
            // explicitly labelled inference for review.
            let direct_match = proposal.supporting_evidence.iter().any(|index| {
                evidence
                    .get(*index)
                    .is_some_and(|hit| direct_proposal_matches(proposal, &hit.passage))
            });
            let draft = ClaimDraftV1 {
                schema_version: CLAIM_SCHEMA_VERSION,
                topic: topic.to_owned(),
                subject: proposal.subject.clone(),
                predicate: proposal.predicate.clone(),
                object: proposal.object.clone(),
                subject_aliases: proposal.subject_aliases.clone(),
                object_aliases: proposal.object_aliases.clone(),
                inferred: proposal.inferred
                    || !is_direct_predicate(&proposal.predicate)
                    || !direct_match,
                evidence: items,
            };
            let allowed = draft
                .evidence
                .iter()
                .map(|item| item.citation_uri.as_str())
                .collect();
            assess_claim(&draft, &allowed)?;
            Ok(draft)
        })
        .collect()
}

/// The cancellable extraction workflow used by the desktop. The local model
/// sees text plus ordinal indexes only; URI and chunk identity never enter its
/// output contract.
pub async fn extract_retained_claims<P: InferenceProvider>(
    provider: &P,
    store: &ClaimStore,
    retrieval: &RetrievalService,
    topic: &str,
    query: &str,
    cancellation: &CancellationToken,
) -> Result<Vec<PersistedClaim>, ClaimError> {
    if cancellation.is_cancelled() {
        return Err(ClaimError::Inference(InferenceError::Cancelled));
    }
    if topic.trim().is_empty() || query.trim().is_empty() {
        return Err(ClaimError::InvalidExtraction);
    }
    let evidence = retrieval.search(query, MAX_EXTRACTED_CLAIMS)?;
    if evidence.is_empty() {
        return Ok(Vec::new());
    }
    let prompt = evidence
        .iter()
        .enumerate()
        .map(|(index, hit)| format!("[{index}] {}", hit.passage))
        .collect::<Vec<_>>()
        .join("\n\n");
    let request = StructuredGenerationRequest {
        system: "Extract only source-grounded claims from supplied retained passages. Return JSON with zero-based evidence indexes only; never citations, URIs, chunk IDs, or unsupported facts.".into(),
        prompt: format!("Topic: {}\n\nRetained evidence:\n{}", topic.trim(), prompt),
        output_schema: serde_json::json!({"type":"object","additionalProperties":false,"required":["claims"],"properties":{"claims":{"type":"array","maxItems":MAX_EXTRACTED_CLAIMS}}}),
        max_output_tokens: 512,
    };
    request.validate()?;
    let response = provider.generate_structured(&request, cancellation).await?;
    if cancellation.is_cancelled() {
        return Err(ClaimError::Inference(InferenceError::Cancelled));
    }
    let proposed: IndexedClaimExtractionResponseV1 =
        serde_json::from_str(&response.content).map_err(|_| ClaimError::InvalidExtraction)?;
    let drafts = drafts_from_indexed_proposals(topic.trim(), &evidence, &proposed.claims)?;
    let mut stored = Vec::with_capacity(drafts.len());
    for draft in drafts {
        if cancellation.is_cancelled() {
            return Err(ClaimError::Inference(InferenceError::Cancelled));
        }
        stored.push(store.record_from_retrieval(&draft, retrieval)?);
    }
    Ok(stored)
}

/// User-facing labels are derived from evidence state, never model prose.
pub fn claim_warnings(
    status: ClaimStatusV1,
    metrics: &DossierMetricsV1,
    source_count: usize,
    inferred: bool,
) -> Vec<&'static str> {
    let mut warnings = Vec::new();
    if status == ClaimStatusV1::Disputed || metrics.disputed {
        warnings.push("Disputed: retained sources contain contradicting evidence.");
    }
    if metrics.freshness < 1.0 {
        warnings.push("May be stale: one or more sources have not been checked recently.");
    }
    if source_count < 2 {
        warnings.push("Single-source: this statement has only one independent source.");
    }
    if inferred {
        warnings.push(
            "Inferred: this statement is a labeled inference, not a direct source statement.",
        );
    }
    warnings
}

#[derive(Clone)]
pub struct ClaimStore {
    database: Arc<Mutex<Database>>,
}

impl ClaimStore {
    pub fn new(database: Arc<Mutex<Database>>) -> Self {
        Self { database }
    }

    /// Store only a claim whose caller-bound citations resolve to retained chunks.
    fn record(
        &self,
        claim: &ClaimDraftV1,
        citation_chunks: &HashMap<String, Uuid>,
    ) -> Result<PersistedClaim, ClaimError> {
        let allowed = citation_chunks.keys().map(String::as_str).collect();
        let status = assess_claim(claim, &allowed)?;
        let database = self.database.lock().map_err(|_| ClaimError::DatabaseLock)?;
        let transaction = database.connection().unchecked_transaction()?;
        let topic: Option<String> = transaction
            .query_row(
                "SELECT id FROM topics WHERE label = ?1 ORDER BY id LIMIT 1",
                [&claim.topic],
                |row| row.get(0),
            )
            .optional()?;
        let topic_id = match topic {
            Some(id) => Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
            None => {
                let id = Uuid::new_v4();
                transaction.execute("INSERT INTO topics (id, label, aliases_json, freshness_status, unresolved_questions_json) VALUES (?1, ?2, ?3, 'unknown', '[]')", params![id.to_string(), claim.topic, serde_json::json!([claim.topic]).to_string()])?;
                id
            }
        };
        let claim_id = Uuid::new_v4();
        let (stored_status, confidence) = match status {
            ClaimStatusV1::Supported => ("supported", 0.75_f64),
            ClaimStatusV1::Disputed => ("disputed", 0.5_f64),
        };
        let now = Utc::now().to_rfc3339();
        transaction.execute("INSERT INTO claims (id, subject, predicate, object, topic_id, status, confidence, first_seen_at, last_verified_at, inferred) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)", params![claim_id.to_string(), claim.subject, claim.predicate, claim.object, topic_id.to_string(), stored_status, confidence, now, claim.inferred])?;
        for item in &claim.evidence {
            let chunk = citation_chunks
                .get(&item.citation_uri)
                .ok_or(ClaimError::InvalidCitation)?;
            let relation = match item.relationship {
                EvidenceRelationshipV1::Supporting => "supporting",
                EvidenceRelationshipV1::Contradicting => "contradicting",
            };
            transaction.execute(
                "INSERT INTO claim_evidence (claim_id, chunk_id, relationship) VALUES (?1, ?2, ?3)",
                params![claim_id.to_string(), chunk.to_string(), relation],
            )?;
        }
        let subject_entity =
            upsert_entity(&transaction, &claim.subject, &claim.subject_aliases, &now)?;
        let object_entity =
            upsert_entity(&transaction, &claim.object, &claim.object_aliases, &now)?;
        // A self-link is valid evidence but not a useful entity relationship;
        // retain the claim and omit only the redundant graph edge.
        if subject_entity != object_entity {
            transaction.execute(
                "INSERT INTO entity_relationships (id, subject_entity_id, predicate, object_entity_id, claim_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![Uuid::new_v4().to_string(), subject_entity.to_string(), claim.predicate, object_entity.to_string(), claim_id.to_string(), now],
            )?;
        }
        transaction.commit()?;
        Ok(PersistedClaim {
            claim_id,
            topic_id,
            status,
        })
    }

    /// Resolve every cited passage through retrieval before persistence. This
    /// deliberately derives chunk identifiers from Pinky's citation parser,
    /// rather than accepting identifiers from a model or frontend caller.
    pub fn record_from_retrieval(
        &self,
        claim: &ClaimDraftV1,
        retrieval: &RetrievalService,
    ) -> Result<PersistedClaim, ClaimError> {
        let mut bindings = HashMap::new();
        let mut supporting_passages = Vec::new();
        for evidence in &claim.evidence {
            let passage = retrieval.open_citation(&evidence.citation_uri)?;
            if evidence.relationship == EvidenceRelationshipV1::Supporting {
                supporting_passages.push(passage.passage);
            }
            bindings.insert(evidence.citation_uri.clone(), passage.chunk_id);
        }
        if !claim.inferred
            && !supporting_passages
                .iter()
                .any(|passage| direct_claim_matches(claim, passage))
        {
            return Err(ClaimError::UnsupportedEvidence);
        }
        self.record(claim, &bindings)
    }

    /// Consolidate the retained evidence for one topic. The query deliberately
    /// follows claim_evidence to current sources; aliases and summaries cannot
    /// increase a score on their own.
    pub fn dossier(
        &self,
        topic_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<TopicDossierV1, ClaimError> {
        let database = self.database.lock().map_err(|_| ClaimError::DatabaseLock)?;
        let (label, unresolved_json): (String, String) = database.connection().query_row(
            "SELECT label, unresolved_questions_json FROM topics WHERE id = ?1",
            [topic_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let mut statement = database.connection().prepare(
            "SELECT s.id, s.authority_tier, s.last_checked_at, c.status, c.inferred
             FROM claims c
             JOIN claim_evidence ce ON ce.claim_id = c.id
             JOIN chunks ch ON ch.id = ce.chunk_id
             JOIN source_versions sv ON sv.id = ch.source_version_id
             JOIN sources s ON s.id = sv.source_id
             WHERE c.topic_id = ?1 AND s.state = 'active'",
        )?;
        let mut rows = statement.query([topic_id.to_string()])?;
        let stale_before = now - Duration::days(STALE_AFTER_DAYS);
        let mut seen_sources = HashSet::new();
        let mut evidence = Vec::new();
        let mut disputed = false;
        let mut inferred = false;
        while let Some(row) = rows.next()? {
            let source_id: String = row.get(0)?;
            let tier: u8 = row.get(1)?;
            let checked: Option<String> = row.get(2)?;
            let fresh = checked
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc) >= stale_before)
                .unwrap_or(false);
            let duplicate = !seen_sources.insert(source_id);
            let status: String = row.get(3)?;
            disputed |= status == "disputed";
            inferred |= row.get::<_, bool>(4)?;
            evidence.push((tier, fresh, duplicate));
        }
        let metrics = score_dossier(&evidence, disputed);
        let unresolved_questions: Vec<String> =
            serde_json::from_str(&unresolved_json).unwrap_or_default();
        let unresolved_question_score = (unresolved_questions.len() as f32 / 5.0).min(1.0);
        let warnings = claim_warnings(
            if disputed {
                ClaimStatusV1::Disputed
            } else {
                ClaimStatusV1::Supported
            },
            &metrics,
            seen_sources.len(),
            inferred,
        )
        .into_iter()
        .map(str::to_owned)
        .collect();
        drop(rows);
        drop(statement);
        database.connection().execute(
            "UPDATE topics SET coverage_score = ?1, authority_score = ?2,
             independence_score = ?3, freshness_score = ?4,
             freshness_status = ?5, unresolved_questions_json = ?6,
             unresolved_question_score = ?7, last_consolidated_at = ?8 WHERE id = ?9",
            params![
                metrics.coverage * 100.0,
                metrics.authority * 100.0,
                metrics.independence * 100.0,
                metrics.freshness * 100.0,
                if metrics.freshness < 1.0 { "stale" } else { "fresh" },
                serde_json::to_string(&unresolved_questions).map_err(|_| rusqlite::Error::InvalidQuery)?,
                unresolved_question_score * 100.0,
                now.to_rfc3339(),
                topic_id.to_string()
            ],
        )?;
        drop(database);
        let claims = self.claim_reviews(topic_id, now)?;
        Ok(TopicDossierV1 {
            topic_id,
            label,
            metrics,
            unresolved_questions,
            unresolved_question_score,
            warnings,
            claims,
        })
    }

    fn claim_reviews(
        &self,
        topic_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Vec<ClaimReviewV1>, ClaimError> {
        let database = self.database.lock().map_err(|_| ClaimError::DatabaseLock)?;
        let mut statement = database.connection().prepare(
            "SELECT c.id, c.subject, c.predicate, c.object, c.status, c.inferred,
                    ce.relationship, s.id, sv.id, ch.ordinal, s.last_checked_at
             FROM claims c JOIN claim_evidence ce ON ce.claim_id = c.id
             JOIN chunks ch ON ch.id = ce.chunk_id
             JOIN source_versions sv ON sv.id = ch.source_version_id
             JOIN sources s ON s.id = sv.source_id
             WHERE c.topic_id = ?1 ORDER BY c.first_seen_at, c.id, ce.relationship, ch.ordinal",
        )?;
        let stale_before = now - Duration::days(STALE_AFTER_DAYS);
        let mut grouped: std::collections::BTreeMap<
            Uuid,
            (
                String,
                ClaimStatusV1,
                bool,
                HashSet<String>,
                bool,
                Vec<ClaimEvidenceV1>,
            ),
        > = std::collections::BTreeMap::new();
        let mut rows = statement.query([topic_id.to_string()])?;
        while let Some(row) = rows.next()? {
            let id = Uuid::parse_str(&row.get::<_, String>(0)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let status = if row.get::<_, String>(4)? == "disputed" {
                ClaimStatusV1::Disputed
            } else {
                ClaimStatusV1::Supported
            };
            let source_id: String = row.get(7)?;
            let checked: Option<String> = row.get(10)?;
            let fresh = checked
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc) >= stale_before)
                .unwrap_or(false);
            let relationship = if row.get::<_, String>(6)? == "contradicting" {
                EvidenceRelationshipV1::Contradicting
            } else {
                EvidenceRelationshipV1::Supporting
            };
            let claim_statement = format!(
                "{} {} {}",
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?
            );
            let inferred: bool = row.get(5)?;
            let source_version: String = row.get(8)?;
            let ordinal: u64 = row.get(9)?;
            let entry = grouped.entry(id).or_insert_with(|| {
                (
                    claim_statement,
                    status,
                    inferred,
                    HashSet::new(),
                    true,
                    Vec::new(),
                )
            });
            entry.3.insert(source_id);
            entry.4 &= fresh;
            entry.5.push(ClaimEvidenceV1 {
                citation_uri: format!(
                    "pinky://source/{}/version/{}#chunk-{}",
                    row.get::<_, String>(7)?,
                    source_version,
                    ordinal
                ),
                relationship,
            });
        }
        Ok(grouped
            .into_iter()
            .map(
                |(claim_id, (statement, status, inferred, sources, fresh, evidence))| {
                    let metrics = DossierMetricsV1 {
                        coverage: 0.0,
                        authority: 0.0,
                        independence: 0.0,
                        freshness: if fresh { 1.0 } else { 0.0 },
                        disputed: status == ClaimStatusV1::Disputed,
                    };
                    ClaimReviewV1 {
                        claim_id,
                        statement,
                        status,
                        inferred,
                        warnings: claim_warnings(status, &metrics, sources.len(), inferred)
                            .into_iter()
                            .map(str::to_owned)
                            .collect(),
                        evidence,
                    }
                },
            )
            .collect())
    }

    /// Read the source inventory inside the encrypted vault and return only a
    /// bounded set of stale, refreshable sources. This deliberately does not
    /// fetch anything: network execution and its permissions are R11's
    /// boundary.
    pub fn due_refreshes(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<RefreshScheduleV1, ClaimError> {
        let database = self.database.lock().map_err(|_| ClaimError::DatabaseLock)?;
        let mut statement = database.connection().prepare(
            "SELECT s.id, s.refresh_policy, s.last_checked_at, rs.next_due_at
             FROM sources s
             LEFT JOIN source_refresh_schedule rs ON rs.source_id = s.id
             WHERE s.state = 'active'",
        )?;
        let candidates = statement
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let checked: Option<String> = row.get(2)?;
                let next_due: Option<String> = row.get(3)?;
                Ok((RefreshCandidateV1 {
                    source_id: Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    refresh_policy: row.get(1)?,
                    last_checked_at: checked
                        .as_deref()
                        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                        .map(|value| value.with_timezone(&Utc)),
                }, next_due))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        // A persisted retry/next-due value is authoritative across process
        // restarts.  Re-evaluating source freshness must not pull that work
        // forward and defeat bounded backoff.
        let eligible = candidates
            .into_iter()
            .filter(|(_, next_due)| {
                next_due
                    .as_deref()
                    .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                    .is_none_or(|due| due.with_timezone(&Utc) <= now)
            })
            .map(|(candidate, _)| candidate)
            .collect::<Vec<_>>();
        Ok(schedule_refreshes(&eligible, now, limit))
    }

    /// Persist a bounded refresh plan inside SQLCipher. Scheduling is durable;
    /// R11 may later claim these rows to perform permitted public-web work.
    pub fn schedule_due_refreshes(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<RefreshScheduleV1, ClaimError> {
        let schedule = self.due_refreshes(now, limit)?;
        let database = self.database.lock().map_err(|_| ClaimError::DatabaseLock)?;
        let transaction = database.connection().unchecked_transaction()?;
        for source_id in &schedule.due_source_ids {
            transaction.execute(
                "INSERT INTO source_refresh_schedule (source_id, next_due_at, state, attempts, last_error, updated_at)
                 VALUES (?1, ?2, 'scheduled', 0, NULL, ?2)
                 ON CONFLICT(source_id) DO UPDATE SET
                   next_due_at = CASE WHEN source_refresh_schedule.state IN ('claimed', 'retry') THEN source_refresh_schedule.next_due_at ELSE excluded.next_due_at END,
                   state = CASE WHEN source_refresh_schedule.state IN ('claimed', 'retry') THEN source_refresh_schedule.state ELSE 'scheduled' END,
                   updated_at = excluded.updated_at",
                params![source_id.to_string(), now.to_rfc3339()],
            )?;
        }
        transaction.commit()?;
        Ok(schedule)
    }

    /// Explicit user-managed unresolved questions.  Conflicts may contribute
    /// to a dossier warning, but must never silently invent a question.
    pub fn set_unresolved_questions(
        &self,
        topic_id: Uuid,
        questions: &[String],
    ) -> Result<(), ClaimError> {
        if questions.len() > MAX_UNRESOLVED_QUESTIONS {
            return Err(ClaimError::InvalidExtraction);
        }
        let mut seen = HashSet::new();
        let mut retained = Vec::with_capacity(questions.len());
        for question in questions {
            let question = question.trim();
            if question.is_empty()
                || question.len() > MAX_CLAIM_FIELD_BYTES
                || !seen.insert(question.to_lowercase())
            {
                return Err(ClaimError::InvalidExtraction);
            }
            retained.push(question);
        }
        let database = self.database.lock().map_err(|_| ClaimError::DatabaseLock)?;
        let changed = database.connection().execute(
            "UPDATE topics SET unresolved_questions_json = ?1 WHERE id = ?2",
            params![
                serde_json::to_string(&retained).map_err(|_| rusqlite::Error::InvalidQuery)?,
                topic_id.to_string()
            ],
        )?;
        if changed != 1 {
            return Err(ClaimError::InvalidExtraction);
        }
        Ok(())
    }

    /// Consolidate every topic in a stable order for the dossier view. Each
    /// dossier is scored from retained claim evidence, not from a model summary.
    pub fn dossiers(&self, now: DateTime<Utc>) -> Result<Vec<TopicDossierV1>, ClaimError> {
        let topic_ids = {
            let database = self.database.lock().map_err(|_| ClaimError::DatabaseLock)?;
            let mut statement = database
                .connection()
                .prepare("SELECT id FROM topics ORDER BY label COLLATE NOCASE, id")?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .map(|result| {
                    Uuid::parse_str(&result?).map_err(|_| rusqlite::Error::InvalidQuery.into())
                })
                .collect::<Result<Vec<_>, ClaimError>>()?;
            ids
        };
        topic_ids
            .into_iter()
            .map(|topic_id| self.dossier(topic_id, now))
            .collect()
    }
}

/// The direct-claim grammar intentionally stays narrow and transparent: a
/// retained passage must contain the normalized subject–predicate–object
/// phrase. Other model/user proposals must be explicitly labelled inferred.
fn direct_claim_matches(claim: &ClaimDraftV1, passage: &str) -> bool {
    is_direct_predicate(&claim.predicate)
        && direct_proposal_matches_fields(&claim.subject, &claim.predicate, &claim.object, passage)
}

/// The typed direct-claim grammar.  These are intentionally simple copular
/// and possession/action relations that can be verified with a deterministic
/// contiguous passage match; anything richer needs human review as inference.
fn is_direct_predicate(predicate: &str) -> bool {
    matches!(
        predicate.trim().to_ascii_lowercase().as_str(),
        "is" | "was"
            | "are"
            | "has"
            | "have"
            | "uses"
            | "used"
            | "targets"
            | "supports"
            | "contains"
            | "requires"
            | "owns"
    )
}

fn direct_proposal_matches(proposal: &IndexedClaimProposalV1, passage: &str) -> bool {
    direct_proposal_matches_fields(
        &proposal.subject,
        &proposal.predicate,
        &proposal.object,
        passage,
    )
}

fn direct_proposal_matches_fields(
    subject: &str,
    predicate: &str,
    object: &str,
    passage: &str,
) -> bool {
    let phrase = format!("{} {} {}", subject, predicate, object)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    !phrase.is_empty() && passage.to_lowercase().contains(&phrase)
}

fn upsert_entity(
    transaction: &rusqlite::Transaction<'_>,
    canonical_name: &str,
    aliases: &[String],
    now: &str,
) -> Result<Uuid, ClaimError> {
    validate_aliases(canonical_name, aliases)?;
    let normalized = canonical_name.trim().to_lowercase();
    let existing: Option<String> = transaction
        .query_row(
            "SELECT e.id FROM entities e LEFT JOIN entity_aliases a ON a.entity_id = e.id
         WHERE lower(e.canonical_name) = ?1 OR a.normalized_alias = ?1 ORDER BY e.id LIMIT 1",
            [&normalized],
            |row| row.get(0),
        )
        .optional()?;
    let id = match existing {
        Some(id) => Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        None => {
            let id = Uuid::new_v4();
            transaction.execute(
                "INSERT INTO entities (id, canonical_name, entity_type, created_at, updated_at) VALUES (?1, ?2, 'named_thing', ?3, ?3)",
                params![id.to_string(), canonical_name.trim(), now],
            )?;
            id
        }
    };
    for alias in aliases {
        transaction.execute(
            "INSERT INTO entity_aliases (entity_id, alias, normalized_alias) VALUES (?1, ?2, ?3)
             ON CONFLICT(normalized_alias) DO NOTHING",
            params![id.to_string(), alias.trim(), alias.trim().to_lowercase()],
        )?;
    }
    Ok(id)
}

fn validate_aliases(canonical_name: &str, aliases: &[String]) -> Result<(), ClaimError> {
    if aliases.len() > MAX_ENTITY_ALIASES {
        return Err(ClaimError::InvalidAliases);
    }
    let canonical = canonical_name.trim().to_lowercase();
    let mut seen = HashSet::new();
    for alias in aliases {
        let alias = alias.trim();
        if alias.is_empty()
            || alias.len() > MAX_CLAIM_FIELD_BYTES
            || alias.to_lowercase() == canonical
            || !seen.insert(alias.to_lowercase())
        {
            return Err(ClaimError::InvalidAliases);
        }
    }
    Ok(())
}

/// Validate the storage-ready claim shape and derive its status. `allowed` is
/// the exact citation set supplied by retrieval; model-generated URIs fail
/// closed before any database write can occur.
pub fn assess_claim(
    claim: &ClaimDraftV1,
    allowed: &HashSet<&str>,
) -> Result<ClaimStatusV1, ClaimError> {
    if claim.schema_version != CLAIM_SCHEMA_VERSION {
        return Err(ClaimError::UnsupportedSchema);
    }
    for (field, value) in [
        ("topic", claim.topic.as_str()),
        ("subject", claim.subject.as_str()),
        ("predicate", claim.predicate.as_str()),
        ("object", claim.object.as_str()),
    ] {
        if value.trim().is_empty() || value.len() > MAX_CLAIM_FIELD_BYTES {
            return Err(ClaimError::InvalidField { field });
        }
    }
    if claim.evidence.is_empty() || claim.evidence.len() > MAX_CLAIM_EVIDENCE {
        return Err(ClaimError::InvalidEvidenceCount);
    }
    let mut seen = HashSet::new();
    let mut supporting = false;
    let mut contradicting = false;
    for evidence in &claim.evidence {
        if !allowed.contains(evidence.citation_uri.as_str()) {
            return Err(ClaimError::InvalidCitation);
        }
        // A retained passage has one evidentiary role for a claim.  In
        // particular, a caller must not be able to manufacture a dispute by
        // submitting the same passage once as support and once as a
        // contradiction.
        if !seen.insert(&evidence.citation_uri) {
            return Err(ClaimError::DuplicateEvidence);
        }
        match evidence.relationship {
            EvidenceRelationshipV1::Supporting => supporting = true,
            EvidenceRelationshipV1::Contradicting => contradicting = true,
        }
    }
    if !supporting {
        return Err(ClaimError::Unsupported);
    }
    Ok(if contradicting {
        ClaimStatusV1::Disputed
    } else {
        ClaimStatusV1::Supported
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        InferenceFuture, InferenceMetrics, InferenceProvider, InferenceResponse, MountVerifier,
        ObjectStore, Vault,
    };
    use std::path::Path;
    use zeroize::Zeroizing;

    struct NeverModel;
    impl InferenceProvider for NeverModel {
        fn generate_structured<'a>(
            &'a self,
            _: &'a StructuredGenerationRequest,
            _: &'a CancellationToken,
        ) -> InferenceFuture<'a> {
            Box::pin(async { panic!("cancelled extraction must not invoke a model") })
        }
    }

    struct StaticModel(String);
    impl InferenceProvider for StaticModel {
        fn generate_structured<'a>(
            &'a self,
            _: &'a StructuredGenerationRequest,
            _: &'a CancellationToken,
        ) -> InferenceFuture<'a> {
            Box::pin(async move {
                Ok(InferenceResponse {
                    model: "fixture".into(),
                    content: self.0.clone(),
                    done_reason: Some("stop".into()),
                    metrics: InferenceMetrics::default(),
                })
            })
        }
    }

    struct Mounted;
    impl MountVerifier for Mounted {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(true)
        }
    }

    fn draft(evidence: Vec<ClaimEvidenceV1>) -> ClaimDraftV1 {
        ClaimDraftV1 {
            schema_version: CLAIM_SCHEMA_VERSION,
            topic: "release plan".into(),
            subject: "Pinky".into(),
            predicate: "targets".into(),
            object: "a private trusted circle".into(),
            subject_aliases: vec![],
            object_aliases: vec![],
            inferred: false,
            evidence,
        }
    }

    #[test]
    fn preserves_contradiction_instead_of_overwriting_support() {
        let allowed = HashSet::from(["pinky://one", "pinky://two"]);
        let status = assess_claim(
            &draft(vec![
                ClaimEvidenceV1 {
                    citation_uri: "pinky://one".into(),
                    relationship: EvidenceRelationshipV1::Supporting,
                },
                ClaimEvidenceV1 {
                    citation_uri: "pinky://two".into(),
                    relationship: EvidenceRelationshipV1::Contradicting,
                },
            ]),
            &allowed,
        )
        .unwrap();
        assert_eq!(status, ClaimStatusV1::Disputed);
    }

    #[test]
    fn rejects_uncited_or_contradiction_only_claims() {
        let allowed = HashSet::from(["pinky://one", "pinky://two"]);
        assert!(matches!(
            assess_claim(
                &draft(vec![ClaimEvidenceV1 {
                    citation_uri: "pinky://invented".into(),
                    relationship: EvidenceRelationshipV1::Supporting
                }]),
                &allowed
            ),
            Err(ClaimError::InvalidCitation)
        ));
        assert!(matches!(
            assess_claim(
                &draft(vec![ClaimEvidenceV1 {
                    citation_uri: "pinky://one".into(),
                    relationship: EvidenceRelationshipV1::Contradicting
                }]),
                &allowed
            ),
            Err(ClaimError::Unsupported)
        ));
        assert!(matches!(
            assess_claim(
                &draft(vec![
                    ClaimEvidenceV1 {
                        citation_uri: "pinky://one".into(),
                        relationship: EvidenceRelationshipV1::Supporting,
                    },
                    ClaimEvidenceV1 {
                        citation_uri: "pinky://one".into(),
                        relationship: EvidenceRelationshipV1::Contradicting,
                    },
                ]),
                &allowed
            ),
            Err(ClaimError::DuplicateEvidence)
        ));
    }

    #[test]
    fn dossier_scores_preserve_source_diversity_freshness_and_dispute() {
        let score = score_dossier(
            &[(1, true, false), (3, false, false), (3, true, true)],
            true,
        );
        assert_eq!(score.coverage, 0.6);
        assert_eq!(score.authority, 2.0 / 3.0);
        assert_eq!(score.independence, 2.0 / 3.0);
        assert_eq!(score.freshness, 2.0 / 3.0);
        assert!(score.disputed);
        assert_eq!(score_dossier(&[], false).coverage, 0.0);
    }

    #[test]
    fn refresh_schedule_is_stable_bounded_and_excludes_file_watchers() {
        let now = DateTime::parse_from_rfc3339("2026-09-29T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let old = now - Duration::days(STALE_AFTER_DAYS + 1);
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let schedule = schedule_refreshes(
            &[
                RefreshCandidateV1 {
                    source_id: second,
                    refresh_policy: "weekly".into(),
                    last_checked_at: None,
                },
                RefreshCandidateV1 {
                    source_id: first,
                    refresh_policy: "daily".into(),
                    last_checked_at: Some(old),
                },
                RefreshCandidateV1 {
                    source_id: Uuid::from_u128(3),
                    refresh_policy: "filesystem_event".into(),
                    last_checked_at: None,
                },
            ],
            now,
            1,
        );
        assert_eq!(schedule.due_source_ids, vec![second]);
        assert_eq!(schedule.skipped_over_budget, 1);
    }

    #[test]
    fn evidence_quality_warnings_are_derived_from_state() {
        let warnings = claim_warnings(
            ClaimStatusV1::Disputed,
            &DossierMetricsV1 {
                coverage: 0.2,
                authority: 0.5,
                independence: 1.0,
                freshness: 0.5,
                disputed: true,
            },
            1,
            true,
        );
        assert_eq!(warnings.len(), 4);
        assert!(warnings[0].starts_with("Disputed"));
        assert!(warnings[1].starts_with("May be stale"));
    }

    #[test]
    fn aliases_are_bounded_unique_and_not_canonical_name() {
        assert!(validate_aliases("Pinky", &["P".into(), "Pink".into()]).is_ok());
        assert!(matches!(
            validate_aliases("Pinky", &["pinky".into()]),
            Err(ClaimError::InvalidAliases)
        ));
        assert!(matches!(
            validate_aliases("Pinky", &["P".into(), "p".into()]),
            Err(ClaimError::InvalidAliases)
        ));
    }

    #[test]
    fn direct_claim_matching_requires_the_bounded_phrase() {
        let claim = draft(vec![ClaimEvidenceV1 {
            citation_uri: "pinky://one".into(),
            relationship: EvidenceRelationshipV1::Supporting,
        }]);
        assert!(direct_claim_matches(
            &claim,
            "Pinky targets a private trusted circle with retained evidence."
        ));
        assert!(!direct_claim_matches(
            &claim,
            "A different unrelated statement."
        ));
    }

    #[test]
    fn indexed_extraction_cannot_reuse_or_invent_evidence_indexes() {
        let hit = SearchHit {
            source_id: Uuid::new_v4(),
            version_id: Uuid::new_v4(),
            chunk_id: Uuid::new_v4(),
            ordinal: 0,
            display_name: "fixture".into(),
            heading: None,
            citation_uri: "pinky://source/a/version/b#chunk-0".into(),
            passage: "Pinky has source-grounded evidence.".into(),
            score: 1.0,
            coordinates: serde_json::json!({}),
            retrieved_at: "2026-09-29T00:00:00Z".into(),
        };
        let proposal = IndexedClaimProposalV1 {
            subject: "Pinky".into(),
            predicate: "has".into(),
            object: "source-grounded evidence".into(),
            subject_aliases: vec![],
            object_aliases: vec![],
            inferred: false,
            supporting_evidence: vec![0],
            contradicting_evidence: vec![0],
        };
        assert!(matches!(
            drafts_from_indexed_proposals("topic", &[hit.clone()], &[proposal]),
            Err(ClaimError::InvalidExtraction)
        ));
        let mut invalid = IndexedClaimProposalV1 {
            subject: "Pinky".into(),
            predicate: "has".into(),
            object: "source-grounded evidence".into(),
            subject_aliases: vec![],
            object_aliases: vec![],
            inferred: false,
            supporting_evidence: vec![1],
            contradicting_evidence: vec![],
        };
        assert!(matches!(
            drafts_from_indexed_proposals("topic", &[hit.clone()], &[invalid.clone()]),
            Err(ClaimError::InvalidExtraction)
        ));
        invalid.supporting_evidence = vec![0];
        invalid.predicate = "implies".into();
        let drafts = drafts_from_indexed_proposals("topic", &[hit], &[invalid]).unwrap();
        assert!(drafts[0].inferred);
    }

    #[test]
    fn persists_all_evidence_and_consolidates_a_disputed_dossier() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, Zeroizing::new(vec![0x73; 32])).unwrap(),
        ));
        let objects = ObjectStore::new(vault);
        let approved = tempfile::tempdir().unwrap();
        let source = approved.path().join("facts.txt");
        let conflicting_source = approved.path().join("conflict.txt");
        std::fs::write(&source, "Pinky has source-grounded evidence.").unwrap();
        std::fs::write(
            &conflicting_source,
            "Pinky has source-grounded evidence according to a conflicting record.",
        )
        .unwrap();
        crate::LocalIngestor::new(database.clone(), objects.clone())
            .ingest(approved.path(), &source)
            .unwrap();
        crate::LocalIngestor::new(database.clone(), objects.clone())
            .ingest(approved.path(), &conflicting_source)
            .unwrap();
        let retrieval = RetrievalService::new(database.clone(), objects);
        let citations = retrieval
            .search("source-grounded", 2)
            .unwrap()
            .into_iter()
            .map(|hit| hit.citation_uri)
            .collect::<Vec<_>>();
        assert_eq!(citations.len(), 2);
        let stored = ClaimStore::new(database.clone())
            .record_from_retrieval(
                &ClaimDraftV1 {
                    schema_version: CLAIM_SCHEMA_VERSION,
                    topic: "evidence quality".into(),
                    subject: "Pinky".into(),
                    predicate: "has".into(),
                    object: "source-grounded evidence".into(),
                    subject_aliases: vec!["Pinky app".into()],
                    object_aliases: vec![],
                    inferred: false,
                    evidence: vec![
                        ClaimEvidenceV1 {
                            citation_uri: citations[0].clone(),
                            relationship: EvidenceRelationshipV1::Supporting,
                        },
                        ClaimEvidenceV1 {
                            citation_uri: citations[1].clone(),
                            relationship: EvidenceRelationshipV1::Contradicting,
                        },
                    ],
                },
                &retrieval,
            )
            .unwrap();
        assert_eq!(stored.status, ClaimStatusV1::Disputed);
        let dossier = ClaimStore::new(database.clone())
            .dossier(stored.topic_id, Utc::now())
            .unwrap();
        assert!(dossier.metrics.disputed);
        assert_eq!(dossier.metrics.coverage, 0.4);
        assert_eq!(dossier.unresolved_question_score, 0.0);
        assert!(dossier.unresolved_questions.is_empty());
        assert!(dossier
            .warnings
            .iter()
            .any(|warning| warning.starts_with("Disputed")));
        assert!(!dossier
            .warnings
            .iter()
            .any(|warning| warning.starts_with("Single-source")));
        let database_guard = database.lock().unwrap();
        let relations: i64 = database_guard
            .connection()
            .query_row("SELECT count(*) FROM entity_relationships", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(relations, 1);
        let persisted_scores: (f64, f64, f64, f64, f64) = database_guard
            .connection()
            .query_row(
                "SELECT coverage_score, authority_score, independence_score,
                        freshness_score, unresolved_question_score
                 FROM topics WHERE id = ?1",
                [stored.topic_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            persisted_scores,
            (
                (dossier.metrics.coverage * 100.0) as f64,
                (dossier.metrics.authority * 100.0) as f64,
                (dossier.metrics.independence * 100.0) as f64,
                (dossier.metrics.freshness * 100.0) as f64,
                (dossier.unresolved_question_score * 100.0) as f64,
            )
        );
        drop(database_guard);
        let due = ClaimStore::new(database.clone())
            .due_refreshes(Utc::now(), 1)
            .unwrap();
        // Local retained files stay event-driven, even when their source is
        // part of a dossier; R10 must not turn them into network work.
        assert!(due.due_source_ids.is_empty());
    }

    #[test]
    fn unresolved_questions_are_explicitly_managed_not_synthesized_from_conflict() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, Zeroizing::new(vec![0x22; 32])).unwrap(),
        ));
        let topic = Uuid::new_v4();
        database.lock().unwrap().connection().execute(
            "INSERT INTO topics (id, label, aliases_json, freshness_status, unresolved_questions_json) VALUES (?1, 'topic', '[]', 'unknown', '[]')",
            [topic.to_string()],
        ).unwrap();
        let store = ClaimStore::new(database.clone());
        store
            .set_unresolved_questions(topic, &["Which version is current?".into()])
            .unwrap();
        assert!(matches!(
            store.set_unresolved_questions(topic, &["same".into(), "Same".into()]),
            Err(ClaimError::InvalidExtraction)
        ));
        let dossier = store.dossier(topic, Utc::now()).unwrap();
        assert_eq!(
            dossier.unresolved_questions,
            vec!["Which version is current?"]
        );
    }

    #[test]
    fn refresh_schedule_is_durable_and_preserves_retry_state() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let reopened_vault = vault.clone();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, Zeroizing::new(vec![0x44; 32])).unwrap(),
        ));
        let objects = ObjectStore::new(vault.clone());
        let approved = tempfile::tempdir().unwrap();
        let file = approved.path().join("refreshable.txt");
        std::fs::write(&file, "retained refresh fixture").unwrap();
        let ingested = crate::LocalIngestor::new(database.clone(), objects)
            .ingest(approved.path(), &file)
            .unwrap();
        let old = (Utc::now() - Duration::days(STALE_AFTER_DAYS + 1)).to_rfc3339();
        database
            .lock()
            .unwrap()
            .connection()
            .execute(
                "UPDATE sources SET refresh_policy = 'weekly', last_checked_at = ?1 WHERE id = ?2",
                params![old, ingested.source_id.to_string()],
            )
            .unwrap();
        let store = ClaimStore::new(database.clone());
        assert_eq!(
            store
                .schedule_due_refreshes(Utc::now(), 1)
                .unwrap()
                .due_source_ids,
            vec![ingested.source_id]
        );
        database.lock().unwrap().connection().execute(
            "UPDATE source_refresh_schedule SET state = 'retry', attempts = 3, next_due_at = '2099-01-01T00:00:00Z' WHERE source_id = ?1",
            [ingested.source_id.to_string()],
        ).unwrap();
        // A subsequent scheduler pass must not erase the retry count or
        // execute or pull the work forward; R11 alone will claim and perform
        // web refreshes.
        assert!(store.schedule_due_refreshes(Utc::now(), 1).unwrap().due_source_ids.is_empty());
        let row: (String, i64) = database
            .lock()
            .unwrap()
            .connection()
            .query_row(
                "SELECT state, attempts FROM source_refresh_schedule WHERE source_id = ?1",
                [ingested.source_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("retry".into(), 3));
        drop(store);
        drop(database);
        let reopened = Arc::new(Mutex::new(
            Database::open(&reopened_vault, Zeroizing::new(vec![0x44; 32])).unwrap(),
        ));
        assert!(ClaimStore::new(reopened)
            .due_refreshes(Utc::now(), 1)
            .unwrap()
            .due_source_ids
            .is_empty());
    }

    #[tokio::test]
    async fn cancelled_extraction_never_calls_the_model_or_persists_a_claim() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, Zeroizing::new(vec![0x55; 32])).unwrap(),
        ));
        let retrieval = RetrievalService::new(database.clone(), ObjectStore::new(vault));
        let store = ClaimStore::new(database);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            extract_retained_claims(
                &NeverModel,
                &store,
                &retrieval,
                "topic",
                "query",
                &cancellation
            )
            .await,
            Err(ClaimError::Inference(InferenceError::Cancelled))
        ));
    }

    #[tokio::test]
    async fn retained_extraction_covers_claim_kinds_and_rejects_model_citations() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let reopened_vault = vault.clone();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, Zeroizing::new(vec![0x66; 32])).unwrap(),
        ));
        let approved = tempfile::tempdir().unwrap();
        for name in ["first.txt", "second.txt"] {
            let path = approved.path().join(name);
            std::fs::write(&path, "Pinky has retained evidence.").unwrap();
            crate::LocalIngestor::new(database.clone(), ObjectStore::new(vault.clone()))
                .ingest(approved.path(), &path)
                .unwrap();
        }
        let retrieval = RetrievalService::new(database.clone(), ObjectStore::new(vault));
        let store = ClaimStore::new(database.clone());
        let model = StaticModel(serde_json::json!({"claims":[
            {"subject":"Pinky","predicate":"has","object":"retained evidence","subject_aliases":["Pinky app"],"supporting_evidence":[0],"contradicting_evidence":[1]},
            {"subject":"Pinky","predicate":"implies","object":"retained evidence","supporting_evidence":[0]}
        ]}).to_string());
        let stored = extract_retained_claims(
            &model,
            &store,
            &retrieval,
            "quality",
            "retained evidence",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].status, ClaimStatusV1::Disputed);
        let dossier = store.dossier(stored[0].topic_id, Utc::now()).unwrap();
        assert!(dossier.metrics.disputed);
        assert!(dossier.claims.iter().any(|claim| claim.inferred));
        assert!(dossier.claims.iter().any(|claim| claim.evidence.len() == 2));
        let malicious = StaticModel(serde_json::json!({"claims":[{"subject":"Pinky","predicate":"has","object":"retained evidence","supporting_evidence":[0],"citation_uri":"pinky://invented"}]}).to_string());
        assert!(matches!(
            extract_retained_claims(
                &malicious,
                &store,
                &retrieval,
                "quality",
                "retained evidence",
                &CancellationToken::new()
            )
            .await,
            Err(ClaimError::InvalidExtraction)
        ));
        drop(store);
        drop(retrieval);
        drop(database);
        let reopened = Database::open(&reopened_vault, Zeroizing::new(vec![0x66; 32])).unwrap();
        let count: i64 = reopened
            .connection()
            .query_row("SELECT count(*) FROM claims", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
