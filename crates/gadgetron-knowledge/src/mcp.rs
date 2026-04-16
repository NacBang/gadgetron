//! MCP tool provider for the knowledge layer.
//!
//! Implements `gadgetron_core::agent::tools::McpToolProvider` for the
//! `"knowledge"` category. Exposes five tools:
//!
//! - `wiki.list` — T1 Read — list all pages
//! - `wiki.get` — T1 Read — fetch a page by name
//! - `wiki.search` — T1 Read — full-text search
//! - `wiki.write` — T2 Write — create or overwrite a page
//! - `web.search` — T1 Read — optional, present only when
//!   `KnowledgeConfig.search` is configured
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §6.2 + §6.3`.
//!
//! # Error mapping
//!
//! Every method returns `Result<ToolResult, McpError>` per the trait
//! contract. `WikiError` values are mapped to generic `McpError`
//! variants at the boundary so:
//!
//! - Raw user input does not leak back through the error text
//! - Request-specific details (`bytes`/`limit` for PageTooLarge,
//!   `pattern` name for CredentialBlocked) DO surface — they're
//!   operator-actionable, not attacker-useful
//! - Path-traversal attempts map to `McpError::InvalidArgs` with a
//!   fixed string (no path echo)
//! - Git corruption and I/O errors map to `McpError::Execution` with
//!   a generic "storage error" message
//!
//! # Web search
//!
//! `web.search` is registered only when `KnowledgeConfig.search` is set
//! at construction time. Providers with no search configured simply
//! omit the tool from their manifest — the agent never sees it. This
//! is preferred over runtime checks because Claude Code caches the
//! tool list from the manifest.

use std::sync::Arc;

use async_trait::async_trait;
use gadgetron_core::agent::tools::{McpError, McpToolProvider, Tier, ToolResult, ToolSchema};
use gadgetron_core::error::WikiErrorKind;
use serde_json::{json, Value};

use crate::config::KnowledgeConfig;
use crate::error::{SearchError, WikiError};
use crate::search::{SearxngClient, WebSearch};
use crate::wiki::Wiki;

/// The concrete `McpToolProvider` for knowledge-layer tools.
pub struct KnowledgeToolProvider {
    wiki: Arc<Wiki>,
    web_search: Option<Arc<dyn WebSearch>>,
    max_search_results: usize,
}

impl KnowledgeToolProvider {
    /// Build the provider from a validated `KnowledgeConfig`.
    ///
    /// - Opens/initializes the wiki repo at `config.wiki_path`.
    /// - Constructs a `SearxngClient` if `config.search` is set.
    /// - Records `config.search.max_results` as the cap for
    ///   `wiki.search` and `web.search` result counts (both tools
    ///   share the cap for operator simplicity).
    pub fn new(config: KnowledgeConfig) -> Result<Self, WikiError> {
        let wiki_config = config.to_wiki_config().map_err(|msg| {
            WikiError::kind_with_message(
                WikiErrorKind::GitCorruption {
                    path: String::new(),
                    reason: msg.clone(),
                },
                format!("knowledge config translation failed: {msg}"),
            )
        })?;
        let wiki = Arc::new(Wiki::open(wiki_config)?);

        let (web_search, max_search_results): (Option<Arc<dyn WebSearch>>, usize) = match config
            .search
        {
            Some(sc) => {
                let max = sc.max_results as usize;
                let client = SearxngClient::new(&sc).map_err(|e| {
                    WikiError::kind_with_message(
                        WikiErrorKind::GitCorruption {
                            path: String::new(),
                            reason: format!("searxng client construction failed: {e}"),
                        },
                        "failed to build SearXNG client from [knowledge.search] config".to_string(),
                    )
                })?;
                (Some(Arc::new(client) as Arc<dyn WebSearch>), max.max(1))
            }
            None => (None, 10),
        };

        Ok(Self {
            wiki,
            web_search,
            max_search_results,
        })
    }

    /// Construct directly from an already-opened `Wiki` + optional
    /// `WebSearch`. Used by tests and by callers that want to share a
    /// `Wiki` across multiple provider instances.
    pub fn with_components(
        wiki: Arc<Wiki>,
        web_search: Option<Arc<dyn WebSearch>>,
        max_search_results: usize,
    ) -> Self {
        Self {
            wiki,
            web_search,
            max_search_results: max_search_results.max(1),
        }
    }
}

// ---------------------------------------------------------------------------
// McpToolProvider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl McpToolProvider for KnowledgeToolProvider {
    fn category(&self) -> &'static str {
        "knowledge"
    }

    fn tool_schemas(&self) -> Vec<ToolSchema> {
        let mut out = vec![
            schema_wiki_list(),
            schema_wiki_get(),
            schema_wiki_search(),
            schema_wiki_write(),
            schema_wiki_delete(),
            schema_wiki_rename(),
        ];
        if self.web_search.is_some() {
            out.push(schema_web_search());
        }
        out
    }

    async fn call(&self, name: &str, args: Value) -> Result<ToolResult, McpError> {
        match name {
            "wiki.list" => self.call_wiki_list().await,
            "wiki.get" => self.call_wiki_get(args),
            "wiki.search" => self.call_wiki_search(args),
            "wiki.write" => self.call_wiki_write(args),
            "wiki.delete" => self.call_wiki_delete(args),
            "wiki.rename" => self.call_wiki_rename(args),
            "web.search" => self.call_web_search(args).await,
            other => Err(McpError::UnknownTool(other.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool schemas
// ---------------------------------------------------------------------------

fn schema_wiki_list() -> ToolSchema {
    ToolSchema {
        name: "wiki.list".into(),
        tier: Tier::Read,
        description: "List all pages in the Kairos wiki. Returns page names \
            (use forward slashes for subdirectories). Call wiki.list first to \
            discover what pages exist before searching or fetching by name."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_wiki_get() -> ToolSchema {
    ToolSchema {
        name: "wiki.get".into(),
        tier: Tier::Read,
        description: "Fetch a wiki page by its logical name. Use wiki.get \
            when you already know the exact page name (e.g. from a previous \
            wiki.list or wiki.search result). Page names use forward slashes \
            for subdirectories and do NOT include the .md suffix."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 1, "maxLength": 256 }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_wiki_search() -> ToolSchema {
    ToolSchema {
        name: "wiki.search".into(),
        tier: Tier::Read,
        description: "Search wiki pages by keyword when you don't know the \
            exact page name. Returns up to max_results matching pages with a \
            relevance score. Use wiki.get when you know the exact page name."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": 512 },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "default": 5
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_wiki_write() -> ToolSchema {
    ToolSchema {
        name: "wiki.write".into(),
        tier: Tier::Write,
        description: "Write or overwrite a wiki page. Content is markdown. \
            Auto-commits to git on success. Default size limit is 1 MiB. Path \
            must not contain '..' or absolute paths. Writes containing \
            unambiguous credentials (PEM keys, AWS access keys, GCP service \
            accounts) are rejected before touching disk."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 1, "maxLength": 256 },
                "content": { "type": "string", "minLength": 0 }
            },
            "required": ["name", "content"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_wiki_delete() -> ToolSchema {
    ToolSchema {
        name: "wiki.delete".into(),
        tier: Tier::Write,
        description: "Delete a wiki page. Soft delete by default: the page is \
            moved to `_archived/<YYYY-MM-DD>/<name>.md` with a git commit. \
            The operator can permanently remove archived pages later. Use \
            when the user asks to remove stale or wrong information."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 1, "maxLength": 256 }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_wiki_rename() -> ToolSchema {
    ToolSchema {
        name: "wiki.rename".into(),
        tier: Tier::Write,
        description: "Rename or move a wiki page. Both `from` and `to` are \
            page names without the `.md` extension. Forward slashes are \
            treated as subdirectories. Fails with a conflict error if the \
            destination already exists. Use for reorganizing or correcting \
            page paths."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "from": { "type": "string", "minLength": 1, "maxLength": 256 },
                "to":   { "type": "string", "minLength": 1, "maxLength": 256 }
            },
            "required": ["from", "to"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_web_search() -> ToolSchema {
    ToolSchema {
        name: "web.search".into(),
        tier: Tier::Read,
        description: "Search the web for information not in the wiki. Returns \
            up to 10 results (title, URL, snippet) via a self-hosted SearXNG \
            proxy. Use when the user's question cannot be answered from the \
            wiki alone."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": 512 }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

// ---------------------------------------------------------------------------
// Tool dispatch — WikiError → McpError mapping
// ---------------------------------------------------------------------------

impl KnowledgeToolProvider {
    async fn call_wiki_list(&self) -> Result<ToolResult, McpError> {
        let entries = self.wiki.list().map_err(map_wiki_err_generic)?;
        Ok(ToolResult {
            content: json!({
                "pages": entries.into_iter().map(|e| e.name).collect::<Vec<_>>()
            }),
            is_error: false,
        })
    }

    fn call_wiki_get(&self, args: Value) -> Result<ToolResult, McpError> {
        let name = required_string_arg(&args, "name")?;
        match self.wiki.read(&name) {
            Ok(content) => Ok(ToolResult {
                content: json!({
                    "name": name,
                    "content": content,
                }),
                is_error: false,
            }),
            Err(e) => Err(map_wiki_err_read(e, &name)),
        }
    }

    fn call_wiki_search(&self, args: Value) -> Result<ToolResult, McpError> {
        let query = required_string_arg(&args, "query")?;
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(5)
            .min(50);
        let hits = self
            .wiki
            .search(&query, max_results)
            .map_err(map_wiki_err_generic)?;
        Ok(ToolResult {
            content: json!({
                "query": query,
                "hits": hits,
            }),
            is_error: false,
        })
    }

    fn call_wiki_write(&self, args: Value) -> Result<ToolResult, McpError> {
        let name = required_string_arg(&args, "name")?;
        let content = required_string_arg(&args, "content")?;
        match self.wiki.write(&name, &content) {
            Ok(result) => Ok(ToolResult {
                content: json!({
                    "name": result.name,
                    "bytes": result.bytes,
                    "commit_oid": result.commit_oid,
                }),
                is_error: false,
            }),
            Err(e) => Err(map_wiki_err_write(e, &name)),
        }
    }

    fn call_wiki_delete(&self, args: Value) -> Result<ToolResult, McpError> {
        let name = required_string_arg(&args, "name")?;
        match self.wiki.delete(&name) {
            Ok(archive_path) => Ok(ToolResult {
                content: json!({
                    "name": name,
                    "archived_to": archive_path,
                    "message": "soft-deleted; archived copy preserved in _archived/",
                }),
                is_error: false,
            }),
            Err(e) => Err(map_wiki_err_read(e, &name)),
        }
    }

    fn call_wiki_rename(&self, args: Value) -> Result<ToolResult, McpError> {
        let from = required_string_arg(&args, "from")?;
        let to = required_string_arg(&args, "to")?;
        match self.wiki.rename(&from, &to) {
            Ok(result) => Ok(ToolResult {
                content: json!({
                    "from": from,
                    "to": result.name,
                    "commit_oid": result.commit_oid,
                }),
                is_error: false,
            }),
            Err(e) => Err(map_wiki_err_write(e, &to)),
        }
    }

    async fn call_web_search(&self, args: Value) -> Result<ToolResult, McpError> {
        let Some(client) = &self.web_search else {
            return Err(McpError::Denied {
                reason: "web.search is not configured on this server".into(),
            });
        };
        let query = required_string_arg(&args, "query")?;
        match client.search(&query).await {
            Ok(results) => {
                let capped: Vec<_> = results.into_iter().take(self.max_search_results).collect();
                Ok(ToolResult {
                    content: json!({
                        "query": query,
                        "results": capped,
                    }),
                    is_error: false,
                })
            }
            Err(e) => Err(map_search_err(e)),
        }
    }
}

fn required_string_arg(args: &Value, field: &str) -> Result<String, McpError> {
    match args.get(field) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(Value::String(_)) => Err(McpError::InvalidArgs(format!(
            "field '{field}' must not be empty"
        ))),
        Some(_) => Err(McpError::InvalidArgs(format!(
            "field '{field}' must be a string"
        ))),
        None => Err(McpError::InvalidArgs(format!(
            "missing required field '{field}'"
        ))),
    }
}

/// Map a `WikiError` from a read/list/search operation to `McpError`.
/// `WikiErrorKind` is `#[non_exhaustive]` — the default arm covers
/// future variants.
fn map_wiki_err_generic(err: WikiError) -> McpError {
    match err.kind_ref() {
        Some(WikiErrorKind::PathEscape { .. }) => McpError::InvalidArgs("invalid page path".into()),
        _ => McpError::Execution("wiki operation failed".into()),
    }
}

/// Map a `WikiError` from `Wiki::read`. Missing files come through as
/// `WikiError::Io(NotFound)` which should be surfaced distinctly so the
/// agent can react (try wiki.list, try a different name).
fn map_wiki_err_read(err: WikiError, name: &str) -> McpError {
    match err {
        WikiError::Io(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
            McpError::Execution(format!("page {name:?} not found"))
        }
        other => map_wiki_err_generic(other),
    }
}

/// Map a `WikiError` from `Wiki::write`. Surfaces the specific
/// operator-actionable variants (size limit, credential block) with
/// structured details.
///
/// `WikiErrorKind` is `#[non_exhaustive]`; the default arm maps any
/// future variant to `Execution("wiki storage error")` until this
/// function is explicitly updated.
fn map_wiki_err_write(err: WikiError, _name: &str) -> McpError {
    match err.kind_ref() {
        Some(WikiErrorKind::PathEscape { .. }) => McpError::InvalidArgs("invalid page path".into()),
        Some(WikiErrorKind::PageTooLarge { bytes, limit, .. }) => McpError::InvalidArgs(format!(
            "page too large: {bytes} bytes exceeds the {limit}-byte limit"
        )),
        Some(WikiErrorKind::CredentialBlocked { pattern, .. }) => McpError::Denied {
            reason: format!(
                "credential pattern {pattern:?} detected in content — refusing to write"
            ),
        },
        Some(WikiErrorKind::Conflict { .. }) => {
            McpError::Execution("wiki git conflict — resolve manually and retry".into())
        }
        _ => McpError::Execution("wiki storage error".into()),
    }
}

fn map_search_err(err: SearchError) -> McpError {
    match err {
        SearchError::Http(_) => McpError::Execution("web search upstream HTTP error".into()),
        SearchError::Parse(_) => McpError::Execution("web search response parse failed".into()),
        SearchError::Upstream(_) => McpError::Execution("web search upstream error".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WikiConfig;
    use tempfile::TempDir;

    fn fresh_provider_no_search() -> (TempDir, KnowledgeToolProvider) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "Test".into(),
            git_author_email: "t@test.local".into(),
            max_page_bytes: 1024 * 1024,
        };
        let wiki = Arc::new(Wiki::open(cfg).unwrap());
        let provider = KnowledgeToolProvider::with_components(wiki, None, 10);
        (dir, provider)
    }

    #[test]
    fn category_is_knowledge() {
        let (_dir, p) = fresh_provider_no_search();
        assert_eq!(p.category(), "knowledge");
    }

    #[test]
    fn tool_schemas_no_search_has_six_tools() {
        let (_dir, p) = fresh_provider_no_search();
        let schemas = p.tool_schemas();
        // list/get/search/write/delete/rename. The search subcat adds a
        // seventh (`web.search`) only when `[knowledge.search]` is
        // configured, which `fresh_provider_no_search` deliberately omits.
        assert_eq!(schemas.len(), 6);
        let names: Vec<_> = schemas.iter().map(|s| s.name.as_str()).collect();
        for expected in [
            "wiki.list",
            "wiki.get",
            "wiki.search",
            "wiki.write",
            "wiki.delete",
            "wiki.rename",
        ] {
            assert!(names.contains(&expected), "missing {expected:?} in {names:?}");
        }
        assert!(!names.contains(&"web.search"));
    }

    #[test]
    fn wiki_list_tier_is_read_wiki_write_tier_is_write() {
        let (_dir, p) = fresh_provider_no_search();
        let schemas = p.tool_schemas();
        let list = schemas.iter().find(|s| s.name == "wiki.list").unwrap();
        let write = schemas.iter().find(|s| s.name == "wiki.write").unwrap();
        assert!(matches!(list.tier, Tier::Read));
        assert!(matches!(write.tier, Tier::Write));
    }

    #[tokio::test]
    async fn wiki_write_then_get_round_trips_content() {
        let (_dir, p) = fresh_provider_no_search();
        let write_result = p
            .call(
                "wiki.write",
                json!({"name": "home", "content": "# Home\n\nbody"}),
            )
            .await
            .unwrap();
        assert!(!write_result.is_error);
        assert_eq!(write_result.content["bytes"], 12);

        let get_result = p.call("wiki.get", json!({"name": "home"})).await.unwrap();
        assert_eq!(get_result.content["content"], "# Home\n\nbody");
    }

    #[tokio::test]
    async fn wiki_list_reflects_writes() {
        let (_dir, p) = fresh_provider_no_search();
        p.call("wiki.write", json!({"name": "a", "content": "x"}))
            .await
            .unwrap();
        p.call("wiki.write", json!({"name": "b", "content": "y"}))
            .await
            .unwrap();
        let result = p.call("wiki.list", json!({})).await.unwrap();
        let pages = result.content["pages"].as_array().unwrap();
        let names: Vec<&str> = pages.iter().filter_map(|v| v.as_str()).collect();
        // A fresh wiki is seeded with README / operator / runbook pages at
        // `Wiki::open` time (see `seeds/`). Assert that our two writes appear
        // in addition to the seeds rather than pinning an exact page count.
        assert!(names.contains(&"a"), "expected 'a' in {names:?}");
        assert!(names.contains(&"b"), "expected 'b' in {names:?}");
        assert!(pages.len() >= 2);
    }

    #[tokio::test]
    async fn wiki_search_finds_by_keyword() {
        let (_dir, p) = fresh_provider_no_search();
        p.call(
            "wiki.write",
            json!({"name": "notes", "content": "quarterly review tomorrow"}),
        )
        .await
        .unwrap();
        p.call(
            "wiki.write",
            json!({"name": "grocery", "content": "milk bread"}),
        )
        .await
        .unwrap();
        let result = p
            .call("wiki.search", json!({"query": "quarterly"}))
            .await
            .unwrap();
        let hits = result.content["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn wiki_write_pem_block_returns_denied() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call(
                "wiki.write",
                json!({
                    "name": "leaked",
                    "content": "-----BEGIN RSA PRIVATE KEY-----\nbody\n-----END RSA PRIVATE KEY-----"
                }),
            )
            .await
            .expect_err("should be blocked");
        match err {
            McpError::Denied { reason } => {
                assert!(reason.contains("pem_private_key"), "reason: {reason}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn wiki_write_path_traversal_returns_invalid_args() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("wiki.write", json!({"name": "../escape", "content": "x"}))
            .await
            .expect_err("path escape");
        assert!(matches!(err, McpError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn wiki_write_oversize_returns_invalid_args_with_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "T".into(),
            git_author_email: "t@t.local".into(),
            max_page_bytes: 10,
        };
        let wiki = Arc::new(Wiki::open(cfg).unwrap());
        let p = KnowledgeToolProvider::with_components(wiki, None, 10);
        let err = p
            .call(
                "wiki.write",
                json!({"name": "big", "content": "x".repeat(100)}),
            )
            .await
            .expect_err("too large");
        match err {
            McpError::InvalidArgs(msg) => {
                assert!(msg.contains("page too large"));
                assert!(msg.contains("100"));
                assert!(msg.contains("10"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn wiki_get_missing_field_returns_invalid_args() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("wiki.get", json!({}))
            .await
            .expect_err("missing name");
        match err {
            McpError::InvalidArgs(msg) => assert!(msg.contains("'name'")),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn wiki_get_wrong_type_returns_invalid_args() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("wiki.get", json!({"name": 42}))
            .await
            .expect_err("wrong type");
        assert!(matches!(err, McpError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn wiki_get_missing_file_returns_execution_not_found() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("wiki.get", json!({"name": "ghost"}))
            .await
            .expect_err("not found");
        match err {
            McpError::Execution(msg) => assert!(msg.contains("not found")),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_tool_returns_unknown_tool_error() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("wiki.nonexistent", json!({}))
            .await
            .expect_err("unknown");
        match err {
            McpError::UnknownTool(name) => assert_eq!(name, "wiki.nonexistent"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn web_search_without_config_returns_denied() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("web.search", json!({"query": "rust"}))
            .await
            .expect_err("denied");
        match err {
            McpError::Denied { reason } => assert!(reason.contains("not configured")),
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
