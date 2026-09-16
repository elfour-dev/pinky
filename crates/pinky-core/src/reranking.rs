//! Bounded, provider-neutral reranking for hybrid retrieval.
//!
//! Reranking deliberately runs after lexical/vector fusion and before context
//! selection.  It is deterministic and local, so a missing optional model
//! cannot silently change citation selection or make the retrieval path
//! unbounded.  A future cross-encoder can implement the same bounded contract
//! without changing callers.

use std::collections::HashSet;

use crate::retrieval::SearchHit;

/// Maximum number of fused candidates sent to a reranker.
pub const MAX_RERANK_CANDIDATES: usize = 30;
/// Maximum number of passages returned to the answer/context selector.
pub const MAX_RERANK_RESULTS: usize = 12;

/// Rerank at most [`MAX_RERANK_CANDIDATES`] fused hits and return at most
/// [`MAX_RERANK_RESULTS`] citations. The fused score is used as one component
/// of the returned bounded rerank score.
pub fn rerank_hits(query: &str, candidates: &[SearchHit], limit: usize) -> Vec<SearchHit> {
    let limit = limit.min(MAX_RERANK_RESULTS);
    if limit == 0 || candidates.is_empty() {
        return Vec::new();
    }

    let bounded = candidates.iter().take(MAX_RERANK_CANDIDATES);
    let query_tokens = token_set(query);
    let normalized_query = normalized_tokens(query);
    let maximum_fused = bounded.clone().map(|hit| hit.score).fold(0.0_f32, f32::max);
    let mut scored = bounded
        .enumerate()
        .map(|(index, hit)| {
            let text = searchable_text(hit);
            let tokens = token_set(&text);
            let coverage = if query_tokens.is_empty() {
                0.0
            } else {
                query_tokens
                    .iter()
                    .filter(|token| tokens.contains(*token))
                    .count() as f32
                    / query_tokens.len() as f32
            };
            let phrase = if !normalized_query.is_empty()
                && normalized_tokens(&text).contains(&normalized_query)
            {
                1.0
            } else {
                0.0
            };
            let fused = if maximum_fused > 0.0 {
                (hit.score / maximum_fused).clamp(0.0, 1.0)
            } else {
                0.0
            };
            // Keep fusion influential for semantic/vector-only hits, while
            // rewarding passages that actually cover the user's terms.
            let score = (0.55 * fused + 0.35 * coverage + 0.10 * phrase).clamp(0.0, 1.0);
            (index, score)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score.total_cmp(left_score).then_with(|| {
            candidates[*left_index]
                .citation_uri
                .cmp(&candidates[*right_index].citation_uri)
        })
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(index, score)| {
            let mut hit = candidates[index].clone();
            hit.score = score;
            hit
        })
        .collect()
}

fn searchable_text(hit: &SearchHit) -> String {
    let mut text = String::with_capacity(
        hit.display_name.len()
            + hit.heading.as_ref().map_or(0, String::len)
            + hit.passage.len()
            + 2,
    );
    text.push_str(&hit.display_name);
    text.push(' ');
    if let Some(heading) = &hit.heading {
        text.push_str(heading);
        text.push(' ');
    }
    text.push_str(&hit.passage);
    text
}

fn token_set(value: &str) -> HashSet<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() > 1)
        .map(|token| token.to_lowercase())
        .collect()
}

fn normalized_tokens(value: &str) -> String {
    let mut tokens = value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() > 1)
        .map(|token| token.to_lowercase())
        .collect::<Vec<_>>();
    tokens.shrink_to_fit();
    tokens.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use uuid::Uuid;

    fn hit(score: f32, passage: &str, ordinal: u64) -> SearchHit {
        let source = Uuid::new_v4();
        let version = Uuid::new_v4();
        SearchHit {
            score,
            citation_uri: format!("pinky://source/{source}/version/{version}#chunk-{ordinal}"),
            source_id: source,
            version_id: version,
            chunk_id: Uuid::new_v4(),
            ordinal,
            display_name: "notes.md".into(),
            heading: None,
            passage: passage.into(),
            coordinates: Value::Null,
            retrieved_at: "2026-09-16T00:00:00Z".into(),
        }
    }

    #[test]
    fn reranking_rewards_query_coverage_without_dropping_vector_only_hits() {
        let ranked = rerank_hits(
            "alpha beta",
            &[
                hit(1.0, "A semantic passage with no exact query terms", 1),
                hit(0.8, "The alpha and beta values are retained here", 2),
            ],
            2,
        );
        assert_eq!(ranked.len(), 2);
        assert!(ranked[0].passage.contains("alpha and beta"));
        assert!(ranked.iter().any(|candidate| candidate.score > 0.0));
    }

    #[test]
    fn reranking_is_bounded_and_caps_results() {
        let candidates = (0..(MAX_RERANK_CANDIDATES + 4))
            .map(|index| hit(1.0, "candidate passage", index as u64))
            .collect::<Vec<_>>();
        let ranked = rerank_hits("candidate", &candidates, usize::MAX);
        assert_eq!(ranked.len(), MAX_RERANK_RESULTS);
    }

    #[test]
    #[ignore = "target-host acceptance: run with --ignored to measure warm reranking p95"]
    fn reranking_warm_p95_stays_below_half_second() {
        let candidates = (0..MAX_RERANK_CANDIDATES)
            .map(|index| {
                hit(
                    1.0,
                    "candidate retained passage with alpha beta",
                    index as u64,
                )
            })
            .collect::<Vec<_>>();
        let mut timings = Vec::with_capacity(1000);
        for _ in 0..1000 {
            let started = std::time::Instant::now();
            let _ = rerank_hits("alpha beta", &candidates, MAX_RERANK_RESULTS);
            timings.push(started.elapsed());
        }
        timings.sort_unstable();
        let p95 = timings[timings.len() * 95 / 100];
        assert!(
            p95 < std::time::Duration::from_millis(500),
            "rerank p95 was {p95:?}"
        );
    }
}
