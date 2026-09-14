use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const RRF_K: f32 = 60.0;
pub const MAX_CHUNKS_PER_SOURCE_VERSION: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedChunk {
    pub chunk_id: Uuid,
    pub source_version_id: Uuid,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FusedChunk {
    pub chunk_id: Uuid,
    pub source_version_id: Uuid,
    pub lexical_score: Option<f32>,
    pub vector_score: Option<f32>,
    pub fused_score: f32,
}

pub fn reciprocal_rank_fusion(
    lexical: &[RankedChunk],
    vector: &[RankedChunk],
    limit: usize,
) -> Vec<FusedChunk> {
    let mut fused = HashMap::<Uuid, FusedChunk>::new();
    for (rank, candidate) in lexical.iter().enumerate() {
        let entry = fused
            .entry(candidate.chunk_id)
            .or_insert_with(|| FusedChunk {
                chunk_id: candidate.chunk_id,
                source_version_id: candidate.source_version_id,
                lexical_score: None,
                vector_score: None,
                fused_score: 0.0,
            });
        entry.lexical_score = Some(candidate.score);
        entry.fused_score += 1.0 / (RRF_K + rank as f32 + 1.0);
    }
    for (rank, candidate) in vector.iter().enumerate() {
        let entry = fused
            .entry(candidate.chunk_id)
            .or_insert_with(|| FusedChunk {
                chunk_id: candidate.chunk_id,
                source_version_id: candidate.source_version_id,
                lexical_score: None,
                vector_score: None,
                fused_score: 0.0,
            });
        entry.vector_score = Some(candidate.score);
        entry.fused_score += 1.0 / (RRF_K + rank as f32 + 1.0);
    }
    let mut fused = fused.into_values().collect::<Vec<_>>();
    fused.sort_by(|left, right| {
        right
            .fused_score
            .total_cmp(&left.fused_score)
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    let mut per_version = HashMap::<Uuid, usize>::new();
    fused
        .into_iter()
        .filter(|candidate| {
            let count = per_version.entry(candidate.source_version_id).or_default();
            if *count >= MAX_CHUNKS_PER_SOURCE_VERSION {
                false
            } else {
                *count += 1;
                true
            }
        })
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(chunk_id: Uuid, source_version_id: Uuid, score: f32) -> RankedChunk {
        RankedChunk {
            chunk_id,
            source_version_id,
            score,
        }
    }

    #[test]
    fn rrf_rewards_chunks_found_by_both_retrievers() {
        let version = Uuid::new_v4();
        let shared = Uuid::new_v4();
        let lexical_only = Uuid::new_v4();
        let vector_only = Uuid::new_v4();
        let fused = reciprocal_rank_fusion(
            &[
                candidate(lexical_only, version, 9.0),
                candidate(shared, version, 8.0),
            ],
            &[
                candidate(vector_only, version, 0.99),
                candidate(shared, version, 0.95),
            ],
            10,
        );
        assert_eq!(fused[0].chunk_id, shared);
        assert!(fused[0].lexical_score.is_some());
        assert!(fused[0].vector_score.is_some());
    }

    #[test]
    fn deduplicates_and_limits_each_source_version_to_three_chunks() {
        let crowded = Uuid::new_v4();
        let independent = Uuid::new_v4();
        let crowded_chunks = (0..5)
            .map(|index| candidate(Uuid::new_v4(), crowded, 10.0 - index as f32))
            .collect::<Vec<_>>();
        let independent_chunk = candidate(Uuid::new_v4(), independent, 0.1);
        let fused = reciprocal_rank_fusion(
            &crowded_chunks,
            std::slice::from_ref(&independent_chunk),
            10,
        );
        assert_eq!(
            fused
                .iter()
                .filter(|candidate| candidate.source_version_id == crowded)
                .count(),
            3
        );
        assert!(fused
            .iter()
            .any(|candidate| candidate.chunk_id == independent_chunk.chunk_id));
    }
}
