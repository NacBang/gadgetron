//! Embedding provider abstraction for semantic wiki indexing.
//!
//! Spec: `docs/design/phase2/05-knowledge-semantic.md §6`.

mod openai_compat;

use async_trait::async_trait;
use thiserror::Error;

pub use openai_compat::OpenAiCompatEmbedding;

/// Pluggable embedding backend.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync + 'static {
    /// Embed `texts` in order. Returned vectors must match `dimension()`.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError>;

    /// Dimension of the vectors produced by this provider.
    fn dimension(&self) -> usize;

    /// Stable model name for tracing and diagnostics.
    fn model_name(&self) -> &str;
}

/// Embedding transport / validation failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EmbeddingError {
    #[error("embedding provider HTTP error: {0}")]
    Http(String),

    #[error("embedding provider returned {got} dimensions, expected {expected}")]
    DimensionMismatch { got: usize, expected: usize },

    #[error("embedding provider response parse failed")]
    Parse,

    #[error("embedding provider timeout after {seconds}s")]
    Timeout { seconds: u64 },

    #[error("embedding provider auth failed")]
    Auth,
}
