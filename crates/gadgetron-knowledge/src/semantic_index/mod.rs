//! `SemanticPgVectorIndex` — [`KnowledgeIndex`] wrapping the existing
//! `SemanticBackend` (pgvector + OpenAI-compat embeddings).
//!
//! # Role
//!
//! Default semantic search plug. Registered as `"semantic-pgvector"` per
//! authority doc §2.1.3. Mode is [`KnowledgeQueryMode::Semantic`] — the
//! service routes `Semantic` and `Hybrid` queries to this plug.
//!
//! # Relationship to `SemanticBackend`
//!
//! `SemanticBackend` stays where it lived in `semantic.rs` — it is NOT
//! refactored. This adapter is a thin reification of the trait surface
//! over it. The preservation of `SemanticBackend` is a deliberate scope
//! constraint: the semantic module already has 10+ tests exercising its
//! hybrid search / reindex / rename paths, and the W3-KL-1 PR does not
//! want to disturb that test surface while the plug architecture is
//! still crystallizing.
//!
//! # Hybrid vs Semantic
//!
//! `SemanticBackend::hybrid_search` already runs keyword + semantic
//! fusion inside pgvector. We expose it here under
//! `KnowledgeQueryMode::Semantic`, NOT `Hybrid` — the `Hybrid` mode at
//! the service level is reserved for "fuse results across multiple
//! index plugs" (e.g. this plug + `WikiKeywordIndex`). Treating the
//! pgvector-internal RRF as the `Semantic` mode's output is the right
//! layering: it maintains one coherent definition of `Hybrid` at the
//! `KnowledgeService` level.

use std::sync::Arc;

use async_trait::async_trait;
use gadgetron_core::bundle::PlugId;
use gadgetron_core::error::{GadgetronError, KnowledgeErrorKind};
use gadgetron_core::knowledge::{
    AuthenticatedContext, KnowledgeChangeEvent, KnowledgeHit, KnowledgeHitKind, KnowledgeIndex,
    KnowledgeQuery, KnowledgeQueryMode, KnowledgeResult,
};

use crate::semantic::SemanticBackend;

/// Semantic search index adapter over pgvector.
///
/// Holds one `Arc<SemanticBackend>` — cheap to clone, shared with
/// `KnowledgeService`'s other consumers (e.g. `maintenance::run_reindex`
/// already points at a `SemanticBackend` directly during the backfill
/// path).
#[derive(Debug)]
pub struct SemanticPgVectorIndex {
    plug_id: PlugId,
    backend: Arc<SemanticBackend>,
}

impl SemanticPgVectorIndex {
    pub fn new(backend: Arc<SemanticBackend>) -> Result<Self, GadgetronError> {
        let plug_id =
            PlugId::new("semantic-pgvector").map_err(|e| GadgetronError::Config(e.to_string()))?;
        Ok(Self { plug_id, backend })
    }

    /// Test-only: override the plug id.
    #[doc(hidden)]
    pub fn with_plug_id(backend: Arc<SemanticBackend>, plug_id: PlugId) -> Self {
        Self { plug_id, backend }
    }
}

fn backend_err(message: impl Into<String>) -> GadgetronError {
    GadgetronError::Knowledge {
        kind: KnowledgeErrorKind::BackendUnavailable {
            plug: "semantic-pgvector".into(),
        },
        message: message.into(),
    }
}

#[async_trait]
impl KnowledgeIndex for SemanticPgVectorIndex {
    fn plug_id(&self) -> &PlugId {
        &self.plug_id
    }

    fn mode(&self) -> KnowledgeQueryMode {
        KnowledgeQueryMode::Semantic
    }

    async fn search(
        &self,
        _actor: &AuthenticatedContext,
        query: &KnowledgeQuery,
    ) -> KnowledgeResult<Vec<KnowledgeHit>> {
        if query.text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let limit = usize::try_from(query.limit).unwrap_or(usize::MAX).max(1);
        let raw = self
            .backend
            .hybrid_search(&query.text, limit)
            .await
            .map_err(|e| backend_err(format!("semantic hybrid_search failed: {e}")))?;

        let out = raw
            .into_iter()
            .map(|hit| KnowledgeHit {
                path: hit.page_name,
                title: hit.section,
                snippet: hit.snippet.unwrap_or_default(),
                score: hit.score,
                source_plug: self.plug_id.clone(),
                source_kind: KnowledgeHitKind::SearchIndex,
            })
            .collect();
        Ok(out)
    }

    async fn reset(&self) -> KnowledgeResult<()> {
        self.backend
            .truncate_all()
            .await
            .map_err(|e| backend_err(format!("semantic truncate_all failed: {e}")))
    }

    async fn apply(
        &self,
        _actor: &AuthenticatedContext,
        event: KnowledgeChangeEvent,
    ) -> KnowledgeResult<()> {
        match event {
            KnowledgeChangeEvent::Upsert { document } => {
                // `index_page` expects raw markdown (frontmatter-preserving)
                // so the `SemanticBackend` re-parses frontmatter and slices
                // into chunks. The document's `markdown` field carries the
                // body only (frontmatter is stripped at the store boundary
                // in `LlmWikiStore::assemble_document`), so we re-synthesize
                // a minimal raw content string here that the semantic
                // backend can still parse.
                //
                // If frontmatter data is available in `document.frontmatter`,
                // prepend it as a TOML frontmatter block so the semantic
                // backend preserves created/updated/source metadata.
                let raw = reconstruct_raw_page(&document.frontmatter, &document.markdown);
                self.backend
                    .index_page(&document.path, &raw)
                    .await
                    .map_err(|e| backend_err(format!("semantic index_page failed: {e}")))
            }
            KnowledgeChangeEvent::Delete { path, .. } => self
                .backend
                .delete_page(&path)
                .await
                .map_err(|e| backend_err(format!("semantic delete_page failed: {e}"))),
            KnowledgeChangeEvent::Rename { from, to, .. } => self
                .backend
                .rename_page(&from, &to)
                .await
                .map_err(|e| backend_err(format!("semantic rename_page failed: {e}"))),
        }
    }
}

/// Rebuild a raw markdown page (frontmatter + body) from the split
/// `KnowledgeDocument` fields.
///
/// Required because `SemanticBackend::index_page` parses frontmatter
/// internally — we cannot pass it a pre-split document without a
/// parallel entrypoint. Synthesizing a fresh frontmatter block from the
/// JSON `Value` keeps the surface minimal and avoids leaking adapter
/// details into `SemanticBackend`.
fn reconstruct_raw_page(frontmatter: &serde_json::Value, body: &str) -> String {
    // Empty / null frontmatter: return the body as-is.
    if matches!(frontmatter, serde_json::Value::Null) {
        return body.to_string();
    }
    let Some(obj) = frontmatter.as_object() else {
        return body.to_string();
    };
    if obj.is_empty() {
        return body.to_string();
    }

    // Serialize back to TOML. Failures fall back to body-only — the
    // semantic backend still indexes, it just loses frontmatter fidelity
    // for that specific page.
    let toml_value = json_to_toml(frontmatter);
    let Some(toml_value) = toml_value else {
        return body.to_string();
    };
    let Ok(toml_str) = toml::to_string(&toml_value) else {
        return body.to_string();
    };
    format!("---\n{toml_str}---\n{body}")
}

/// Minimal JSON → TOML adapter for frontmatter preservation.
///
/// TOML forbids null and mixed-type arrays; anything incompatible
/// returns `None` (the caller falls back to body-only indexing).
fn json_to_toml(value: &serde_json::Value) -> Option<toml::Value> {
    use serde_json::Value as J;
    use toml::Value as T;
    Some(match value {
        J::Null => return None,
        J::Bool(b) => T::Boolean(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                T::Integer(i)
            } else if let Some(f) = n.as_f64() {
                T::Float(f)
            } else {
                return None;
            }
        }
        J::String(s) => T::String(s.clone()),
        J::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for v in arr {
                out.push(json_to_toml(v)?);
            }
            T::Array(out)
        }
        J::Object(obj) => {
            let mut tbl = toml::map::Map::new();
            for (k, v) in obj {
                if let Some(tv) = json_to_toml(v) {
                    tbl.insert(k.clone(), tv);
                }
            }
            T::Table(tbl)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstruct_handles_null_frontmatter() {
        let out = reconstruct_raw_page(&serde_json::Value::Null, "# Body");
        assert_eq!(out, "# Body");
    }

    #[test]
    fn reconstruct_handles_empty_object_frontmatter() {
        let out = reconstruct_raw_page(&serde_json::json!({}), "# Body");
        assert_eq!(out, "# Body");
    }

    #[test]
    fn reconstruct_preserves_simple_frontmatter() {
        let out = reconstruct_raw_page(
            &serde_json::json!({"source": "seed", "confidence": "high"}),
            "# Body",
        );
        assert!(out.starts_with("---\n"));
        assert!(out.contains("source = \"seed\""));
        assert!(out.contains("confidence = \"high\""));
        assert!(out.ends_with("---\n# Body") || out.contains("\n# Body"));
    }

    // Note: online tests against a real pgvector instance live in
    // `gadget.rs::semantic_write_sync_indexes_into_postgres_*` and in
    // `service.rs` integration tests. The adapter surface tested here is
    // the reconstruction shim + construction path — anything touching
    // `SemanticBackend::index_page` requires a live Postgres.
}
