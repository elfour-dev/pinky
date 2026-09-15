use std::{future::Future, pin::Pin};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    qdrant::{QdrantClient, QdrantError, VectorPoint},
    retrieval::{IndexedChunk, RetrievalError, RetrievalService},
};

pub const MAX_EMBEDDING_INPUTS: usize = 64;
pub const MAX_EMBEDDING_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_EMBEDDING_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_EMBEDDING_ERROR_BYTES: usize = 32 * 1024;
pub const DEFAULT_EMBEDDING_BATCH_SIZE: usize = 32;

#[derive(Debug, Error)]
pub enum EmbeddingError {
    #[error("embedding request must contain between one and {MAX_EMBEDDING_INPUTS} inputs")]
    InvalidInputCount,
    #[error("embedding input is empty or exceeds the size limit")]
    InvalidInput,
    #[error("embedding request was cancelled")]
    Cancelled,
    #[error("embedding request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("embedding provider rejected a request with HTTP {status}: {body}")]
    Rejected { status: u16, body: String },
    #[error("embedding response exceeded the {limit}-byte limit")]
    ResponseTooLarge { limit: usize },
    #[error("embedding provider returned malformed output")]
    MalformedResponse,
    #[error("embedding response model `{found}` did not match `{expected}`")]
    ModelMismatch { expected: String, found: String },
    #[error("embedding provider returned a remote or cloud model")]
    RemoteResponse,
    #[error("embedding provider returned invalid vectors")]
    InvalidVectors,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingResponse {
    pub model: String,
    pub vectors: Vec<Vec<f32>>,
}

pub type EmbeddingFuture<'a> =
    Pin<Box<dyn Future<Output = Result<EmbeddingResponse, EmbeddingError>> + Send + 'a>>;

pub trait EmbeddingProvider: Send + Sync {
    fn embed<'a>(
        &'a self,
        inputs: &'a [String],
        cancellation: &'a CancellationToken,
    ) -> EmbeddingFuture<'a>;
}

#[derive(Debug, Error)]
pub enum EmbeddingIndexError {
    #[error("embedding index operation was cancelled")]
    Cancelled,
    #[error("retained chunk loading failed: {0}")]
    Retrieval(#[from] RetrievalError),
    #[error("embedding provider failed: {0}")]
    Provider(#[from] EmbeddingError),
    #[error("Qdrant request failed: {0}")]
    Qdrant(#[from] QdrantError),
    #[error("embedding provider returned {found} vectors for {expected} chunks")]
    CountMismatch { found: usize, expected: usize },
    #[error("embedding dimension changed from {expected} to {found}")]
    DimensionMismatch { found: usize, expected: usize },
}

#[derive(Debug, Clone, Copy)]
pub struct EmbeddingIndexer {
    batch_size: usize,
}

impl Default for EmbeddingIndexer {
    fn default() -> Self {
        Self::new(DEFAULT_EMBEDDING_BATCH_SIZE)
    }
}

impl EmbeddingIndexer {
    pub const fn new(batch_size: usize) -> Self {
        Self {
            batch_size: if batch_size == 0 {
                DEFAULT_EMBEDDING_BATCH_SIZE
            } else if batch_size > MAX_EMBEDDING_INPUTS {
                MAX_EMBEDDING_INPUTS
            } else {
                batch_size
            },
        }
    }

    pub async fn index_chunks<P: EmbeddingProvider + ?Sized>(
        &self,
        provider: &P,
        qdrant: &QdrantClient,
        chunks: &[IndexedChunk],
        cancellation: &CancellationToken,
    ) -> Result<usize, EmbeddingIndexError> {
        let mut dimension = None;
        let mut indexed = 0;
        for batch in chunks.chunks(self.batch_size) {
            if cancellation.is_cancelled() {
                return Err(EmbeddingIndexError::Cancelled);
            }
            let inputs = batch
                .iter()
                .map(|chunk| chunk.body.clone())
                .collect::<Vec<_>>();
            let response = provider.embed(&inputs, cancellation).await?;
            if response.vectors.len() != batch.len() {
                return Err(EmbeddingIndexError::CountMismatch {
                    found: response.vectors.len(),
                    expected: batch.len(),
                });
            }
            let batch_dimension = validate_vectors(&response.vectors)?;
            if let Some(expected) = dimension {
                if expected != batch_dimension {
                    return Err(EmbeddingIndexError::DimensionMismatch {
                        found: batch_dimension,
                        expected,
                    });
                }
            } else {
                qdrant.ensure_collection(batch_dimension).await?;
                dimension = Some(batch_dimension);
            }
            let points = batch
                .iter()
                .zip(response.vectors)
                .map(|(chunk, vector)| VectorPoint {
                    id: chunk.chunk_id,
                    source_version_id: chunk.version_id,
                    vector,
                })
                .collect::<Vec<_>>();
            qdrant.upsert(batch_dimension, &points).await?;
            indexed += points.len();
        }
        Ok(indexed)
    }

    pub async fn index_current_chunks<P: EmbeddingProvider + ?Sized>(
        &self,
        provider: &P,
        qdrant: &QdrantClient,
        retrieval: &RetrievalService,
        cancellation: &CancellationToken,
    ) -> Result<usize, EmbeddingIndexError> {
        let chunks = retrieval.current_chunks()?;
        self.index_chunks(provider, qdrant, &chunks, cancellation)
            .await
    }
}

fn validate_vectors(vectors: &[Vec<f32>]) -> Result<usize, EmbeddingError> {
    let dimension = vectors.first().map(Vec::len).unwrap_or_default();
    if dimension == 0 {
        return Err(EmbeddingError::InvalidVectors);
    }
    for vector in vectors {
        if vector.len() != dimension
            || vector.iter().any(|value| !value.is_finite())
            || vector.iter().map(|value| value * value).sum::<f32>() <= f32::EPSILON
        {
            return Err(EmbeddingError::InvalidVectors);
        }
    }
    Ok(dimension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_non_empty_finite_vectors_with_a_stable_dimension() {
        assert_eq!(
            validate_vectors(&[vec![1.0, 0.0], vec![0.5, 0.5]]).unwrap(),
            2
        );
        assert!(matches!(
            validate_vectors(&[vec![1.0], vec![1.0, 0.0]]),
            Err(EmbeddingError::InvalidVectors)
        ));
        assert!(matches!(
            validate_vectors(&[vec![f32::NAN, 0.0]]),
            Err(EmbeddingError::InvalidVectors)
        ));
        assert!(matches!(
            validate_vectors(&[vec![0.0, 0.0]]),
            Err(EmbeddingError::InvalidVectors)
        ));
    }

    #[test]
    fn indexer_batch_size_is_bounded_and_never_zero() {
        assert_eq!(
            EmbeddingIndexer::new(0).batch_size,
            DEFAULT_EMBEDDING_BATCH_SIZE
        );
        assert_eq!(
            EmbeddingIndexer::new(usize::MAX).batch_size,
            MAX_EMBEDDING_INPUTS
        );
        assert_eq!(EmbeddingIndexer::new(4).batch_size, 4);
    }
}
