//! `WikiKeywordIndex` — [`KnowledgeIndex`] over the existing in-memory
//! `InvertedIndex`.
//!
//! # Role
//!
//! Default keyword search plug for the knowledge plane. Registered as
//! `"wiki-keyword"` per authority doc §2.1.3. Mode is
//! [`KnowledgeQueryMode::Keyword`] — the service dispatches `Keyword`
//! and `Hybrid` queries to this plug.
//!
//! # Design
//!
//! - Wraps a `Mutex<InvertedIndex>` so incremental `apply` calls update
//!   the same index instance the `search` path reads from. The mutex
//!   guards the inner map only; it is held for a tight critical section
//!   around `add_page` / `remove_page` / `search`.
//! - The index rebuild in `Wiki::search` (full re-scan on every call)
//!   is NOT reused here — this adapter maintains an incremental index,
//!   applied event-by-event. Behaviour difference is invisible to the
//!   caller (same results), but the hot path no longer pays an O(pages)
//!   cost per search once P2B lands.
//!
//! # Thread-safety
//!
//! `Mutex` (not `RwLock`) because writes are small + frequent and the
//! `InvertedIndex::search` call is O(unique-tokens-in-query). Benchmark
//! (not yet landed) would pick `parking_lot::RwLock` if search latency
//! grows beyond MVP budget.

use std::sync::Mutex;

use async_trait::async_trait;
use gadgetron_core::bundle::PlugId;
use gadgetron_core::error::{GadgetronError, KnowledgeErrorKind};
use gadgetron_core::knowledge::{
    AuthenticatedContext, KnowledgeChangeEvent, KnowledgeHit, KnowledgeHitKind, KnowledgeIndex,
    KnowledgeQuery, KnowledgeQueryMode, KnowledgeResult,
};

use crate::wiki::InvertedIndex;

/// Keyword search index backed by `InvertedIndex`.
///
/// Cheap to construct (empty `InvertedIndex`). Built-up progressively via
/// `KnowledgeIndex::apply` as canonical writes fanout.
#[derive(Debug)]
pub struct WikiKeywordIndex {
    plug_id: PlugId,
    inner: Mutex<InvertedIndex>,
}

impl WikiKeywordIndex {
    pub fn new() -> Result<Self, GadgetronError> {
        let plug_id =
            PlugId::new("wiki-keyword").map_err(|e| GadgetronError::Config(e.to_string()))?;
        Ok(Self {
            plug_id,
            inner: Mutex::new(InvertedIndex::new()),
        })
    }

    /// Test-only constructor for plug id override.
    #[doc(hidden)]
    pub fn with_plug_id(plug_id: PlugId) -> Self {
        Self {
            plug_id,
            inner: Mutex::new(InvertedIndex::new()),
        }
    }
}

impl Default for WikiKeywordIndex {
    fn default() -> Self {
        Self::new().expect("wiki-keyword is a valid kebab-case plug id")
    }
}

// Internal helper — uniformly report lock poisoning as a backend-unavailable
// error. A poisoned mutex is a rust bug path (a panicking applier), so we
// surface it as a daemon-side failure, not a query-side one.
fn map_poison<T>(result: Result<T, std::sync::PoisonError<T>>) -> KnowledgeResult<T> {
    result.map_err(|_| GadgetronError::Knowledge {
        kind: KnowledgeErrorKind::BackendUnavailable {
            plug: "wiki-keyword".into(),
        },
        message: "wiki-keyword inverted index mutex poisoned".into(),
    })
}

#[async_trait]
impl KnowledgeIndex for WikiKeywordIndex {
    fn plug_id(&self) -> &PlugId {
        &self.plug_id
    }

    fn mode(&self) -> KnowledgeQueryMode {
        KnowledgeQueryMode::Keyword
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
        let guard = map_poison(self.inner.lock())?;
        let hits = guard.search(&query.text, limit);
        drop(guard);

        let out = hits
            .into_iter()
            .map(|hit| KnowledgeHit {
                path: hit.name,
                title: None,
                snippet: hit.snippet.unwrap_or_default(),
                score: hit.score,
                source_plug: self.plug_id.clone(),
                source_kind: KnowledgeHitKind::SearchIndex,
            })
            .collect();
        Ok(out)
    }

    async fn reset(&self) -> KnowledgeResult<()> {
        let mut guard = map_poison(self.inner.lock())?;
        *guard = InvertedIndex::new();
        Ok(())
    }

    async fn apply(
        &self,
        _actor: &AuthenticatedContext,
        event: KnowledgeChangeEvent,
    ) -> KnowledgeResult<()> {
        let mut guard = map_poison(self.inner.lock())?;
        match event {
            KnowledgeChangeEvent::Upsert { document } => {
                // `add_page` overwrites any existing entry by name.
                guard.add_page(document.path, &document.markdown);
            }
            KnowledgeChangeEvent::Delete { path, .. } => {
                guard.remove_page(&path);
            }
            KnowledgeChangeEvent::Rename {
                from,
                to: _,
                document,
            } => {
                guard.remove_page(&from);
                guard.add_page(document.path, &document.markdown);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gadgetron_core::knowledge::KnowledgeDocument;

    fn plug(s: &str) -> PlugId {
        PlugId::new(s).unwrap()
    }

    fn doc(path: &str, body: &str) -> KnowledgeDocument {
        KnowledgeDocument {
            path: path.to_string(),
            title: None,
            markdown: body.to_string(),
            frontmatter: serde_json::Value::Null,
            canonical_plug: plug("llm-wiki"),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn mode_is_keyword() {
        let idx = WikiKeywordIndex::new().unwrap();
        assert_eq!(idx.mode(), KnowledgeQueryMode::Keyword);
        assert_eq!(idx.plug_id().as_str(), "wiki-keyword");
    }

    #[tokio::test]
    async fn apply_upsert_then_search_returns_hit() {
        let idx = WikiKeywordIndex::new().unwrap();
        let actor = AuthenticatedContext;
        idx.apply(
            &actor,
            KnowledgeChangeEvent::Upsert {
                document: doc("home", "quarterly review with the team"),
            },
        )
        .await
        .unwrap();

        let hits = idx
            .search(
                &actor,
                &KnowledgeQuery {
                    text: "quarterly".into(),
                    limit: 10,
                    mode: KnowledgeQueryMode::Keyword,
                    include_relations: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "home");
        assert_eq!(hits[0].source_plug.as_str(), "wiki-keyword");
        assert_eq!(hits[0].source_kind, KnowledgeHitKind::SearchIndex);
    }

    #[tokio::test]
    async fn apply_delete_removes_page_from_hits() {
        let idx = WikiKeywordIndex::new().unwrap();
        let actor = AuthenticatedContext;
        idx.apply(
            &actor,
            KnowledgeChangeEvent::Upsert {
                document: doc("home", "quarterly review"),
            },
        )
        .await
        .unwrap();
        idx.apply(
            &actor,
            KnowledgeChangeEvent::Delete {
                path: "home".into(),
                deleted_at: Utc::now(),
            },
        )
        .await
        .unwrap();

        let hits = idx
            .search(
                &actor,
                &KnowledgeQuery {
                    text: "quarterly".into(),
                    limit: 10,
                    mode: KnowledgeQueryMode::Keyword,
                    include_relations: false,
                },
            )
            .await
            .unwrap();
        assert!(hits.is_empty(), "expected no hits after delete: {hits:?}");
    }

    #[tokio::test]
    async fn apply_rename_updates_hit_path() {
        let idx = WikiKeywordIndex::new().unwrap();
        let actor = AuthenticatedContext;
        idx.apply(
            &actor,
            KnowledgeChangeEvent::Upsert {
                document: doc("old", "quarterly review"),
            },
        )
        .await
        .unwrap();

        let new_doc = doc("new", "quarterly review");
        idx.apply(
            &actor,
            KnowledgeChangeEvent::Rename {
                from: "old".into(),
                to: "new".into(),
                document: new_doc,
            },
        )
        .await
        .unwrap();

        let hits = idx
            .search(
                &actor,
                &KnowledgeQuery {
                    text: "quarterly".into(),
                    limit: 10,
                    mode: KnowledgeQueryMode::Keyword,
                    include_relations: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "new");
    }

    #[tokio::test]
    async fn reset_drops_all_indexed_state() {
        let idx = WikiKeywordIndex::new().unwrap();
        let actor = AuthenticatedContext;
        idx.apply(
            &actor,
            KnowledgeChangeEvent::Upsert {
                document: doc("home", "quarterly review"),
            },
        )
        .await
        .unwrap();

        idx.reset().await.unwrap();
        let hits = idx
            .search(
                &actor,
                &KnowledgeQuery {
                    text: "quarterly".into(),
                    limit: 10,
                    mode: KnowledgeQueryMode::Keyword,
                    include_relations: false,
                },
            )
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn empty_query_returns_no_hits() {
        let idx = WikiKeywordIndex::new().unwrap();
        let actor = AuthenticatedContext;
        idx.apply(
            &actor,
            KnowledgeChangeEvent::Upsert {
                document: doc("home", "body"),
            },
        )
        .await
        .unwrap();
        let hits = idx
            .search(
                &actor,
                &KnowledgeQuery {
                    text: "  ".into(),
                    limit: 10,
                    mode: KnowledgeQueryMode::Keyword,
                    include_relations: false,
                },
            )
            .await
            .unwrap();
        assert!(hits.is_empty());
    }
}
