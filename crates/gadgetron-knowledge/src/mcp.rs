//! MCP tool provider for the knowledge layer.
//!
//! Implements `gadgetron_core::agent::tools::McpToolProvider` for the
//! `"knowledge"` category. Exposes wiki CRUD/search plus optional
//! `web.search`.

use std::sync::Arc;

use async_trait::async_trait;
use gadgetron_core::agent::config::{EnvResolver, StdEnv};
use gadgetron_core::agent::tools::{McpError, McpToolProvider, Tier, ToolResult, ToolSchema};
use gadgetron_core::error::WikiErrorKind;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::config::{EmbeddingWriteMode, KnowledgeConfig};
use crate::error::{SearchError, WikiError};
use crate::search::{SearxngClient, WebSearch};
use crate::semantic::{normalize_page_content, SemanticBackend, SemanticSearchHit};
use crate::wiki::{Wiki, WikiSearchHit};

/// The concrete `McpToolProvider` for knowledge-layer tools.
pub struct KnowledgeToolProvider {
    wiki: Arc<Wiki>,
    web_search: Option<Arc<dyn WebSearch>>,
    semantic: Option<Arc<SemanticBackend>>,
    max_search_results: usize,
}

impl KnowledgeToolProvider {
    /// Build the provider from a validated `KnowledgeConfig`.
    ///
    /// `pg_pool` is optional so the same config can still expose keyword-only
    /// wiki tools when semantic indexing is configured but PostgreSQL is not
    /// reachable in the current process.
    pub fn new(config: KnowledgeConfig, pg_pool: Option<PgPool>) -> Result<Self, WikiError> {
        Self::new_with_env(config, pg_pool, &StdEnv)
    }

    pub fn new_with_env(
        config: KnowledgeConfig,
        pg_pool: Option<PgPool>,
        env: &dyn EnvResolver,
    ) -> Result<Self, WikiError> {
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

        let semantic =
            SemanticBackend::from_config(pg_pool, config.embedding.as_ref(), env)?.map(Arc::new);

        Ok(Self {
            wiki,
            web_search,
            semantic,
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
        Self::with_components_and_semantic(wiki, web_search, None, max_search_results)
    }

    pub(crate) fn with_components_and_semantic(
        wiki: Arc<Wiki>,
        web_search: Option<Arc<dyn WebSearch>>,
        semantic: Option<Arc<SemanticBackend>>,
        max_search_results: usize,
    ) -> Self {
        Self {
            wiki,
            web_search,
            semantic,
            max_search_results: max_search_results.max(1),
        }
    }
}

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
            "wiki.search" => self.call_wiki_search(args).await,
            "wiki.write" => self.call_wiki_write(args).await,
            "wiki.delete" => self.call_wiki_delete(args).await,
            "wiki.rename" => self.call_wiki_rename(args).await,
            "web.search" => self.call_web_search(args).await,
            other => Err(McpError::UnknownTool(other.to_string())),
        }
    }
}

fn schema_wiki_list() -> ToolSchema {
    ToolSchema {
        name: "wiki.list".into(),
        tier: Tier::Read,
        description: "List all pages in the Penny wiki. Returns page names \
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
        description: "Semantic + keyword search over the wiki. Returns pages \
            ranked by relevance with a short snippet."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": 512 },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "default": 10
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

    async fn call_wiki_search(&self, args: Value) -> Result<ToolResult, McpError> {
        let query = required_string_arg(&args, "query")?;
        let limit = parse_search_limit(&args);

        let hits = match &self.semantic {
            Some(semantic) => match semantic.hybrid_search(&query, limit).await {
                Ok(hits) => semantic_hits_to_payload(hits),
                Err(error) => {
                    tracing::warn!(
                        target: "knowledge_semantic",
                        error = ?error,
                        "hybrid wiki.search failed; falling back to keyword-only search"
                    );
                    keyword_hits_to_payload(
                        self.wiki
                            .search(&query, limit)
                            .map_err(map_wiki_err_generic)?,
                    )
                }
            },
            None => keyword_hits_to_payload(
                self.wiki
                    .search(&query, limit)
                    .map_err(map_wiki_err_generic)?,
            ),
        };

        Ok(ToolResult {
            content: json!({
                "query": query,
                "hits": hits,
            }),
            is_error: false,
        })
    }

    async fn call_wiki_write(&self, args: Value) -> Result<ToolResult, McpError> {
        let name = required_string_arg(&args, "name")?;
        let raw_content = required_string_arg(&args, "content")?;
        let content = if self.semantic.is_some() {
            normalize_page_content(&raw_content).map_err(|e| map_wiki_err_write(e, &name))?
        } else {
            raw_content
        };

        match self.wiki.write(&name, &content) {
            Ok(result) => {
                if let Some(semantic) = &self.semantic {
                    let semantic = semantic.clone();
                    let page_name = result.name.clone();
                    match semantic.write_mode() {
                        EmbeddingWriteMode::Sync => {
                            if let Err(error) = semantic.index_page(&page_name, &content).await {
                                tracing::warn!(
                                    target: "knowledge_semantic",
                                    page = %page_name,
                                    error = ?error,
                                    "semantic index update failed after wiki.write"
                                );
                            }
                        }
                        EmbeddingWriteMode::Async => spawn_semantic_task(async move {
                            if let Err(error) = semantic.index_page(&page_name, &content).await {
                                tracing::warn!(
                                    target: "knowledge_semantic",
                                    page = %page_name,
                                    error = ?error,
                                    "semantic index update failed after wiki.write"
                                );
                            }
                        }),
                    }
                }

                Ok(ToolResult {
                    content: json!({
                        "name": result.name,
                        "bytes": result.bytes,
                        "commit_oid": result.commit_oid,
                    }),
                    is_error: false,
                })
            }
            Err(e) => Err(map_wiki_err_write(e, &name)),
        }
    }

    async fn call_wiki_delete(&self, args: Value) -> Result<ToolResult, McpError> {
        let name = required_string_arg(&args, "name")?;
        match self.wiki.delete(&name) {
            Ok(archive_path) => {
                if let Some(semantic) = &self.semantic {
                    let semantic = semantic.clone();
                    let page_name = name.clone();
                    match semantic.write_mode() {
                        EmbeddingWriteMode::Sync => {
                            if let Err(error) = semantic.delete_page(&page_name).await {
                                tracing::warn!(
                                    target: "knowledge_semantic",
                                    page = %page_name,
                                    error = ?error,
                                    "semantic delete cleanup failed after wiki.delete"
                                );
                            }
                        }
                        EmbeddingWriteMode::Async => spawn_semantic_task(async move {
                            if let Err(error) = semantic.delete_page(&page_name).await {
                                tracing::warn!(
                                    target: "knowledge_semantic",
                                    page = %page_name,
                                    error = ?error,
                                    "semantic delete cleanup failed after wiki.delete"
                                );
                            }
                        }),
                    }
                }

                Ok(ToolResult {
                    content: json!({
                        "name": name,
                        "archived_to": archive_path,
                        "message": "soft-deleted; archived copy preserved in _archived/",
                    }),
                    is_error: false,
                })
            }
            Err(e) => Err(map_wiki_err_read(e, &name)),
        }
    }

    async fn call_wiki_rename(&self, args: Value) -> Result<ToolResult, McpError> {
        let from = required_string_arg(&args, "from")?;
        let to = required_string_arg(&args, "to")?;
        match self.wiki.rename(&from, &to) {
            Ok(result) => {
                if let Some(semantic) = &self.semantic {
                    let semantic = semantic.clone();
                    let from_page = from.clone();
                    let to_page = result.name.clone();
                    match semantic.write_mode() {
                        EmbeddingWriteMode::Sync => {
                            if let Err(error) = semantic.rename_page(&from_page, &to_page).await {
                                tracing::warn!(
                                    target: "knowledge_semantic",
                                    from = %from_page,
                                    to = %to_page,
                                    error = ?error,
                                    "semantic rename cleanup failed after wiki.rename"
                                );
                            }
                        }
                        EmbeddingWriteMode::Async => spawn_semantic_task(async move {
                            if let Err(error) = semantic.rename_page(&from_page, &to_page).await {
                                tracing::warn!(
                                    target: "knowledge_semantic",
                                    from = %from_page,
                                    to = %to_page,
                                    error = ?error,
                                    "semantic rename cleanup failed after wiki.rename"
                                );
                            }
                        }),
                    }
                }

                Ok(ToolResult {
                    content: json!({
                        "from": from,
                        "to": result.name,
                        "commit_oid": result.commit_oid,
                    }),
                    is_error: false,
                })
            }
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

fn spawn_semantic_task<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(future);
}

#[derive(Debug, Clone, Serialize)]
struct SearchHitPayload {
    page_name: String,
    score: f32,
    section: Option<String>,
    snippet: Option<String>,
}

fn semantic_hits_to_payload(hits: Vec<SemanticSearchHit>) -> Vec<SearchHitPayload> {
    hits.into_iter()
        .map(|hit| SearchHitPayload {
            page_name: hit.page_name,
            score: hit.score,
            section: hit.section,
            snippet: hit.snippet,
        })
        .collect()
}

fn keyword_hits_to_payload(hits: Vec<WikiSearchHit>) -> Vec<SearchHitPayload> {
    hits.into_iter()
        .map(|hit| SearchHitPayload {
            page_name: hit.name,
            score: hit.score,
            section: None,
            snippet: hit.snippet,
        })
        .collect()
}

fn parse_search_limit(args: &Value) -> usize {
    args.get("limit")
        .and_then(|v| v.as_u64())
        .or_else(|| args.get("max_results").and_then(|v| v.as_u64()))
        .map(|n| n as usize)
        .unwrap_or(10)
        .clamp(1, 50)
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

fn map_wiki_err_generic(err: WikiError) -> McpError {
    match err.kind_ref() {
        Some(WikiErrorKind::PathEscape { .. }) => McpError::InvalidArgs("invalid page path".into()),
        _ => McpError::Execution("wiki operation failed".into()),
    }
}

fn map_wiki_err_read(err: WikiError, name: &str) -> McpError {
    match err {
        WikiError::Io(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
            McpError::Execution(format!("page {name:?} not found"))
        }
        other => map_wiki_err_generic(other),
    }
}

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
    use std::hash::{Hash, Hasher};

    use super::*;
    use crate::embedding::{EmbeddingError, EmbeddingProvider};
    use async_trait::async_trait;
    use gadgetron_testing::harness::pg::PgHarness;
    use tempfile::TempDir;

    use crate::config::WikiConfig;

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

    #[derive(Clone)]
    struct FakeEmbeddingProvider {
        dimension: usize,
    }

    impl FakeEmbeddingProvider {
        fn new(dimension: usize) -> Self {
            Self { dimension }
        }

        fn embed_one(&self, text: &str) -> Vec<f32> {
            let mut out = vec![0.0; self.dimension];
            for token in crate::wiki::tokenize(text) {
                let idx = stable_hash(&token) % self.dimension;
                out[idx] += 1.0;
            }
            out
        }
    }

    #[async_trait]
    impl EmbeddingProvider for FakeEmbeddingProvider {
        async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
            Ok(texts.iter().map(|text| self.embed_one(text)).collect())
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        fn model_name(&self) -> &str {
            "fake-test-embedding"
        }
    }

    fn stable_hash(text: &str) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish() as usize
    }

    async fn semantic_pg_available() -> bool {
        let admin_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
        let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
        else {
            return false;
        };

        let available: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
            "SELECT default_version FROM pg_available_extensions WHERE name = 'vector'",
        )
        .fetch_optional(&pool)
        .await;
        pool.close().await;
        matches!(available, Ok(Some(_)))
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
            assert!(
                names.contains(&expected),
                "missing {expected:?} in {names:?}"
            );
        }
        assert!(!names.contains(&"web.search"));
    }

    #[test]
    fn wiki_search_schema_uses_limit_field() {
        let (_dir, p) = fresh_provider_no_search();
        let schema = p
            .tool_schemas()
            .into_iter()
            .find(|s| s.name == "wiki.search")
            .expect("wiki.search schema");
        assert!(schema.input_schema["properties"].get("limit").is_some());
        assert!(schema.input_schema["properties"]
            .get("max_results")
            .is_none());
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
    async fn wiki_write_then_get_round_trips_content_without_semantic_mode() {
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
            .call("wiki.search", json!({"query": "quarterly", "limit": 10}))
            .await
            .unwrap();
        let hits = result.content["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["page_name"], "notes");
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

    #[tokio::test]
    async fn semantic_write_sync_indexes_into_postgres_and_searches_by_page_name() {
        if !semantic_pg_available().await {
            eprintln!("skipping semantic pg test: vector extension is unavailable");
            return;
        }
        let harness = PgHarness::new().await;
        let dir = tempfile::tempdir().unwrap();
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "Test".into(),
            git_author_email: "test@example.local".into(),
            max_page_bytes: 1024 * 1024,
        };
        let wiki = Arc::new(Wiki::open(cfg).unwrap());
        let semantic = Arc::new(SemanticBackend::new(
            harness.pool.clone(),
            Arc::new(FakeEmbeddingProvider::new(1536)) as Arc<dyn EmbeddingProvider>,
            EmbeddingWriteMode::Sync,
        ));
        let provider =
            KnowledgeToolProvider::with_components_and_semantic(wiki, None, Some(semantic), 10);

        provider
            .call(
                "wiki.write",
                json!({
                    "name": "incidents/fan-boot",
                    "content": "# Fan Boot\n\n## Symptom\n\nGPU fan error during boot.\n\n## Fix\n\nReseat the power cable."
                }),
            )
            .await
            .expect("write");

        let chunk_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM wiki_chunks WHERE page_name = $1")
                .bind("incidents/fan-boot")
                .fetch_one(&harness.pool)
                .await
                .expect("count chunks");
        assert!(chunk_count > 0, "expected indexed chunks");

        let result = provider
            .call("wiki.search", json!({"query": "fan boot gpu", "limit": 5}))
            .await
            .expect("search");
        let hits = result.content["hits"].as_array().expect("hits array");
        assert!(!hits.is_empty(), "semantic search should return a hit");
        assert_eq!(hits[0]["page_name"], "incidents/fan-boot");

        harness.cleanup().await;
    }

    #[tokio::test]
    async fn knowledge_provider_new_with_env_enables_semantic_when_pool_and_key_exist() {
        if !semantic_pg_available().await {
            eprintln!("skipping semantic pg test: vector extension is unavailable");
            return;
        }
        let harness = PgHarness::new().await;
        let dir = tempfile::tempdir().unwrap();
        let raw = format!(
            r#"
[knowledge]
wiki_path = "{}"
wiki_autocommit = true

[knowledge.embedding]
provider = "openai_compat"
base_url = "https://example.invalid/v1"
api_key_env = "OPENAI_API_KEY"
model = "text-embedding-3-small"
dimension = 1536
write_mode = "sync"
timeout_secs = 5
"#,
            dir.path().join("wiki").display()
        );
        let cfg = KnowledgeConfig::extract_from_toml_str(&raw)
            .expect("parse")
            .expect("knowledge cfg");
        let env = gadgetron_core::agent::config::FakeEnv::new().with("OPENAI_API_KEY", "sk-test");
        let provider =
            KnowledgeToolProvider::new_with_env(cfg, Some(harness.pool.clone()), &env).unwrap();

        let schema = provider
            .tool_schemas()
            .into_iter()
            .find(|s| s.name == "wiki.search")
            .expect("search schema");
        assert!(schema.input_schema["properties"].get("limit").is_some());

        harness.cleanup().await;
    }
}
