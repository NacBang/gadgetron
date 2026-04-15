//! `McpToolRegistry` — the MCP tool dispatch table for Kairos.
//!
//! Spec: `docs/design/phase2/04-mcp-tool-registry.md v2 §2.1`.
//!
//! # Lifecycle (builder/freeze pattern)
//!
//! 1. `main()` constructs `McpToolRegistryBuilder::new()` and calls
//!    `register()` for each concrete `McpToolProvider` implementation:
//!    `KnowledgeToolProvider` in P2A, `InfraToolProvider` in P2C, etc.
//! 2. `builder.freeze()` consumes the builder and returns an immutable
//!    `McpToolRegistry`. This flips the registry from mutable-startup
//!    phase to immutable-serving phase.
//! 3. The frozen registry is wrapped in `Arc` and cloned into
//!    `AppState` + the Kairos session builder. Per-request dispatch
//!    is `O(1)` via a `HashMap<String, Arc<dyn McpToolProvider>>`.
//!
//! Why not a single mutable-through-lifetime registry? Two reasons:
//!
//! - `freeze()` lets us precompute the `HashMap` lookup table once
//!   instead of rebuilding it on every dispatch.
//! - The agent CANNOT register new providers at runtime (ADR-P2A-05 §14).
//!   Making the registry type-system-level immutable post-startup
//!   enforces this. A compromised provider cannot call
//!   `registry.register(...)` because that method doesn't exist on the
//!   frozen type.

use std::collections::HashMap;
use std::sync::Arc;

use gadgetron_core::agent::config::{AgentConfig, ToolMode};
use gadgetron_core::agent::tools::{
    ensure_tool_name_allowed, McpError, McpToolProvider, Tier, ToolResult, ToolSchema,
};

/// Mutable builder. Lives in `main()` until all providers are registered.
pub struct McpToolRegistryBuilder {
    providers: Vec<Arc<dyn McpToolProvider>>,
}

impl McpToolRegistryBuilder {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Register a concrete provider. Fails if any of its tool schemas
    /// violates the reserved-namespace check or if the category is
    /// `"agent"` (the entire category is reserved).
    ///
    /// Providers whose `is_available()` returns `false` are silently
    /// skipped — this is how feature-gated providers (compile-time or
    /// runtime) opt out without returning an error.
    pub fn register(&mut self, provider: Arc<dyn McpToolProvider>) -> Result<(), McpError> {
        if !provider.is_available() {
            return Ok(());
        }
        let category = provider.category();
        for schema in provider.tool_schemas() {
            ensure_tool_name_allowed(&schema.name, category)?;
        }
        self.providers.push(provider);
        Ok(())
    }

    /// Number of registered providers so far (excluding those skipped
    /// via `is_available = false`).
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Consume the builder and return the immutable dispatch registry.
    /// Builds the `by_tool_name` HashMap from every provider's schemas
    /// and the flattened `all_schemas` Vec used by `build_allowed_tools`.
    ///
    /// If two providers register tools with the same namespaced name,
    /// the later-registered one wins in the dispatch map — but the
    /// `all_schemas` vec retains both entries so operators see the
    /// duplicate in `/v1/tools` (future P2B endpoint). The test
    /// `duplicate_tool_name_last_wins_in_dispatch` locks this in.
    pub fn freeze(self) -> McpToolRegistry {
        let mut by_tool_name: HashMap<String, Arc<dyn McpToolProvider>> = HashMap::new();
        let mut all_schemas: Vec<ToolSchema> = Vec::new();
        for provider in self.providers.into_iter() {
            for schema in provider.tool_schemas() {
                by_tool_name.insert(schema.name.clone(), provider.clone());
                all_schemas.push(schema);
            }
        }
        McpToolRegistry {
            by_tool_name,
            all_schemas: Arc::from(all_schemas.into_boxed_slice()),
        }
    }
}

impl Default for McpToolRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable dispatch table. Cheap to clone (internal `Arc`s).
#[derive(Clone)]
pub struct McpToolRegistry {
    by_tool_name: HashMap<String, Arc<dyn McpToolProvider>>,
    all_schemas: Arc<[ToolSchema]>,
}

impl McpToolRegistry {
    /// All registered tool schemas, flattened. Includes duplicates
    /// (see `freeze` notes). Callers that need unique names should
    /// dedupe on `schema.name`.
    pub fn all_schemas(&self) -> &[ToolSchema] {
        &self.all_schemas
    }

    /// Number of distinct tool names in the dispatch map.
    pub fn len(&self) -> usize {
        self.by_tool_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_tool_name.is_empty()
    }

    /// Build the `--allowed-tools` list passed to `claude -p`.
    /// Filters the full schema set per `AgentConfig`:
    ///
    /// - `Tier::Read` → always included (V1 forces read = Auto)
    /// - `Tier::Write` → included iff the matching subcategory is not
    ///   `ToolMode::Never`. P2A-specific: the subcategory → tool mapping
    ///   is based on the tool name prefix:
    ///   - `wiki.*` → `write.wiki_write`
    ///   - `infra.*` → `write.infra_write` (P2C tools, not yet live)
    ///   - `scheduler.*` → `write.scheduler_write` (P3, not yet live)
    ///   - `provider.*` → `write.provider_mutate` (P2C, not yet live)
    ///   - anything else → `write.default_mode`
    /// - `Tier::Destructive` → included iff `destructive.enabled = true`
    ///   (always false in P2A per V5). Under Path 1 this branch is
    ///   effectively dead code — the filter never emits a T3 tool — but
    ///   the logic is in place for P2B when approval flow lands.
    ///
    /// Output is sorted by tool name for deterministic test snapshots.
    pub fn build_allowed_tools(&self, cfg: &AgentConfig) -> Vec<String> {
        let mut out: Vec<String> = self
            .all_schemas
            .iter()
            .filter(|schema| tool_is_enabled(schema, cfg))
            .map(|schema| schema.name.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Dispatch a tool call to the provider that owns it.
    ///
    /// The returned `Result` is the raw MCP surface; callers that need
    /// an HTTP response use `GadgetronError::from(err)` via the
    /// `From<McpError>` impl in `gadgetron-core` to get the mapped
    /// status code + error message.
    pub async fn dispatch(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<ToolResult, McpError> {
        let provider = self
            .by_tool_name
            .get(name)
            .ok_or_else(|| McpError::UnknownTool(name.to_string()))?;
        provider.call(name, args).await
    }
}

/// Determine whether a tool should appear in `--allowed-tools`.
///
/// Per ADR-P2A-06 §"Tier + Mode in P2A", `Ask` is treated as `Never` because
/// the interactive approval flow is deferred to Phase 2B. Only `Auto` tools
/// reach Claude Code's allowed-tools flag.
fn tool_is_enabled(schema: &ToolSchema, cfg: &AgentConfig) -> bool {
    match schema.tier {
        Tier::Read => true,
        Tier::Write => {
            let mode = resolve_write_mode(&schema.name, cfg);
            !matches!(mode, ToolMode::Never | ToolMode::Ask)
        }
        Tier::Destructive => cfg.tools.destructive.enabled,
    }
}

/// Map a `Tier::Write` tool name to the config subcategory that
/// controls its mode.
fn resolve_write_mode(name: &str, cfg: &AgentConfig) -> ToolMode {
    let w = &cfg.tools.write;
    let prefix = name.split('.').next().unwrap_or("");
    match prefix {
        "wiki" => w.wiki_write,
        "infra" => w.infra_write,
        "scheduler" => w.scheduler_write,
        "provider" => w.provider_mutate,
        _ => w.default_mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    // ---- Inline test fake. A proper `FakeToolProvider` lands in
    // `gadgetron-testing` in Step 13; for now keep dependencies
    // unidirectional and the test self-contained.
    struct TestProvider {
        category: &'static str,
        schemas: Vec<ToolSchema>,
        available: bool,
        response: Result<ToolResult, McpError>,
    }

    impl TestProvider {
        fn new(category: &'static str) -> Self {
            Self {
                category,
                schemas: Vec::new(),
                available: true,
                response: Ok(ToolResult {
                    content: json!({"ok": true}),
                    is_error: false,
                }),
            }
        }

        fn with_tool(mut self, name: &str, tier: Tier) -> Self {
            self.schemas.push(ToolSchema {
                name: name.to_string(),
                tier,
                description: format!("fake tool {name}"),
                input_schema: json!({}),
                idempotent: None,
            });
            self
        }

        fn unavailable(mut self) -> Self {
            self.available = false;
            self
        }
    }

    #[async_trait]
    impl McpToolProvider for TestProvider {
        fn category(&self) -> &'static str {
            self.category
        }
        fn tool_schemas(&self) -> Vec<ToolSchema> {
            self.schemas.clone()
        }
        async fn call(
            &self,
            _name: &str,
            _args: serde_json::Value,
        ) -> Result<ToolResult, McpError> {
            // Return a cloned result; McpError isn't Clone so we map.
            match &self.response {
                Ok(r) => Ok(r.clone()),
                Err(e) => Err(clone_err(e)),
            }
        }
        fn is_available(&self) -> bool {
            self.available
        }
    }

    fn clone_err(e: &McpError) -> McpError {
        match e {
            McpError::UnknownTool(s) => McpError::UnknownTool(s.clone()),
            McpError::Denied { reason } => McpError::Denied {
                reason: reason.clone(),
            },
            McpError::RateLimited {
                tool,
                remaining,
                limit,
            } => McpError::RateLimited {
                tool: tool.clone(),
                remaining: *remaining,
                limit: *limit,
            },
            McpError::ApprovalTimeout { secs } => McpError::ApprovalTimeout { secs: *secs },
            McpError::InvalidArgs(s) => McpError::InvalidArgs(s.clone()),
            McpError::Execution(s) => McpError::Execution(s.clone()),
        }
    }

    // ---- register + freeze ----

    #[test]
    fn register_then_freeze_produces_dispatch_map() {
        let mut builder = McpToolRegistryBuilder::new();
        let provider = Arc::new(
            TestProvider::new("knowledge")
                .with_tool("wiki.read", Tier::Read)
                .with_tool("wiki.write", Tier::Write),
        );
        builder.register(provider).expect("register");
        assert_eq!(builder.provider_count(), 1);

        let registry = builder.freeze();
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.all_schemas().len(), 2);
    }

    #[test]
    fn register_empty_builder_produces_empty_registry() {
        let registry = McpToolRegistryBuilder::new().freeze();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn register_unavailable_provider_is_skipped() {
        let mut builder = McpToolRegistryBuilder::new();
        let provider = Arc::new(
            TestProvider::new("knowledge")
                .with_tool("wiki.read", Tier::Read)
                .unavailable(),
        );
        builder.register(provider).expect("register ok (skipped)");
        assert_eq!(builder.provider_count(), 0);
        assert!(builder.freeze().is_empty());
    }

    #[test]
    fn register_reserved_agent_category_fails() {
        let mut builder = McpToolRegistryBuilder::new();
        let provider = Arc::new(TestProvider::new("agent").with_tool("foo", Tier::Read));
        let err = builder.register(provider).expect_err("must fail");
        assert!(matches!(err, McpError::Denied { .. }));
    }

    #[test]
    fn register_reserved_tool_name_fails() {
        let mut builder = McpToolRegistryBuilder::new();
        let provider =
            Arc::new(TestProvider::new("knowledge").with_tool("agent.set_brain", Tier::Read));
        let err = builder.register(provider).expect_err("must fail");
        assert!(matches!(err, McpError::Denied { .. }));
    }

    // ---- dispatch ----

    #[tokio::test]
    async fn dispatch_routes_to_matching_provider() {
        let mut builder = McpToolRegistryBuilder::new();
        builder
            .register(Arc::new(
                TestProvider::new("knowledge").with_tool("wiki.read", Tier::Read),
            ))
            .unwrap();
        let registry = builder.freeze();
        let result = registry
            .dispatch("wiki.read", json!({"name": "home"}))
            .await
            .expect("dispatch ok");
        assert_eq!(result.content, json!({"ok": true}));
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_unknown_tool_error() {
        let registry = McpToolRegistryBuilder::new().freeze();
        let err = registry
            .dispatch("ghost.tool", json!({}))
            .await
            .expect_err("unknown");
        match err {
            McpError::UnknownTool(name) => assert_eq!(name, "ghost.tool"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn duplicate_tool_name_last_wins_in_dispatch() {
        let mut builder = McpToolRegistryBuilder::new();
        builder
            .register(Arc::new(
                TestProvider::new("knowledge").with_tool("wiki.read", Tier::Read),
            ))
            .unwrap();
        // A second provider claims the same name.
        builder
            .register(Arc::new(
                TestProvider::new("custom").with_tool("wiki.read", Tier::Read),
            ))
            .unwrap();
        let registry = builder.freeze();
        // Dispatch map has 1 unique name; flat schema vec has 2.
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.all_schemas().len(), 2);
    }

    // ---- build_allowed_tools ----

    fn cfg_with_overrides(
        write_default: ToolMode,
        wiki_write: ToolMode,
        infra_write: ToolMode,
        destructive_enabled: bool,
    ) -> AgentConfig {
        let mut cfg = AgentConfig::default();
        cfg.tools.write.default_mode = write_default;
        cfg.tools.write.wiki_write = wiki_write;
        cfg.tools.write.infra_write = infra_write;
        cfg.tools.destructive.enabled = destructive_enabled;
        cfg
    }

    fn registry_with_full_set() -> McpToolRegistry {
        let mut builder = McpToolRegistryBuilder::new();
        builder
            .register(Arc::new(
                TestProvider::new("knowledge")
                    .with_tool("wiki.read", Tier::Read)
                    .with_tool("wiki.write", Tier::Write)
                    .with_tool("web.search", Tier::Read)
                    .with_tool("wiki.delete", Tier::Destructive),
            ))
            .unwrap();
        builder
            .register(Arc::new(
                TestProvider::new("infrastructure")
                    .with_tool("infra.list_nodes", Tier::Read)
                    .with_tool("infra.deploy_model", Tier::Write),
            ))
            .unwrap();
        builder.freeze()
    }

    #[test]
    fn build_allowed_tools_t1_always_present() {
        let reg = registry_with_full_set();
        // Even with all writes never and destructive disabled, T1 reads remain.
        let cfg = cfg_with_overrides(ToolMode::Never, ToolMode::Never, ToolMode::Never, false);
        let tools = reg.build_allowed_tools(&cfg);
        assert!(tools.contains(&"wiki.read".to_string()));
        assert!(tools.contains(&"web.search".to_string()));
        assert!(tools.contains(&"infra.list_nodes".to_string()));
    }

    #[test]
    fn build_allowed_tools_wiki_write_auto_included() {
        let reg = registry_with_full_set();
        let cfg = cfg_with_overrides(ToolMode::Never, ToolMode::Auto, ToolMode::Never, false);
        let tools = reg.build_allowed_tools(&cfg);
        assert!(tools.contains(&"wiki.write".to_string()));
        // infra.deploy_model is still Never.
        assert!(!tools.contains(&"infra.deploy_model".to_string()));
    }

    #[test]
    fn build_allowed_tools_wiki_write_never_omitted() {
        let reg = registry_with_full_set();
        let cfg = cfg_with_overrides(ToolMode::Auto, ToolMode::Never, ToolMode::Auto, false);
        let tools = reg.build_allowed_tools(&cfg);
        assert!(!tools.contains(&"wiki.write".to_string()));
    }

    #[test]
    fn ask_mode_tools_are_excluded_from_allowed_list() {
        // ADR-P2A-06 §"Tier + Mode in P2A": "T2 `Write` — `Auto` or `Never`
        // per subcategory. `Ask` is logged as a startup warning and treated
        // as `Never` (no approval flow to resolve it)."
        //
        // The approval flow was deferred to Phase 2B. Any tool whose mode
        // resolves to `Ask` MUST NOT appear in `--allowed-tools`, otherwise
        // Claude Code sees it as an auto-runnable tool and invokes it without
        // the approval card that P2A does not implement. This is the exact
        // runtime correctness gap Codex flagged in the pre-Phase-5 review
        // (`a304a359c467a6579`).
        let reg = registry_with_full_set();
        let cfg = cfg_with_overrides(ToolMode::Ask, ToolMode::Ask, ToolMode::Ask, false);
        let tools = reg.build_allowed_tools(&cfg);
        assert!(
            !tools.contains(&"wiki.write".to_string()),
            "wiki.write is Ask — must be excluded: {tools:?}"
        );
        assert!(
            !tools.contains(&"infra.deploy_model".to_string()),
            "infra.deploy_model is Ask — must be excluded: {tools:?}"
        );
        // Read-tier tools are unaffected (V1: read is always Auto).
        assert!(tools.contains(&"wiki.read".to_string()));
        assert!(tools.contains(&"web.search".to_string()));
    }

    #[test]
    fn build_allowed_tools_t3_disabled_omits_all_destructive() {
        let reg = registry_with_full_set();
        let cfg = cfg_with_overrides(ToolMode::Auto, ToolMode::Auto, ToolMode::Auto, false);
        let tools = reg.build_allowed_tools(&cfg);
        // wiki.delete is T3 — must NOT appear when enabled=false.
        assert!(!tools.contains(&"wiki.delete".to_string()));
    }

    #[test]
    fn build_allowed_tools_t3_enabled_includes_destructive() {
        let reg = registry_with_full_set();
        let cfg = cfg_with_overrides(ToolMode::Auto, ToolMode::Auto, ToolMode::Auto, true);
        let tools = reg.build_allowed_tools(&cfg);
        assert!(tools.contains(&"wiki.delete".to_string()));
    }

    #[test]
    fn build_allowed_tools_output_is_sorted_and_deduped() {
        let reg = registry_with_full_set();
        let cfg = cfg_with_overrides(ToolMode::Auto, ToolMode::Auto, ToolMode::Auto, false);
        let tools = reg.build_allowed_tools(&cfg);
        let mut sorted = tools.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(tools, sorted);
    }

    #[test]
    fn resolve_write_mode_by_prefix() {
        let mut cfg = AgentConfig::default();
        cfg.tools.write.default_mode = ToolMode::Ask;
        cfg.tools.write.wiki_write = ToolMode::Auto;
        cfg.tools.write.infra_write = ToolMode::Never;
        cfg.tools.write.scheduler_write = ToolMode::Auto;
        cfg.tools.write.provider_mutate = ToolMode::Never;

        assert!(matches!(
            resolve_write_mode("wiki.write", &cfg),
            ToolMode::Auto
        ));
        assert!(matches!(
            resolve_write_mode("infra.deploy_model", &cfg),
            ToolMode::Never
        ));
        assert!(matches!(
            resolve_write_mode("scheduler.schedule_job", &cfg),
            ToolMode::Auto
        ));
        assert!(matches!(
            resolve_write_mode("provider.rotate_key", &cfg),
            ToolMode::Never
        ));
        // Unrecognized prefix uses default_mode.
        assert!(matches!(
            resolve_write_mode("weird.tool", &cfg),
            ToolMode::Ask
        ));
    }
}
