//! Knowledge-layer configuration.
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §7`.
//!
//! Exposes two layered config types:
//!
//! - `KnowledgeConfig` — the toml surface under `[knowledge]`. Includes
//!   `wiki_path`, `wiki_autocommit`, `wiki_git_author`, `wiki_max_page_bytes`,
//!   optional nested `[knowledge.search]` / `[knowledge.embedding]`
//!   sections, and a `[knowledge.reindex]` policy block.
//! - `WikiConfig` — the internal runtime config consumed by `Wiki::open`.
//!   Derived from `KnowledgeConfig::to_wiki_config()` after git-author
//!   auto-detection.

use gadgetron_core::agent::config::{EnvResolver, StdEnv};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the semantic embedding subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Provider name. P2A supports only `openai_compat`.
    #[serde(default = "default_embedding_provider")]
    pub provider: String,

    /// Base URL for the provider. Requests POST to `{base_url}/embeddings`.
    #[serde(default = "default_embedding_base_url")]
    pub base_url: String,

    /// Name of the environment variable that stores the embedding API key.
    #[serde(default = "default_embedding_api_key_env")]
    pub api_key_env: String,

    /// Embedding model name.
    #[serde(default = "default_embedding_model")]
    pub model: String,

    /// Expected embedding dimension.
    #[serde(default = "default_embedding_dimension")]
    pub dimension: usize,

    /// Whether wiki writes wait for embedding/index work.
    #[serde(default)]
    pub write_mode: EmbeddingWriteMode,

    /// Per-request timeout in seconds.
    #[serde(default = "default_embedding_timeout_secs")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingWriteMode {
    #[default]
    Async,
    Sync,
}

fn default_embedding_provider() -> String {
    "openai_compat".to_string()
}
fn default_embedding_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}
fn default_embedding_api_key_env() -> String {
    "OPENAI_API_KEY".to_string()
}
fn default_embedding_model() -> String {
    "text-embedding-3-small".to_string()
}
fn default_embedding_dimension() -> usize {
    1536
}
fn default_embedding_timeout_secs() -> u64 {
    30
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: default_embedding_provider(),
            base_url: default_embedding_base_url(),
            api_key_env: default_embedding_api_key_env(),
            model: default_embedding_model(),
            dimension: default_embedding_dimension(),
            write_mode: EmbeddingWriteMode::default(),
            timeout_secs: default_embedding_timeout_secs(),
        }
    }
}

impl EmbeddingConfig {
    pub fn validate(&self) -> Result<reqwest::Url, String> {
        self.validate_with_env(&StdEnv)
    }

    pub fn validate_with_env(&self, env: &dyn EnvResolver) -> Result<reqwest::Url, String> {
        if self.provider != "openai_compat" {
            return Err(format!(
                "knowledge.embedding.provider currently supports only \"openai_compat\"; got {:?}",
                self.provider
            ));
        }
        if !(1..=8192).contains(&self.dimension) {
            return Err(format!(
                "knowledge.embedding.dimension must be in [1, 8192]; got {}",
                self.dimension
            ));
        }
        if !(1..=300).contains(&self.timeout_secs) {
            return Err(format!(
                "knowledge.embedding.timeout_secs must be in [1, 300]; got {}",
                self.timeout_secs
            ));
        }
        if self.model.trim().is_empty() {
            return Err("knowledge.embedding.model must not be empty".to_string());
        }
        if self.api_key_env.trim().is_empty() {
            return Err("knowledge.embedding.api_key_env must not be empty".to_string());
        }

        let url = reqwest::Url::parse(&self.base_url).map_err(|e| {
            format!(
                "knowledge.embedding.base_url must be a valid URL: {e} \
                 (got {val:?})",
                val = self.base_url
            )
        })?;
        match url.scheme() {
            "http" | "https" => {}
            other => {
                return Err(format!(
                    "knowledge.embedding.base_url scheme must be http or https; got {other:?}"
                ))
            }
        }

        let Some(value) = env.get(&self.api_key_env) else {
            return Err(format!(
                "knowledge.embedding.api_key_env {:?} is not set in the environment",
                self.api_key_env
            ));
        };
        if value.trim().is_empty() {
            return Err(format!(
                "knowledge.embedding.api_key_env {:?} is set but empty",
                self.api_key_env
            ));
        }

        Ok(url)
    }
}

/// Configuration for the web search subsystem.
///
/// All fields serialize with snake_case keys so the toml shape matches
/// `[knowledge.search]` in `gadgetron.toml`. `searxng_url` is stored as a
/// `String` to keep serde simple — `validate()` parses it into a
/// `reqwest::Url` and enforces the http(s) scheme constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// SearXNG instance base URL. The client issues GETs to
    /// `{searxng_url}/search?q=...&format=json`.
    ///
    /// Env: `GADGETRON_KNOWLEDGE_SEARCH_SEARXNG_URL`
    pub searxng_url: String,

    /// HTTP timeout for a single search request in seconds.
    /// Range [1, 60]. Default 10.
    ///
    /// Env: `GADGETRON_KNOWLEDGE_SEARCH_TIMEOUT_SECS`
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Maximum number of results to return from a single search. The
    /// SearXNG server may return more; we truncate to this count after
    /// parsing. Default 10.
    ///
    /// Env: `GADGETRON_KNOWLEDGE_SEARCH_MAX_RESULTS`
    #[serde(default = "default_max_results")]
    pub max_results: u16,
}

fn default_timeout_secs() -> u64 {
    10
}
fn default_max_results() -> u16 {
    10
}

impl SearchConfig {
    /// Validate the config. Out-of-range fields produce a static error
    /// message suitable for an operator to act on.
    ///
    /// Also parses `searxng_url` into a `reqwest::Url` and returns the
    /// parsed form alongside `Ok`. The caller stashes this into
    /// `SearxngClient::new` to avoid re-parsing at every request.
    pub fn validate(&self) -> Result<reqwest::Url, String> {
        if !(1..=60).contains(&self.timeout_secs) {
            return Err(format!(
                "knowledge.search.timeout_secs must be in [1, 60]; got {}",
                self.timeout_secs
            ));
        }
        if self.max_results == 0 || self.max_results > 100 {
            return Err(format!(
                "knowledge.search.max_results must be in [1, 100]; got {}",
                self.max_results
            ));
        }
        let url = reqwest::Url::parse(&self.searxng_url).map_err(|e| {
            format!(
                "knowledge.search.searxng_url must be a valid URL: {e} \
                 (got {val:?})",
                val = self.searxng_url
            )
        })?;
        match url.scheme() {
            "http" | "https" => {}
            other => {
                return Err(format!(
                    "knowledge.search.searxng_url scheme must be http or https; got {other:?}"
                ))
            }
        }
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(url: &str) -> SearchConfig {
        SearchConfig {
            searxng_url: url.to_string(),
            timeout_secs: 10,
            max_results: 10,
        }
    }

    #[test]
    fn default_config_validates() {
        assert!(cfg("http://127.0.0.1:8888").validate().is_ok());
        assert!(cfg("https://search.example.com").validate().is_ok());
    }

    #[test]
    fn rejects_zero_max_results() {
        let mut c = cfg("http://localhost");
        c.max_results = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_oversize_max_results() {
        let mut c = cfg("http://localhost");
        c.max_results = 200;
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_out_of_range_timeout() {
        let mut c = cfg("http://localhost");
        c.timeout_secs = 0;
        assert!(c.validate().is_err());
        c.timeout_secs = 61;
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_non_http_scheme() {
        let c = cfg("ftp://example.com");
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_garbage_url() {
        let c = cfg("not a valid url at all");
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_returns_parsed_url() {
        let c = cfg("http://127.0.0.1:8888");
        let parsed = c.validate().expect("valid");
        assert_eq!(parsed.scheme(), "http");
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert_eq!(parsed.port(), Some(8888));
    }
}

/// Startup / audit policy for semantic wiki indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReindexConfig {
    /// Run reindex automatically when the server boots.
    #[serde(default = "default_true")]
    pub on_startup: bool,

    /// Startup behavior. `async` remains the default P2A choice.
    #[serde(default)]
    pub on_startup_mode: ReindexStartupMode,

    /// Age threshold used by `gadgetron wiki audit`.
    #[serde(default = "default_stale_threshold_days")]
    pub stale_threshold_days: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReindexStartupMode {
    #[default]
    Async,
    Sync,
    Incremental,
    Full,
}

fn default_stale_threshold_days() -> u16 {
    90
}

impl Default for ReindexConfig {
    fn default() -> Self {
        Self {
            on_startup: default_true(),
            on_startup_mode: ReindexStartupMode::default(),
            stale_threshold_days: default_stale_threshold_days(),
        }
    }
}

impl ReindexConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=3650).contains(&self.stale_threshold_days) {
            return Err(format!(
                "knowledge.reindex.stale_threshold_days must be in [1, 3650]; got {}",
                self.stale_threshold_days
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// KnowledgeConfig — toml surface under `[knowledge]`
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeConfig {
    /// Wiki storage root. Created + git-init'ed on first run if absent.
    /// Env: `GADGETRON_KNOWLEDGE_WIKI_PATH`
    ///
    /// # P2A shortcut — single-wiki shape
    ///
    /// P2A ships a single-user MVP, so `[knowledge]` declares exactly one
    /// wiki root here. The office-hours design doc of 2026-04-16
    /// (Type 1 Decision #3) records that this shape will grow into a
    /// named registry in P2B+:
    ///
    /// ```toml
    /// # Future shape (P2B+, NOT yet implemented):
    /// [[knowledge.wiki_registry]]
    /// name  = "private"
    /// path  = "/srv/gadgetron/wiki/private"
    /// scope = "private"    # WikiScope::Private — visible only to owner_id
    ///
    /// [[knowledge.wiki_registry]]
    /// name  = "team"
    /// path  = "/srv/gadgetron/wiki/team"
    /// scope = "team"       # WikiScope::Team — visible to tenant_id members
    ///
    /// [[knowledge.wiki_registry]]
    /// name  = "public"
    /// path  = "/srv/gadgetron/wiki/public"
    /// scope = "public"     # WikiScope::Public — visible to all tenants
    /// ```
    ///
    /// ## Forward-compat guarantee
    ///
    /// When `wiki_registry` lands, the loader will treat `wiki_path` as
    /// sugar for a single `WikiEntry { name: "default", path: wiki_path,
    /// scope: WikiScope::Private }`. Operators whose `gadgetron.toml` only
    /// sets `wiki_path` keep working with zero config churn — the upgrade
    /// is a pure superset.
    ///
    /// ## Why this matters now
    ///
    /// The multi-tenant audit columns landed in PR A7.5 (`owner_id`,
    /// `tenant_id` on `ToolAuditEvent::ToolCallCompleted`) specifically so
    /// the audit trail can grow into this registry without a schema
    /// migration. Wiki scoping is the matching write-side evolution — the
    /// `WikiScope` enum is the piece that ties `owner_id`/`tenant_id` back
    /// to which pages each principal can see.
    pub wiki_path: PathBuf,

    /// Auto-commit on every write. If false, writes are staged but
    /// never committed — the operator handles commits manually.
    /// Default true.
    /// Env: `GADGETRON_KNOWLEDGE_WIKI_AUTOCOMMIT`
    #[serde(default = "default_true")]
    pub wiki_autocommit: bool,

    /// Git author identity in `"Name <email>"` format. If `None`,
    /// `KnowledgeConfig::to_wiki_config` auto-detects from the global
    /// gitconfig and falls back to `"Penny <penny@gadgetron.local>"`.
    /// Env: `GADGETRON_KNOWLEDGE_WIKI_GIT_AUTHOR`
    #[serde(default)]
    pub wiki_git_author: Option<String>,

    /// Maximum bytes per wiki page. Range [1, 100 MiB]. Default 1 MiB.
    /// Env: `GADGETRON_KNOWLEDGE_WIKI_MAX_PAGE_BYTES`
    #[serde(default = "default_max_page_bytes")]
    pub wiki_max_page_bytes: usize,

    /// Nested `[knowledge.search]` block. When `None`, the `web.search`
    /// MCP tool is omitted from the `KnowledgeToolProvider` manifest.
    #[serde(default)]
    pub search: Option<SearchConfig>,

    /// Optional embedding/index configuration. Left unset until semantic
    /// indexing is explicitly enabled.
    #[serde(default)]
    pub embedding: Option<EmbeddingConfig>,

    /// Reindex and audit policy. Defaults stay active even when semantic
    /// indexing is not yet wired into runtime calls.
    #[serde(default)]
    pub reindex: ReindexConfig,
}

fn default_true() -> bool {
    true
}
fn default_max_page_bytes() -> usize {
    1_048_576
}

impl KnowledgeConfig {
    /// Extract a `[knowledge]` section from a raw `gadgetron.toml`
    /// content string. Returns `None` when the section is absent —
    /// callers treat a missing `[knowledge]` as "knowledge layer
    /// disabled, don't register Penny".
    ///
    /// Returns `Err(String)` only on malformed toml OR when the
    /// `[knowledge]` section is present but fails deserialization
    /// (e.g. wrong field types). A missing section is NOT an error.
    pub fn extract_from_toml_str(raw: &str) -> Result<Option<Self>, String> {
        let value: toml::Value = raw.parse().map_err(|e| format!("toml parse failed: {e}"))?;
        let Some(section) = value.get("knowledge") else {
            return Ok(None);
        };
        let cfg: Self = <Self as serde::Deserialize>::deserialize(section.clone())
            .map_err(|e| format!("[knowledge] section failed to deserialize: {e}"))?;
        Ok(Some(cfg))
    }

    /// Rewrite relative `wiki_path` to an absolute path resolved against
    /// `config_dir` (the directory containing `gadgetron.toml`). Absolute
    /// paths are left untouched.
    ///
    /// Why: `gadgetron mcp serve` runs as a grandchild process spawned by
    /// Claude Code with cwd pinned to `~/.gadgetron/penny/work/`, so a
    /// relative `wiki_path = "./.gadgetron/wiki"` in the operator's TOML
    /// would resolve to the wrong directory. Resolving against the config
    /// file's own directory makes the path cwd-independent.
    pub fn resolve_relative_paths(&mut self, config_dir: &std::path::Path) {
        if self.wiki_path.is_relative() {
            self.wiki_path = config_dir.join(&self.wiki_path);
        }
    }

    /// Validates at load time. Rules:
    /// - `wiki_path` parent must exist (wiki_path itself may not —
    ///   the knowledge layer initializes the repo lazily on first use).
    /// - `wiki_max_page_bytes` must be in [1, 100 MiB]
    /// - Nested `search` config (if present) passes its own validate.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_with_env(&StdEnv)
    }

    /// Same as `validate`, but with an injected environment resolver so
    /// tests can exercise `embedding.api_key_env` without mutating the
    /// process-global environment.
    pub fn validate_with_env(&self, env: &dyn EnvResolver) -> Result<(), String> {
        let parent = self.wiki_path.parent().ok_or_else(|| {
            format!(
                "wiki_path must not be the filesystem root: {}",
                self.wiki_path.display()
            )
        })?;
        // Empty parent is the current directory — always exists.
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(format!(
                "wiki_path parent does not exist: {}",
                parent.display()
            ));
        }
        if !(1..=104_857_600).contains(&self.wiki_max_page_bytes) {
            return Err(format!(
                "wiki_max_page_bytes must be in [1, 100 MiB]; got {}",
                self.wiki_max_page_bytes
            ));
        }
        if let Some(sc) = &self.search {
            sc.validate()
                .map_err(|e| format!("knowledge.search: {e}"))?;
        }
        if let Some(ec) = &self.embedding {
            ec.validate_with_env(env)
                .map_err(|e| format!("knowledge.embedding: {e}"))?;
        }
        self.reindex
            .validate()
            .map_err(|e| format!("knowledge.reindex: {e}"))?;
        Ok(())
    }

    /// Build a runtime `WikiConfig` from this operator-facing config.
    /// Parses `wiki_git_author` (or auto-detects via git2 Config) into
    /// name + email pairs suitable for `git2::Signature::now`.
    pub fn to_wiki_config(&self) -> Result<WikiConfig, String> {
        let (git_author_name, git_author_email) = match &self.wiki_git_author {
            Some(author) => parse_author(author)?,
            None => autodetect_git_author_or_fallback(),
        };
        Ok(WikiConfig {
            root: self.wiki_path.clone(),
            autocommit: self.wiki_autocommit,
            git_author_name,
            git_author_email,
            max_page_bytes: self.wiki_max_page_bytes,
        })
    }
}

fn parse_author(author: &str) -> Result<(String, String), String> {
    // "Name <email>" format.
    let lt = author.find(" <").ok_or_else(|| {
        format!("wiki_git_author must be in 'Name <email>' format, got: {author}")
    })?;
    let gt = author.rfind('>').ok_or_else(|| {
        format!("wiki_git_author must be in 'Name <email>' format, got: {author}")
    })?;
    if gt <= lt + 2 {
        return Err(format!(
            "wiki_git_author must be in 'Name <email>' format, got: {author}"
        ));
    }
    let name = author[..lt].trim().to_string();
    let email = author[lt + 2..gt].trim().to_string();
    if name.is_empty() || email.is_empty() {
        return Err(format!(
            "wiki_git_author must be in 'Name <email>' format, got: {author}"
        ));
    }
    Ok((name, email))
}

fn autodetect_git_author_or_fallback() -> (String, String) {
    if let Ok(config) = git2::Config::open_default() {
        let name = config.get_string("user.name").ok();
        let email = config.get_string("user.email").ok();
        if let (Some(n), Some(e)) = (name, email) {
            if !n.is_empty() && !e.is_empty() {
                return (n, e);
            }
        }
    }
    // Fallback when the system gitconfig is missing or empty.
    tracing::warn!(
        target: "knowledge_config",
        "git config user.name / user.email not set — falling back to 'Penny <penny@gadgetron.local>'"
    );
    ("Penny".to_string(), "penny@gadgetron.local".to_string())
}

// ---------------------------------------------------------------------------
// WikiConfig — internal runtime config
// ---------------------------------------------------------------------------

/// Runtime-derived wiki config consumed by `Wiki::open`. Produced by
/// `KnowledgeConfig::to_wiki_config()`.
#[derive(Debug, Clone)]
pub struct WikiConfig {
    pub root: PathBuf,
    pub autocommit: bool,
    pub git_author_name: String,
    pub git_author_email: String,
    pub max_page_bytes: usize,
}

#[cfg(test)]
mod knowledge_config_tests {
    use super::*;
    use gadgetron_core::agent::config::FakeEnv;

    #[test]
    fn parse_author_valid() {
        let (n, e) = parse_author("Alice <alice@example.com>").unwrap();
        assert_eq!(n, "Alice");
        assert_eq!(e, "alice@example.com");
    }

    #[test]
    fn parse_author_multi_word_name() {
        let (n, e) = parse_author("Alice Smith <alice.smith@example.com>").unwrap();
        assert_eq!(n, "Alice Smith");
        assert_eq!(e, "alice.smith@example.com");
    }

    #[test]
    fn parse_author_missing_angle() {
        assert!(parse_author("Alice alice@example.com").is_err());
    }

    #[test]
    fn parse_author_empty_email() {
        assert!(parse_author("Alice <>").is_err());
    }

    #[test]
    fn knowledge_config_validates_with_existing_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = KnowledgeConfig {
            wiki_path: tmp.path().join("wiki"),
            wiki_autocommit: true,
            wiki_git_author: Some("Test <test@example.com>".into()),
            wiki_max_page_bytes: 1024,
            search: None,
            embedding: None,
            reindex: ReindexConfig::default(),
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn knowledge_config_rejects_missing_parent() {
        let cfg = KnowledgeConfig {
            wiki_path: "/definitely/not/here/ever/wiki".into(),
            wiki_autocommit: true,
            wiki_git_author: None,
            wiki_max_page_bytes: 1024,
            search: None,
            embedding: None,
            reindex: ReindexConfig::default(),
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn knowledge_config_rejects_oversize_max_page_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = KnowledgeConfig {
            wiki_path: tmp.path().join("wiki"),
            wiki_autocommit: true,
            wiki_git_author: None,
            wiki_max_page_bytes: 200_000_000, // 200 MiB
            search: None,
            embedding: None,
            reindex: ReindexConfig::default(),
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn knowledge_config_to_wiki_config_with_explicit_author() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = KnowledgeConfig {
            wiki_path: tmp.path().join("wiki"),
            wiki_autocommit: false,
            wiki_git_author: Some("Penny <k@g.local>".into()),
            wiki_max_page_bytes: 2048,
            search: None,
            embedding: None,
            reindex: ReindexConfig::default(),
        };
        let wc = cfg.to_wiki_config().unwrap();
        assert_eq!(wc.git_author_name, "Penny");
        assert_eq!(wc.git_author_email, "k@g.local");
        assert_eq!(wc.max_page_bytes, 2048);
        assert!(!wc.autocommit);
    }

    #[test]
    fn embedding_config_validates_with_injected_env() {
        let env = FakeEnv::new().with("OPENAI_API_KEY", "sk-test");
        let parsed = EmbeddingConfig::default()
            .validate_with_env(&env)
            .expect("valid config");
        assert_eq!(parsed.scheme(), "https");
    }

    #[test]
    fn embedding_config_rejects_missing_api_key_env() {
        let env = FakeEnv::new();
        let err = EmbeddingConfig::default()
            .validate_with_env(&env)
            .expect_err("missing env must fail");
        assert!(err.contains("OPENAI_API_KEY"));
    }

    #[test]
    fn embedding_config_rejects_invalid_dimension() {
        let env = FakeEnv::new().with("OPENAI_API_KEY", "sk-test");
        let cfg = EmbeddingConfig {
            dimension: 0,
            ..EmbeddingConfig::default()
        };
        let err = cfg
            .validate_with_env(&env)
            .expect_err("dimension 0 must fail");
        assert!(err.contains("dimension"));
    }

    #[test]
    fn reindex_config_rejects_out_of_range_stale_threshold() {
        let cfg = ReindexConfig {
            stale_threshold_days: 0,
            ..ReindexConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn knowledge_config_extracts_embedding_and_reindex_sections() {
        let raw = r#"
[knowledge]
wiki_path = "/tmp/wiki"
wiki_autocommit = true
wiki_max_page_bytes = 1048576

[knowledge.embedding]
provider = "openai_compat"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
model = "text-embedding-3-small"
dimension = 2
write_mode = "sync"
timeout_secs = 45

[knowledge.reindex]
on_startup = false
on_startup_mode = "full"
stale_threshold_days = 30
"#;

        let cfg = KnowledgeConfig::extract_from_toml_str(raw)
            .expect("parse")
            .expect("knowledge section");
        let embedding = cfg.embedding.expect("embedding section");
        assert_eq!(embedding.provider, "openai_compat");
        assert_eq!(embedding.dimension, 2);
        assert_eq!(embedding.write_mode, EmbeddingWriteMode::Sync);
        assert_eq!(cfg.reindex.on_startup_mode, ReindexStartupMode::Full);
        assert_eq!(cfg.reindex.stale_threshold_days, 30);
    }
}
