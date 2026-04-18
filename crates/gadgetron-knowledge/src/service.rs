//! `KnowledgeService` — orchestration layer for the knowledge plane.
//!
//! The service owns one canonical [`KnowledgeStore`] plus any number of
//! derived [`KnowledgeIndex`] and [`KnowledgeRelationEngine`] plugs. Every
//! external caller (Gadget, CLI, Web UI) hits the service; the service
//! routes through the canonical store and fans out change events to
//! derived backends.
//!
//! # Write algorithm (authority doc §2.2.3)
//!
//! 1. caller ACL/validation at boundary (W3-KL-1 placeholder — actor is
//!    not yet consulted for ACL, that lands in `09-knowledge-acl.md`)
//! 2. `canonical_store.put()` — success defines user-visible success
//! 3. synthesize [`KnowledgeChangeEvent`]
//! 4. fanout via `FuturesUnordered` to every enabled index + relation
//!    plug in parallel
//! 5. `store_only` (default): derived failures collected into the
//!    receipt's `derived_failures: Vec<PlugId>`
//! 6. `await_derived`: first derived failure wins — promoted to a
//!    `GadgetronError::Knowledge(DerivedApplyFailed)`
//!
//! # Search algorithm (authority doc §2.2.4)
//!
//! - `Keyword` / `Semantic`: dispatch to plugs whose `mode()` matches
//! - `Hybrid`: dispatch to keyword + semantic plugs, RRF-fuse results
//! - `Relations`: dispatch to relation engines only
//! - `Auto`: dispatch to all enabled search plugs; relation engines only
//!   if `include_relations = true`
//!
//! Result fusion dedups by `path` (first hit wins, higher score wins on
//! duplicate).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use futures::FutureExt;
use gadgetron_core::bundle::PlugId;
use gadgetron_core::error::{GadgetronError, KnowledgeErrorKind};
use gadgetron_core::knowledge::{
    AuthenticatedContext, KnowledgeChangeEvent, KnowledgeDocument, KnowledgeHit, KnowledgeHitKind,
    KnowledgeIndex, KnowledgePutRequest, KnowledgeQuery, KnowledgeQueryMode,
    KnowledgeRelationEngine, KnowledgeStore, KnowledgeTraversalQuery, KnowledgeTraversalResult,
    KnowledgeWriteConsistency, KnowledgeWriteReceipt,
};

/// Reciprocal-rank-fusion constant matching `semantic::RRF_K`. Keeping
/// the same value across service-level fusion and pgvector-internal fusion
/// means `Hybrid` and `Semantic` produce comparable score magnitudes when
/// a caller swaps modes.
const RRF_K: f32 = 60.0;

/// Knowledge plane service.
///
/// Built via [`KnowledgeServiceBuilder`]. Not clonable directly (the
/// builder hands back an `Arc<Self>` — typical registration path) but
/// `Arc<KnowledgeService>` is cheap to clone.
pub struct KnowledgeService {
    canonical_store: Arc<dyn KnowledgeStore>,
    /// Sorted by `plug_id` for deterministic iteration (`BTreeMap` not
    /// `HashMap`); necessary so audit logs and `gadgetron bundle info`
    /// emit a stable plug ordering.
    indexes: BTreeMap<PlugId, Arc<dyn KnowledgeIndex>>,
    relation_engines: BTreeMap<PlugId, Arc<dyn KnowledgeRelationEngine>>,
    write_consistency: KnowledgeWriteConsistency,
}

impl std::fmt::Debug for KnowledgeService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KnowledgeService")
            .field("canonical_store", &self.canonical_store.plug_id())
            .field(
                "indexes",
                &self.indexes.keys().map(|p| p.as_str()).collect::<Vec<_>>(),
            )
            .field(
                "relation_engines",
                &self
                    .relation_engines
                    .keys()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("write_consistency", &self.write_consistency)
            .finish()
    }
}

/// Builder for [`KnowledgeService`].
///
/// Registration order: `canonical_store()` is mandatory; indexes and
/// relation engines are additive. `build()` validates that the canonical
/// store is set and produces an `Arc<KnowledgeService>`.
#[derive(Default)]
pub struct KnowledgeServiceBuilder {
    canonical_store: Option<Arc<dyn KnowledgeStore>>,
    indexes: BTreeMap<PlugId, Arc<dyn KnowledgeIndex>>,
    relation_engines: BTreeMap<PlugId, Arc<dyn KnowledgeRelationEngine>>,
    write_consistency: KnowledgeWriteConsistency,
}

impl KnowledgeServiceBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn canonical_store(mut self, store: Arc<dyn KnowledgeStore>) -> Self {
        self.canonical_store = Some(store);
        self
    }

    pub fn add_index(mut self, index: Arc<dyn KnowledgeIndex>) -> Self {
        self.indexes.insert(index.plug_id().clone(), index);
        self
    }

    pub fn add_relation_engine(mut self, engine: Arc<dyn KnowledgeRelationEngine>) -> Self {
        self.relation_engines
            .insert(engine.plug_id().clone(), engine);
        self
    }

    pub fn write_consistency(mut self, consistency: KnowledgeWriteConsistency) -> Self {
        self.write_consistency = consistency;
        self
    }

    pub fn build(self) -> Result<Arc<KnowledgeService>, GadgetronError> {
        let canonical_store = self.canonical_store.ok_or_else(|| {
            GadgetronError::Config(
                "KnowledgeServiceBuilder requires a canonical_store before build()".into(),
            )
        })?;
        Ok(Arc::new(KnowledgeService {
            canonical_store,
            indexes: self.indexes,
            relation_engines: self.relation_engines,
            write_consistency: self.write_consistency,
        }))
    }
}

// ---------------------------------------------------------------------------
// Read-only accessors
// ---------------------------------------------------------------------------

impl KnowledgeService {
    pub fn canonical_plug(&self) -> &PlugId {
        self.canonical_store.plug_id()
    }

    pub fn index_plugs(&self) -> impl Iterator<Item = &PlugId> {
        self.indexes.keys()
    }

    pub fn relation_plugs(&self) -> impl Iterator<Item = &PlugId> {
        self.relation_engines.keys()
    }

    pub fn write_consistency(&self) -> KnowledgeWriteConsistency {
        self.write_consistency
    }

    /// Return a lightweight health snapshot of all registered plugs.
    ///
    /// Used by the workbench gateway projection (W3-WEB-2) to build the
    /// bootstrap response without coupling the gateway to internal plug types.
    ///
    /// Health is currently structural (plug registered = healthy). Runtime
    /// liveness checks (stale index, backend ping) are deferred to P2B
    /// health monitors.
    pub fn plug_health_snapshot(&self) -> Vec<PlugSnapshot> {
        let mut snaps: Vec<PlugSnapshot> = Vec::new();

        // Canonical store is always role "canonical".
        snaps.push(PlugSnapshot {
            id: self.canonical_store.plug_id().as_str().to_string(),
            role: "canonical".into(),
            healthy: true,
            note: None,
        });

        // Each index is role "search".
        for id in self.indexes.keys() {
            snaps.push(PlugSnapshot {
                id: id.as_str().to_string(),
                role: "search".into(),
                healthy: true,
                note: None,
            });
        }

        // Each relation engine is role "relation".
        for id in self.relation_engines.keys() {
            snaps.push(PlugSnapshot {
                id: id.as_str().to_string(),
                role: "relation".into(),
                healthy: true,
                note: None,
            });
        }

        snaps
    }
}

/// Lightweight health descriptor for a single knowledge plug.
///
/// Produced by [`KnowledgeService::plug_health_snapshot`] and consumed by
/// the workbench bootstrap projection.
#[derive(Debug, Clone)]
pub struct PlugSnapshot {
    pub id: String,
    /// `"canonical"` | `"search"` | `"relation"`
    pub role: String,
    pub healthy: bool,
    pub note: Option<String>,
}

// ---------------------------------------------------------------------------
// Reads: list / get
// ---------------------------------------------------------------------------

impl KnowledgeService {
    #[tracing::instrument(level = "info", skip(self, actor))]
    pub async fn list(&self, actor: &AuthenticatedContext) -> Result<Vec<String>, GadgetronError> {
        self.canonical_store.list(actor).await
    }

    #[tracing::instrument(level = "info", skip(self, actor), fields(canonical_plug = %self.canonical_store.plug_id()))]
    pub async fn get(
        &self,
        actor: &AuthenticatedContext,
        path: &str,
    ) -> Result<Option<KnowledgeDocument>, GadgetronError> {
        self.canonical_store.get(actor, path).await
    }
}

// ---------------------------------------------------------------------------
// Writes: write / delete / rename (+ fanout)
// ---------------------------------------------------------------------------

impl KnowledgeService {
    #[tracing::instrument(
        level = "info",
        name = "knowledge.write",
        skip(self, actor, request),
        fields(
            path = %request.path,
            canonical_plug = %self.canonical_store.plug_id(),
            write_consistency = ?self.write_consistency
        )
    )]
    pub async fn write(
        &self,
        actor: &AuthenticatedContext,
        request: KnowledgePutRequest,
    ) -> Result<KnowledgeWriteReceipt, GadgetronError> {
        // Step 2: canonical store is authoritative.
        let mut receipt = self.canonical_store.put(actor, request.clone()).await?;

        // Step 3: re-read post-write document so derived backends get the
        // store-normalized form (git may canonicalize whitespace, etc.).
        // If re-read fails, we still surface the canonical write success
        // but skip fanout — the operator can rerun `reindex` to recover.
        let Some(document) = self.canonical_store.get(actor, &receipt.path).await? else {
            tracing::warn!(
                target: "knowledge_service",
                path = %receipt.path,
                "canonical write succeeded but post-read returned None; skipping derived fanout"
            );
            return Ok(receipt);
        };

        let event = KnowledgeChangeEvent::Upsert { document };
        receipt.derived_failures = self.fanout_change(actor, &event).await?;
        Ok(receipt)
    }

    #[tracing::instrument(level = "info", name = "knowledge.delete", skip(self, actor), fields(canonical_plug = %self.canonical_store.plug_id()))]
    pub async fn delete(
        &self,
        actor: &AuthenticatedContext,
        path: &str,
    ) -> Result<Vec<PlugId>, GadgetronError> {
        self.canonical_store.delete(actor, path).await?;
        let event = KnowledgeChangeEvent::Delete {
            path: path.to_string(),
            deleted_at: chrono::Utc::now(),
        };
        self.fanout_change(actor, &event).await
    }

    #[tracing::instrument(level = "info", name = "knowledge.rename", skip(self, actor), fields(canonical_plug = %self.canonical_store.plug_id()))]
    pub async fn rename(
        &self,
        actor: &AuthenticatedContext,
        from: &str,
        to: &str,
    ) -> Result<KnowledgeWriteReceipt, GadgetronError> {
        let mut receipt = self.canonical_store.rename(actor, from, to).await?;
        // Re-read the renamed document so derived backends see the full
        // post-rename doc (frontmatter may carry updated_at bumps).
        let Some(document) = self.canonical_store.get(actor, &receipt.path).await? else {
            tracing::warn!(
                target: "knowledge_service",
                path = %receipt.path,
                "canonical rename succeeded but post-read returned None; skipping derived fanout"
            );
            return Ok(receipt);
        };
        let event = KnowledgeChangeEvent::Rename {
            from: from.to_string(),
            to: to.to_string(),
            document,
        };
        receipt.derived_failures = self.fanout_change(actor, &event).await?;
        Ok(receipt)
    }

    /// Fanout a canonical change event to every derived backend.
    ///
    /// Returns `Ok(derived_failures)` under `StoreOnly`, and promotes the
    /// first derived failure to `Err` under `AwaitDerived`. Either way
    /// every fanout future is `.await`ed — no backend is silently dropped.
    async fn fanout_change(
        &self,
        actor: &AuthenticatedContext,
        event: &KnowledgeChangeEvent,
    ) -> Result<Vec<PlugId>, GadgetronError> {
        // No derived plugs -> nothing to do. Cheap path — avoids paying
        // for FuturesUnordered setup on llm-wiki-only deployments.
        if self.indexes.is_empty() && self.relation_engines.is_empty() {
            return Ok(Vec::new());
        }

        // Each async block has a unique anonymous type; `FuturesUnordered`
        // requires a uniform Fut, so boxing to `BoxFuture` is needed to
        // fanout across both index-apply and relation-apply futures in
        // one collection. The Box alloc is one per fanout op and amortizes
        // against the actual backend call.
        let mut futs: FuturesUnordered<BoxFuture<'_, (PlugId, Result<(), GadgetronError>)>> =
            FuturesUnordered::new();
        for (id, index) in &self.indexes {
            let id = id.clone();
            let index = index.clone();
            let event = event.clone();
            let actor = *actor;
            futs.push(
                async move {
                    let result = index.apply(&actor, event).await;
                    (id, result)
                }
                .boxed(),
            );
        }
        for (id, engine) in &self.relation_engines {
            let id = id.clone();
            let engine = engine.clone();
            let event = event.clone();
            let actor = *actor;
            futs.push(
                async move {
                    let result = engine.apply(&actor, event).await;
                    (id, result)
                }
                .boxed(),
            );
        }

        let mut failures: Vec<PlugId> = Vec::new();
        let mut first_err_for_await_derived: Option<(PlugId, GadgetronError)> = None;
        while let Some((plug, result)) = futs.next().await {
            if let Err(err) = result {
                tracing::warn!(
                    target: "knowledge_service",
                    target_plug = %plug,
                    error_code = %err.error_code(),
                    "knowledge.apply_change failed on derived plug"
                );
                if first_err_for_await_derived.is_none() {
                    first_err_for_await_derived = Some((plug.clone(), err));
                }
                failures.push(plug);
            }
        }

        match self.write_consistency {
            KnowledgeWriteConsistency::StoreOnly => Ok(failures),
            KnowledgeWriteConsistency::AwaitDerived => match first_err_for_await_derived {
                Some((plug, _underlying)) => Err(GadgetronError::Knowledge {
                    kind: KnowledgeErrorKind::DerivedApplyFailed {
                        plug: plug.to_string(),
                    },
                    message: format!(
                        "await_derived consistency: derived plug {plug} failed; canonical write is reversible via `gadgetron reindex`"
                    ),
                }),
                None => Ok(Vec::new()),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Searches: search / traverse / reindex_all
// ---------------------------------------------------------------------------

impl KnowledgeService {
    #[tracing::instrument(
        level = "info",
        name = "knowledge.search",
        skip(self, actor, query),
        fields(mode = ?query.mode, limit = query.limit, text_len = query.text.len())
    )]
    pub async fn search(
        &self,
        actor: &AuthenticatedContext,
        query: &KnowledgeQuery,
    ) -> Result<Vec<KnowledgeHit>, GadgetronError> {
        if query.text.trim().is_empty() {
            return Ok(Vec::new());
        }
        if query.limit == 0 {
            return Err(GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::InvalidQuery {
                    reason: "limit must be >= 1".into(),
                },
                message: "knowledge.search limit=0".into(),
            });
        }

        // Step 1: pick which plugs to dispatch to.
        let picks = self.pick_search_plugs(query.mode);
        if picks.is_empty() && !matches!(query.mode, KnowledgeQueryMode::Relations) {
            // No indexes configured for the requested mode -> empty result.
            // Not an error: an operator may have disabled semantic for a
            // keyword-only deploy.
            return Ok(Vec::new());
        }

        // Step 2: fanout.
        let mut futs = FuturesUnordered::new();
        for plug in &picks {
            let plug = plug.clone();
            let index = self
                .indexes
                .get(&plug)
                .expect("pick_search_plugs returned a registered id")
                .clone();
            let query = query.clone();
            let actor = *actor;
            futs.push(async move {
                let result = index.search(&actor, &query).await;
                (plug, result)
            });
        }

        let mut per_plug_hits: Vec<Vec<KnowledgeHit>> = Vec::new();
        while let Some((plug, result)) = futs.next().await {
            match result {
                Ok(hits) => per_plug_hits.push(hits),
                Err(err) => {
                    tracing::warn!(
                        target: "knowledge_service",
                        target_plug = %plug,
                        error_code = %err.error_code(),
                        "knowledge.search plug failed; excluding from fused result"
                    );
                    // Authority doc §2.2.4: a failed plug is excluded, NOT
                    // fatal — keyword-only fallback is explicitly desired.
                }
            }
        }

        let mut fused = fuse_hits(per_plug_hits);

        // Optional relation augmentation for `Auto` + `include_relations`.
        if matches!(query.mode, KnowledgeQueryMode::Relations) || query.include_relations {
            let rel_hits = self.collect_relation_hits(actor, query).await;
            fused = merge_by_path(fused, rel_hits);
        }

        let limit = usize::try_from(query.limit).unwrap_or(usize::MAX).max(1);
        fused.truncate(limit);
        Ok(fused)
    }

    #[tracing::instrument(
        level = "info",
        name = "knowledge.traverse",
        skip(self, actor),
        fields(seed_path = %query.seed_path, max_depth = query.max_depth)
    )]
    pub async fn traverse(
        &self,
        actor: &AuthenticatedContext,
        query: &KnowledgeTraversalQuery,
    ) -> Result<Vec<KnowledgeTraversalResult>, GadgetronError> {
        if query.seed_path.trim().is_empty() {
            return Err(GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::InvalidQuery {
                    reason: "seed_path must not be empty".into(),
                },
                message: "knowledge.traverse empty seed".into(),
            });
        }
        if self.relation_engines.is_empty() {
            return Ok(Vec::new());
        }
        let mut futs = FuturesUnordered::new();
        for (plug, engine) in &self.relation_engines {
            let plug = plug.clone();
            let engine = engine.clone();
            let query = query.clone();
            let actor = *actor;
            futs.push(async move {
                let result = engine.traverse(&actor, &query).await;
                (plug, result)
            });
        }
        let mut out = Vec::new();
        while let Some((plug, result)) = futs.next().await {
            match result {
                Ok(r) => out.push(r),
                Err(err) => tracing::warn!(
                    target: "knowledge_service",
                    target_plug = %plug,
                    error_code = %err.error_code(),
                    "knowledge.traverse plug failed; excluding from result"
                ),
            }
        }
        Ok(out)
    }

    /// Full reindex: reset every derived plug, then replay canonical
    /// `list` -> `get` -> `Upsert` events through `apply`.
    ///
    /// This is the audit/recovery path for when derived backends have
    /// diverged from canonical state (e.g. Postgres restore from a stale
    /// dump). Errors from a single plug are logged but do NOT abort the
    /// overall reindex — the operator wants "best effort" here.
    #[tracing::instrument(
        level = "info",
        name = "knowledge.reindex",
        skip(self, actor),
        fields(
            canonical_plug = %self.canonical_store.plug_id(),
            index_count = self.indexes.len(),
            relation_count = self.relation_engines.len()
        )
    )]
    pub async fn reindex_all(&self, actor: &AuthenticatedContext) -> Result<(), GadgetronError> {
        // Step 1: reset all derived plugs.
        for (plug, idx) in &self.indexes {
            if let Err(err) = idx.reset().await {
                tracing::warn!(
                    target: "knowledge_service",
                    target_plug = %plug,
                    error_code = %err.error_code(),
                    "index reset failed during reindex_all"
                );
            }
        }
        for (plug, eng) in &self.relation_engines {
            if let Err(err) = eng.reset().await {
                tracing::warn!(
                    target: "knowledge_service",
                    target_plug = %plug,
                    error_code = %err.error_code(),
                    "relation engine reset failed during reindex_all"
                );
            }
        }

        // Step 2: replay canonical docs as Upsert events.
        let paths = self.canonical_store.list(actor).await?;
        for path in paths {
            let Some(doc) = self.canonical_store.get(actor, &path).await? else {
                continue;
            };
            let event = KnowledgeChangeEvent::Upsert { document: doc };
            let _ignored_failures = self.fanout_change(actor, &event).await;
            // Under StoreOnly, the Vec<PlugId> return is stored in logs
            // via `fanout_change`'s own tracing. Under AwaitDerived, a
            // single plug failure aborts the fanout of THIS document but
            // we continue with the next path — reindex is best-effort.
        }
        Ok(())
    }

    fn pick_search_plugs(&self, mode: KnowledgeQueryMode) -> Vec<PlugId> {
        match mode {
            KnowledgeQueryMode::Keyword => self
                .indexes
                .iter()
                .filter(|(_, idx)| matches!(idx.mode(), KnowledgeQueryMode::Keyword))
                .map(|(p, _)| p.clone())
                .collect(),
            KnowledgeQueryMode::Semantic => self
                .indexes
                .iter()
                .filter(|(_, idx)| matches!(idx.mode(), KnowledgeQueryMode::Semantic))
                .map(|(p, _)| p.clone())
                .collect(),
            KnowledgeQueryMode::Hybrid => self
                .indexes
                .iter()
                .filter(|(_, idx)| {
                    matches!(
                        idx.mode(),
                        KnowledgeQueryMode::Keyword
                            | KnowledgeQueryMode::Semantic
                            | KnowledgeQueryMode::Hybrid
                    )
                })
                .map(|(p, _)| p.clone())
                .collect(),
            KnowledgeQueryMode::Relations => Vec::new(),
            KnowledgeQueryMode::Auto => self.indexes.keys().cloned().collect(),
        }
    }

    async fn collect_relation_hits(
        &self,
        actor: &AuthenticatedContext,
        query: &KnowledgeQuery,
    ) -> Vec<KnowledgeHit> {
        let mut out = Vec::new();
        for (plug, eng) in &self.relation_engines {
            // Use the query text as seed_path — this is a best-effort
            // bridge from keyword search to relation engines when a
            // caller requests `include_relations`. A proper API
            // (`knowledge.traverse`) is the authoritative entry point.
            let q = KnowledgeTraversalQuery {
                seed_path: query.text.clone(),
                relation: None,
                max_depth: 2,
                limit: query.limit,
            };
            match eng.traverse(actor, &q).await {
                Ok(r) => {
                    for hit in r.nodes {
                        out.push(KnowledgeHit {
                            source_kind: KnowledgeHitKind::RelationEdge,
                            source_plug: plug.clone(),
                            ..hit
                        });
                    }
                }
                Err(err) => tracing::warn!(
                    target: "knowledge_service",
                    target_plug = %plug,
                    error_code = %err.error_code(),
                    "relation augmentation failed; excluding from result"
                ),
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Fusion helpers (authority doc §2.2.4)
// ---------------------------------------------------------------------------

/// RRF-fuse per-plug hit lists into a single ranked Vec.
///
/// Dedup key: `path`. If multiple plugs return the same path, the higher
/// final fused score wins (with snippet/title carried from the higher-
/// ranked contribution).
fn fuse_hits(per_plug: Vec<Vec<KnowledgeHit>>) -> Vec<KnowledgeHit> {
    if per_plug.is_empty() {
        return Vec::new();
    }
    if per_plug.len() == 1 {
        // Single-plug result — no fusion math needed. Still dedup in case
        // the plug emitted duplicate paths (which it SHOULD NOT, but the
        // trait does not enforce).
        return dedup_by_path(per_plug.into_iter().next().unwrap());
    }

    let mut fused: HashMap<String, FusedState> = HashMap::new();
    for list in per_plug {
        for (rank, hit) in list.into_iter().enumerate() {
            let contribution = 1.0 / (RRF_K + rank as f32 + 1.0);
            let state = fused.entry(hit.path.clone()).or_insert_with(|| FusedState {
                best_rank: rank,
                hit: hit.clone(),
                score: 0.0,
            });
            state.score += contribution;
            if rank < state.best_rank {
                state.best_rank = rank;
                state.hit = hit;
            }
        }
    }

    let mut out: Vec<KnowledgeHit> = fused
        .into_values()
        .map(|mut s| {
            s.hit.score = s.score;
            s.hit
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

struct FusedState {
    best_rank: usize,
    hit: KnowledgeHit,
    score: f32,
}

fn dedup_by_path(mut hits: Vec<KnowledgeHit>) -> Vec<KnowledgeHit> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut i = 0;
    while i < hits.len() {
        if let Some(&prev) = seen.get(&hits[i].path) {
            // Keep the higher-scored one.
            if hits[i].score > hits[prev].score {
                hits.swap(i, prev);
            }
            hits.remove(i);
            continue;
        }
        seen.insert(hits[i].path.clone(), i);
        i += 1;
    }
    hits
}

fn merge_by_path(
    mut primary: Vec<KnowledgeHit>,
    additions: Vec<KnowledgeHit>,
) -> Vec<KnowledgeHit> {
    let mut seen: std::collections::HashSet<String> =
        primary.iter().map(|h| h.path.clone()).collect();
    for add in additions {
        if !seen.contains(&add.path) {
            seen.insert(add.path.clone());
            primary.push(add);
        }
    }
    primary
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use gadgetron_core::knowledge::{
        KnowledgeChangeEvent, KnowledgeDocument, KnowledgeHit, KnowledgeHitKind, KnowledgeQuery,
        KnowledgeQueryMode, KnowledgeRelationEdge, KnowledgeTraversalResult,
    };
    use std::sync::Mutex;

    fn plug(s: &str) -> PlugId {
        PlugId::new(s).unwrap()
    }

    fn actor() -> AuthenticatedContext {
        AuthenticatedContext
    }

    // ---- Fakes ----

    #[derive(Debug)]
    struct FakeStore {
        plug_id: PlugId,
        docs: Mutex<HashMap<String, KnowledgeDocument>>,
        force_rename_missing_get: Mutex<bool>,
    }

    impl FakeStore {
        fn new(plug_name: &str) -> Self {
            Self {
                plug_id: plug(plug_name),
                docs: Mutex::new(HashMap::new()),
                force_rename_missing_get: Mutex::new(false),
            }
        }

        fn seeded(plug_name: &str, seeds: &[(&str, &str)]) -> Self {
            let s = Self::new(plug_name);
            for (path, body) in seeds {
                s.docs.lock().unwrap().insert(
                    (*path).to_string(),
                    KnowledgeDocument {
                        path: (*path).to_string(),
                        title: None,
                        markdown: (*body).to_string(),
                        frontmatter: serde_json::Value::Null,
                        canonical_plug: plug(plug_name),
                        updated_at: Utc::now(),
                    },
                );
            }
            s
        }
    }

    #[async_trait]
    impl KnowledgeStore for FakeStore {
        fn plug_id(&self) -> &PlugId {
            &self.plug_id
        }

        async fn list(&self, _: &AuthenticatedContext) -> Result<Vec<String>, GadgetronError> {
            Ok(self.docs.lock().unwrap().keys().cloned().collect())
        }

        async fn get(
            &self,
            _: &AuthenticatedContext,
            path: &str,
        ) -> Result<Option<KnowledgeDocument>, GadgetronError> {
            Ok(self.docs.lock().unwrap().get(path).cloned())
        }

        async fn put(
            &self,
            _: &AuthenticatedContext,
            request: KnowledgePutRequest,
        ) -> Result<KnowledgeWriteReceipt, GadgetronError> {
            let doc = KnowledgeDocument {
                path: request.path.clone(),
                title: None,
                markdown: request.markdown,
                frontmatter: serde_json::Value::Null,
                canonical_plug: self.plug_id.clone(),
                updated_at: Utc::now(),
            };
            self.docs.lock().unwrap().insert(request.path.clone(), doc);
            Ok(KnowledgeWriteReceipt {
                path: request.path,
                canonical_plug: self.plug_id.clone(),
                revision: "fake-rev".into(),
                derived_failures: Vec::new(),
            })
        }

        async fn delete(&self, _: &AuthenticatedContext, path: &str) -> Result<(), GadgetronError> {
            self.docs.lock().unwrap().remove(path);
            Ok(())
        }

        async fn rename(
            &self,
            _: &AuthenticatedContext,
            from: &str,
            to: &str,
        ) -> Result<KnowledgeWriteReceipt, GadgetronError> {
            let doc = self.docs.lock().unwrap().remove(from);
            if let Some(mut d) = doc {
                d.path = to.to_string();
                if !*self.force_rename_missing_get.lock().unwrap() {
                    self.docs.lock().unwrap().insert(to.to_string(), d);
                }
            }
            Ok(KnowledgeWriteReceipt {
                path: to.to_string(),
                canonical_plug: self.plug_id.clone(),
                revision: "fake-rev".into(),
                derived_failures: Vec::new(),
            })
        }
    }

    #[derive(Debug)]
    struct FakeIndex {
        plug_id: PlugId,
        mode: KnowledgeQueryMode,
        applied: Mutex<Vec<KnowledgeChangeEvent>>,
        fail_apply: bool,
        canned_hits: Vec<KnowledgeHit>,
        reset_count: Mutex<usize>,
    }

    impl FakeIndex {
        fn new(plug_name: &str, mode: KnowledgeQueryMode) -> Arc<Self> {
            Arc::new(Self {
                plug_id: plug(plug_name),
                mode,
                applied: Mutex::new(Vec::new()),
                fail_apply: false,
                canned_hits: Vec::new(),
                reset_count: Mutex::new(0),
            })
        }

        fn failing(plug_name: &str, mode: KnowledgeQueryMode) -> Arc<Self> {
            Arc::new(Self {
                plug_id: plug(plug_name),
                mode,
                applied: Mutex::new(Vec::new()),
                fail_apply: true,
                canned_hits: Vec::new(),
                reset_count: Mutex::new(0),
            })
        }

        fn with_hits(
            plug_name: &str,
            mode: KnowledgeQueryMode,
            hits: Vec<KnowledgeHit>,
        ) -> Arc<Self> {
            Arc::new(Self {
                plug_id: plug(plug_name),
                mode,
                applied: Mutex::new(Vec::new()),
                fail_apply: false,
                canned_hits: hits,
                reset_count: Mutex::new(0),
            })
        }
    }

    #[async_trait]
    impl KnowledgeIndex for FakeIndex {
        fn plug_id(&self) -> &PlugId {
            &self.plug_id
        }

        fn mode(&self) -> KnowledgeQueryMode {
            self.mode
        }

        async fn search(
            &self,
            _: &AuthenticatedContext,
            _q: &KnowledgeQuery,
        ) -> Result<Vec<KnowledgeHit>, GadgetronError> {
            Ok(self.canned_hits.clone())
        }

        async fn reset(&self) -> Result<(), GadgetronError> {
            *self.reset_count.lock().unwrap() += 1;
            Ok(())
        }

        async fn apply(
            &self,
            _: &AuthenticatedContext,
            event: KnowledgeChangeEvent,
        ) -> Result<(), GadgetronError> {
            if self.fail_apply {
                return Err(GadgetronError::Knowledge {
                    kind: KnowledgeErrorKind::BackendUnavailable {
                        plug: self.plug_id.to_string(),
                    },
                    message: "simulated failure".into(),
                });
            }
            self.applied.lock().unwrap().push(event);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FakeRelation {
        plug_id: PlugId,
        applied: Mutex<Vec<KnowledgeChangeEvent>>,
        reset_count: Mutex<usize>,
    }

    impl FakeRelation {
        fn new(plug_name: &str) -> Arc<Self> {
            Arc::new(Self {
                plug_id: plug(plug_name),
                applied: Mutex::new(Vec::new()),
                reset_count: Mutex::new(0),
            })
        }
    }

    #[async_trait]
    impl KnowledgeRelationEngine for FakeRelation {
        fn plug_id(&self) -> &PlugId {
            &self.plug_id
        }

        async fn traverse(
            &self,
            _: &AuthenticatedContext,
            _q: &KnowledgeTraversalQuery,
        ) -> Result<KnowledgeTraversalResult, GadgetronError> {
            Ok(KnowledgeTraversalResult {
                nodes: vec![KnowledgeHit {
                    path: "related-page".into(),
                    title: None,
                    snippet: "".into(),
                    score: 0.1,
                    source_plug: self.plug_id.clone(),
                    source_kind: KnowledgeHitKind::RelationEdge,
                }],
                edges: vec![KnowledgeRelationEdge {
                    from_path: "seed".into(),
                    to_path: "related-page".into(),
                    relation: "mentions".into(),
                }],
                source_plug: self.plug_id.clone(),
            })
        }

        async fn reset(&self) -> Result<(), GadgetronError> {
            *self.reset_count.lock().unwrap() += 1;
            Ok(())
        }

        async fn apply(
            &self,
            _: &AuthenticatedContext,
            event: KnowledgeChangeEvent,
        ) -> Result<(), GadgetronError> {
            self.applied.lock().unwrap().push(event);
            Ok(())
        }
    }

    // ---- write path ----

    #[tokio::test]
    async fn write_canonical_succeeds_fanout_failure_store_only() {
        // Authority doc §4.2 "write_canonical_succeeds_fanout_failure_store_only":
        // canonical OK + derived failing -> write succeeds, `derived_failures`
        // populated, error is NOT promoted.
        let store = Arc::new(FakeStore::new("llm-wiki"));
        let good = FakeIndex::new("wiki-keyword", KnowledgeQueryMode::Keyword);
        let bad = FakeIndex::failing("semantic-pgvector", KnowledgeQueryMode::Semantic);
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store.clone())
            .add_index(good.clone())
            .add_index(bad.clone())
            .write_consistency(KnowledgeWriteConsistency::StoreOnly)
            .build()
            .unwrap();

        let receipt = svc
            .write(
                &actor(),
                KnowledgePutRequest {
                    path: "home".into(),
                    markdown: "# Home".into(),
                    create_only: false,
                    overwrite: false,
                },
            )
            .await
            .expect("store_only must not propagate derived failure");

        assert_eq!(receipt.path, "home");
        assert_eq!(
            receipt.derived_failures,
            vec![plug("semantic-pgvector")],
            "derived failure must surface via receipt"
        );
        assert_eq!(good.applied.lock().unwrap().len(), 1);
        assert_eq!(bad.applied.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn write_await_derived_promotes_failure() {
        // Authority doc §4.2 "write_await_derived_promotes_failure":
        // `await_derived` consistency -> derived fail -> write error.
        let store = Arc::new(FakeStore::new("llm-wiki"));
        let bad = FakeIndex::failing("semantic-pgvector", KnowledgeQueryMode::Semantic);
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store.clone())
            .add_index(bad.clone())
            .write_consistency(KnowledgeWriteConsistency::AwaitDerived)
            .build()
            .unwrap();

        let err = svc
            .write(
                &actor(),
                KnowledgePutRequest {
                    path: "home".into(),
                    markdown: "# Home".into(),
                    create_only: false,
                    overwrite: false,
                },
            )
            .await
            .expect_err("await_derived must promote failure");

        match err {
            GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::DerivedApplyFailed { plug },
                ..
            } => assert_eq!(plug, "semantic-pgvector"),
            other => panic!("wrong variant: {other:?}"),
        }

        // Canonical write still happened — `await_derived` doesn't roll
        // back the store, only surfaces as an error.
        assert!(store.docs.lock().unwrap().contains_key("home"));
    }

    #[tokio::test]
    async fn write_with_no_derived_plugs_is_no_op_fanout() {
        let store = Arc::new(FakeStore::new("llm-wiki"));
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .build()
            .unwrap();
        let receipt = svc
            .write(
                &actor(),
                KnowledgePutRequest {
                    path: "home".into(),
                    markdown: "# Home".into(),
                    create_only: false,
                    overwrite: false,
                },
            )
            .await
            .unwrap();
        assert!(receipt.derived_failures.is_empty());
    }

    // ---- search path ----

    #[tokio::test]
    async fn search_mode_routes_correctly() {
        // Keyword mode → only keyword-plug hits; Semantic mode → only
        // semantic-plug hits; Hybrid → both.
        let store = Arc::new(FakeStore::seeded("llm-wiki", &[]));
        let kw_hit = KnowledgeHit {
            path: "kw".into(),
            title: None,
            snippet: "keyword".into(),
            score: 1.0,
            source_plug: plug("wiki-keyword"),
            source_kind: KnowledgeHitKind::SearchIndex,
        };
        let sem_hit = KnowledgeHit {
            path: "sem".into(),
            title: None,
            snippet: "semantic".into(),
            score: 1.0,
            source_plug: plug("semantic-pgvector"),
            source_kind: KnowledgeHitKind::SearchIndex,
        };
        let kw = FakeIndex::with_hits(
            "wiki-keyword",
            KnowledgeQueryMode::Keyword,
            vec![kw_hit.clone()],
        );
        let sem = FakeIndex::with_hits(
            "semantic-pgvector",
            KnowledgeQueryMode::Semantic,
            vec![sem_hit.clone()],
        );
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .add_index(kw)
            .add_index(sem)
            .build()
            .unwrap();

        // Keyword mode: only the keyword plug contributes.
        let hits = svc
            .search(
                &actor(),
                &KnowledgeQuery {
                    text: "any".into(),
                    limit: 10,
                    mode: KnowledgeQueryMode::Keyword,
                    include_relations: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "kw");

        // Semantic mode: only the semantic plug contributes.
        let hits = svc
            .search(
                &actor(),
                &KnowledgeQuery {
                    text: "any".into(),
                    limit: 10,
                    mode: KnowledgeQueryMode::Semantic,
                    include_relations: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "sem");

        // Hybrid: both contribute.
        let hits = svc
            .search(
                &actor(),
                &KnowledgeQuery {
                    text: "any".into(),
                    limit: 10,
                    mode: KnowledgeQueryMode::Hybrid,
                    include_relations: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert!(paths.contains(&"kw"));
        assert!(paths.contains(&"sem"));
    }

    #[tokio::test]
    async fn search_result_fusion_dedups_by_path() {
        // Same path returned by two different plugs → one result after
        // dedup; score becomes the fused sum.
        let shared_path = "shared";
        let hit_a = KnowledgeHit {
            path: shared_path.into(),
            title: None,
            snippet: "from kw".into(),
            score: 1.0,
            source_plug: plug("wiki-keyword"),
            source_kind: KnowledgeHitKind::SearchIndex,
        };
        let hit_b = KnowledgeHit {
            path: shared_path.into(),
            title: None,
            snippet: "from sem".into(),
            score: 0.5,
            source_plug: plug("semantic-pgvector"),
            source_kind: KnowledgeHitKind::SearchIndex,
        };
        let store = Arc::new(FakeStore::new("llm-wiki"));
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .add_index(FakeIndex::with_hits(
                "wiki-keyword",
                KnowledgeQueryMode::Keyword,
                vec![hit_a],
            ))
            .add_index(FakeIndex::with_hits(
                "semantic-pgvector",
                KnowledgeQueryMode::Semantic,
                vec![hit_b],
            ))
            .build()
            .unwrap();

        let hits = svc
            .search(
                &actor(),
                &KnowledgeQuery {
                    text: "any".into(),
                    limit: 10,
                    mode: KnowledgeQueryMode::Hybrid,
                    include_relations: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), 1, "duplicate path must dedup: {hits:?}");
        assert_eq!(hits[0].path, shared_path);
        // Fused score is the RRF sum — both plugs contribute at rank 0,
        // so score ≈ 2.0/(RRF_K + 1) = 2.0/61.
        let expected = 2.0 / (RRF_K + 1.0);
        assert!(
            (hits[0].score - expected).abs() < 0.001,
            "expected ~{expected}, got {}",
            hits[0].score
        );
    }

    #[tokio::test]
    async fn search_empty_query_short_circuits() {
        let store = Arc::new(FakeStore::new("llm-wiki"));
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .add_index(FakeIndex::new("wiki-keyword", KnowledgeQueryMode::Keyword))
            .build()
            .unwrap();
        let hits = svc
            .search(
                &actor(),
                &KnowledgeQuery {
                    text: "   ".into(),
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
    async fn search_limit_zero_is_invalid_query() {
        let store = Arc::new(FakeStore::new("llm-wiki"));
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .build()
            .unwrap();
        let err = svc
            .search(
                &actor(),
                &KnowledgeQuery {
                    text: "x".into(),
                    limit: 0,
                    mode: KnowledgeQueryMode::Keyword,
                    include_relations: false,
                },
            )
            .await
            .expect_err("limit=0 must error");
        match err {
            GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::InvalidQuery { .. },
                ..
            } => (),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_failing_plug_does_not_abort_fanout() {
        // Authority doc §2.2.4: a failed plug is excluded, not fatal.
        let store = Arc::new(FakeStore::new("llm-wiki"));
        let good_hit = KnowledgeHit {
            path: "found".into(),
            title: None,
            snippet: "".into(),
            score: 1.0,
            source_plug: plug("wiki-keyword"),
            source_kind: KnowledgeHitKind::SearchIndex,
        };
        let good =
            FakeIndex::with_hits("wiki-keyword", KnowledgeQueryMode::Keyword, vec![good_hit]);

        // A failing semantic plug (FakeIndex.fail_apply doesn't affect
        // search; build a dedicated search-failing fake inline).
        #[derive(Debug)]
        struct SearchFail {
            plug_id: PlugId,
        }
        #[async_trait]
        impl KnowledgeIndex for SearchFail {
            fn plug_id(&self) -> &PlugId {
                &self.plug_id
            }
            fn mode(&self) -> KnowledgeQueryMode {
                KnowledgeQueryMode::Semantic
            }
            async fn search(
                &self,
                _: &AuthenticatedContext,
                _q: &KnowledgeQuery,
            ) -> Result<Vec<KnowledgeHit>, GadgetronError> {
                Err(GadgetronError::Knowledge {
                    kind: KnowledgeErrorKind::BackendUnavailable {
                        plug: "semantic-pgvector".into(),
                    },
                    message: "down".into(),
                })
            }
            async fn reset(&self) -> Result<(), GadgetronError> {
                Ok(())
            }
            async fn apply(
                &self,
                _: &AuthenticatedContext,
                _e: KnowledgeChangeEvent,
            ) -> Result<(), GadgetronError> {
                Ok(())
            }
        }
        let failing = Arc::new(SearchFail {
            plug_id: plug("semantic-pgvector"),
        }) as Arc<dyn KnowledgeIndex>;

        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .add_index(good)
            .add_index(failing)
            .build()
            .unwrap();
        let hits = svc
            .search(
                &actor(),
                &KnowledgeQuery {
                    text: "x".into(),
                    limit: 10,
                    mode: KnowledgeQueryMode::Hybrid,
                    include_relations: false,
                },
            )
            .await
            .expect("partial failure is not fatal");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "found");
    }

    // ---- reindex path ----

    #[tokio::test]
    async fn reindex_all_resets_and_replays() {
        // Authority doc §4.2 "reindex_all_resets_and_replays":
        // canonical docs → derived backends cleared + re-applied.
        let store = Arc::new(FakeStore::seeded(
            "llm-wiki",
            &[("home", "# Home"), ("notes/a", "alpha")],
        ));
        let idx = FakeIndex::new("wiki-keyword", KnowledgeQueryMode::Keyword);
        let rel = FakeRelation::new("graphify");
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store.clone())
            .add_index(idx.clone())
            .add_relation_engine(rel.clone())
            .build()
            .unwrap();

        svc.reindex_all(&actor()).await.unwrap();

        assert_eq!(*idx.reset_count.lock().unwrap(), 1, "index must be reset");
        assert_eq!(
            *rel.reset_count.lock().unwrap(),
            1,
            "relation engine must be reset"
        );

        let idx_events = idx.applied.lock().unwrap();
        assert_eq!(
            idx_events.len(),
            2,
            "two docs → two upserts: {idx_events:?}"
        );
        for e in idx_events.iter() {
            assert!(matches!(e, KnowledgeChangeEvent::Upsert { .. }));
        }

        let rel_events = rel.applied.lock().unwrap();
        assert_eq!(rel_events.len(), 2);
    }

    // ---- delete / rename fanout ----

    #[tokio::test]
    async fn delete_fans_out_delete_event() {
        let store = Arc::new(FakeStore::seeded("llm-wiki", &[("home", "body")]));
        let idx = FakeIndex::new("wiki-keyword", KnowledgeQueryMode::Keyword);
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store.clone())
            .add_index(idx.clone())
            .build()
            .unwrap();
        let failures = svc.delete(&actor(), "home").await.unwrap();
        assert!(failures.is_empty());
        let events = idx.applied.lock().unwrap();
        assert!(matches!(
            events.first(),
            Some(KnowledgeChangeEvent::Delete { .. })
        ));
    }

    #[tokio::test]
    async fn rename_fans_out_rename_event_with_post_rename_doc() {
        let store = Arc::new(FakeStore::seeded("llm-wiki", &[("old", "body")]));
        let idx = FakeIndex::new("wiki-keyword", KnowledgeQueryMode::Keyword);
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store.clone())
            .add_index(idx.clone())
            .build()
            .unwrap();
        let receipt = svc.rename(&actor(), "old", "new").await.unwrap();
        assert_eq!(receipt.path, "new");
        let events = idx.applied.lock().unwrap();
        match events.first() {
            Some(KnowledgeChangeEvent::Rename { from, to, document }) => {
                assert_eq!(from, "old");
                assert_eq!(to, "new");
                assert_eq!(document.path, "new");
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    // ---- traverse path ----

    #[tokio::test]
    async fn traverse_empty_seed_is_invalid_query() {
        let store = Arc::new(FakeStore::new("llm-wiki"));
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .add_relation_engine(FakeRelation::new("graphify"))
            .build()
            .unwrap();
        let err = svc
            .traverse(
                &actor(),
                &KnowledgeTraversalQuery {
                    seed_path: "   ".into(),
                    relation: None,
                    max_depth: 3,
                    limit: 10,
                },
            )
            .await
            .expect_err("empty seed must error");
        assert!(matches!(
            err,
            GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::InvalidQuery { .. },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn traverse_without_relation_engines_returns_empty() {
        let store = Arc::new(FakeStore::new("llm-wiki"));
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .build()
            .unwrap();
        let out = svc
            .traverse(
                &actor(),
                &KnowledgeTraversalQuery {
                    seed_path: "home".into(),
                    relation: None,
                    max_depth: 3,
                    limit: 10,
                },
            )
            .await
            .unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn traverse_dispatches_to_all_relation_engines() {
        let store = Arc::new(FakeStore::new("llm-wiki"));
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .add_relation_engine(FakeRelation::new("graphify"))
            .add_relation_engine(FakeRelation::new("obsidian-links"))
            .build()
            .unwrap();
        let out = svc
            .traverse(
                &actor(),
                &KnowledgeTraversalQuery {
                    seed_path: "home".into(),
                    relation: None,
                    max_depth: 3,
                    limit: 10,
                },
            )
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
    }

    // ---- builder validation ----

    #[test]
    fn builder_requires_canonical_store() {
        let err = KnowledgeServiceBuilder::new()
            .build()
            .expect_err("missing store");
        match err {
            GadgetronError::Config(msg) => assert!(msg.contains("canonical_store")),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn builder_default_write_consistency_is_store_only() {
        let store = Arc::new(FakeStore::new("llm-wiki"));
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .build()
            .unwrap();
        assert_eq!(
            svc.write_consistency(),
            KnowledgeWriteConsistency::StoreOnly
        );
    }
}
