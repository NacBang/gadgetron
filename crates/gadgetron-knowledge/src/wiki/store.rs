//! `Wiki` aggregate — orchestrates `fs::resolve_path`, `git::autocommit`,
//! `secrets::check_block_patterns`, and `index::InvertedIndex` into a
//! single read/write/list/search surface.
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §4.1 + §4.3 + §4.4`.
//!
//! # Thread-safety
//!
//! `git2::Repository` is `Send` but **not** `Sync` — it is an opaque handle
//! over a mutable C struct. To keep `Wiki` as `Sync` (required by the
//! `McpToolProvider: Send + Sync + 'static` bound), this type does NOT
//! hold the repository. Instead, every mutating operation re-opens the
//! repo at the configured root. libgit2's open path is ~1ms and bounded
//! by stat(2), so for P2A's single-user rate this is well under noise.
//!
//! When multi-user P2C lands, a sharded repo pool (N workers, each with
//! a dedicated `Mutex<Repository>`) can replace the re-open — same public
//! API.

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::config::WikiConfig;
use crate::error::WikiError;
use gadgetron_core::error::WikiErrorKind;

use super::git::{autocommit, open_or_init, signature};
use super::index::{InvertedIndex, WikiSearchHit};

const MD_SUFFIX: &str = ".md";

/// Result of a successful `Wiki::write` call.
#[derive(Debug, Clone)]
pub struct WriteResult {
    /// Page name as the operator supplied it.
    pub name: String,
    /// Oid of the new commit, `None` when `autocommit` is disabled.
    pub commit_oid: Option<String>,
    /// Bytes written to disk.
    pub bytes: usize,
}

/// Single wiki page listing entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WikiListEntry {
    /// Relative page name without the `.md` suffix. Uses forward slashes
    /// for subdirectories.
    pub name: String,
}

/// Read/write orchestrator for a git-backed markdown wiki.
///
/// Holds only the configuration — the `git2::Repository` is opened per
/// operation. See module docs for the thread-safety rationale.
#[derive(Debug, Clone)]
pub struct Wiki {
    config: WikiConfig,
}

impl Wiki {
    /// Open or initialize the wiki at `config.root`.
    ///
    /// Creates the root directory if absent. Runs `git init` + an empty
    /// initial commit on first open via `git::open_or_init`.
    pub fn open(config: WikiConfig) -> Result<Self, WikiError> {
        if !config.root.exists() {
            fs::create_dir_all(&config.root)?;
        }
        // Run once to force creation + initial commit. Dropped immediately.
        let _ = open_or_init(
            &config.root,
            &config.git_author_name,
            &config.git_author_email,
        )?;
        Ok(Self { config })
    }

    pub fn config(&self) -> &WikiConfig {
        &self.config
    }

    /// List every `.md` page under the wiki root. Recursive. Output is
    /// sorted by name for deterministic tool replies.
    pub fn list(&self) -> Result<Vec<WikiListEntry>, WikiError> {
        let mut out = Vec::new();
        walk_markdown(&self.config.root, &self.config.root, &mut out)?;
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Read a wiki page by logical name. Name is sanitized through
    /// `fs::resolve_path` (defeats path traversal).
    ///
    /// Returns the raw markdown string.
    pub fn read(&self, name: &str) -> Result<String, WikiError> {
        let canonical = super::fs::resolve_path(&self.config.root, name)?;
        fs::read_to_string(&canonical).map_err(WikiError::Io)
    }

    /// Write or overwrite a wiki page. Enforces the M5 7-layer pipeline
    /// from `01 v3 §4.4`:
    ///
    /// 1. Size cap — short-circuits adversarial payloads cheaply.
    /// 2. BLOCK patterns (PEM / AKIA / GCP) — refuse with
    ///    `CredentialBlocked`; the write NEVER touches disk.
    /// 3. AUDIT patterns — `tracing::warn!` per match, do NOT block.
    /// 4. Path resolution — M3 guard via `fs::resolve_path`.
    /// 5. Create parent directories (only AFTER M3 passes).
    /// 6. Open with `O_NOFOLLOW` to defeat symlink swap at the final
    ///    component (unix only).
    /// 7. Auto-commit — if `config.autocommit == true`, stage + commit
    ///    with an abstract message via `git::autocommit`.
    pub fn write(&self, name: &str, content: &str) -> Result<WriteResult, WikiError> {
        // Layer 1 — size cap
        if content.len() > self.config.max_page_bytes {
            return Err(WikiError::kind(WikiErrorKind::PageTooLarge {
                path: name.to_string(),
                bytes: content.len(),
                limit: self.config.max_page_bytes,
            }));
        }

        // Layer 2 — BLOCK patterns
        if let Some(first) = super::secrets::check_block_patterns(content)
            .into_iter()
            .next()
        {
            return Err(WikiError::kind(WikiErrorKind::CredentialBlocked {
                path: name.to_string(),
                pattern: first.pattern_name.to_string(),
            }));
        }

        // Layer 3 — AUDIT patterns (non-blocking)
        for m in super::secrets::check_audit_patterns(content) {
            tracing::warn!(
                target: "wiki_audit",
                pattern = %m.pattern_name,
                page = %name,
                "wiki_write_secret_suspected"
            );
        }

        // Layer 4 — path resolution (M3)
        let canonical = super::fs::resolve_path(&self.config.root, name)?;

        // Layer 5 — create parent directories only AFTER M3 passes
        if let Some(parent) = canonical.parent() {
            fs::create_dir_all(parent).map_err(WikiError::Io)?;
        }

        // Layer 6 — open with O_NOFOLLOW on unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&canonical)
                .map_err(WikiError::Io)?;
            file.write_all(content.as_bytes()).map_err(WikiError::Io)?;
            file.flush().map_err(WikiError::Io)?;
        }
        #[cfg(not(unix))]
        {
            fs::write(&canonical, content.as_bytes()).map_err(WikiError::Io)?;
        }

        // Layer 7 — auto-commit
        let commit_oid = if self.config.autocommit {
            let repo = open_or_init(
                &self.config.root,
                &self.config.git_author_name,
                &self.config.git_author_email,
            )?;
            let sig = signature(&self.config.git_author_name, &self.config.git_author_email)?;
            let rel = canonical
                .strip_prefix(&self.config.root)
                .map_err(|_| {
                    WikiError::kind_with_message(
                        WikiErrorKind::GitCorruption {
                            path: name.to_string(),
                            reason: "resolved path escapes wiki root after write".into(),
                        },
                        format!("wiki_path escape after write for {name:?}"),
                    )
                })?
                .to_path_buf();
            Some(autocommit(&repo, &rel, &sig)?.to_string())
        } else {
            None
        };

        Ok(WriteResult {
            name: name.to_string(),
            commit_oid,
            bytes: content.len(),
        })
    }

    /// Full-text search across all pages. Rebuilds the inverted index on
    /// every call — P2A scale (<10k pages) makes this ~20-50 ms which is
    /// well below tool-call latency budget. Future P2B can cache under a
    /// `Mutex<Option<InvertedIndex>>` invalidated on write.
    pub fn search(&self, query: &str, max_results: usize) -> Result<Vec<WikiSearchHit>, WikiError> {
        let entries = self.list()?;
        let mut idx = InvertedIndex::new();
        for entry in entries {
            // Ignore per-page read errors — one unreadable page shouldn't
            // nuke the whole search. Log via tracing for diagnostics.
            match self.read(&entry.name) {
                Ok(content) => idx.add_page(entry.name, &content),
                Err(e) => {
                    tracing::warn!(
                        target: "wiki_search",
                        page = %entry.name,
                        error = ?e,
                        "skipping unreadable page during search index build"
                    );
                }
            }
        }
        Ok(idx.search(query, max_results))
    }
}

/// Recursively walk `dir` collecting `.md` files into `out`. Entry names
/// are computed relative to `root` and trimmed of the `.md` suffix.
fn walk_markdown(root: &Path, dir: &Path, out: &mut Vec<WikiListEntry>) -> Result<(), WikiError> {
    // Skip the .git directory specifically — it's not user content.
    let is_git = dir.file_name().map(|n| n == ".git").unwrap_or(false);
    if is_git {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(WikiError::Io)? {
        let entry = entry.map_err(WikiError::Io)?;
        let path = entry.path();
        if path.is_dir() {
            walk_markdown(root, &path, out)?;
            continue;
        }
        let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !fname.ends_with(MD_SUFFIX) {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| {
                WikiError::kind_with_message(
                    WikiErrorKind::GitCorruption {
                        path: path.display().to_string(),
                        reason: "walked path escapes wiki root".into(),
                    },
                    "wiki walk produced a path outside the configured root",
                )
            })?
            .to_path_buf();
        let rel_str = rel.to_string_lossy().into_owned();
        let name = rel_str
            .strip_suffix(MD_SUFFIX)
            .unwrap_or(&rel_str)
            .to_string();
        out.push(WikiListEntry { name });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_wiki() -> (TempDir, Wiki) {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "Test".into(),
            git_author_email: "test@example.local".into(),
            max_page_bytes: 1024 * 1024,
        };
        let wiki = Wiki::open(cfg).expect("open");
        (dir, wiki)
    }

    // ---- write ----

    #[test]
    fn write_happy_path_creates_file_and_commit() {
        let (_dir, wiki) = fresh_wiki();
        let result = wiki.write("home", "# Home\n\nContent.").expect("write");
        assert_eq!(result.name, "home");
        assert_eq!(result.bytes, "# Home\n\nContent.".len());
        assert!(result.commit_oid.is_some());
        // File is on disk.
        let content = wiki.read("home").expect("read back");
        assert_eq!(content, "# Home\n\nContent.");
    }

    #[test]
    fn write_rejects_page_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "T".into(),
            git_author_email: "t@t.local".into(),
            max_page_bytes: 100,
        };
        let wiki = Wiki::open(cfg).unwrap();
        let content = "x".repeat(101);
        let err = wiki.write("big", &content).expect_err("too large");
        match err.kind_ref() {
            Some(WikiErrorKind::PageTooLarge { bytes, limit, .. }) => {
                assert_eq!(*bytes, 101);
                assert_eq!(*limit, 100);
            }
            other => panic!("wrong kind: {other:?}"),
        }
    }

    #[test]
    fn write_rejects_pem_private_key() {
        let (_dir, wiki) = fresh_wiki();
        let content = "before\n-----BEGIN RSA PRIVATE KEY-----\nbody\n-----END\n";
        let err = wiki.write("leaked", content).expect_err("block");
        match err.kind_ref() {
            Some(WikiErrorKind::CredentialBlocked { pattern, .. }) => {
                assert_eq!(pattern, "pem_private_key");
            }
            other => panic!("wrong kind: {other:?}"),
        }
    }

    #[test]
    fn write_rejects_aws_access_key() {
        let (_dir, wiki) = fresh_wiki();
        let content = "aws key: AKIAIOSFODNN7EXAMPLE in notes";
        let err = wiki.write("aws", content).expect_err("block");
        assert!(matches!(
            err.kind_ref(),
            Some(WikiErrorKind::CredentialBlocked { .. })
        ));
    }

    #[test]
    fn write_rejects_path_traversal() {
        let (_dir, wiki) = fresh_wiki();
        let err = wiki.write("../escape", "malicious").expect_err("rejected");
        assert!(matches!(
            err.kind_ref(),
            Some(WikiErrorKind::PathEscape { .. })
        ));
    }

    #[test]
    fn write_nested_name_creates_parent_dir() {
        let (_dir, wiki) = fresh_wiki();
        wiki.write("notes/2026/Q2", "quarterly review")
            .expect("write");
        let content = wiki.read("notes/2026/Q2").expect("read");
        assert_eq!(content, "quarterly review");
    }

    #[test]
    fn write_with_autocommit_disabled_leaves_file_staged_but_not_committed() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: false,
            git_author_name: "T".into(),
            git_author_email: "t@t.local".into(),
            max_page_bytes: 1024,
        };
        let wiki = Wiki::open(cfg).unwrap();
        let result = wiki.write("page", "content").unwrap();
        assert!(result.commit_oid.is_none());
        assert_eq!(wiki.read("page").unwrap(), "content");
    }

    #[test]
    fn write_audit_patterns_do_not_block() {
        // Anthropic API key is AUDIT-only → write succeeds with a warn log.
        let (_dir, wiki) = fresh_wiki();
        let content = "note: sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ABCDEFGH";
        let result = wiki.write("audit", content);
        assert!(result.is_ok(), "audit pattern must not block: {result:?}");
    }

    // ---- list ----

    #[test]
    fn list_empty_wiki_is_empty() {
        let (_dir, wiki) = fresh_wiki();
        assert!(wiki.list().unwrap().is_empty());
    }

    #[test]
    fn list_returns_sorted_page_names() {
        let (_dir, wiki) = fresh_wiki();
        wiki.write("zebra", "z").unwrap();
        wiki.write("apple", "a").unwrap();
        wiki.write("mango", "m").unwrap();
        let entries = wiki.list().unwrap();
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["apple", "mango", "zebra"]
        );
    }

    #[test]
    fn list_includes_nested_pages_with_forward_slashes() {
        let (_dir, wiki) = fresh_wiki();
        wiki.write("notes/2026/Q2", "a").unwrap();
        wiki.write("home", "b").unwrap();
        let names: Vec<String> = wiki.list().unwrap().into_iter().map(|e| e.name).collect();
        assert!(names.contains(&"home".to_string()));
        assert!(names.contains(&"notes/2026/Q2".to_string()));
    }

    #[test]
    fn list_skips_dot_git_directory() {
        let (_dir, wiki) = fresh_wiki();
        // The .git/HEAD file should not appear as a page.
        let entries = wiki.list().unwrap();
        assert!(
            entries.iter().all(|e| !e.name.contains(".git")),
            ".git should be skipped: {entries:?}"
        );
    }

    // ---- search ----

    #[test]
    fn search_returns_relevant_pages() {
        let (_dir, wiki) = fresh_wiki();
        wiki.write("meeting-notes", "quarterly review with the team")
            .unwrap();
        wiki.write("grocery-list", "milk, bread, eggs").unwrap();
        let hits = wiki.search("quarterly", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "meeting-notes");
    }

    #[test]
    fn search_respects_max_results() {
        let (_dir, wiki) = fresh_wiki();
        for i in 1..=5 {
            wiki.write(&format!("page{i}"), "shared keyword").unwrap();
        }
        let hits = wiki.search("keyword", 3).unwrap();
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let (_dir, wiki) = fresh_wiki();
        wiki.write("home", "some content").unwrap();
        assert!(wiki.search("", 10).unwrap().is_empty());
    }
}
