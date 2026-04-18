//! `LlmWikiStore` — [`KnowledgeStore`] adapter over the legacy `Wiki`
//! git-backed markdown aggregate.
//!
//! # Role
//!
//! In the W3-KL-1 cutover, the knowledge plane stops talking to `Wiki`
//! directly. `LlmWikiStore` is the **canonical store** that `KnowledgeService`
//! holds an `Arc<dyn KnowledgeStore>` pointer to. All six legacy `wiki.*`
//! gadget surfaces (`list`, `get`, `write`, `delete`, `rename`, `import`)
//! now route through this adapter instead of touching `Wiki` directly.
//!
//! # Error translation
//!
//! The adapter maps every `WikiError` variant to a `GadgetronError::Knowledge
//! { kind: KnowledgeErrorKind::*, .. }` at this boundary per authority doc
//! §2.4.1. `WikiErrorKind::PageNotFound` → `KnowledgeErrorKind::
//! DocumentNotFound`, path/size/credential failures → `InvalidQuery`,
//! git/IO failures → `BackendUnavailable`. This means the knowledge plane
//! surface NEVER leaks `WikiError` / `WikiErrorKind` — those stay inside
//! the llm-wiki implementation.
//!
//! # Plug id
//!
//! Fixed to `"llm-wiki"` per authority doc §2.1.3 registration snippet.
//! `LlmWikiStore::new` returns a `Result` because [`PlugId`] validation
//! fails loudly on bad kebab-case — but `"llm-wiki"` is a compile-time
//! constant so production code never trips the error path.
//!
//! # Preservation of `Wiki`
//!
//! `Wiki` itself is NOT modified: seeds, git autocommit, M5 credential
//! pipeline, path sanitization all stay where they are. The adapter only
//! adds a stable trait surface on top. This is critical for the 576+ wiki
//! tests that still exercise `Wiki` directly — they keep passing.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use gadgetron_core::bundle::PlugId;
use gadgetron_core::error::{GadgetronError, KnowledgeErrorKind, WikiErrorKind};
use gadgetron_core::knowledge::{
    AuthenticatedContext, KnowledgeDocument, KnowledgePutRequest, KnowledgeResult, KnowledgeStore,
    KnowledgeWriteReceipt,
};

use crate::error::WikiError;
use crate::wiki::{parse_page, Wiki};

/// Canonical [`KnowledgeStore`] implementation backed by a git-managed
/// markdown wiki (`Wiki` aggregate in `wiki::store`).
///
/// Cheap to clone (shares one `Arc<Wiki>`). Registration:
///
/// ```ignore
/// ctx.plugs.knowledge_stores.register(
///     PlugId::new("llm-wiki")?,
///     Arc::new(LlmWikiStore::new(cfg)?),
/// );
/// ```
///
/// The `plug_id` is fixed at construction — attempts to rename it require
/// a wire-compat decision because `[knowledge] canonical_store = "llm-wiki"`
/// is documented in every example config.
#[derive(Debug, Clone)]
pub struct LlmWikiStore {
    wiki: Arc<Wiki>,
    plug_id: PlugId,
}

impl LlmWikiStore {
    /// Wrap an already-opened `Wiki`. The plug id is fixed to `"llm-wiki"`.
    pub fn new(wiki: Arc<Wiki>) -> Result<Self, GadgetronError> {
        let plug_id = PlugId::new("llm-wiki").map_err(|e| GadgetronError::Config(e.to_string()))?;
        Ok(Self { wiki, plug_id })
    }

    /// Construct with an explicit plug id — test-only surface for
    /// multi-wiki scenarios (P3 preview). Production wiring MUST use
    /// [`Self::new`] so the `"llm-wiki"` string matches every example
    /// config.
    #[doc(hidden)]
    pub fn with_plug_id(wiki: Arc<Wiki>, plug_id: PlugId) -> Self {
        Self { wiki, plug_id }
    }

    /// Expose the inner `Wiki` — needed by `maintenance::run_reindex` and
    /// by tests that want to bypass the trait layer. Production code
    /// SHOULD NOT use this; go through `KnowledgeService` instead.
    pub fn wiki(&self) -> &Arc<Wiki> {
        &self.wiki
    }

    /// Build a `KnowledgeDocument` from a raw wiki read.
    ///
    /// Parses frontmatter via `wiki::parse_page`. Falls back to
    /// `serde_json::Value::Null` + raw markdown if parsing fails —
    /// downstream indexes can still tokenize the body, and the error is
    /// reported via tracing.
    fn assemble_document(&self, path: &str, raw: String) -> KnowledgeDocument {
        let (frontmatter, body) = match parse_page(&raw) {
            Ok(parsed) => {
                let fm_json =
                    serde_json::to_value(&parsed.frontmatter).unwrap_or(serde_json::Value::Null);
                (fm_json, parsed.body)
            }
            Err(e) => {
                tracing::warn!(
                    target: "llm_wiki_store",
                    page = %path,
                    error = ?e,
                    "wiki frontmatter parse failed; returning raw markdown without frontmatter"
                );
                (serde_json::Value::Null, raw.clone())
            }
        };
        KnowledgeDocument {
            path: path.to_string(),
            title: extract_title(&body),
            markdown: body,
            frontmatter,
            canonical_plug: self.plug_id.clone(),
            updated_at: Utc::now(),
        }
    }
}

/// Pull the first `# H1` from a markdown body as the document title.
/// Returns `None` when the first meaningful line is not a heading.
fn extract_title(body: &str) -> Option<String> {
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            return Some(rest.trim().to_string());
        }
        // First non-empty line is not a heading — give up (don't scan
        // further because that's not how users read titles).
        return None;
    }
    None
}

#[async_trait]
impl KnowledgeStore for LlmWikiStore {
    fn plug_id(&self) -> &PlugId {
        &self.plug_id
    }

    async fn list(&self, _actor: &AuthenticatedContext) -> KnowledgeResult<Vec<String>> {
        // `Wiki::list` is synchronous + fast (<100ms for <10k pages). No
        // `spawn_blocking` needed.
        let entries = self.wiki.list().map_err(wiki_err_to_knowledge)?;
        Ok(entries.into_iter().map(|e| e.name).collect())
    }

    async fn get(
        &self,
        _actor: &AuthenticatedContext,
        path: &str,
    ) -> KnowledgeResult<Option<KnowledgeDocument>> {
        match self.wiki.read(path) {
            Ok(raw) => Ok(Some(self.assemble_document(path, raw))),
            Err(WikiError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => match err.kind_ref() {
                // Invariant per trait docs: a missing path returns
                // `Ok(None)`, not an error. Surface the PathEscape
                // variant as `InvalidQuery` instead.
                Some(WikiErrorKind::PageNotFound { .. }) => Ok(None),
                _ => Err(wiki_err_to_knowledge(err)),
            },
        }
    }

    async fn put(
        &self,
        _actor: &AuthenticatedContext,
        request: KnowledgePutRequest,
    ) -> KnowledgeResult<KnowledgeWriteReceipt> {
        // `create_only` / `overwrite` hints are advisory in W3-KL-1 —
        // the underlying `Wiki::write` always overwrites. Honoring
        // `create_only` requires a `get`-then-`put` race window that the
        // authority doc punts to W3-KL-2 (import/ACL). For now the hints
        // pass through but do not block.
        if request.create_only && self.wiki.read(&request.path).map(|_| true).unwrap_or(false) {
            return Err(knowledge_err(
                KnowledgeErrorKind::InvalidQuery {
                    reason: format!(
                        "create_only=true but path {:?} already exists",
                        request.path
                    ),
                },
                "create_only write collided with existing page",
            ));
        }
        let write_result = self
            .wiki
            .write(&request.path, &request.markdown)
            .map_err(wiki_err_to_knowledge)?;
        Ok(KnowledgeWriteReceipt {
            path: write_result.name,
            canonical_plug: self.plug_id.clone(),
            revision: write_result
                .commit_oid
                .unwrap_or_else(|| "uncommitted".to_string()),
            derived_failures: Vec::new(),
        })
    }

    async fn delete(&self, _actor: &AuthenticatedContext, path: &str) -> KnowledgeResult<()> {
        // `Wiki::delete` is soft (archive-to-_archived/<date>). The
        // `KnowledgeChangeEvent::Delete` fanout in the service covers
        // index cleanup.
        self.wiki.delete(path).map_err(wiki_err_to_knowledge)?;
        Ok(())
    }

    async fn rename(
        &self,
        _actor: &AuthenticatedContext,
        from: &str,
        to: &str,
    ) -> KnowledgeResult<KnowledgeWriteReceipt> {
        let result = self.wiki.rename(from, to).map_err(wiki_err_to_knowledge)?;
        Ok(KnowledgeWriteReceipt {
            path: result.name,
            canonical_plug: self.plug_id.clone(),
            revision: result
                .commit_oid
                .unwrap_or_else(|| "uncommitted".to_string()),
            derived_failures: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// WikiError → GadgetronError::Knowledge translation
//
// The translation table is authoritative for the service boundary: outside
// the `llm-wiki` plug, callers only ever see `KnowledgeErrorKind`.
// ---------------------------------------------------------------------------

fn knowledge_err(kind: KnowledgeErrorKind, message: impl Into<String>) -> GadgetronError {
    GadgetronError::Knowledge {
        kind,
        message: message.into(),
    }
}

fn wiki_err_to_knowledge(err: WikiError) -> GadgetronError {
    match err {
        WikiError::Kind { kind, message } => match kind {
            WikiErrorKind::PageNotFound { path } => {
                knowledge_err(KnowledgeErrorKind::DocumentNotFound { path }, message)
            }
            WikiErrorKind::PathEscape { input } => knowledge_err(
                KnowledgeErrorKind::InvalidQuery {
                    reason: format!("invalid path: {input:?}"),
                },
                message,
            ),
            WikiErrorKind::PageTooLarge { path, bytes, limit } => knowledge_err(
                KnowledgeErrorKind::InvalidQuery {
                    reason: format!("page {path:?} size {bytes} bytes exceeds {limit}-byte limit"),
                },
                message,
            ),
            WikiErrorKind::CredentialBlocked { path, pattern } => knowledge_err(
                KnowledgeErrorKind::InvalidQuery {
                    reason: format!(
                        "credential pattern {pattern:?} detected in {path:?}; write refused"
                    ),
                },
                message,
            ),
            WikiErrorKind::Conflict { path } => knowledge_err(
                KnowledgeErrorKind::BackendUnavailable {
                    plug: "llm-wiki".into(),
                },
                format!("git conflict on path {path:?} ({message})"),
            ),
            WikiErrorKind::GitCorruption { path, reason } => knowledge_err(
                KnowledgeErrorKind::BackendUnavailable {
                    plug: "llm-wiki".into(),
                },
                format!("git backend unavailable for {path:?} ({reason}): {message}"),
            ),
            // `WikiErrorKind` is `#[non_exhaustive]`; future variants land
            // as `BackendUnavailable` by default so the llm-wiki plug surface
            // fails closed rather than leaking an uncategorized error.
            _ => knowledge_err(
                KnowledgeErrorKind::BackendUnavailable {
                    plug: "llm-wiki".into(),
                },
                format!("unclassified wiki error: {message}"),
            ),
        },
        WikiError::Io(io) => knowledge_err(
            KnowledgeErrorKind::BackendUnavailable {
                plug: "llm-wiki".into(),
            },
            format!("wiki filesystem error: {io}"),
        ),
        WikiError::Frontmatter(msg) => knowledge_err(
            KnowledgeErrorKind::InvalidQuery {
                reason: format!("frontmatter parse failed: {msg}"),
            },
            msg,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WikiConfig;
    use tempfile::TempDir;

    fn fresh_store() -> (TempDir, LlmWikiStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "Test".into(),
            git_author_email: "test@example.local".into(),
            max_page_bytes: 1024 * 1024,
        };
        let wiki = Arc::new(Wiki::open(cfg).expect("open"));
        let store = LlmWikiStore::new(wiki).expect("construct");
        (dir, store)
    }

    #[test]
    fn plug_id_is_llm_wiki() {
        let (_dir, store) = fresh_store();
        assert_eq!(store.plug_id().as_str(), "llm-wiki");
    }

    #[tokio::test]
    async fn put_then_get_roundtrips_markdown_and_title() {
        let (_dir, store) = fresh_store();
        let actor = AuthenticatedContext;

        let receipt = store
            .put(
                &actor,
                KnowledgePutRequest {
                    path: "home".into(),
                    markdown: "# Home\n\nBody text.".into(),
                    create_only: false,
                    overwrite: false,
                },
            )
            .await
            .expect("put");
        assert_eq!(receipt.path, "home");
        assert_eq!(receipt.canonical_plug.as_str(), "llm-wiki");
        assert!(!receipt.revision.is_empty());
        assert!(receipt.derived_failures.is_empty());

        let doc = store
            .get(&actor, "home")
            .await
            .expect("get ok")
            .expect("doc present");
        assert_eq!(doc.path, "home");
        assert_eq!(doc.title.as_deref(), Some("Home"));
        assert!(doc.markdown.contains("Body text"));
        assert_eq!(doc.canonical_plug.as_str(), "llm-wiki");
    }

    #[tokio::test]
    async fn get_missing_page_returns_ok_none_not_error() {
        // Trait invariant: missing path = `Ok(None)`, not
        // `DocumentNotFound` error. This preserves the ability to
        // distinguish "absent" from "backend down" without string
        // matching on error messages.
        let (_dir, store) = fresh_store();
        let actor = AuthenticatedContext;
        let res = store.get(&actor, "absent-page").await.expect("ok none");
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn put_create_only_rejects_existing_path() {
        let (_dir, store) = fresh_store();
        let actor = AuthenticatedContext;
        store
            .put(
                &actor,
                KnowledgePutRequest {
                    path: "home".into(),
                    markdown: "# v1".into(),
                    create_only: false,
                    overwrite: false,
                },
            )
            .await
            .expect("first write");

        let err = store
            .put(
                &actor,
                KnowledgePutRequest {
                    path: "home".into(),
                    markdown: "# v2".into(),
                    create_only: true,
                    overwrite: false,
                },
            )
            .await
            .expect_err("create_only collision");

        match err {
            GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::InvalidQuery { reason },
                ..
            } => assert!(reason.contains("create_only"), "reason: {reason}"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn put_rejects_page_too_large_as_invalid_query() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "T".into(),
            git_author_email: "t@t.local".into(),
            max_page_bytes: 10,
        };
        let wiki = Arc::new(Wiki::open(cfg).unwrap());
        let store = LlmWikiStore::new(wiki).unwrap();
        let actor = AuthenticatedContext;

        let err = store
            .put(
                &actor,
                KnowledgePutRequest {
                    path: "big".into(),
                    markdown: "x".repeat(100),
                    create_only: false,
                    overwrite: false,
                },
            )
            .await
            .expect_err("too large");
        match err {
            GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::InvalidQuery { reason },
                ..
            } => {
                assert!(reason.contains("100"));
                assert!(reason.contains("10"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn put_rejects_credential_block() {
        let (_dir, store) = fresh_store();
        let actor = AuthenticatedContext;
        let err = store
            .put(
                &actor,
                KnowledgePutRequest {
                    path: "secret".into(),
                    markdown:
                        "-----BEGIN RSA PRIVATE KEY-----\nbody\n-----END RSA PRIVATE KEY-----"
                            .into(),
                    create_only: false,
                    overwrite: false,
                },
            )
            .await
            .expect_err("credential blocked");
        match err {
            GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::InvalidQuery { reason },
                ..
            } => assert!(reason.contains("pem_private_key"), "reason: {reason}"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_missing_page_translates_to_document_not_found() {
        let (_dir, store) = fresh_store();
        let actor = AuthenticatedContext;
        let err = store.delete(&actor, "ghost").await.expect_err("not found");
        match err {
            GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::DocumentNotFound { path },
                ..
            } => assert_eq!(path, "ghost"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn rename_missing_from_translates_to_document_not_found() {
        let (_dir, store) = fresh_store();
        let actor = AuthenticatedContext;
        let err = store
            .rename(&actor, "ghost", "home")
            .await
            .expect_err("missing from");
        match err {
            GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::DocumentNotFound { .. },
                ..
            } => (),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_returns_seed_pages() {
        let (_dir, store) = fresh_store();
        let actor = AuthenticatedContext;
        let paths = store.list(&actor).await.expect("list");
        assert!(!paths.is_empty(), "fresh wiki must surface seed pages");
    }

    #[test]
    fn extract_title_handles_non_heading_first_line() {
        assert_eq!(extract_title("# Hello\nrest"), Some("Hello".into()));
        assert_eq!(extract_title("\n\n# Pad"), Some("Pad".into()));
        assert_eq!(extract_title("Prose first, then body"), None);
        assert_eq!(extract_title(""), None);
    }
}
