# 01 — Knowledge Layer Detailed Implementation Spec (`gadgetron-knowledge`)

> **Status**: Draft v3 (addressed chief-architect + dx + security + qa Round 1 feedback; Round 2 review addressed (4 reviewers cross-check)) — **§4 search subsystem extended by `05-knowledge-semantic.md` (ADR-P2A-07) as of 2026-04-16**
> **Author**: PM (Claude)
> **Date**: 2026-04-13 (v3), 2026-04-16 (§4 semantic extension pointer)
> **Parent**: `docs/design/phase2/00-overview.md` v2 (APPROVED)
> **Scope**: `gadgetron-knowledge` crate (`KnowledgeToolProvider` implementation) + `gadgetron-core::error::GadgetronError::Wiki` variant + reference-level documentation for the MCP protocol surface the knowledge provider exposes. The stdio MCP server transport is **hosted in `gadgetron-penny::mcp_server`**, not in this crate — see `02-penny-agent.md §5` and `04-mcp-tool-registry.md §2.1` for the authoritative transport implementation. References below to `gadgetron penny init` are retained as **future bootstrap UX debt**, not current trunk CLI contract. `gadgetron-penny` LlmProvider + subprocess is `02-penny-agent.md`.
> **Implementation determinism**: every type signature, config field, error code, and test name is explicit. No TBD.

## Table of Contents

1. Scope & Non-Scope
   1.1 Bootstrap UX stdout contract (future)
2. Crate layout & Cargo.toml
3. Public API surface (`lib.rs`)
4. Wiki subsystem
5. Web search subsystem
6. MCP server
7. Configuration
8. Error types (`WikiErrorKind` in core, local wrapper)
9. Security enforcement mapping
10. Testing strategy
11. Open items / 02 handoff
12. Review provenance

---

## 1. Scope & Non-Scope

### In scope
- `gadgetron-knowledge` crate: wiki store, web search proxy, `KnowledgeToolProvider` (implements `gadgetron_core::agent::tools::McpToolProvider`)
- **`gadgetron-core::error::GadgetronError::Wiki { kind: WikiErrorKind, message: String }` variant** (moved here from 02 per dx A6 and chief-architect B1)
- `gadgetron-cli` gains `gadgetron mcp serve` subcommand which delegates to `gadgetron_penny::serve_stdio`, serving a frozen `McpToolRegistry` assembled from `KnowledgeToolProvider` + future providers
- future bootstrap UX fills the gap currently covered by manual `gadgetron.toml` authoring
- Configuration schema `[knowledge]` and `[knowledge.search]` sections in `gadgetron.toml`

### Out of scope — deferred to `02-penny-agent.md`
- `gadgetron-penny` crate: `LlmProvider` impl, Claude Code subprocess management, stream-json → OpenAI SSE translation
- `GadgetronError::Penny { kind: PennyErrorKind, .. }` variant in core
- `redact_stderr()` implementation (subprocess stderr — 02)
- `--allowed-tools` enforcement verification (M4) — blocking for 02

### Preconditions locked from `00-overview.md` v2
- Architecture: penny implements `LlmProvider` via router provider map
- Error taxonomy: `WikiErrorKind` nested variant (this spec adds to core)
- Security mitigations: M1 (tmpfile — 02), M2 (redact — 02), M3 (path traversal), M5 (size + pattern block/audit + git history), M6 (tools_called names), M7 (SearXNG + git permanence disclosures), M8 (P2A risk acceptance)
- OSS stack: `git2`, `pulldown-cmark`, `gray_matter` + `toml`, `reqwest`, `regex`, `once_cell`, plus a manual JSON-RPC 2.0 MCP transport implemented in `gadgetron-penny::mcp_server` (see §6.1 — `rmcp` integration deferred to P2B+)

### 1.1 Bootstrap UX stdout contract (future, dx A4 blocker)

A future bootstrap subcommand or equivalent guided init flow MUST print the following literal output sequence. Any deviation is an implementation bug.

**Success path (all checks pass):**
```
Gadgetron Penny init — bootstrapping personal assistant workspace

  [OK] Claude Code binary: /usr/local/bin/claude (version 2.0.x)
  [OK] git: /usr/bin/git (version 2.40.x)
  [OK] Git identity: Jane Doe <jane@example.com> (from git config)
  [OK] Wiki directory: /Users/jane/.gadgetron/wiki (created)
  [OK] Git repository: initialized with initial commit
  [OK] Starter page: wiki/README.md
  [OK] Config file: /Users/jane/.gadgetron/gadgetron.toml (written)

Next steps:

  1. (Optional) Start a local SearXNG instance for web.search:
     Run SearXNG via Docker or native install, then set
     [knowledge.search].searxng_url in gadgetron.toml.

  2. Create a Gadgetron API key:
     gadgetron key create --scope open_ai_compat

  3. Start Gadgetron (single binary — serves /v1 API and /web UI):
     gadgetron serve --config ~/.gadgetron/gadgetron.toml

  4. Browse to http://localhost:8080/web (gadgetron-web — assistant-ui)
     Open Settings, paste the API key, pick model "penny", start chatting.

Done.
```

**Failure path — Claude Code not found (exit 2):**
```
Error: Claude Code CLI (`claude`) was not found on PATH.

Penny requires Claude Code to be installed and logged in (via Claude Max
subscription). Install it from:

    https://docs.anthropic.com/en/docs/claude-code/overview

After installation, run `claude login` and retry `gadgetron penny init`.
```

**Failure path — git config not set (warning, not error):**
```
  ...
  [WARN] git config user.name/user.email not set
         Falling back to "Penny <penny@gadgetron.local>" for wiki commits.
         To override: set wiki_git_author in ~/.gadgetron/gadgetron.toml,
         or run: git config --global user.name "Your Name"
                 git config --global user.email "you@example.com"
  ...
```

The warning is printed via `eprintln!`, NOT `tracing::warn!` alone (per dx A5). A structured `tracing::warn!` event is also emitted for log aggregators.

**Failure path — wiki_path not writable (exit 2):**
```
Error: Wiki directory is not writable: /opt/readonly/wiki
Permission denied (os error 13).

Fix: choose a different path with `gadgetron penny init --wiki-path <PATH>`,
or ensure the target directory is writable by the current user.
```

**`--docker` flag**: **Deferred to P2B per D-20260414-02 + ADR-P2A-04 (DX-W-B3 resolution Option C)**. In P2A, `penny init --docker` prints the following warning to stderr, exits 0, and does not write any file:

```
[WARN] --docker is not supported in P2A.
       OpenWebUI sibling process was removed (D-20260414-02); the Web UI is now
       embedded in the gadgetron binary and served at http://localhost:8080/web.
       SearXNG (if you want web.search) should be started manually:
           docker run -d --rm --name searxng -p 127.0.0.1:8888:8080 searxng/searxng
       Then set [knowledge.search].searxng_url in ~/.gadgetron/gadgetron.toml.
       --docker will be re-introduced in P2B as SearXNG-only mode if needed.
```

Exit code is **0** (graceful deprecation, not an error). A next-session coder writing a test for `penny init --docker` asserts the exact stderr content above and exit code 0.

**`--wiki-path <PATH>` flag**

- `--wiki-path <PATH>` overrides the default wiki location (`~/.gadgetron/wiki`).
- If an existing `gadgetron.toml` already specifies `[knowledge] wiki_path`, the
  `--wiki-path` CLI value WINS (it overrides the config file).
- The written `gadgetron.toml` always persists the resolved path; re-running
  `penny init --wiki-path /new/path` rewrites the config.
- The success-path `[OK] Wiki directory: …` line always shows the resolved
  absolute path (after symlink resolution via `std::fs::canonicalize`).
- If `--wiki-path` points to an existing non-empty directory that is not a git
  repo, the command fails with exit code 3 and message "Wiki directory exists
  but is not a git repository. Use `--force-init-git` or choose a different path."

Note: `gadgetron doctor` (Phase 1) uses lowercase `[ok]` / `[FAIL]`. `penny init` uses uppercase `[OK]` / `[WARN]` / `[FAIL]` for scanability alongside `[WARN]`. Both forms are intentional; `docs/manual/troubleshooting.md` documents the distinction.

---

## 2. Crate layout & Cargo.toml

### Workspace member addition
```toml
[workspace]
members = [
    # existing ...
    "crates/gadgetron-knowledge",
]
```

### `crates/gadgetron-knowledge/Cargo.toml`

```toml
[package]
name = "gadgetron-knowledge"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
# Workspace
gadgetron-core = { path = "../gadgetron-core" }

# Wiki storage
git2 = "0.19"                                       # libgit2 Rust binding; libgit2 version pinned via lock
pulldown-cmark = { version = "0.12", default-features = false }
gray_matter = { version = "0.2", default-features = false, features = ["toml"] }
walkdir = "2.5"

# Unix file flags for O_NOFOLLOW on write
nix = { version = "0.29", features = ["fs"], optional = true }

# HTTP / JSON / async
tokio = { workspace = true, features = ["full"] }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
reqwest = { workspace = true, features = ["json"] }
# url::Url: parsed SearchConfig.searxng_url. If `url` is not yet in workspace
# `[workspace.dependencies]`, add `url = "2"` there during implementation OR
# switch this line to a crate-local `url = "2"`.
url = { workspace = true }
async-trait = { workspace = true }
futures = { workspace = true }

# Regex + lazy init for M5 credential patterns
regex = "1"
once_cell = "1"

# Date/time for frontmatter
chrono = { workspace = true, features = ["serde"] }

# Error handling
thiserror = { workspace = true }

# Tracing
tracing = { workspace = true }

# Frontmatter TOML parser (not serde_yaml — archived 2024)
toml = { version = "0.8", features = ["parse"] }

[features]
default = ["unix-fs"]
unix-fs = ["dep:nix"]

[dev-dependencies]
tempfile = "3"
proptest = "1"
insta = { version = "1", features = ["yaml"] }
tokio = { workspace = true, features = ["full", "test-util"] }
tracing-test = "0.2"
```

### Module tree

```
crates/gadgetron-knowledge/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── config.rs
│   ├── error.rs         — local WikiError + SearchError; conversion into core GadgetronError::Wiki
│   ├── wiki/
│   │   ├── mod.rs       — Wiki struct (holds Mutex<Option<InvertedIndex>>)
│   │   ├── fs.rs        — resolve_path (pure validation; no side effects)
│   │   ├── git.rs       — open_or_init repo, autocommit with inline SECURITY comment
│   │   ├── link.rs      — parse_links (Obsidian [[link]])
│   │   ├── frontmatter.rs — gray_matter + toml wrapper
│   │   ├── index.rs     — HashMap-based inverted index (lock inside Wiki)
│   │   └── secrets.rs   — credential patterns (3 BLOCK + 2 audit-only)
│   ├── search/
│   │   ├── mod.rs       — WebSearch trait + SearchResult
│   │   └── searxng.rs   — SearxngClient (redirect::limited(3), P2C tag)
│   └── mcp/
│       ├── mod.rs       — serve_stdio entry + rmcp wiring (with manual fallback outline)
│       ├── tools.rs     — dispatch + error mapping (wiki_error_to_tool_result + search_error_to_tool_result)
│       └── schema.rs    — 5 JSON Schema constants
└── tests/
    ├── mcp_conformance.rs
    ├── wiki_git_recovery.rs
    ├── wiki_large_files.rs
    ├── wiki_secret_patterns.rs
    ├── path_traversal_proptest.rs
    ├── fixtures/
    │   ├── searxng_response.json
    │   ├── searxng_empty_response.json
    │   ├── searxng_malformed.json
    │   └── searxng_error_response.json
    └── snapshots/
```

---

## 3. Public API surface (`lib.rs`)

```rust
//! gadgetron-knowledge — knowledge layer for Penny personal assistant.
//!
//! Provides: wiki store (md+git), web search proxy (SearXNG), MCP server (stdio).
//! Consumers: gadgetron-penny (Claude Code subprocess), gadgetron-cli (`mcp serve` subcommand).

#![warn(missing_docs)]

pub mod config;
pub mod error;
pub mod wiki;
pub mod search;
pub mod mcp;

// Re-exports
pub use config::{KnowledgeConfig, SearchConfig};
pub use error::{WikiError, SearchError};
// WikiErrorKind is in gadgetron-core, re-exported here for convenience
pub use gadgetron_core::error::WikiErrorKind;

pub use wiki::{Wiki, WikiConfig, WikiPage, WikiPageMeta, WikiFrontmatter, WikiLink, WikiSearchHit};
pub use search::{WebSearch, SearchResult, SearxngClient};

pub use mcp::serve_stdio;
```

**Public symbols:**
- `KnowledgeConfig`, `SearchConfig`
- `WikiError`, `SearchError`, `WikiErrorKind` (re-exported from core)
- `Wiki`, `WikiConfig`, `WikiPage`, `WikiPageMeta`, `WikiFrontmatter`, `WikiLink`, `WikiSearchHit`
- `WebSearch` (trait), `SearchResult`, `SearxngClient`
- `serve_stdio` (MCP server entry)

Everything else crate-private.

---

## 4. Wiki subsystem

> **Extended by `05-knowledge-semantic.md`** (ADR-P2A-07, 2026-04-16):
> keyword-only inverted index search is superseded by hybrid semantic + ts_rank search (pgvector + embedding provider abstraction). TOML frontmatter, reindex job, and AI auto-extraction pipeline are added in P2A. This section's original search algorithm remains valid for historical context; the production implementation follows `05-knowledge-semantic.md §9` (Search path) going forward.

### 4.1 Core types

```rust
use crate::error::WikiError;
use gadgetron_core::error::WikiErrorKind;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

pub struct Wiki {
    config: WikiConfig,
    repo: git2::Repository,
    // Lazily built; rebuilt on every `write()`. Lock serialises rebuild + search.
    index: Mutex<Option<crate::wiki::index::InvertedIndex>>,
}

#[derive(Debug, Clone)]
pub struct WikiConfig {
    pub root: PathBuf,                  // canonical absolute path
    pub autocommit: bool,
    pub git_author_name: String,
    pub git_author_email: String,
    pub max_page_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct WikiPage {
    pub name: String,
    pub path: PathBuf,
    pub frontmatter: WikiFrontmatter,
    pub body: String,
    pub modified: SystemTime,
}

#[derive(Debug, Clone)]
pub struct WikiPageMeta {
    pub name: String,
    pub path: PathBuf,
    pub title: Option<String>,
    pub modified: SystemTime,
    pub byte_size: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WikiFrontmatter {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub created: Option<DateTime<Utc>>,
    #[serde(flatten)]
    pub extra: HashMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WikiLink {
    pub target: String,
    pub alias: Option<String>,
    pub heading: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WikiSearchHit {
    pub name: String,
    pub score: f32,
    pub snippet: String,
}
```

### 4.2 Path resolution — M3 enforcement (pure function, no side effects)

**Security critical.** Chief-architect B2 + security A2: `resolve_path` is a PURE validation function. It does NOT call `create_dir_all` or any other side-effecting operation. All filesystem state mutation happens in the caller (`Wiki::write`).

```rust
//! Path resolution for wiki pages. Enforces M3 (path traversal) per 00-overview §8.
//!
//! CRITICAL: This function is pure — no filesystem mutation. The `write` caller
//! is responsible for creating parent directories AFTER validation passes.
//!
//! NOTE on URL-encoding: `%2e%2e` is NOT decoded by Linux/macOS filesystems —
//! the kernel treats it as a literal filename token, not `..`. The pre-canonicalize
//! rejection list catches `..` as an actual path component; `%2e%2e` is allowed
//! through as a weird-but-harmless filename. See §10.3 proptest for confirmation.

use crate::error::WikiError;
use gadgetron_core::error::WikiErrorKind;
use std::path::{Component, Path, PathBuf};

/// Resolves a user-supplied page name to an absolute path within `root`.
/// Returns a canonicalized path ready to pass to `open()` in read mode, or a
/// computed (but not yet existing) path safe to pass to the write caller.
///
/// # Pre-canonicalize rejections (R1-R6)
///
/// R1. Input is empty or > 256 bytes → `PathEscape`
/// R2. Input contains null bytes or control characters (`< 0x20`) → `PathEscape`
/// R3. Input contains backslash (`\`) → `PathEscape` (Windows UNC attempt)
/// R4. Input is absolute (Path::is_absolute) or starts with `~` → `PathEscape`
/// R5. Any Component::ParentDir (`..`) is present → `PathEscape`
/// R6. Any Component that is not Normal or CurDir → `PathEscape`
///
/// # Canonicalization
///
/// After R1-R6 pass:
/// 1. Normalize: append `.md` suffix if missing
/// 2. Compute `candidate = root.join(normalized)`
/// 3. If `candidate` exists:
///      `canonical = canonicalize(candidate)` (follows symlinks)
/// 4. If `candidate` does not exist (write path):
///      Canonicalize the **existing** ancestor of `candidate`, then append the
///      remaining (non-existent) segments. Does NOT create directories.
/// 5. Prefix check: `canonical.starts_with(canonicalize(root))`
///
/// If the prefix check fails the path escaped via symlink → `PathEscape`.
///
/// # Returns
/// - `Ok(PathBuf)` — canonical path that the caller may read from or write to
/// - `Err(WikiError { kind: PathEscape { input }, .. })` on any rejection
///
/// # Caller responsibility for writes
/// The caller (`Wiki::write`) must, after receiving `Ok(canonical_path)`:
///   1. `std::fs::create_dir_all(canonical_path.parent())` to create subdirs
///   2. Open the final file with `O_NOFOLLOW` to defeat symlink-swap TOCTOU
///      (see §4.4 write flow)
///
/// # Proptest corpus
/// See `tests/path_traversal_proptest.rs` — must cover R1-R6 negatives plus
/// the happy-path acceptance property.
pub fn resolve_path(root: &Path, user_input: &str) -> Result<PathBuf, WikiError> {
    // R1
    if user_input.is_empty() || user_input.len() > 256 {
        return Err(WikiError::path_escape(user_input));
    }
    // R2
    if user_input.contains('\0') || user_input.chars().any(|c| c.is_control()) {
        return Err(WikiError::path_escape(user_input));
    }
    // R3
    if user_input.contains('\\') {
        return Err(WikiError::path_escape(user_input));
    }
    // R4
    if Path::new(user_input).is_absolute() || user_input.starts_with('~') {
        return Err(WikiError::path_escape(user_input));
    }
    // R5 + R6
    for segment in Path::new(user_input).components() {
        match segment {
            Component::ParentDir => return Err(WikiError::path_escape(user_input)),
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(WikiError::path_escape(user_input)),
        }
    }

    // Normalize: append .md if missing
    let normalized = if user_input.ends_with(".md") {
        user_input.to_string()
    } else {
        format!("{user_input}.md")
    };

    let candidate = root.join(&normalized);
    let canonical_root = std::fs::canonicalize(root).map_err(WikiError::Io)?;

    // For existing files (reads): canonicalize directly.
    // For non-existent (writes): canonicalize the deepest existing ancestor,
    // then append the remaining segments. NO create_dir_all here.
    let canonical = if candidate.exists() {
        std::fs::canonicalize(&candidate).map_err(WikiError::Io)?
    } else {
        canonicalize_with_missing_tail(&candidate)?
    };

    if !canonical.starts_with(&canonical_root) {
        return Err(WikiError::path_escape(user_input));
    }
    Ok(canonical)
}

/// Walks up `path` until an existing ancestor is found, canonicalizes it, and
/// rejoins the non-existent tail. No filesystem mutation. Used for write paths.
fn canonicalize_with_missing_tail(path: &Path) -> Result<PathBuf, WikiError> {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    while !existing.exists() {
        let parent = existing.parent().ok_or_else(|| WikiError::path_escape(""))?;
        let name = existing.file_name().ok_or_else(|| WikiError::path_escape(""))?;
        tail.push(name);
        existing = parent.to_path_buf();
    }
    let mut canonical = std::fs::canonicalize(&existing).map_err(WikiError::Io)?;
    for segment in tail.into_iter().rev() {
        canonical.push(segment);
    }
    Ok(canonical)
}
```

### 4.3 Read operations

```rust
impl Wiki {
    /// Opens or initializes a wiki repo. Validates config.
    pub fn open_or_init(config: WikiConfig) -> Result<Self, WikiError> { /* ... */ }

    /// Lists all `.md` files under root recursively. Metadata only.
    pub fn list(&self) -> Result<Vec<WikiPageMeta>, WikiError> { /* walkdir */ }

    /// Fetches a page. Uses `resolve_path` for M3 guard.
    pub fn get(&self, name: &str) -> Result<WikiPage, WikiError> { /* ... */ }

    /// Full-text search via cached inverted index. Rebuilds if None.
    /// Holds self.index lock for the duration of the search (brief).
    pub fn search(&self, query: &str, max_results: usize) -> Result<Vec<WikiSearchHit>, WikiError> {
        let mut guard = self.index.lock().unwrap();
        if guard.is_none() {
            *guard = Some(crate::wiki::index::InvertedIndex::build(self)?);
        }
        Ok(guard.as_ref().unwrap().search(query, max_results))
    }

    /// Backlinks: pages containing `[[target]]` or `[[target|...]]` or `[[target#...]]`.
    pub fn backlinks(&self, target: &str) -> Result<Vec<WikiPageMeta>, WikiError> { /* ... */ }
}
```

### 4.4 Write operations — M5 enforcement (3-layer)

**M5 ordering** (security A2 clarification):
1. **Size cap** (cheap check, short-circuits adversarial big payloads)
2. **Credential BLOCK patterns** (PEM, AKIA, GCP) — refuse write, return `CredentialBlocked`
3. **Credential AUDIT patterns** (generic_secret, bearer_token) — warn log, continue
4. **Path resolution** (M3 guard)
5. **`create_dir_all` on canonical parent** (side effect — happens AFTER validation)
6. **Open with `O_NOFOLLOW`** (defeats symlink-swap TOCTOU at final component)
7. **Write + auto-commit** (abstract commit message)

```rust
impl Wiki {
    /// Writes or overwrites a page. Enforces M5 in the order above.
    ///
    /// # SECURITY (M5 audits vs M5 blocks vs git history permanence)
    /// - BLOCKED patterns (PEM, AKIA, GCP) return CredentialBlocked — never written.
    /// - AUDITED patterns (generic_secret, bearer_token) are written but emit a
    ///   `wiki_write_secret_suspected` audit entry. Once written, the content is
    ///   permanent in git history (see 00-overview §10 SEC-7 Disclosure 1).
    ///
    /// # Errors
    /// - `PageTooLarge { path, bytes, limit }` → HTTP 413
    /// - `CredentialBlocked { path, pattern }` → HTTP 422
    /// - `PathEscape { input }` → HTTP 400
    /// - `Io` → HTTP 500
    /// - `GitCorruption { path, reason }` → HTTP 503
    /// - `Conflict { path }` → HTTP 409
    pub fn write(&self, name: &str, content: &str) -> Result<WriteResult, WikiError> {
        // Layer 1: size cap
        if content.len() > self.config.max_page_bytes {
            return Err(WikiError::kind(WikiErrorKind::PageTooLarge {
                path: name.to_string(),
                bytes: content.len(),
                limit: self.config.max_page_bytes,
            }));
        }

        // Layer 2: BLOCK patterns (M5)
        for block_match in crate::wiki::secrets::check_block_patterns(content) {
            return Err(WikiError::kind(WikiErrorKind::CredentialBlocked {
                path: name.to_string(),
                pattern: block_match.pattern_name.to_string(),
            }));
        }

        // Layer 3: AUDIT patterns (M5) — do NOT block
        // SECURITY: M5 audits only; bypassed credentials are permanent in git
        // history — see 00-overview §10 SEC-7 Disclosure 1.
        for audit_match in crate::wiki::secrets::check_audit_patterns(content) {
            tracing::warn!(
                target: "wiki_audit",
                pattern = %audit_match.pattern_name,
                page = %name,
                "wiki_write_secret_suspected"
            );
        }

        // Layer 4: path resolution (M3)
        let canonical = crate::wiki::fs::resolve_path(&self.config.root, name)?;

        // Layer 5: create parent dirs AFTER path resolution has validated the prefix
        if let Some(parent) = canonical.parent() {
            std::fs::create_dir_all(parent).map_err(WikiError::Io)?;
        }

        // Layer 6: open with O_NOFOLLOW (defeats symlink swap at final component)
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&canonical)
                .map_err(WikiError::Io)?;
            use std::io::Write;
            file.write_all(content.as_bytes()).map_err(WikiError::Io)?;
        }

        // Layer 7: auto-commit (if enabled)
        let commit_oid = if self.config.autocommit {
            let sig = git2::Signature::now(
                &self.config.git_author_name,
                &self.config.git_author_email,
            )?;
            let rel = canonical.strip_prefix(&self.config.root).unwrap().to_path_buf();
            let oid = crate::wiki::git::autocommit(&self.repo, &rel, &sig)?;
            Some(oid)
        } else {
            None
        };

        // Invalidate index cache so next search rebuilds
        *self.index.lock().unwrap() = None;

        Ok(WriteResult { name: name.to_string(), commit_oid })
    }
}

#[derive(Debug, Clone)]
pub struct WriteResult {
    pub name: String,
    pub commit_oid: Option<git2::Oid>,  // native git2 type (per chief-arch N1)
}
```

**Test additions** (from qa + security):
- `resolve_path_write_rejects_symlink_swap_at_final_component` — create `wiki_root/target.md` as a symlink to `/etc/passwd`, call `write`, expect `Io` error from `O_NOFOLLOW` refusal
- `resolve_path_does_not_create_dirs_on_traversal_attempt` — call `resolve_path` with traversal input, verify no directories were created on disk

### 4.5 Git backend

```rust
// wiki/git.rs

/// Opens an existing git repo or creates one at `root` + empty initial commit.
pub fn open_or_init(
    root: &Path,
    author_name: &str,
    author_email: &str,
) -> Result<git2::Repository, WikiError> { /* ... */ }

/// Stages and commits a single file change.
///
/// # Commit message policy (M5 + SEC-7)
/// Abstract format: `"auto-commit: {name} {iso8601 UTC}"`.
/// NEVER includes: request_id, user query text, response text, content excerpts.
///
/// # Corruption mapping
/// - `git2::ErrorClass::Index` + `ErrorCode::Locked` → `GitCorruption { reason: "index locked" }`
/// - Detached HEAD → attempt reattach; if fails → `GitCorruption { reason: "detached HEAD" }`
/// - Missing tree/blob → `GitCorruption { reason: "missing objects" }`
/// - Unresolved merge conflict on working tree → `Conflict`
pub fn autocommit(
    repo: &git2::Repository,
    relative_path: &Path,
    signature: &git2::Signature,
) -> Result<git2::Oid, WikiError> { /* ... */ }
```

### 4.6 Obsidian link parser (`wiki/link.rs`)

```rust
/// Parses Obsidian-style `[[link]]` from markdown content.
///
/// # Grammar (ABNF)
/// link       = "[[" target [pipe-alias] [heading] "]]"
/// target     = 1*(not-pipe-not-bracket-not-hash)
/// pipe-alias = "|" 1*(not-bracket-not-hash)
/// heading    = "#" 1*(not-bracket)
///
/// Links inside code blocks (fenced ``` and inline `) are NOT parsed.
///
/// # Error handling
/// Malformed inputs (unclosed `[[`, double `]]]]`, nested `[[inner[[outer]]]]`)
/// are handled gracefully — `parse_links` never panics. See proptest corpus.
pub fn parse_links(content: &str) -> Vec<WikiLink> { /* ... */ }
```

### 4.7 Full-text search (`wiki/index.rs`) — HashMap not BTreeMap (chief-arch N2)

```rust
use std::collections::{HashMap, HashSet};

/// In-memory inverted index over wiki pages.
/// - `terms`: `HashMap<String /* token */, HashSet<String /* page name */>>`
/// - O(1) average-case insertion; sort only at query time for ranking
/// - Rebuilt on every write via `Wiki::write` → `*index = None`
/// - Lock inside `Wiki.index: Mutex<Option<InvertedIndex>>` — serializes
///   concurrent rebuilds
///
/// Expected scale: <10k pages. Rebuild cost: ~20-50ms measured on a 2020 MBP.
pub struct InvertedIndex {
    terms: HashMap<String, HashSet<String>>,
    page_count: usize,
}

impl InvertedIndex {
    pub fn build(wiki: &crate::wiki::Wiki) -> Result<Self, crate::error::WikiError> { /* ... */ }
    pub fn search(&self, query: &str, max_results: usize) -> Vec<crate::wiki::WikiSearchHit> { /* ... */ }
}
```

### 4.8 Credential patterns (`wiki/secrets.rs`) — 3 BLOCK + 2 audit (security A3)

```rust
use once_cell::sync::Lazy;
use regex::Regex;

pub struct SecretPatternMatch {
    pub pattern_name: &'static str,
    pub position: usize,
}

/// Patterns that BLOCK writes. These have near-zero false positive rate and
/// represent unambiguous high-severity credentials.
static BLOCK_PATTERNS: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        ("pem_private_key", Regex::new(
            r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"
        ).unwrap()),
        ("aws_access_key_id", Regex::new(r"AKIA[0-9A-Z]{16}").unwrap()),
        ("gcp_service_account", Regex::new(r#""private_key_id"\s*:\s*"[a-f0-9]{40}""#).unwrap()),
    ]
});

/// Patterns that log audit warnings but do NOT block. These have higher false
/// positive rates and blocking would frustrate legitimate use.
static AUDIT_PATTERNS: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        ("anthropic_api_key", Regex::new(r"sk-ant-[a-zA-Z0-9_\-]{40,}").unwrap()),
        ("gadgetron_api_key", Regex::new(r"gad_(live|test)_[a-f0-9]{32}").unwrap()),
        ("bearer_token", Regex::new(r"(?i)bearer\s+[A-Za-z0-9._\-]{32,}").unwrap()),
        ("generic_secret", Regex::new(
            r"(?i)(api[_-]?key|secret|token)\s*[:=]\s*[A-Za-z0-9+/]{20,}"
        ).unwrap()),
    ]
});

pub fn check_block_patterns(content: &str) -> Vec<SecretPatternMatch> {
    BLOCK_PATTERNS.iter()
        .flat_map(|(name, re)| re.find_iter(content).map(move |m| SecretPatternMatch {
            pattern_name: name, position: m.start(),
        }))
        .collect()
}

pub fn check_audit_patterns(content: &str) -> Vec<SecretPatternMatch> {
    AUDIT_PATTERNS.iter()
        .flat_map(|(name, re)| re.find_iter(content).map(move |m| SecretPatternMatch {
            pattern_name: name, position: m.start(),
        }))
        .collect()
}
```

---

## 5. Web search subsystem

### 5.1 WebSearch trait (`search/mod.rs`)

```rust
use async_trait::async_trait;

#[async_trait]
pub trait WebSearch: Send + Sync {
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SearchError>;
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub engine: String,
}
```

### 5.2 SearxngClient (`search/searxng.rs`) — SSRF mitigation

```rust
use std::time::Duration;
use reqwest::redirect::Policy;
use crate::error::SearchError;

pub struct SearxngClient {
    base_url: url::Url,
    http: reqwest::Client,
    max_results: u16,
}

impl SearxngClient {
    pub fn new(config: &crate::config::SearchConfig) -> Result<Self, SearchError> {
        // [P2C-SECURITY-REOPEN]: SSRF risk — SearxngClient will follow the
        // configured base_url wherever it points. For P2A this is user-accepted
        // (user owns the config). For P2C multi-user, add an IP allow-list
        // validator that rejects RFC-1918 link-local (169.254.0.0/16), metadata
        // (169.254.169.254), and loopback overrides.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .redirect(Policy::limited(3))  // mitigate open redirect to metadata endpoints
            .user_agent("gadgetron-knowledge/0.2")
            .build()
            .map_err(SearchError::Http)?;
        Ok(Self { base_url: config.searxng_url.clone(), http, max_results: config.max_results })
    }
}

#[async_trait::async_trait]
impl crate::search::WebSearch for SearxngClient {
    /// GETs `{base_url}/search?q={query}&format=json`.
    ///
    /// # SECURITY (SearchError::Parse sanitization — per security A4)
    /// On parse failure, `SearchError::Parse(String)` MUST be constructed with
    /// a fixed static message like `"search response parse failed"`. The raw
    /// response body, serde error detail, and any upstream content MUST NOT
    /// be included in the error string. Future reviewers: enforce via code
    /// review; test `parse_error_text_does_not_include_response_body`.
    async fn search(&self, query: &str) -> Result<Vec<crate::search::SearchResult>, SearchError> {
        let url = format!("{}/search", self.base_url);
        let resp = self.http
            .get(&url)
            .query(&[("q", query), ("format", "json")])
            .send()
            .await
            .map_err(SearchError::Http)?;

        if !resp.status().is_success() {
            return Err(SearchError::Parse("searxng upstream non-200".to_string()));
        }

        let body = resp.bytes().await.map_err(SearchError::Http)?;
        // NOTE: construct error with fixed string, never include `err.to_string()`.
        let parsed: SearxngResponse = serde_json::from_slice(&body)
            .map_err(|_e| SearchError::Parse("search response parse failed".to_string()))?;

        Ok(parsed.results.into_iter().take(10).map(|r| crate::search::SearchResult {
            title: r.title,
            url: r.url,
            snippet: r.content,
            engine: r.engine,
        }).collect())
    }
}

#[derive(serde::Deserialize)]
struct SearxngResponse {
    results: Vec<SearxngResult>,
}

#[derive(serde::Deserialize)]
struct SearxngResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    engine: String,
}
```

---

## 6. MCP server

> **Implementation note (2026-04-15)**: The stdio MCP server is hosted in `crates/gadgetron-penny/src/mcp_server.rs`, not in `gadgetron-knowledge`. `gadgetron-knowledge` provides only `KnowledgeToolProvider`, which is registered into `McpToolRegistry` at startup and served by the penny stdio runtime. This section describes the **logical protocol surface** the knowledge provider exposes — §6.1 and §6.2 are reference-level pseudocode for schema shape, dispatch, and JSON-RPC envelopes. The authoritative transport code and `gadgetron mcp serve` entry point live in `02-penny-agent.md §5` + `04-mcp-tool-registry.md §2.1`. Any discrepancy between the pseudocode here and the actual `gadgetron-penny::mcp_server` implementation is resolved in favor of the penny code.

### 6.1 Manual MCP transport (reference — real code in `gadgetron-penny::mcp_server`)

P2A ships a manual JSON-RPC 2.0 MCP transport. The authoritative implementation is `crates/gadgetron-penny/src/mcp_server.rs`; the pseudocode below documents the protocol contract the knowledge layer assumes. `rmcp` crate integration is deferred to P2B+ when its API stabilizes (re-evaluation at P2A→P2B transition).

```rust
//! MCP server for gadgetron-knowledge. Stdio transport, per-request lifecycle.

use crate::config::KnowledgeConfig;
use crate::wiki::Wiki;
use std::sync::Arc;

pub async fn serve_stdio(config: KnowledgeConfig) -> Result<(), Box<dyn std::error::Error>> {
    let wiki = Arc::new(Wiki::open_or_init(config.to_wiki_config()?)?);
    let web_search: Option<Arc<dyn crate::search::WebSearch>> = match config.search.as_ref() {
        Some(sc) => Some(Arc::new(crate::search::SearxngClient::new(sc)?)),
        None => None,
    };
    let server = crate::mcp::tools::KnowledgeServer::new(wiki, web_search);

    // P2A: always use the manual MCP implementation.
    // rmcp integration is deferred to P2B+ (see §6.1 rationale above).
    manual_mcp::serve_stdio(server).await?;

    Ok(())
}
```

#### Manual MCP implementation outline (`src/mcp/manual_mcp.rs`)

Implements MCP stdio protocol for `initialize`, `initialized`, `tools/list`, and `tools/call` over line-delimited JSON-RPC 2.0. This is the authoritative P2A implementation path.

```rust
// src/mcp/manual_mcp.rs

/// JSON-RPC 2.0 request envelope (subset used by MCP).
#[derive(serde::Deserialize)]
struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(serde::Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(serde::Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

pub async fn serve_stdio(server: crate::mcp::tools::KnowledgeServer)
    -> Result<(), Box<dyn std::error::Error>>
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 { break; }  // EOF — parent (Claude Code) exited

        let request: RpcRequest = match serde_json::from_str(line.trim()) {
            Ok(r) => r,
            Err(_) => continue,  // ignore malformed
        };

        let result = match request.method.as_str() {
            "initialize" => {
                // MCP protocol handshake — required before tools/list and tools/call
                let handshake = serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": request.id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "gadgetron-knowledge", "version": env!("CARGO_PKG_VERSION") }
                    }
                }))?;
                stdout.write_all(&handshake).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
                continue;
            }
            "initialized" => {
                // Notification; no response. Just ack the state machine.
                continue;
            }
            "tools/list" => Ok(server.handle_tools_list().await),
            "tools/call" => {
                let name = request.params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = request.params.get("arguments").cloned().unwrap_or_default();
                Ok(server.handle_tools_call(name, args).await)
            }
            other => Err(RpcError {
                code: -32601,
                message: format!("method not found: {other}"),
            }),
        };

        let response = match result {
            Ok(result) => RpcResponse { jsonrpc: "2.0", id: request.id, result: Some(result), error: None },
            Err(err) => RpcResponse { jsonrpc: "2.0", id: request.id, result: None, error: Some(err) },
        };
        let serialized = serde_json::to_vec(&response)?;
        stdout.write_all(&serialized).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }
    Ok(())
}
```

### 6.2 Tool schemas (`mcp/schema.rs`) — with `max_results` + rewritten descriptions (dx A1, A2)

```rust
use serde_json::{json, Value};

/// wiki.list
pub fn schema_wiki_list() -> Value {
    json!({
        "name": "wiki.list",
        "description": "List all pages in the Penny wiki. Returns page names, optional \
                        titles (from frontmatter or first H1), and modification times. Use \
                        this to discover what pages exist before searching or fetching.",
        "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
    })
}

pub fn schema_wiki_get() -> Value {
    json!({
        "name": "wiki.get",
        "description": "Fetch a wiki page by its logical name. Returns the markdown body \
                        and parsed frontmatter. Use wiki.get when you already know the \
                        exact page name (e.g. from a previous wiki.list or wiki.search result). \
                        Page names use forward slashes for subdirectories.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 1, "maxLength": 256 }
            },
            "required": ["name"],
            "additionalProperties": false
        }
    })
}

pub fn schema_wiki_search() -> Value {
    json!({
        "name": "wiki.search",
        "description": "Search wiki pages by keyword when you don't know the exact page \
                        name. Returns up to max_results matching pages with a 200-char \
                        snippet each. Use wiki.get when you know the exact page name.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": 512 },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "default": 5,
                    "description": "Maximum hits to return (default 5)"
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }
    })
}

pub fn schema_wiki_write() -> Value {
    json!({
        "name": "wiki.write",
        "description": "Write or overwrite a wiki page. Content is markdown, optionally \
                        with TOML frontmatter. Auto-commits to git on success. Size limit: \
                        1 MiB default. Path must not contain '..' or absolute paths. Will \
                        reject writes containing unambiguous credentials (PEM keys, AWS \
                        access keys).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 1, "maxLength": 256 },
                "content": { "type": "string", "minLength": 0 }
            },
            "required": ["name", "content"],
            "additionalProperties": false
        }
    })
}

pub fn schema_web_search() -> Value {
    json!({
        "name": "web.search",
        "description": "Search the web for information not in the wiki. Returns up to 10 \
                        results with title, URL, and snippet from Google, Bing, DuckDuckGo, \
                        and Brave via a self-hosted SearXNG proxy.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": 512 }
            },
            "required": ["query"],
            "additionalProperties": false
        }
    })
}
```

### 6.3 Tool dispatch (`mcp/tools.rs`)

```rust
use crate::wiki::Wiki;
use crate::search::WebSearch;
use crate::error::{WikiError, SearchError};
use gadgetron_core::error::WikiErrorKind;
use std::sync::Arc;

pub struct KnowledgeServer {
    wiki: Arc<Wiki>,
    web_search: Option<Arc<dyn WebSearch>>,
}

impl KnowledgeServer {
    pub fn new(wiki: Arc<Wiki>, web_search: Option<Arc<dyn WebSearch>>) -> Self {
        Self { wiki, web_search }
    }

    pub async fn handle_tools_list(&self) -> serde_json::Value {
        let mut tools = vec![
            crate::mcp::schema::schema_wiki_list(),
            crate::mcp::schema::schema_wiki_get(),
            crate::mcp::schema::schema_wiki_search(),
            crate::mcp::schema::schema_wiki_write(),
        ];
        if self.web_search.is_some() {
            tools.push(crate::mcp::schema::schema_web_search());
        }
        serde_json::json!({ "tools": tools })
    }

    pub async fn handle_tools_call(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        match name {
            "wiki.list"   => self.call_wiki_list().await,
            "wiki.get"    => self.call_wiki_get(args).await,
            "wiki.search" => self.call_wiki_search(args).await,
            "wiki.write"  => self.call_wiki_write(args).await,
            "web.search" if self.web_search.is_some() => self.call_web_search(args).await,
            _ => Self::tool_error_result("unknown tool"),
        }
    }

    fn tool_error_result(msg: &str) -> serde_json::Value {
        serde_json::json!({
            "isError": true,
            "content": [{ "type": "text", "text": msg }]
        })
    }

    /// WikiError → MCP tool result error. Redacts raw user input.
    /// Error text includes `bytes`/`limit` for PageTooLarge per dx A3.
    fn wiki_error_to_tool_result(err: WikiError) -> serde_json::Value {
        let msg: String = match err.kind_ref() {
            Some(WikiErrorKind::PathEscape { .. }) => "invalid page path".to_string(),
            Some(WikiErrorKind::PageTooLarge { bytes, limit, .. }) => {
                format!("Page too large: {bytes} bytes exceeds the {limit}-byte limit. Split the content into multiple smaller pages.")
            }
            Some(WikiErrorKind::CredentialBlocked { pattern, .. }) => {
                format!("Credential detected in content (pattern: {pattern}). Wiki writes must not contain unambiguous secrets. Remove the credential and retry.")
            }
            Some(WikiErrorKind::GitCorruption { .. }) => "wiki git repository error".to_string(),
            Some(WikiErrorKind::Conflict { .. }) => "wiki conflict".to_string(),
            None => "wiki error".to_string(),
        };
        Self::tool_error_result(&msg)
    }

    /// SearchError → MCP tool result error.
    /// Error text is generic — NEVER includes response body or serde error detail.
    fn search_error_to_tool_result(err: SearchError) -> serde_json::Value {
        let msg = match err {
            SearchError::Http(_)     => "web search upstream HTTP error",
            SearchError::Parse(_)    => "web search response parse failed",
            SearchError::Upstream(_) => "web search upstream error",
        };
        Self::tool_error_result(msg)
    }
}
```

### 6.4 Error mapping summary

- **MCP tool result error** (`isError: true`): user-visible via Claude Code agent. Generic text, no raw input leak. Returned for: PathEscape, PageTooLarge, CredentialBlocked, search failures, not-found.
- **MCP protocol error** (JSON-RPC `error` field): for malformed requests or irrecoverable server state.
- **No HTTP errors**: MCP server errors never bubble up as HTTP — they stay inside the subprocess-to-claude channel.

---

## 7. Configuration (`config.rs`)

```rust
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KnowledgeConfig {
    pub wiki_path: PathBuf,
    #[serde(default = "default_true")]
    pub wiki_autocommit: bool,
    #[serde(default)]
    pub wiki_git_author: Option<String>,
    #[serde(default = "default_max_page_bytes")]
    pub wiki_max_page_bytes: usize,
    #[serde(default)]
    pub search: Option<SearchConfig>,
}

/// SearchConfig is the authoritative field definition for [knowledge.search] in gadgetron.toml.
/// 00-overview.md §6 and ADR-P2A-03 must match these field names and defaults exactly.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SearchConfig {
    // [P2C-SECURITY-REOPEN]: SSRF risk — restrict to non-link-local/non-metadata
    // ranges. See crates/gadgetron-knowledge/src/search/searxng.rs for the P2A
    // mitigation (reqwest redirect::limited(3)) and the P2C TODO.
    pub searxng_url: url::Url,  // validated at load time: http(s), non-empty host
    #[serde(default = "default_search_timeout")]
    pub timeout_secs: u64,  // [1, 60]; default 10
    #[serde(default = "default_max_results")]
    pub max_results: u16,  // [1, 50]; default 5
}

fn default_true() -> bool { true }
fn default_max_page_bytes() -> usize { 1_048_576 }
fn default_search_timeout() -> u64 { 10 }
fn default_max_results() -> u16 { 5 }

impl KnowledgeConfig {
    /// Validates at load time. Rules:
    /// - wiki_path parent must exist and be writable
    /// - wiki_max_page_bytes must be in [1, 100 MiB]
    /// - searxng_url if present must be http(s):// with non-empty hostname
    /// - timeout_secs must be in [1, 60]
    pub fn validate(&self) -> Result<(), String> {
        // Wiki path itself may not exist yet (penny init creates it).
        // What MUST exist: the parent directory, and it must be writable.
        let parent = self.wiki_path.parent()
            .ok_or_else(|| format!("wiki_path must not be filesystem root: {}", self.wiki_path.display()))?;
        if !parent.exists() {
            return Err(format!("wiki_path parent does not exist: {}", parent.display()));
        }
        let metadata = std::fs::metadata(parent)
            .map_err(|e| format!("cannot stat wiki_path parent {}: {}", parent.display(), e))?;
        if metadata.permissions().readonly() {
            return Err(format!("wiki_path parent is read-only: {}", parent.display()));
        }
        // wiki_max_page_bytes bounds
        if !(1..=104_857_600).contains(&self.wiki_max_page_bytes) {
            return Err(format!("wiki_max_page_bytes must be in [1, 100 MiB]; got {}", self.wiki_max_page_bytes));
        }
        // SearchConfig validation (if present)
        if let Some(sc) = &self.search {
            let host = sc.searxng_url.host_str().unwrap_or("");
            if host.is_empty() {
                return Err("searxng_url must have a non-empty hostname".to_string());
            }
            let scheme = sc.searxng_url.scheme();
            if scheme != "http" && scheme != "https" {
                return Err(format!("searxng_url scheme must be http or https, got: {scheme}"));
            }
            if !(1..=60).contains(&sc.timeout_secs) {
                return Err(format!("timeout_secs must be in [1, 60]; got {}", sc.timeout_secs));
            }
        }
        Ok(())
    }

    /// Builds WikiConfig from KnowledgeConfig. Field-for-field mapping.
    /// Auto-detects git author if wiki_git_author is None,
    /// falling back to "Penny <penny@gadgetron.local>" with eprintln! warning.
    pub fn to_wiki_config(&self) -> Result<crate::wiki::WikiConfig, String> {
        let (git_author_name, git_author_email) = match &self.wiki_git_author {
            Some(author) => {
                // Parse "Name <email>" format
                if let Some(lt) = author.find(" <") {
                    let name = author[..lt].to_string();
                    let email = author[lt+2..author.len()-1].to_string();
                    (name, email)
                } else {
                    return Err(format!(
                        "wiki_git_author must be in 'Name <email>' format, got: {author}"
                    ));
                }
            }
            None => autodetect_git_author_or_fallback(),
        };
        Ok(crate::wiki::WikiConfig {
            root: self.wiki_path.clone(),
            autocommit: self.wiki_autocommit,
            git_author_name,
            git_author_email,
            max_page_bytes: self.wiki_max_page_bytes,
        })
    }
}

/// Detects git author from the global/system gitconfig.
///
/// Uses `git2::Config::open_default()` (reads global + system gitconfig).
/// Does NOT shell out to `git` — keeps gadgetron self-contained and avoids PATH dependency.
fn autodetect_git_author() -> Option<String> {
    let config = git2::Config::open_default().ok()?;
    let name = config.get_string("user.name").ok()?;
    let email = config.get_string("user.email").ok()?;
    if name.is_empty() || email.is_empty() {
        return None;
    }
    Some(format!("{} <{}>", name, email))
}

/// Wraps autodetect with eprintln! fallback for use in penny init and to_wiki_config().
fn autodetect_git_author_or_fallback() -> (String, String) {
    match autodetect_git_author() {
        Some(author) => {
            // Split "Name <email>" into (name, email) components
            if let Some(lt) = author.find(" <") {
                let name = author[..lt].to_string();
                let email = author[lt+2..author.len()-1].to_string();
                return (name, email);
            }
            ("Penny".to_string(), "penny@gadgetron.local".to_string())
        }
        None => {
            eprintln!("  [WARN] git config user.name/user.email not set");
            eprintln!("         Falling back to \"Penny <penny@gadgetron.local>\" for wiki commits.");
            eprintln!("         To override: set wiki_git_author in ~/.gadgetron/gadgetron.toml,");
            eprintln!("         or run: git config --global user.name \"Your Name\"");
            eprintln!("                 git config --global user.email \"you@example.com\"");
            tracing::warn!("wiki_git_author fallback used");
            ("Penny".to_string(), "penny@gadgetron.local".to_string())
        }
    }
}
```

---

## 8. Error types (`error.rs`)

### 8.1 `WikiErrorKind` is defined in `gadgetron-core`

Per chief-arch B1 + dx A6: `WikiErrorKind` is **canonical in `gadgetron-core::error`**. This spec adds the definition to core. The knowledge crate re-exports it.

```rust
// gadgetron-core/src/error.rs (additions — added by THIS spec, not 02)

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WikiErrorKind {
    /// Path traversal attempt. HTTP 400, code wiki_invalid_path.
    PathEscape { input: String },
    /// Content exceeds wiki_max_page_bytes. HTTP 413, code wiki_page_too_large.
    /// All 3 fields needed for actionable error message (path + actual + limit).
    PageTooLarge { path: String, bytes: usize, limit: usize },
    /// Content matches a BLOCK pattern (PEM / AKIA / GCP). HTTP 422.
    CredentialBlocked { path: String, pattern: String },
    /// Git repo in inconsistent state. HTTP 503.
    GitCorruption { path: String, reason: String },
    /// Merge conflict during autocommit. HTTP 409.
    Conflict { path: String },
}

impl std::fmt::Display for WikiErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PathEscape { .. }        => write!(f, "path_escape"),
            Self::PageTooLarge { .. }      => write!(f, "page_too_large"),
            Self::CredentialBlocked { .. } => write!(f, "credential_blocked"),
            Self::GitCorruption { .. }     => write!(f, "git_corruption"),
            Self::Conflict { .. }          => write!(f, "conflict"),
        }
    }
}

// Added to GadgetronError enum:
//
//   #[error("Wiki error ({kind}): {message}")]
//   Wiki { kind: WikiErrorKind, message: String },
//
// error_code / error_type / http_status_code additions:
//   Wiki { kind: PathEscape, .. }        → "wiki_invalid_path"   / invalid_request_error / 400
//   Wiki { kind: PageTooLarge, .. }      → "wiki_page_too_large" / invalid_request_error / 413
//   Wiki { kind: CredentialBlocked, .. } → "wiki_credential_blocked" / invalid_request_error / 422
//   Wiki { kind: GitCorruption, .. }     → "wiki_git_corrupted"  / server_error / 503
//   Wiki { kind: Conflict, .. }          → "wiki_conflict"       / server_error / 409
```

**Test updates** in `gadgetron-core/src/error.rs`:
- `all_twelve_variants_exist` → `all_thirteen_variants_exist` (Wiki added, Penny in 02)
- New assertions for all 5 WikiErrorKind codes/types/statuses

### 8.2 Knowledge-local wrapper (`error.rs`)

```rust
use thiserror::Error;
use gadgetron_core::error::WikiErrorKind;

#[derive(Error, Debug)]
pub enum WikiError {
    #[error("wiki I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("git: {0}")]
    Git(#[from] git2::Error),

    #[error("frontmatter: {0}")]
    Frontmatter(String),

    #[error("wiki ({kind}): {message}")]
    Kind { kind: WikiErrorKind, message: String },
}

impl WikiError {
    pub fn kind(kind: WikiErrorKind) -> Self {
        let msg = kind.to_string();
        Self::Kind { kind, message: msg }
    }

    pub fn path_escape(input: &str) -> Self {
        Self::kind(WikiErrorKind::PathEscape { input: input.to_string() })
    }

    pub fn kind_ref(&self) -> Option<&WikiErrorKind> {
        match self {
            Self::Kind { kind, .. } => Some(kind),
            _ => None,
        }
    }
}

// Conversion into core GadgetronError
// Note: Io + Frontmatter both map here to GitCorruption; manual triage required.
// Post-merge refactor: split into distinct Io, Parse, GitCorruption variants — see §11.
impl From<WikiError> for gadgetron_core::error::GadgetronError {
    fn from(err: WikiError) -> Self {
        match err {
            WikiError::Kind { kind, message } =>
                gadgetron_core::error::GadgetronError::Wiki { kind, message },
            WikiError::Io(e) =>
                gadgetron_core::error::GadgetronError::Wiki {
                    kind: WikiErrorKind::GitCorruption {
                        path: String::new(),
                        reason: e.to_string(),
                    },
                    // User message deliberately says "wiki storage error" — not "git error" —
                    // so operators check disk space and filesystem permissions first,
                    // not the git repository. Run `git status` in the wiki directory
                    // OR check disk space and filesystem permissions.
                    message: format!(
                        "wiki storage error — run `git status` in the wiki directory OR \
                         check disk space and filesystem permissions (reason: {})",
                        e
                    ),
                },
            WikiError::Git(e) =>
                gadgetron_core::error::GadgetronError::Wiki {
                    kind: WikiErrorKind::GitCorruption {
                        path: String::new(),
                        reason: e.to_string(),
                    },
                    message: format!(
                        "wiki storage error — run `git status` in the wiki directory OR \
                         check disk space and filesystem permissions (reason: {})",
                        e
                    ),
                },
            WikiError::Frontmatter(msg) =>
                gadgetron_core::error::GadgetronError::Wiki {
                    kind: WikiErrorKind::GitCorruption { path: String::new(), reason: msg.clone() },
                    // Frontmatter parse errors often indicate file corruption.
                    // User message says "wiki storage error" for the same triage reason.
                    message: format!(
                        "wiki storage error — run `git status` in the wiki directory OR \
                         check disk space and filesystem permissions (reason: {})",
                        msg
                    ),
                },
        }
    }
}

#[derive(Error, Debug)]
pub enum SearchError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("timeout")]
    Timeout,
    #[error("parse: {0}")]
    Parse(String),     // SECURITY: construction must use fixed strings only
    #[error("config: {0}")]
    Config(String),
}
```

---

## 9. Security enforcement mapping

| Mitigation | Code location | Test |
|---|---|---|
| **M3** Path traversal pre-rejection (R1-R6) | `wiki/fs.rs::resolve_path` | `tests/path_traversal_proptest.rs` + unit tests |
| **M3** `O_NOFOLLOW` on final write | `wiki/mod.rs::Wiki::write` (layer 6) | `resolve_path_write_rejects_symlink_swap_at_final_component` |
| **M3** No side effects in guard | `wiki/fs.rs::resolve_path` (pure) | `resolve_path_does_not_create_dirs_on_traversal_attempt` |
| **M5** Size cap | `wiki/mod.rs::Wiki::write` (layer 1) | `tests/wiki_large_files.rs` |
| **M5** Credential BLOCK patterns (PEM, AKIA, GCP) | `wiki/secrets.rs::check_block_patterns` + `Wiki::write` layer 2 | `tests/wiki_secret_patterns.rs::blocks_pem`, `blocks_akia`, `blocks_gcp` |
| **M5** Credential AUDIT patterns | `wiki/secrets.rs::check_audit_patterns` + `Wiki::write` layer 3 | `tests/wiki_secret_patterns.rs::audits_anthropic`, etc. |
| **M5** Abstract git commit messages | `wiki/git.rs::autocommit` (hardcoded format) | `wiki::git::tests::commit_message_is_abstract` |
| **M5** Git history permanence inline comment | `wiki/mod.rs::Wiki::write` layer 3 | code-review only |
| **M6** `tools_called` names only | `mcp/tools.rs` dispatch (no arg logging) | `mcp_conformance.rs::tool_call_audit_log_does_not_contain_arguments` |
| **M7** Git history permanence disclosure | `docs/manual/penny.md` (pre-merge gate) | manual review |
| **M7** SearXNG privacy disclosure | `docs/manual/penny.md` (pre-merge gate) | manual review |
| **SSRF** `redirect::limited(3)` | `search/searxng.rs::SearxngClient::new` | integration test with mock redirect |

Out-of-scope (02): M1 tempfile, M2 stderr redaction, M4 `--allowed-tools` verification, M8 risk acceptance ADR.

---

## 10. Testing strategy

### 10.1 Unit tests

| Module | Test file | Tests |
|---|---|---|
| `wiki::fs` | `src/wiki/fs.rs #[cfg(test)]` | `resolve_path_rejects_parent_dir`, `resolve_path_rejects_absolute`, `resolve_path_rejects_null_bytes`, `resolve_path_rejects_backslash`, `resolve_path_rejects_too_long`, `resolve_path_rejects_empty`, `resolve_path_rejects_control_chars`, `resolve_path_rejects_tilde_prefix`, `resolve_path_accepts_valid_simple`, `resolve_path_accepts_subdirectory`, `resolve_path_rejects_symlink_out_of_root`, `resolve_path_accepts_nfc_nfd_equivalent`, `resolve_path_does_not_create_dirs_on_traversal_attempt`, `canonicalize_with_missing_tail_basic`, `canonicalize_with_missing_tail_deep` |
| `wiki::link` | `src/wiki/link.rs #[cfg(test)]` | `parse_plain_link`, `parse_alias_link`, `parse_heading_link`, `parse_alias_and_heading`, `parse_multiple_links`, `parse_skips_code_block`, `parse_handles_korean`, `parse_handles_unicode`, `parse_unclosed_returns_empty`, `parse_double_close_returns_empty`, `parse_nested_returns_outer_only` |
| `wiki::git` | `src/wiki/git.rs #[cfg(test)]` | `open_or_init_creates_new_repo`, `open_or_init_reuses_existing_repo`, `autocommit_writes_commit`, `commit_message_is_abstract`, `autocommit_returns_oid` |
| `wiki::index` | `src/wiki/index.rs #[cfg(test)]` | `build_from_empty_wiki`, `build_from_two_pages`, `search_finds_term`, `search_scores_multi_term`, `search_limits_to_max_results` |
| `wiki::secrets` | `src/wiki/secrets.rs #[cfg(test)]` | `block_pem_rsa`, `block_pem_ec`, `block_pem_openssh`, `block_aws_akia`, `block_gcp_private_key_id`, `audit_anthropic_key`, `audit_gadgetron_key`, `audit_bearer_token`, `audit_generic_secret`, `no_match_on_clean_content` |
| `wiki::frontmatter` | `src/wiki/frontmatter.rs #[cfg(test)]` | `parse_toml_frontmatter`, `parse_no_frontmatter`, `parse_malformed_returns_err`, `parse_toml_frontmatter_with_bom`, `parse_date_without_time_component_defaults_to_none`, `parse_empty_frontmatter_block_returns_default`, `parse_duplicate_key_last_wins` |
| `search::searxng` | `src/search/searxng.rs #[cfg(test)]` | `parse_response_with_fixture`, `parse_empty_results`, `parse_malformed_json_returns_parse_error`, `parse_error_text_does_not_include_response_body`, `parse_missing_required_fields_skips`, `build_url_escapes_query`, `redirect_policy_is_limited` |
| `config` | `src/config.rs #[cfg(test)]` | `validate_accepts_defaults`, `validate_rejects_zero_max_page`, `validate_rejects_max_page_above_cap`, `validate_accepts_http_url_with_port`, `validate_accepts_https_url_with_path`, `validate_rejects_non_http_scheme`, `validate_rejects_empty_hostname_url`, `validate_rejects_timeout_zero`, `validate_rejects_timeout_above_sixty`, `autodetect_git_author_from_git_config`, `autodetect_git_author_fallback_emits_eprintln` |

### 10.2 Integration tests

| Test file | Purpose |
|---|---|
| `tests/mcp_conformance.rs` | rmcp/manual server round-trips; see §10.5 |
| `tests/wiki_git_recovery.rs` | 4 corruption scenarios; concrete setup in §10.6 |
| `tests/wiki_large_files.rs` | M5 size cap — reject 1 MiB + 1 byte |
| `tests/wiki_secret_patterns.rs` | BLOCK semantics for 3 patterns + AUDIT log for 4 patterns |
| `tests/path_traversal_proptest.rs` | §10.3 generators + positive proptest |

### 10.3 Proptest corpus — `resolve_path`

```rust
use proptest::prelude::*;
use gadgetron_knowledge::wiki::fs::resolve_path;

fn traversal_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        10 => "\\PC*".prop_map(String::from),  // arbitrary strings (down-weighted)
        5  => prop::collection::vec(Just("../".to_string()), 1..5).prop_map(|v| v.join("") + "target.md"),
        5  => prop::sample::select(&[
                "/etc/passwd".to_string(),
                "C:\\Windows".to_string(),
                "~/secrets".to_string(),
            ]),
        3  => "[a-z]+\x00[a-z]+".prop_map(String::from),
        3  => prop::sample::select(&[
                "café.md".to_string(),          // NFC
                "cafe\u{0301}.md".to_string(),  // NFD
                "프로젝트/카이로스".to_string(),
            ]),
        3  => "\\\\server\\share\\foo".prop_map(String::from),
        5  => prop::sample::select(&[  // mixed traversal with valid prefix
                "valid/subdir/../../etc/passwd".to_string(),
                "a/b/../../../root/.ssh/id_rsa".to_string(),
                "projects/foo/../../../etc/shadow".to_string(),
            ]),
    ]
}

/// Positive proptest — valid names must produce Ok (qa blocker).
fn valid_name_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_/-]{0,63}".prop_map(|s| {
        if s.ends_with(".md") { s } else { format!("{s}.md") }
    })
}

// Workspace-wide proptest convention: see docs/design/testing/harness.md §2.10
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        max_shrink_iters: 4096,
        ..ProptestConfig::default()
    })]

    #[test]
    fn resolve_path_never_escapes_root(input in traversal_strategy()) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        match resolve_path(root, &input) {
            Ok(canonical) => {
                let canonical_root = std::fs::canonicalize(root).unwrap();
                prop_assert!(canonical.starts_with(&canonical_root));
            }
            Err(_) => {}  // rejection acceptable
        }
    }

    #[test]
    fn resolve_path_accepts_valid_names(name in valid_name_strategy()) {
        prop_assume!(!name.contains("..") && !name.contains('\0'));
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_path(tmp.path(), &name);
        prop_assert!(result.is_ok(), "expected Ok for valid input {:?}, got {:?}", name, result);
    }
}
```

### 10.4 Proptest corpus — `parse_links`

```rust
fn valid_link_strategy() -> impl Strategy<Value = String> {
    let target = "[A-Za-z0-9 가-힣/_.-]{1,32}";
    let alias = "[A-Za-z0-9 가-힣 ]{0,32}";
    let heading = "[A-Za-z0-9 가-힣 ]{0,32}";
    (target, proptest::option::of(alias), proptest::option::of(heading))
        .prop_map(|(t, a, h)| {
            let mut s = format!("[[{t}");
            if let Some(a) = a { s.push('|'); s.push_str(&a); }
            if let Some(h) = h { s.push('#'); s.push_str(&h); }
            s.push_str("]]");
            s
        })
}

fn malformed_link_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        "\\[\\[[a-z]{1,10}".prop_map(String::from),                    // unclosed
        "\\[\\[[a-z]{1,10}\\]\\]\\]\\]".prop_map(String::from),        // double-close
        "\\[\\[[a-z]{1,5}\\[\\[[a-z]{1,5}\\]\\]\\]\\]".prop_map(String::from),  // nested
        "\\[\\[[|]{5,}\\]\\]".prop_map(String::from),                  // many pipes
        "\\[\\[#{5,}\\]\\]".prop_map(String::from),                    // only heading
    ]
}

// Workspace-wide proptest convention: see docs/design/testing/harness.md §2.10
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        max_shrink_iters: 4096,
        ..ProptestConfig::default()
    })]

    #[test]
    fn parse_links_never_panics_on_valid(body in valid_link_strategy()) {
        let _ = gadgetron_knowledge::wiki::link::parse_links(&body);
    }

    #[test]
    fn parse_links_never_panics_on_malformed(body in malformed_link_strategy()) {
        let _ = gadgetron_knowledge::wiki::link::parse_links(&body);
    }
}
```

### 10.5 MCP conformance tests

```rust
// tests/mcp_conformance.rs

#[tokio::test]
async fn tools_list_returns_four_tools_without_search() {
    let fx = KnowledgeFixture::new_without_search().await;
    let tools = fx.client.list_tools().await.unwrap();
    let names: Vec<String> = tools["tools"]
        .as_array().unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["wiki.list", "wiki.get", "wiki.search", "wiki.write"]
    );
}

#[tokio::test]
async fn tools_list_includes_web_search_when_configured() {
    let fx = KnowledgeFixture::new_with_searxng_mock().await;
    let tools = fx.client.list_tools().await.unwrap();
    let names: Vec<String> = tools["tools"]
        .as_array().unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names.len(), 5);
    assert!(names.contains(&"web.search".to_string()));
}

#[tokio::test] async fn tools_list_is_idempotent() {
    let fx = KnowledgeFixture::new_without_search().await;
    let a = fx.client.list_tools().await.unwrap();
    let b = fx.client.list_tools().await.unwrap();
    assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap());
}

#[tokio::test]
async fn wiki_get_returns_page_content() {
    let fx = KnowledgeFixture::new_without_search().await;
    fx.wiki_write_raw("greet", "# Hello\n\nBody line.").await;
    let result = fx.client.call_tool("wiki.get", json!({ "name": "greet" })).await.unwrap();
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("# Hello"));
    assert!(text.contains("Body line."));
}

#[tokio::test]
async fn wiki_get_path_traversal_returns_tool_error() {
    let fx = KnowledgeFixture::new_without_search().await;
    let result = fx.client.call_tool(
        "wiki.get",
        json!({ "name": "../../etc/passwd" }),
    ).await.unwrap();
    assert_eq!(result["isError"], true);
    assert!(result["content"][0]["text"]
        .as_str().unwrap()
        .contains("invalid page path"));
}

#[tokio::test]
async fn wiki_write_rejects_oversize() {
    let fx = KnowledgeFixture::new_without_search().await;
    let body = "A".repeat(1_048_577); // 1 MiB + 1 byte over default cap
    let result = fx.client.call_tool(
        "wiki.write",
        json!({ "name": "big", "content": body }),
    ).await.unwrap();
    assert_eq!(result["isError"], true);
    assert!(result["content"][0]["text"]
        .as_str().unwrap()
        .contains("Page too large"));
}

#[tokio::test] async fn wiki_write_rejects_pem_private_key_block() {
    let fx = KnowledgeFixture::new_without_search().await;
    let body = "here is my key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA...";
    let result = fx.client.call_tool("wiki.write", json!({
        "name": "leaked",
        "content": body,
    })).await.unwrap();
    assert_eq!(result["isError"], true);
    assert!(result["content"][0]["text"].as_str().unwrap().contains("Credential detected"));
}

#[tokio::test] async fn unknown_tool_returns_tool_error_not_panic() {
    let fx = KnowledgeFixture::new_without_search().await;
    let result = fx.client.call_tool("nonexistent_tool_xyz", json!({})).await.unwrap();
    assert_eq!(result["isError"], true);
}

#[tokio::test] async fn wiki_get_missing_required_field_returns_tool_error() {
    let fx = KnowledgeFixture::new_without_search().await;
    let result = fx.client.call_tool("wiki.get", json!({})).await.unwrap();
    assert_eq!(result["isError"], true);
}

#[tokio::test] async fn wiki_get_wrong_argument_type_returns_tool_error() {
    let fx = KnowledgeFixture::new_without_search().await;
    let result = fx.client.call_tool("wiki.get", json!({"name": 42})).await.unwrap();
    assert_eq!(result["isError"], true);
}

#[tokio::test]
async fn tool_call_audit_log_does_not_contain_arguments() {
    // M6: audit log stores tool names only, never arguments (PII leak risk).
    // Use a tool call with sensitive-looking argument text; assert the audit
    // log contains the name but not the text.
    let fx = KnowledgeFixture::new_without_search().await;
    let audit_sink = fx.audit_sink();  // in-memory AuditWriter fake
    fx.call_tool("wiki.get", json!({ "name": "secrets/my-api-key-is-sk-ant-12345" })).await;
    let entries = audit_sink.entries();
    // Find the penny-dispatched entry
    let entry = entries.iter().find(|e| e.penny_dispatched).expect("audit entry");
    // tools_called must contain the tool NAME
    assert!(entry.tools_called.iter().any(|n| n == "wiki.get"));
    // The sensitive arg text must NOT appear anywhere in the serialized entry
    let serialized = serde_json::to_string(&entry).unwrap();
    assert!(!serialized.contains("sk-ant-12345"),
        "audit entry leaked argument text: {serialized}");
    assert!(!serialized.contains("my-api-key"),
        "audit entry leaked argument substring: {serialized}");
}

#[tokio::test]
async fn mcp_initialize_handshake_returns_server_info() {
    // KnowledgeFixture::raw_stdio_connection() spawns the manual_mcp server
    // over in-process stdio pipes without sending initialize first, so the
    // test can validate the handshake response directly.
    let fx = KnowledgeFixture::raw_stdio_connection().await;
    let resp = fx.send_raw(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" }
        }
    })).await;
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(resp["result"]["serverInfo"]["name"], "gadgetron-knowledge");
}
```

**`KnowledgeFixture` API additions** (for the new test above):

- `KnowledgeFixture::raw_stdio_connection() -> Self` — variant constructor that spawns the MCP server over in-process stdio pipes WITHOUT pre-sending `initialize`/`initialized`. Used to test the handshake itself. The existing `KnowledgeFixture::new_without_search()` and `KnowledgeFixture::new_with_search(config)` constructors DO pre-send the `initialize` + `initialized` exchange and are the default for all other conformance tests.
- `fx.send_raw(json: serde_json::Value) -> serde_json::Value` — writes the JSON as a newline-terminated string to the server stdin, reads and deserializes the JSON-RPC response from stdout. Returns the parsed response value for assertion.
- `fx.audit_sink() -> AuditSink` — returns a handle to an in-memory `AuditWriter` fake injected at fixture construction. The `AuditSink` struct holds a `Arc<Mutex<Vec<AuditEntry>>>` shared with the server; `audit_sink.entries()` returns a snapshot clone.
- `fx.call_tool(name: &str, args: serde_json::Value) -> serde_json::Value` — sends a `tools/call` JSON-RPC request and returns the deserialized result value.

**`AuditSink` / `AuditEntry` spec for M6 test:**
```rust
// crates/gadgetron-testing/src/audit_sink.rs

/// In-memory AuditWriter for tests. Injected via KnowledgeFixture.
pub struct AuditSink {
    entries: Arc<Mutex<Vec<AuditEntry>>>,
}

impl AuditSink {
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.entries.lock().unwrap().clone()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditEntry {
    pub penny_dispatched: bool,
    pub tools_called: Vec<String>,  // tool NAMES only — never argument values
    pub subprocess_duration_ms: i64,
    pub request_id: String,
}
```

### 10.6 Git corruption recovery tests — concrete setup

```rust
// tests/wiki_git_recovery.rs

#[test]
fn test_autocommit_on_locked_index() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki = Wiki::open_or_init(test_config(tmp.path())).unwrap();
    std::fs::write(tmp.path().join(".git/index.lock"), "").unwrap();
    let result = wiki.write("page1", "content");
    matches!(result, Err(WikiError::Kind {
        kind: WikiErrorKind::GitCorruption { reason, .. }, ..
    }) if reason.contains("index"));
}

#[test]
fn test_autocommit_on_detached_head() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki = Wiki::open_or_init(test_config(tmp.path())).unwrap();
    wiki.write("first", "body", "note").unwrap();
    // Concrete setup: detach HEAD via git2
    let head_oid = wiki.repo.head().unwrap().target().unwrap();
    wiki.repo.set_head_detached(head_oid).unwrap();
    // autocommit on detached HEAD should return GitCorruption
    let err = wiki.write("second", "body", "note").await.unwrap_err();
    match err {
        WikiError::Kind { kind: WikiErrorKind::GitCorruption { reason, .. }, .. } => {
            // The exact git2 error string may vary across versions; assert a
            // stable substring. If git2 upgrades break this, the test FAILS
            // loudly rather than passing vacuously.
            assert!(
                reason.contains("detached") || reason.contains("HEAD"),
                "expected detached-HEAD diagnostic, got: {reason}"
            );
        }
        other => panic!("expected GitCorruption, got {other:?}"),
    }
}

#[test]
fn test_autocommit_on_missing_objects() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki = Wiki::open_or_init(test_config(tmp.path())).unwrap();
    wiki.write("first", "body").unwrap();
    // Concrete setup: delete a random object file from .git/objects/
    // Find any .git/objects/{xx}/{rest} file and remove it
    let head_commit = wiki.repo.head().unwrap().peel_to_commit().unwrap();
    let tree_oid = head_commit.tree_id();
    let tree_hex = tree_oid.to_string();
    let object_path = tmp.path().join(".git/objects")
        .join(&tree_hex[0..2])
        .join(&tree_hex[2..]);
    std::fs::remove_file(&object_path).ok();
    // Reopen the repo to clear any caches
    let wiki2 = Wiki::open_or_init(test_config(tmp.path())).unwrap();
    let result = wiki2.write("second", "body");
    matches!(result, Err(WikiError::Kind {
        kind: WikiErrorKind::GitCorruption { .. }, ..
    }));
}

#[test]
fn test_autocommit_on_unresolved_merge_conflict() {
    // Setup: create two conflicting commits on same file, merge without resolving
    // Use `repo.merge_commits` or manually stage the conflict markers
    // Expect WikiError::Kind { kind: WikiErrorKind::Conflict { .. }, .. }
}
```

### 10.7 SearXNG fixtures

Four JSON files in `tests/fixtures/`:
- `searxng_response.json` — normal successful response with 3 results
- `searxng_empty_response.json` — `{ "results": [] }`
- `searxng_error_response.json` — HTTP 500 body shape
- `searxng_malformed.json` — intentionally truncated JSON

Tests in `src/search/searxng.rs #[cfg(test)] mod tests`:
- `parse_response_with_fixture`
- `parse_empty_results`
- `parse_malformed_json_returns_parse_error`
- `parse_error_text_does_not_include_response_body` — constructs the error and asserts it is the fixed string, not the input content

### 10.8 Snapshots

Location: `crates/gadgetron-knowledge/tests/snapshots/` (local to the crate, as `insta` convention).

Overview §9 file location table must be amended to include a row for crate-local snapshots (tracked in §11 handoff).

---

## 11. Open items / handoff to 02 and core

| Item | Owner | Blocks |
|---|---|---|
| `gadgetron-core::error::GadgetronError::Wiki { kind, message }` variant | **This spec** (01) | knowledge crate compilation |
| `gadgetron-core::error::GadgetronError::Penny { kind, message }` variant | 02 spec | penny crate compilation |
| `rmcp` maturity verification + feature-gate fallback | PM | MCP server impl |
| `--allowed-tools` enforcement verification (M4) | PM (before 02 finalization) | 02 security posture |
| `gadgetron-cli::cmd_mcp_serve` subcommand wiring in `main.rs` | 02 spec references | subcommand dispatch |
| `gadgetron-cli::cmd_penny_init` subcommand implementation | This spec (§1.1 stdout contract) + 02 spec dispatch | first-run UX |
| `docs/manual/penny.md` (Korean + English) with 2 disclosures (M7) | PM | P2A PR merge gate |
| Test file location table amendment for crate-local snapshots | Overview v2.1 (small edit) | cosmetic |

**Compile sequencing**: `gadgetron-knowledge` requires `gadgetron-core::error::GadgetronError::Wiki` variant to exist, which is added by this spec. `gadgetron-penny` requires `GadgetronError::Penny` variant, added by 02. Both core variant additions should land in a single core PR at the start of P2A implementation, before either knowledge or penny crate is coded.

**Post-v3 refactor**: split `WikiErrorKind::GitCorruption` into distinct `Io`, `Parse`, `GitCorruption` variants (requires gadgetron-core error enum extension). Currently `WikiError::Io` and `WikiError::Frontmatter` both map to `GitCorruption` with a disambiguating user message — see §8 `From<WikiError>` impl and the inline comment there.

**ADR-P2A-01 Part 2 RESOLVED 2026-04-13** — stdin contract is **Option B (TEXT)**: plain text prompt on stdin, `--input-format text` is the default. `feed_stdin` in `02-penny-agent.md` §5 is now unconditional — no feature flag, no branches. The `fake_claude::stdin_echo` scenario assertion is fixed on the text byte-sequence format described in §5. ADR-P2A-01 is ACCEPTED; `CLAUDE_CODE_MIN_VERSION = 2.1.104`.

---

## 12. Review provenance

| Reviewer | Round | v1 verdict | v2 changes |
|---|---|---|---|
| chief-architect | Round 0 + Round 3 | REVISE (B1, B2, N1-N4) | B1 single canonical WikiErrorKind in core, B2 pure resolve_path, N1 git2::Oid, N2 HashMap index, N3 manual MCP fallback inline, N4 compile-sequencing note in §11 |
| dx-product-lead | Round 1.5 usability | REVISE (A1-A6) | A1 rewrote wiki_search + web_search descriptions, A2 max_results default 5, A3 PageTooLarge error text includes bytes/limit, A4 penny init stdout §1.1, A5 eprintln! fallback, A6 Wiki variant moved to 01 scope |
| security-compliance-lead | Round 1.5 security | REVISE (A1-A8) | A1 O_NOFOLLOW on write, A2 create_dir_all moved to caller, A3 PEM/AKIA/GCP BLOCK patterns + CredentialBlocked variant, A4 SearchError::Parse static strings, A5 P2C tag + redirect::limited(3), A6 mixed traversal proptest arms, A7 git history inline SECURITY comment, A8 %2e%2e non-threat doc |
| qa-test-architect | Round 2 testability | REVISE (3 blockers + 6 non-blockers) | happy-path proptest, malformed link proptest arm, concrete detached_head + missing_objects setup, frontmatter edge tests, URL validation edges, MCP idempotency/unknown/malformed tests, SearXNG fixture variants, InvertedIndex Mutex clarification |
| chief-architect | Round 2 | APPROVE WITH MINOR | NIT3: `#[cfg(use-rmcp)]` dead code removed — P2A uses manual path unconditionally; NIT4: WikiError::Io/Frontmatter→GitCorruption message reworded; DET2: validate() placeholder replaced; DET3: git_config_get undefined → git2::Config::open_default() — resolved in v3 |
| dx-product-lead | Round 2 | APPROVE WITH MINOR | B2: --wiki-path flag behavior specified; DET3: SearchConfig field names pinned; DET4: detached-HEAD test uses assert not matches!; NIT1: [ok] vs [OK] note added — resolved in v3 |
| security-compliance-lead | Round 2 | REVISE (4 blockers) | B1-B4 are in 02-penny-agent.md scope (subprocess env, audit request_id, claude_binary validation, redact regex) — not applicable to 01; 01-specific items resolved in v3 |
| qa-test-architect | Round 2 | APPROVE WITH MINOR | NB1: proptest_config added to all 4 proptest blocks; NB2: mcp_initialize_handshake test added; GAP4/NIT3: audit log stub replaced with concrete assertions; raw_stdio_connection fixture variant specified — resolved in v3 |

Next round: implementation (all four Round 2 reviewers: APPROVE WITH MINOR or resolved items).

*End of 01-knowledge-layer.md draft v3. Round 2 review addressed.*
