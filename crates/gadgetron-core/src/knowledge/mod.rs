//! Knowledge plane contract (Phase 2B, W3-KL-1).
//!
//! This module is the `gadgetron-core`-owned seam for the "knowledge is core,
//! capabilities are pluggable" architecture adopted in
//! `docs/design/core/knowledge-plug-architecture.md` (Approved 2026-04-18).
//!
//! # What lives here
//!
//! The **contract** side of the knowledge plane:
//!
//! - Three `async_trait` plug axes: [`KnowledgeStore`], [`KnowledgeIndex`],
//!   [`KnowledgeRelationEngine`].
//! - Wire types ([`KnowledgeDocument`], [`KnowledgeHit`], …) that cross every
//!   boundary in the knowledge pipeline.
//! - Mode / consistency / hit-kind enums with explicit snake_case serde
//!   (`keyword`, `await_derived`, `relation_edge`) so config TOML and audit
//!   JSON share one wire string.
//!
//! # What does NOT live here
//!
//! - `KnowledgeService` orchestration, `LlmWikiStore`, `WikiKeywordIndex`,
//!   `SemanticPgVectorIndex` — those are in `gadgetron-knowledge`.
//! - Any backend-specific logic (`git2`, `sqlx`, external HTTP runtime,
//!   graphify client). Core stays leaf per D-12.
//!
//! # How traits compose
//!
//! Every operation flows through `KnowledgeService`:
//!
//! ```text
//! caller (Gadget/CLI/Web)
//!   -> KnowledgeService
//!     -> canonical KnowledgeStore::put       (authoritative write)
//!     -> fanout KnowledgeChangeEvent
//!         -> KnowledgeIndex::apply   (N derived indexes)
//!         -> KnowledgeRelationEngine::apply (M relation engines)
//! ```
//!
//! The trait split is intentional: stores own source-of-truth, indexes own
//! read-side derivation, relation engines own graph traversal. One flat
//! `KnowledgeEngine` trait was considered and rejected (authority doc §1.3
//! alternative D).
//!
//! # Error model
//!
//! All trait methods return `KnowledgeResult<T>` which is a type alias over
//! [`GadgetronError`](crate::error::GadgetronError). Backend-specific errors
//! are normalized to
//! [`KnowledgeErrorKind`](crate::error::KnowledgeErrorKind) at the service
//! boundary — no `WikiError` or `sqlx::Error` crosses this trait surface.
//!
//! # Serde wire stability
//!
//! `KnowledgeQueryMode`, `KnowledgeWriteConsistency`, and `KnowledgeHitKind`
//! all use `#[serde(rename_all = "snake_case")]`. These strings are baked
//! into `gadgetron.toml`, audit JSON, and MCP tool arguments — they MUST
//! NOT change without a wire-compat decision log entry.

pub mod candidate;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::bundle::PlugId;
use crate::error::GadgetronError;

// Re-export `Arc` for doctests / user-facing snippets. Does NOT add any
// dependency beyond `std`.
#[doc(hidden)]
pub use std::sync::Arc as KnowledgeArc;

// Pull in the core error kind so downstream crates can `use
// gadgetron_core::knowledge::KnowledgeErrorKind;` without a parallel
// `use gadgetron_core::error::KnowledgeErrorKind;` path.
pub use crate::error::KnowledgeErrorKind;

/// Result alias used by every trait method in this module.
///
/// Pinned to [`GadgetronError`] so knowledge backends surface through the
/// same HTTP / error-code taxonomy as every other subsystem. Backend-local
/// error types (`WikiError`, `sqlx::Error`, external runtime RPC errors)
/// are translated to `GadgetronError::Knowledge { kind, message }` at the
/// service boundary per authority doc §2.4.1.
pub type KnowledgeResult<T> = std::result::Result<T, GadgetronError>;

/// Caller identity passed through every knowledge plane call.
///
/// # Deliberate placeholder
///
/// This is a zero-field marker type in W3-KL-1. The 08/09/10 Phase 2B
/// docs (`docs/design/phase2/08-*`, `09-knowledge-acl.md`,
/// `10-penny-permission-inheritance.md`) will promote it to carry
/// `user_id`, `tenant_id`, scopes, and audit correlation. The trait
/// signatures lock the **shape** ("every store/index/relation takes the
/// caller identity") so W3-KL-2 can swell the payload without a
/// breaking-change to the public trait surface.
///
/// Constructing it is cheap (ZST) and safe from anywhere; ACL enforcement
/// is strictly at the service boundary — the placeholder deliberately has
/// no `new()` beyond `Default` so implementations cannot accidentally
/// fabricate an authorized context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuthenticatedContext;

/// Write consistency policy for [`KnowledgeStore::put`] / `delete` /
/// `rename` fanout.
///
/// See authority doc §2.2.3 for the algorithm. The default is `StoreOnly`
/// because canonical store success preserves source-of-truth even if
/// derived backends (pgvector, graphify) are transiently unavailable —
/// flipping to `AwaitDerived` would have the opposite, operator-hostile
/// failure mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeWriteConsistency {
    /// Canonical store success is the user-visible success boundary.
    /// Derived index / relation apply failures are reported as
    /// `derived_failures` in the `KnowledgeWriteReceipt` but the write
    /// itself returns `Ok`.
    #[default]
    StoreOnly,
    /// Block on derived backend apply. Any derived failure is promoted
    /// to a `Knowledge::DerivedApplyFailed` error.
    AwaitDerived,
}

/// Search mode requested by the caller.
///
/// `Auto` = "dispatch across all enabled search plugs"; `Hybrid` forces
/// both keyword + semantic even when `Auto` would have chosen only one.
/// `Relations` targets only [`KnowledgeRelationEngine`] plugs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeQueryMode {
    Keyword,
    Semantic,
    Hybrid,
    Relations,
    #[default]
    Auto,
}

/// Classification of the backend that produced a given [`KnowledgeHit`].
///
/// `Canonical` is reserved for hits that originate from a
/// [`KnowledgeStore`] (e.g. exact-match `get` promoted into a search
/// result). `SearchIndex` is the common case — any [`KnowledgeIndex`]
/// contribution. `RelationEdge` tags graph-traversal hits so UI / Penny
/// prompts can render them with an edge-type badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeHitKind {
    Canonical,
    SearchIndex,
    RelationEdge,
}

/// A canonical knowledge-layer path.
///
/// Wraps a validated path string shaped like `"ops/journal/2026-04-18/restart"`.
/// Used everywhere a path identifies a *knowledge artifact* — not a filesystem
/// file — including [`KnowledgeDocumentWrite`], candidate `proposed_path`,
/// Penny digest `proposed_path` / `canonical_path`, and materialize receipts.
///
/// # Wire shape
///
/// Serialized as a bare JSON string (routed through `String` via
/// `#[serde(try_from = "String", into = "String")]`), so operators and
/// downstream consumers see unchanged wire bytes compared to the
/// pre-drift-fix `String` representation. Deserialization runs
/// [`KnowledgePath::new`] validation — malformed paths reject at the type
/// boundary rather than leaking into downstream stores.
///
/// # Validation rules
///
/// - **non-empty** — `""` rejects as [`KnowledgePathError::Empty`]. Use
///   `Option<KnowledgePath>` to express "no path".
/// - **no path traversal** — `".."` segment, leading `/`, or trailing `/`
///   rejects as [`KnowledgePathError::Traversal`]. Knowledge paths are
///   relative to a canonical root and MUST NOT escape it.
/// - **no control characters** — ASCII `0x00..=0x1F` rejects as
///   [`KnowledgePathError::ControlChar`]. Prevents log / audit / SQL
///   injection via newline-tabbed paths.
/// - **max 1024 bytes** — paths longer than 1 KiB reject as
///   [`KnowledgePathError::TooLong`]. Matches `wiki_max_page_bytes`
///   semantic ceiling; longer paths indicate a bug (e.g. path_rules
///   template expansion loop).
///
/// # Ordering caveat
///
/// Derives `Ord` / `PartialOrd` on the inner `String`, i.e. Rust UTF-8
/// codepoint order. PostgreSQL `ORDER BY path` uses byte order by default —
/// identical for ASCII-only paths, divergent for non-ASCII. In practice
/// path segments are ASCII (`path_rules` expansion uses `YYYY-MM-DD` +
/// snake_case kind + UUIDs), so this is a non-issue. Callers that need
/// deterministic cross-store sort should sort in Rust after fetch.
///
/// # Pg binding
///
/// Bind as `TEXT` via `AsRef<str>` or `.to_string()`. No schema change.
///
/// # Authority
///
/// Spec: `docs/design/core/knowledge-candidate-curation.md` §2.1.
/// Drift-fix PR 2 (D-20260418-26) landed this type; KC-1 had
/// `Option<String>` with a `TODO KC-1b` marker.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
// `try_from = "String"` + `into = "String"` route (de)serialization through
// the String conversions, producing a bare-string wire shape AND running
// validation on deserialize. `#[serde(transparent)]` conflicts with the
// try_from/into attrs and is not needed here.
#[serde(try_from = "String", into = "String")]
pub struct KnowledgePath(String);

/// Failure modes for [`KnowledgePath::new`] / [`KnowledgePath::try_from`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum KnowledgePathError {
    /// The path string is empty. Use `Option<KnowledgePath>` for "no path".
    Empty,
    /// The path contains `..`, a leading `/`, or a trailing `/`.
    /// Canonical paths MUST stay under a relative root.
    Traversal,
    /// The path contains an ASCII control character (`0x00..=0x1F`).
    ControlChar,
    /// The path exceeds `KnowledgePath::MAX_LEN` bytes.
    TooLong {
        /// Actual byte length of the rejected input.
        actual: usize,
    },
}

impl std::fmt::Display for KnowledgePathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("knowledge path must not be empty"),
            Self::Traversal => {
                f.write_str("knowledge path must not contain '..', lead with '/', or end with '/'")
            }
            Self::ControlChar => {
                f.write_str("knowledge path must not contain ASCII control characters")
            }
            Self::TooLong { actual } => write!(
                f,
                "knowledge path must be ≤ {} bytes; got {} bytes",
                KnowledgePath::MAX_LEN,
                actual
            ),
        }
    }
}

impl std::error::Error for KnowledgePathError {}

impl KnowledgePath {
    /// Maximum byte length of a [`KnowledgePath`].
    pub const MAX_LEN: usize = 1024;

    /// Construct a validated [`KnowledgePath`].
    pub fn new(raw: impl Into<String>) -> Result<Self, KnowledgePathError> {
        let s = raw.into();
        if s.is_empty() {
            return Err(KnowledgePathError::Empty);
        }
        if s.len() > Self::MAX_LEN {
            return Err(KnowledgePathError::TooLong { actual: s.len() });
        }
        if s.starts_with('/') || s.ends_with('/') {
            return Err(KnowledgePathError::Traversal);
        }
        for segment in s.split('/') {
            if segment == ".." {
                return Err(KnowledgePathError::Traversal);
            }
        }
        if s.chars().any(|c| c.is_ascii_control()) {
            return Err(KnowledgePathError::ControlChar);
        }
        Ok(Self(s))
    }

    /// Borrow as `&str` — zero-cost view, safe for log / audit / SQL bind.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for KnowledgePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for KnowledgePath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<KnowledgePath> for String {
    fn from(p: KnowledgePath) -> String {
        p.0
    }
}

impl TryFrom<String> for KnowledgePath {
    type Error = KnowledgePathError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for KnowledgePath {
    type Error = KnowledgePathError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl std::str::FromStr for KnowledgePath {
    type Err = KnowledgePathError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// Canonical knowledge document served by a [`KnowledgeStore`].
///
/// `frontmatter` is free-form JSON so wiki YAML/TOML frontmatter or
/// alternate store metadata (e.g. a future `confluence` store) can share
/// one wire shape. Implementations MUST populate `canonical_plug` with the
/// [`PlugId`] of the store that produced the document — this is the
/// citation anchor Penny uses for RAG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeDocument {
    pub path: String,
    pub title: Option<String>,
    pub markdown: String,
    pub frontmatter: serde_json::Value,
    pub canonical_plug: PlugId,
    pub updated_at: DateTime<Utc>,
}

/// Write request for [`KnowledgeStore::put`].
///
/// `create_only` and `overwrite` are mutually exclusive hints; when both
/// are `false` the store uses its default behavior (llm-wiki = overwrite
/// on match, create otherwise).
///
/// # `provenance`
///
/// Free-form `BTreeMap<String, String>` carrying the audit/candidate
/// trace that produced this write. Drift-fix PR 3 (D-20260418-27) added
/// the field so candidate materialization can thread capture-time
/// hint tags / rationale / source-bundle identifiers into the canonical
/// store. Store implementations SHOULD persist the provenance verbatim
/// (e.g. `LlmWikiStore` merges it into page frontmatter under
/// `provenance:`) and MUST NOT drop unknown keys.
///
/// `BTreeMap` ordering is deterministic so the written frontmatter is
/// byte-stable for audit replay. Empty map (default) is the legacy
/// behaviour — no frontmatter delta, no visible operator change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgePutRequest {
    pub path: String,
    pub markdown: String,
    #[serde(default)]
    pub create_only: bool,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub provenance: std::collections::BTreeMap<String, String>,
}

/// Receipt returned by a successful write.
///
/// `derived_failures` is the soul of the `StoreOnly` consistency policy —
/// callers inspect this to detect "canonical succeeded but derived plug X
/// is down" without promoting that to a top-level error. Empty Vec when
/// every derived plug applied successfully (or when there are no derived
/// plugs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeWriteReceipt {
    pub path: String,
    pub canonical_plug: PlugId,
    /// Store-native revision identifier. For `llm-wiki` this is the git
    /// commit OID; future stores map to their own monotone token.
    pub revision: String,
    #[serde(default)]
    pub derived_failures: Vec<PlugId>,
}

/// Search query.
///
/// `limit` is an unsigned wire value so TOML/JSON round-trips are lossless.
/// Callers that need a `usize` convert with `.min(u32::MAX) as usize` —
/// the `KnowledgeService` handles that translation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeQuery {
    pub text: String,
    pub limit: u32,
    pub mode: KnowledgeQueryMode,
    #[serde(default)]
    pub include_relations: bool,
}

/// A single hit in a search result list.
///
/// `source_plug` identifies which backend produced the hit — this is
/// required for citation stability (Penny's RAG responses cite the canonical
/// store, not the index). `score` is the fused rank score after RRF
/// (`KnowledgeService`-level), not the raw backend score; see authority
/// doc §2.2.4 for the fusion algorithm.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeHit {
    pub path: String,
    pub title: Option<String>,
    pub snippet: String,
    pub score: f32,
    pub source_plug: PlugId,
    pub source_kind: KnowledgeHitKind,
}

/// Graph-style traversal query served by [`KnowledgeRelationEngine`].
///
/// `relation` filters on a specific edge type (e.g. `"mentions"`,
/// `"child_of"`) — `None` means "any". `max_depth` is `u8` to make
/// out-of-band values (e.g. `depth = 255`) obviously absurd; production
/// traversal rarely exceeds 3-5 hops.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeTraversalQuery {
    pub seed_path: String,
    pub relation: Option<String>,
    pub max_depth: u8,
    pub limit: u32,
}

/// Traversal response — nodes + edges + source plug.
///
/// `source_plug` names the [`KnowledgeRelationEngine`] that answered; this
/// lets the UI show "from graphify" vs "from the built-in link parser"
/// without a parallel registry lookup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeTraversalResult {
    pub nodes: Vec<KnowledgeHit>,
    pub edges: Vec<KnowledgeRelationEdge>,
    pub source_plug: PlugId,
}

/// Single directed edge.
///
/// `relation` is a free-form string so store-specific edge vocabularies
/// (e.g. `"imports"`, `"authored_by"`, `"derived_from"`) don't need a
/// compile-time enum. Validation lives in the relation engine, not in
/// `gadgetron-core`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeRelationEdge {
    pub from_path: String,
    pub to_path: String,
    pub relation: String,
}

/// Change event produced by a [`KnowledgeStore`] write and consumed by
/// every [`KnowledgeIndex`] + [`KnowledgeRelationEngine`].
///
/// # Why an enum not a struct
///
/// `Upsert` / `Delete` / `Rename` carry fundamentally different payloads
/// — `Upsert` needs the full document, `Delete` only the path, `Rename`
/// needs the new post-rename document (because path, frontmatter, and
/// title may all change in a single atomic operation). Collapsing into a
/// struct with `Option<KnowledgeDocument>` would make the "which fields
/// are meaningful for which kind" contract implicit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KnowledgeChangeEvent {
    /// Create or overwrite. Carries the post-write document so downstream
    /// indexes can diff against their own state.
    Upsert { document: KnowledgeDocument },
    /// Soft- or hard-delete. `deleted_at` is the canonical store's
    /// authoritative timestamp, NOT the service's clock — this matters
    /// for out-of-order apply on eventually-consistent derived backends.
    Delete {
        path: String,
        deleted_at: DateTime<Utc>,
    },
    /// Atomic path change. `document.path` MUST equal `to`. Derived
    /// backends that key by path use this to migrate rows instead of
    /// delete+insert.
    Rename {
        from: String,
        to: String,
        document: KnowledgeDocument,
    },
}

// ---------------------------------------------------------------------------
// Traits — the three plug axes.
//
// All three take `&AuthenticatedContext` on every operation so ACL is
// enforced at the right boundary (authority doc §3.3 interface contract 1).
// Every trait requires `Debug` so registrations can be introspected by
// `gadgetron bundle info` without a parallel debug-view trait.
// ---------------------------------------------------------------------------

/// Canonical knowledge store — source of truth for one or more documents.
///
/// **Exactly one store per `KnowledgeService` is the canonical store**
/// (per `[knowledge] canonical_store` config, validated at startup).
/// Additional stores MAY be registered for `P3`+ multi-store scenarios
/// but `P2B` validation rejects that case.
///
/// # Idempotency contract
///
/// - `put` with the same `(path, markdown)` twice MUST produce the same
///   on-disk state; the returned `KnowledgeWriteReceipt::revision` MAY
///   differ (git backends bump commit OIDs per write).
/// - `delete` on a missing path MUST return a
///   `GadgetronError::Knowledge(DocumentNotFound)`, NOT a success.
///
/// # Thread-safety
///
/// `Send + Sync + 'static` (enforced by the `Arc<dyn KnowledgeStore>`
/// wrapper used by `KnowledgeService`). Implementations that hold a
/// non-Sync handle (`git2::Repository`) must use per-op re-open as
/// `LlmWikiStore` does.
#[async_trait]
pub trait KnowledgeStore: Send + Sync + std::fmt::Debug {
    /// Plug id this store was registered under. Used by
    /// `KnowledgeService` to tag hits and audit events.
    fn plug_id(&self) -> &PlugId;

    /// List all document paths. Ordering is implementation-defined;
    /// callers that need determinism sort at the service layer.
    async fn list(&self, actor: &AuthenticatedContext) -> KnowledgeResult<Vec<String>>;

    /// Fetch a document by path. Returns `Ok(None)` for a missing path
    /// (NOT an error) so callers can distinguish "absent" from "backend
    /// down" without string matching.
    async fn get(
        &self,
        actor: &AuthenticatedContext,
        path: &str,
    ) -> KnowledgeResult<Option<KnowledgeDocument>>;

    /// Create or overwrite a document.
    async fn put(
        &self,
        actor: &AuthenticatedContext,
        request: KnowledgePutRequest,
    ) -> KnowledgeResult<KnowledgeWriteReceipt>;

    /// Delete a document. Soft-delete semantics are store-defined
    /// (`LlmWikiStore` archives to `_archived/<date>/`; future stores
    /// MAY hard-delete).
    async fn delete(&self, actor: &AuthenticatedContext, path: &str) -> KnowledgeResult<()>;

    /// Atomic rename. Failure leaves the store in its pre-call state.
    async fn rename(
        &self,
        actor: &AuthenticatedContext,
        from: &str,
        to: &str,
    ) -> KnowledgeResult<KnowledgeWriteReceipt>;
}

/// Search index derived from one or more [`KnowledgeStore`] instances.
///
/// `KnowledgeIndex` splits along [`KnowledgeQueryMode`]:
/// `WikiKeywordIndex` advertises `Keyword`, `SemanticPgVectorIndex`
/// advertises `Semantic`, a future rerank-ensemble plug might advertise
/// `Hybrid`. The service uses [`Self::mode`] to route queries.
///
/// # Reindex semantics
///
/// - `apply` receives every `KnowledgeChangeEvent` from every canonical
///   write. Ordering across indexes is NOT guaranteed — implementations
///   MUST be convergent under at-least-once delivery.
/// - `reset` drops all state; the service calls this before
///   re-broadcasting a full `list` of `Upsert` events when an operator
///   runs `gadgetron reindex --full`.
#[async_trait]
pub trait KnowledgeIndex: Send + Sync + std::fmt::Debug {
    fn plug_id(&self) -> &PlugId;

    /// Which query modes this index serves. A single index MAY declare
    /// `Hybrid` if it fuses internally (e.g. a rerank ensemble).
    fn mode(&self) -> KnowledgeQueryMode;

    /// Execute a search. Return value is ranked within this index's
    /// scoring; fusion across indexes happens in `KnowledgeService`.
    async fn search(
        &self,
        actor: &AuthenticatedContext,
        query: &KnowledgeQuery,
    ) -> KnowledgeResult<Vec<KnowledgeHit>>;

    /// Drop all indexed state. Paired with `reindex_all` at the service
    /// level.
    async fn reset(&self) -> KnowledgeResult<()>;

    /// Apply a single change event. Indexes decide their own
    /// transactional boundary — `WikiKeywordIndex` updates the inverted
    /// index in-place, `SemanticPgVectorIndex` runs a SQL transaction
    /// per page.
    async fn apply(
        &self,
        actor: &AuthenticatedContext,
        event: KnowledgeChangeEvent,
    ) -> KnowledgeResult<()>;
}

/// Graph / relation traversal engine.
///
/// Relation engines observe the same `KnowledgeChangeEvent` stream as
/// indexes but expose a different read surface: [`Self::traverse`] takes
/// a [`KnowledgeTraversalQuery`] (seed path + relation type + depth) and
/// returns nodes + edges. `graphify` is the first external example; the
/// built-in Obsidian `[[link]]` parser is another candidate relation
/// engine.
#[async_trait]
pub trait KnowledgeRelationEngine: Send + Sync + std::fmt::Debug {
    fn plug_id(&self) -> &PlugId;

    /// Graph traversal starting at `seed_path`. Implementations MUST
    /// respect `max_depth` and `limit`; exceeding either is a silent
    /// truncation (no error).
    async fn traverse(
        &self,
        actor: &AuthenticatedContext,
        query: &KnowledgeTraversalQuery,
    ) -> KnowledgeResult<KnowledgeTraversalResult>;

    /// Drop all cached edges / derived graph state.
    async fn reset(&self) -> KnowledgeResult<()>;

    /// Consume a canonical change event.
    async fn apply(
        &self,
        actor: &AuthenticatedContext,
        event: KnowledgeChangeEvent,
    ) -> KnowledgeResult<()>;
}

// Silence the unused import on the public Arc re-export path — it exists for
// downstream ergonomics only.
#[doc(hidden)]
pub fn _arc_hint<T>(v: T) -> Arc<T> {
    Arc::new(v)
}

// ---------------------------------------------------------------------------
// Tests — contract + serde stability.
//
// Authority doc §4.1 single-crate coverage: serde roundtrips, unknown
// variant rejection, kind displays stable.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{GadgetronError, KnowledgeErrorKind};

    fn plug(s: &str) -> PlugId {
        PlugId::new(s).expect("valid plug id")
    }

    // ---- KnowledgePath — drift-fix PR 2 (D-20260418-26) ----

    #[test]
    fn knowledge_path_accepts_typical_candidate_paths() {
        for ok in [
            "ops/journal/2026-04-18/direct_action",
            "ops/incidents/2026-04-18/fan-boot",
            "README",
            "imports/q4-runbook",
            "ops/journal/2026-04-18/uuid-abcdef",
        ] {
            KnowledgePath::new(ok).unwrap_or_else(|e| panic!("{ok:?} rejected: {e}"));
        }
    }

    #[test]
    fn knowledge_path_rejects_empty() {
        assert_eq!(KnowledgePath::new(""), Err(KnowledgePathError::Empty));
    }

    #[test]
    fn knowledge_path_rejects_leading_slash() {
        assert_eq!(
            KnowledgePath::new("/etc/passwd"),
            Err(KnowledgePathError::Traversal)
        );
    }

    #[test]
    fn knowledge_path_rejects_trailing_slash() {
        assert_eq!(
            KnowledgePath::new("ops/journal/"),
            Err(KnowledgePathError::Traversal)
        );
    }

    #[test]
    fn knowledge_path_rejects_parent_traversal_segment() {
        assert_eq!(
            KnowledgePath::new("ops/../secrets"),
            Err(KnowledgePathError::Traversal)
        );
    }

    #[test]
    fn knowledge_path_rejects_control_character() {
        assert_eq!(
            KnowledgePath::new("ops/journal\n/x"),
            Err(KnowledgePathError::ControlChar)
        );
    }

    #[test]
    fn knowledge_path_rejects_too_long() {
        let s = "a".repeat(KnowledgePath::MAX_LEN + 1);
        match KnowledgePath::new(s) {
            Err(KnowledgePathError::TooLong { actual }) => {
                assert_eq!(actual, KnowledgePath::MAX_LEN + 1);
            }
            other => panic!("expected TooLong, got {other:?}"),
        }
    }

    #[test]
    fn knowledge_path_serializes_as_bare_string() {
        let p = KnowledgePath::new("ops/journal/2026-04-18").unwrap();
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(s, "\"ops/journal/2026-04-18\"");
        let back: KnowledgePath = serde_json::from_str(&s).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn knowledge_path_deserialize_validates() {
        let invalid = "\"/etc/passwd\"";
        let result: Result<KnowledgePath, _> = serde_json::from_str(invalid);
        assert!(
            result.is_err(),
            "deserialize MUST reject path-traversal input, not silently accept it"
        );
    }

    #[test]
    fn knowledge_path_as_str_and_display_match() {
        let p = KnowledgePath::new("ops/journal/x").unwrap();
        assert_eq!(p.as_str(), "ops/journal/x");
        assert_eq!(format!("{p}"), "ops/journal/x");
        assert_eq!(p.as_ref(), "ops/journal/x");
    }

    // ---- KnowledgeWriteConsistency ----

    #[test]
    fn write_consistency_serde_roundtrip_store_only() {
        let value = KnowledgeWriteConsistency::StoreOnly;
        let s = serde_json::to_string(&value).unwrap();
        assert_eq!(s, "\"store_only\"");
        let back: KnowledgeWriteConsistency = serde_json::from_str(&s).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn write_consistency_serde_roundtrip_await_derived() {
        let value = KnowledgeWriteConsistency::AwaitDerived;
        let s = serde_json::to_string(&value).unwrap();
        assert_eq!(s, "\"await_derived\"");
        let back: KnowledgeWriteConsistency = serde_json::from_str(&s).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn write_consistency_default_is_store_only() {
        // Authority doc §2.2.3 — default chosen for source-of-truth safety.
        assert_eq!(
            KnowledgeWriteConsistency::default(),
            KnowledgeWriteConsistency::StoreOnly,
        );
    }

    #[test]
    fn write_consistency_rejects_unknown_variant() {
        let err = serde_json::from_str::<KnowledgeWriteConsistency>("\"eventually\"")
            .expect_err("unknown variant must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("eventually") || msg.contains("variant"),
            "error must cite the unknown variant; got: {msg}"
        );
    }

    // ---- KnowledgeQueryMode ----

    #[test]
    fn query_mode_wire_strings_are_stable() {
        // The snake_case wire form is part of the `[knowledge]` TOML
        // schema + MCP tool arguments; renaming any variant is a
        // breaking wire change.
        for (mode, wire) in [
            (KnowledgeQueryMode::Keyword, "\"keyword\""),
            (KnowledgeQueryMode::Semantic, "\"semantic\""),
            (KnowledgeQueryMode::Hybrid, "\"hybrid\""),
            (KnowledgeQueryMode::Relations, "\"relations\""),
            (KnowledgeQueryMode::Auto, "\"auto\""),
        ] {
            let s = serde_json::to_string(&mode).unwrap();
            assert_eq!(s, wire, "wire string for {mode:?}");
            let back: KnowledgeQueryMode = serde_json::from_str(wire).unwrap();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn query_mode_rejects_unknown_variant() {
        let err = serde_json::from_str::<KnowledgeQueryMode>("\"telepathic\"")
            .expect_err("unknown mode must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("telepathic") || msg.contains("variant"),
            "error must cite the unknown mode; got: {msg}"
        );
    }

    #[test]
    fn query_mode_default_is_auto() {
        assert_eq!(KnowledgeQueryMode::default(), KnowledgeQueryMode::Auto);
    }

    // ---- KnowledgeHitKind ----

    #[test]
    fn hit_kind_wire_strings_stable() {
        for (kind, wire) in [
            (KnowledgeHitKind::Canonical, "\"canonical\""),
            (KnowledgeHitKind::SearchIndex, "\"search_index\""),
            (KnowledgeHitKind::RelationEdge, "\"relation_edge\""),
        ] {
            let s = serde_json::to_string(&kind).unwrap();
            assert_eq!(s, wire);
            let back: KnowledgeHitKind = serde_json::from_str(wire).unwrap();
            assert_eq!(back, kind);
        }
    }

    // ---- KnowledgeChangeEvent ----

    fn fixture_document(path: &str) -> KnowledgeDocument {
        KnowledgeDocument {
            path: path.to_string(),
            title: Some("Fixture".to_string()),
            markdown: "# Body".to_string(),
            frontmatter: serde_json::json!({ "source": "test" }),
            canonical_plug: plug("llm-wiki"),
            // Pin to a stable wall-clock so roundtrip bytes compare.
            updated_at: DateTime::parse_from_rfc3339("2026-04-18T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    #[test]
    fn change_event_upsert_serde_roundtrip() {
        let event = KnowledgeChangeEvent::Upsert {
            document: fixture_document("notes/home"),
        };
        let s = serde_json::to_string(&event).unwrap();
        // Wire shape uses `"kind": "upsert"` (internally-tagged enum per
        // `#[serde(tag = "kind", rename_all = "snake_case")]`).
        assert!(s.contains("\"kind\":\"upsert\""), "wire: {s}");
        let back: KnowledgeChangeEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn change_event_delete_serde_roundtrip() {
        let deleted_at = DateTime::parse_from_rfc3339("2026-04-18T01:02:03Z")
            .unwrap()
            .with_timezone(&Utc);
        let event = KnowledgeChangeEvent::Delete {
            path: "notes/ghost".to_string(),
            deleted_at,
        };
        let s = serde_json::to_string(&event).unwrap();
        assert!(s.contains("\"kind\":\"delete\""), "wire: {s}");
        let back: KnowledgeChangeEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn change_event_rename_serde_roundtrip() {
        let event = KnowledgeChangeEvent::Rename {
            from: "notes/old".to_string(),
            to: "notes/new".to_string(),
            document: fixture_document("notes/new"),
        };
        let s = serde_json::to_string(&event).unwrap();
        assert!(s.contains("\"kind\":\"rename\""), "wire: {s}");
        let back: KnowledgeChangeEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back, event);
    }

    // ---- KnowledgeQuery / KnowledgeHit ----

    #[test]
    fn query_serde_omits_include_relations_default() {
        // `include_relations: false` is the default — MUST round-trip
        // from an explicit-true toml without flipping to false.
        let q = KnowledgeQuery {
            text: "foo".into(),
            limit: 10,
            mode: KnowledgeQueryMode::Hybrid,
            include_relations: true,
        };
        let s = serde_json::to_string(&q).unwrap();
        let back: KnowledgeQuery = serde_json::from_str(&s).unwrap();
        assert!(back.include_relations);
    }

    #[test]
    fn hit_fields_are_required_on_wire() {
        // Missing `source_plug` MUST fail deserialize — this field is
        // the citation anchor for Penny RAG and cannot default.
        let bad = r#"{
            "path": "notes/home",
            "title": null,
            "snippet": "body",
            "score": 0.5,
            "source_kind": "search_index"
        }"#;
        let res = serde_json::from_str::<KnowledgeHit>(bad);
        assert!(res.is_err(), "missing source_plug must fail: {res:?}");
    }

    // ---- KnowledgeErrorKind wire stability (authority doc §4.1) ----

    #[test]
    fn knowledge_error_kind_display_tokens_stable() {
        assert_eq!(
            format!(
                "{}",
                KnowledgeErrorKind::BackendNotRegistered {
                    plug: "llm-wiki".into(),
                }
            ),
            "backend_not_registered"
        );
        assert_eq!(
            format!(
                "{}",
                KnowledgeErrorKind::BackendUnavailable {
                    plug: "graphify".into(),
                }
            ),
            "backend_unavailable"
        );
        assert_eq!(
            format!(
                "{}",
                KnowledgeErrorKind::DocumentNotFound {
                    path: "notes/home".into(),
                }
            ),
            "document_not_found"
        );
        assert_eq!(
            format!(
                "{}",
                KnowledgeErrorKind::InvalidQuery {
                    reason: "empty".into(),
                }
            ),
            "invalid_query"
        );
        assert_eq!(
            format!(
                "{}",
                KnowledgeErrorKind::DerivedApplyFailed {
                    plug: "semantic-pgvector".into(),
                }
            ),
            "derived_apply_failed"
        );
    }

    #[test]
    fn gadgetron_error_knowledge_wires_through_dispatch_methods() {
        let err = GadgetronError::Knowledge {
            kind: KnowledgeErrorKind::DocumentNotFound {
                path: "notes/ghost".into(),
            },
            message: "missing".into(),
        };
        assert_eq!(err.error_code(), "knowledge_document_not_found");
        assert_eq!(err.http_status_code(), 404);
        assert_eq!(err.error_type(), "invalid_request_error");
        let msg = err.error_message();
        assert!(msg.contains("notes/ghost"), "msg: {msg}");
    }

    #[test]
    fn gadgetron_error_knowledge_backend_unavailable_is_503() {
        let err = GadgetronError::Knowledge {
            kind: KnowledgeErrorKind::BackendUnavailable {
                plug: "graphify".into(),
            },
            message: "runtime rpc timeout".into(),
        };
        assert_eq!(err.http_status_code(), 503);
        assert_eq!(err.error_code(), "knowledge_backend_unavailable");
        assert_eq!(err.error_type(), "server_error");
    }

    #[test]
    fn gadgetron_error_knowledge_invalid_query_is_400() {
        let err = GadgetronError::Knowledge {
            kind: KnowledgeErrorKind::InvalidQuery {
                reason: "max_depth > 10".into(),
            },
            message: "validation".into(),
        };
        assert_eq!(err.http_status_code(), 400);
        assert_eq!(err.error_type(), "invalid_request_error");
    }

    // ---- AuthenticatedContext placeholder invariant ----

    #[test]
    fn authenticated_context_is_zero_sized() {
        assert_eq!(std::mem::size_of::<AuthenticatedContext>(), 0);
    }
}
