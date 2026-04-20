//! `GadgetRegistry` — the MCP tool dispatch table for Penny.
//!
//! Spec: `docs/design/phase2/04-gadget-registry.md v2 §2.1`.
//!
//! # Lifecycle (builder/freeze pattern)
//!
//! 1. `main()` constructs `GadgetRegistryBuilder::new()` and calls
//!    `register()` for each concrete `GadgetProvider` implementation:
//!    `KnowledgeToolProvider` in P2A, `InfraToolProvider` in P2C, etc.
//! 2. `builder.freeze()` consumes the builder and returns an immutable
//!    `GadgetRegistry`. This flips the registry from mutable-startup
//!    phase to immutable-serving phase.
//! 3. The frozen registry is wrapped in `Arc` and cloned into
//!    `AppState` + the Penny session builder. Per-request dispatch
//!    is `O(1)` via a `HashMap<String, Arc<dyn GadgetProvider>>`.
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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gadgetron_core::agent::config::{AgentConfig, GadgetMode};
use gadgetron_core::agent::tools::{
    ensure_tool_name_allowed, GadgetError, GadgetProvider, GadgetResult, GadgetSchema, GadgetTier,
};
use gadgetron_core::audit::GadgetMetadata;

/// Mutable builder. Lives in `main()` until all providers are registered.
pub struct GadgetRegistryBuilder {
    providers: Vec<Arc<dyn GadgetProvider>>,
}

impl GadgetRegistryBuilder {
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
    pub fn register(&mut self, provider: Arc<dyn GadgetProvider>) -> Result<(), GadgetError> {
        if !provider.is_available() {
            return Ok(());
        }
        let category = provider.category();
        for schema in provider.gadget_schemas() {
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
    /// The `cfg` argument is captured at freeze time to precompute the
    /// `allowed_names` set used by `dispatch()` for L3 defense-in-depth
    /// (per `04-gadget-registry.md §6 L3` and ADR-P2A-06 Implementation
    /// status addendum item 3). A caller that bypasses
    /// `build_allowed_tools` — for example a direct `gadgetron gadget serve`
    /// consumer — cannot reach a `Never`/`Ask`-mode tool because
    /// `dispatch()` checks the precomputed set before routing to the
    /// provider. The registry becomes stale if `cfg` changes at runtime;
    /// this is acceptable in P2A (no hot-reload), and P2B's approval
    /// flow will thread a live `Arc<AgentConfig>` through the registry.
    ///
    /// If two providers register tools with the same namespaced name,
    /// the later-registered one wins in the dispatch map — but the
    /// `all_schemas` vec retains both entries so operators see the
    /// duplicate in `/v1/tools` (future P2B endpoint). The test
    /// `duplicate_tool_name_last_wins_in_dispatch` locks this in.
    pub fn freeze(self, cfg: &AgentConfig) -> GadgetRegistry {
        let mut by_tool_name: HashMap<String, Arc<dyn GadgetProvider>> = HashMap::new();
        let mut all_schemas: Vec<GadgetSchema> = Vec::new();
        // Denormalized (tier, category) per tool name so the Penny
        // audit emitter in session.rs can fire `ToolCallCompleted`
        // events without another registry lookup on the hot path.
        let mut tool_metadata: HashMap<String, GadgetMetadata> = HashMap::new();
        for provider in self.providers.into_iter() {
            let category = provider.category().to_string();
            for schema in provider.gadget_schemas() {
                tool_metadata.insert(
                    schema.name.clone(),
                    GadgetMetadata {
                        tier: schema.tier.into(),
                        category: category.clone(),
                    },
                );
                by_tool_name.insert(schema.name.clone(), provider.clone());
                all_schemas.push(schema);
            }
        }
        // Precompute the allowed-names set. A tool is in the set iff
        // `tool_is_enabled(schema, cfg)` — same predicate as
        // `build_allowed_tools`, so the L2 (Claude Code argv filter)
        // and L3 (dispatch re-check) gates share a single source of
        // truth for "what is operator-allowed".
        let allowed_names: HashSet<String> = all_schemas
            .iter()
            .filter(|schema| tool_is_enabled(schema, cfg))
            .map(|schema| schema.name.clone())
            .collect();
        GadgetRegistry {
            by_tool_name,
            all_schemas: Arc::from(all_schemas.into_boxed_slice()),
            allowed_names: Arc::new(allowed_names),
            tool_metadata: Arc::new(tool_metadata),
        }
    }
}

impl Default for GadgetRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable dispatch table. Cheap to clone (internal `Arc`s).
#[derive(Clone)]
pub struct GadgetRegistry {
    by_tool_name: HashMap<String, Arc<dyn GadgetProvider>>,
    all_schemas: Arc<[GadgetSchema]>,
    /// Precomputed set of tool names whose tier × mode resolves to
    /// "operator-allowed" under the config passed to `freeze()`. Used
    /// by `dispatch()` for L3 defense-in-depth. Tools NOT in this set
    /// are rejected with `GadgetError::Denied` before the provider is
    /// invoked.
    allowed_names: Arc<HashSet<String>>,
    /// Denormalized `(tier, category)` per tool name. Used by the
    /// Penny audit emitter in `session.rs::drive` to fill
    /// `ToolCallCompleted` events without walking the provider list
    /// on the hot path.
    tool_metadata: Arc<HashMap<String, GadgetMetadata>>,
}

impl GadgetRegistry {
    /// All registered tool schemas, flattened. Includes duplicates
    /// (see `freeze` notes). Callers that need unique names should
    /// dedupe on `schema.name`.
    pub fn all_schemas(&self) -> &[GadgetSchema] {
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
    /// - `GadgetTier::Read` → always included (V1 forces read = Auto)
    /// - `GadgetTier::Write` → included iff the matching subcategory is not
    ///   `GadgetMode::Never`. P2A-specific: the subcategory → tool mapping
    ///   is based on the tool name prefix:
    ///   - `wiki.*` → `write.wiki_write`
    ///   - `infra.*` → `write.infra_write` (P2C tools, not yet live)
    ///   - `scheduler.*` → `write.scheduler_write` (P3, not yet live)
    ///   - `provider.*` → `write.provider_mutate` (P2C, not yet live)
    ///   - anything else → `write.default_mode`
    /// - `GadgetTier::Destructive` → included iff `destructive.enabled = true`
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

    /// Number of distinct tool names that survived the operator-config
    /// gate at freeze time. Tests + metrics use this to introspect the
    /// L3 allowed-set without exposing the internal `HashSet`.
    pub fn allowed_names_len(&self) -> usize {
        self.allowed_names.len()
    }

    /// True if the tool name is operator-allowed under the config
    /// passed to `freeze()`. Used by tests + the L3 gate in `dispatch`.
    pub fn is_tool_allowed(&self, name: &str) -> bool {
        self.allowed_names.contains(name)
    }

    /// Cheap `Arc` clone of the `(tool_name → GadgetMetadata)` snapshot
    /// used by `gadgetron-penny::session::drive` + the stream-level
    /// audit emitter to fill `ToolCallCompleted` events.
    pub fn tool_metadata_snapshot(&self) -> Arc<HashMap<String, GadgetMetadata>> {
        self.tool_metadata.clone()
    }

    /// Dispatch a tool call to the provider that owns it.
    ///
    /// **L3 gate (defense-in-depth)**: per `04-gadget-registry.md §6 L3`
    /// and ADR-P2A-06 Implementation status addendum item 3, this method
    /// re-checks the operator config even though `build_allowed_tools`
    /// already filtered the names Claude Code sees in `--allowed-tools`.
    /// A caller that bypasses that flag — for example a direct
    /// `gadgetron gadget serve` stdio consumer, or a Claude Code
    /// `--dangerously-skip-permissions` bypass — cannot reach a
    /// `Never`/`Ask`-mode tool because the precomputed `allowed_names`
    /// set is consulted BEFORE the provider is invoked.
    ///
    /// Error ordering matters: if the tool is both unknown AND not
    /// allowed, the caller receives `UnknownTool` first so the denial
    /// reason does not leak the existence of a disabled tool to a
    /// probing caller.
    ///
    /// The returned `Result` is the raw MCP surface; callers that need
    /// an HTTP response use `GadgetronError::from(err)` via the
    /// `From<GadgetError>` impl in `gadgetron-core` to get the mapped
    /// status code + error message.
    pub async fn dispatch(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<GadgetResult, GadgetError> {
        // L3 gate: reject disabled tools before provider lookup.
        if !self.allowed_names.contains(name) {
            // Preserve UnknownTool semantics: if the tool is not
            // registered at all, emit UnknownTool so the existing
            // `dispatch_unknown_tool_returns_unknown_tool_error` test
            // (and callers that rely on the distinction) still pass.
            if !self.by_tool_name.contains_key(name) {
                return Err(GadgetError::UnknownGadget(name.to_string()));
            }
            return Err(GadgetError::Denied {
                reason: format!("tool '{name}' disabled by operator config"),
            });
        }
        let provider = self
            .by_tool_name
            .get(name)
            .ok_or_else(|| GadgetError::UnknownGadget(name.to_string()))?;
        provider.call(name, args).await
    }
}

/// `GadgetDispatcher` impl for the gateway seam (workbench direct
/// actions). Delegates to the inherent `dispatch()` method so the L3
/// allowed-names gate is preserved for any path that reaches the
/// registry — Penny's agent loop AND the workbench path both go through
/// the same gate.
#[async_trait::async_trait]
impl gadgetron_core::agent::tools::GadgetDispatcher for GadgetRegistry {
    async fn dispatch_gadget(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<GadgetResult, GadgetError> {
        self.dispatch(name, args).await
    }
}

/// Schema-discovery seam consumed by the gateway's MCP `/v1/tools`
/// endpoint. Returns an owned `Vec` so the caller is insulated from
/// the registry's internal `Arc<[GadgetSchema]>` storage — the cost is
/// one clone per listing (O(schemas), acceptable for a discovery
/// endpoint that runs at single-digit QPS).
impl gadgetron_core::agent::tools::GadgetCatalog for GadgetRegistry {
    fn all_schemas(&self) -> Vec<GadgetSchema> {
        self.all_schemas().to_vec()
    }
}

/// Determine whether a tool should appear in `--allowed-tools`.
///
/// Per ADR-P2A-06 §"Tier + Mode in P2A", `Ask` is treated as `Never` because
/// the interactive approval flow is deferred to Phase 2B. Only `Auto` tools
/// reach Claude Code's allowed-tools flag.
fn tool_is_enabled(schema: &GadgetSchema, cfg: &AgentConfig) -> bool {
    match schema.tier {
        GadgetTier::Read => true,
        GadgetTier::Write => {
            let mode = resolve_write_mode(&schema.name, cfg);
            !matches!(mode, GadgetMode::Never | GadgetMode::Ask)
        }
        GadgetTier::Destructive => cfg.gadgets.destructive.enabled,
    }
}

/// Map a `GadgetTier::Write` tool name to the config subcategory that
/// controls its mode.
fn resolve_write_mode(name: &str, cfg: &AgentConfig) -> GadgetMode {
    let w = &cfg.gadgets.write;
    let prefix = name.split('.').next().unwrap_or("");
    match prefix {
        "wiki" => w.wiki_write,
        "infra" => w.infra_write,
        "scheduler" => w.scheduler_write,
        "provider" => w.provider_mutate,
        "server" => w.server_admin,
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
        schemas: Vec<GadgetSchema>,
        available: bool,
        response: Result<GadgetResult, GadgetError>,
    }

    impl TestProvider {
        fn new(category: &'static str) -> Self {
            Self {
                category,
                schemas: Vec::new(),
                available: true,
                response: Ok(GadgetResult {
                    content: json!({"ok": true}),
                    is_error: false,
                }),
            }
        }

        fn with_tool(mut self, name: &str, tier: GadgetTier) -> Self {
            self.schemas.push(GadgetSchema {
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
    impl GadgetProvider for TestProvider {
        fn category(&self) -> &'static str {
            self.category
        }
        fn gadget_schemas(&self) -> Vec<GadgetSchema> {
            self.schemas.clone()
        }
        async fn call(
            &self,
            _name: &str,
            _args: serde_json::Value,
        ) -> Result<GadgetResult, GadgetError> {
            // Return a cloned result; GadgetError isn't Clone so we map.
            match &self.response {
                Ok(r) => Ok(r.clone()),
                Err(e) => Err(clone_err(e)),
            }
        }
        fn is_available(&self) -> bool {
            self.available
        }
    }

    fn clone_err(e: &GadgetError) -> GadgetError {
        match e {
            GadgetError::UnknownGadget(s) => GadgetError::UnknownGadget(s.clone()),
            GadgetError::Denied { reason } => GadgetError::Denied {
                reason: reason.clone(),
            },
            GadgetError::RateLimited {
                gadget,
                remaining,
                limit,
            } => GadgetError::RateLimited {
                gadget: gadget.clone(),
                remaining: *remaining,
                limit: *limit,
            },
            GadgetError::ApprovalTimeout { secs } => GadgetError::ApprovalTimeout { secs: *secs },
            GadgetError::InvalidArgs(s) => GadgetError::InvalidArgs(s.clone()),
            GadgetError::Execution(s) => GadgetError::Execution(s.clone()),
        }
    }

    // ---- register + freeze ----

    /// Shared default-config helper for tests that don't care about
    /// which tools are allowed by the L3 gate — they just need a valid
    /// `AgentConfig` to pass to `freeze()`. Relies on
    /// `AgentConfig::default()` having `wiki_write = Auto` (per
    /// `01-knowledge-layer.md` tests/registry.rs:76).
    fn default_cfg() -> AgentConfig {
        AgentConfig::default()
    }

    #[test]
    fn register_then_freeze_produces_dispatch_map() {
        let mut builder = GadgetRegistryBuilder::new();
        let provider = Arc::new(
            TestProvider::new("knowledge")
                .with_tool("wiki.read", GadgetTier::Read)
                .with_tool("wiki.write", GadgetTier::Write),
        );
        builder.register(provider).expect("register");
        assert_eq!(builder.provider_count(), 1);

        let registry = builder.freeze(&default_cfg());
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.all_schemas().len(), 2);
    }

    #[test]
    fn register_empty_builder_produces_empty_registry() {
        let registry = GadgetRegistryBuilder::new().freeze(&default_cfg());
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn register_unavailable_provider_is_skipped() {
        let mut builder = GadgetRegistryBuilder::new();
        let provider = Arc::new(
            TestProvider::new("knowledge")
                .with_tool("wiki.read", GadgetTier::Read)
                .unavailable(),
        );
        builder.register(provider).expect("register ok (skipped)");
        assert_eq!(builder.provider_count(), 0);
        assert!(builder.freeze(&default_cfg()).is_empty());
    }

    #[test]
    fn register_reserved_agent_category_fails() {
        let mut builder = GadgetRegistryBuilder::new();
        let provider = Arc::new(TestProvider::new("agent").with_tool("foo", GadgetTier::Read));
        let err = builder.register(provider).expect_err("must fail");
        assert!(matches!(err, GadgetError::Denied { .. }));
    }

    #[test]
    fn register_reserved_tool_name_fails() {
        let mut builder = GadgetRegistryBuilder::new();
        let provider =
            Arc::new(TestProvider::new("knowledge").with_tool("agent.set_brain", GadgetTier::Read));
        let err = builder.register(provider).expect_err("must fail");
        assert!(matches!(err, GadgetError::Denied { .. }));
    }

    // ---- dispatch ----

    #[tokio::test]
    async fn dispatch_routes_to_matching_provider() {
        let mut builder = GadgetRegistryBuilder::new();
        builder
            .register(Arc::new(
                TestProvider::new("knowledge").with_tool("wiki.read", GadgetTier::Read),
            ))
            .unwrap();
        let registry = builder.freeze(&default_cfg());
        let result = registry
            .dispatch("wiki.read", json!({"name": "home"}))
            .await
            .expect("dispatch ok");
        assert_eq!(result.content, json!({"ok": true}));
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_unknown_tool_error() {
        let registry = GadgetRegistryBuilder::new().freeze(&default_cfg());
        let err = registry
            .dispatch("ghost.tool", json!({}))
            .await
            .expect_err("unknown");
        match err {
            GadgetError::UnknownGadget(name) => assert_eq!(name, "ghost.tool"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn duplicate_tool_name_last_wins_in_dispatch() {
        let mut builder = GadgetRegistryBuilder::new();
        builder
            .register(Arc::new(
                TestProvider::new("knowledge").with_tool("wiki.read", GadgetTier::Read),
            ))
            .unwrap();
        // A second provider claims the same name.
        builder
            .register(Arc::new(
                TestProvider::new("custom").with_tool("wiki.read", GadgetTier::Read),
            ))
            .unwrap();
        let registry = builder.freeze(&default_cfg());
        // Dispatch map has 1 unique name; flat schema vec has 2.
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.all_schemas().len(), 2);
    }

    // ---- L3 defense-in-depth (ADR-P2A-06 addendum item 3) ----

    #[tokio::test]
    async fn gadget_server_rejects_never_mode_tool_even_when_dispatched_directly() {
        // A provider registers `wiki.write` as a GadgetTier::Write tool whose
        // `call()` returns `Ok` unconditionally. The registry is frozen
        // against an `AgentConfig` where `wiki_write = Never`. A direct
        // `dispatch("wiki.write")` MUST return `GadgetError::Denied` — the
        // L3 gate rejects the call before the provider's `call` runs.
        //
        // Without the L3 gate the provider's `Ok` return leaks through,
        // and a caller that bypassed `build_allowed_tools` (e.g. a
        // direct `gadgetron gadget serve` stdio consumer or a Claude Code
        // `--dangerously-skip-permissions` abuse) could reach a
        // Never-mode tool. Regression-locked here at the dispatch
        // layer; gadget_server.rs `handle_request` routes through
        // `registry.dispatch` so the L3 check also covers stdio
        // requests without a separate integration test.
        let mut builder = GadgetRegistryBuilder::new();
        builder
            .register(Arc::new(
                TestProvider::new("knowledge").with_tool("wiki.write", GadgetTier::Write),
            ))
            .unwrap();
        let cfg = cfg_with_overrides(
            GadgetMode::Auto,  // default_mode (irrelevant for wiki.*)
            GadgetMode::Never, // wiki_write — the one under test
            GadgetMode::Auto,  // infra_write
            false,
        );
        let registry = builder.freeze(&cfg);
        // The tool exists in the dispatch map …
        assert_eq!(registry.len(), 1);
        // … but is NOT in allowed_names because of Never-mode.
        assert!(!registry.is_tool_allowed("wiki.write"));
        // … and dispatch rejects it with Denied (not UnknownTool).
        let err = registry
            .dispatch("wiki.write", json!({"name": "home", "content": "hi"}))
            .await
            .expect_err("dispatch must reject Never-mode tool");
        match err {
            GadgetError::Denied { reason } => {
                assert!(
                    reason.contains("wiki.write"),
                    "denial reason should mention the tool name: {reason}"
                );
                assert!(
                    reason.contains("operator config"),
                    "denial reason should cite operator config: {reason}"
                );
            }
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_takes_precedence_over_denied() {
        // If a tool is BOTH unknown (not in by_tool_name) AND not in
        // allowed_names, the caller must see UnknownTool — not Denied.
        // This prevents a probing caller from learning whether a
        // specific disabled tool exists by comparing error variants.
        let registry = GadgetRegistryBuilder::new().freeze(&default_cfg());
        let err = registry
            .dispatch("ghost.never.seen", json!({}))
            .await
            .expect_err("must error");
        assert!(
            matches!(err, GadgetError::UnknownGadget(_)),
            "unknown tool must beat Denied: {err:?}"
        );
    }

    #[tokio::test]
    async fn dispatch_ask_mode_tool_is_also_denied() {
        // ADR-P2A-06: Ask === Never in P2A. The L2 build_allowed_tools
        // filter already excludes Ask; the L3 gate must match so the
        // two sources of truth can never drift.
        let mut builder = GadgetRegistryBuilder::new();
        builder
            .register(Arc::new(
                TestProvider::new("knowledge").with_tool("wiki.write", GadgetTier::Write),
            ))
            .unwrap();
        let cfg = cfg_with_overrides(
            GadgetMode::Auto,
            GadgetMode::Ask, // wiki_write in Ask mode
            GadgetMode::Auto,
            false,
        );
        let registry = builder.freeze(&cfg);
        assert!(!registry.is_tool_allowed("wiki.write"));
        let err = registry
            .dispatch("wiki.write", json!({}))
            .await
            .expect_err("Ask must be denied");
        assert!(matches!(err, GadgetError::Denied { .. }));
    }

    // ---- build_allowed_tools ----

    fn cfg_with_overrides(
        write_default: GadgetMode,
        wiki_write: GadgetMode,
        infra_write: GadgetMode,
        destructive_enabled: bool,
    ) -> AgentConfig {
        let mut cfg = AgentConfig::default();
        cfg.gadgets.write.default_mode = write_default;
        cfg.gadgets.write.wiki_write = wiki_write;
        cfg.gadgets.write.infra_write = infra_write;
        cfg.gadgets.destructive.enabled = destructive_enabled;
        cfg
    }

    /// Helper that builds a full-spectrum registry. The `cfg` argument
    /// controls the L3 gate inside `freeze`. Tests that care about
    /// `all_schemas` (not dispatch) pass a permissive `AgentConfig`
    /// via `cfg_with_overrides` or `default_cfg`.
    fn registry_with_full_set_cfg(cfg: &AgentConfig) -> GadgetRegistry {
        let mut builder = GadgetRegistryBuilder::new();
        builder
            .register(Arc::new(
                TestProvider::new("knowledge")
                    .with_tool("wiki.read", GadgetTier::Read)
                    .with_tool("wiki.write", GadgetTier::Write)
                    .with_tool("web.search", GadgetTier::Read)
                    .with_tool("wiki.delete", GadgetTier::Destructive),
            ))
            .unwrap();
        builder
            .register(Arc::new(
                TestProvider::new("infrastructure")
                    .with_tool("infra.list_nodes", GadgetTier::Read)
                    .with_tool("infra.deploy_model", GadgetTier::Write),
            ))
            .unwrap();
        builder.freeze(cfg)
    }

    /// Back-compat helper: freezes with a permissive config that
    /// enables every Write subcategory. Previously this was the only
    /// helper because freeze didn't take cfg.
    fn registry_with_full_set() -> GadgetRegistry {
        registry_with_full_set_cfg(&cfg_with_overrides(
            GadgetMode::Auto,
            GadgetMode::Auto,
            GadgetMode::Auto,
            true, // destructive enabled so the T3 filter tests still see wiki.delete
        ))
    }

    #[test]
    fn build_allowed_tools_t1_always_present() {
        let reg = registry_with_full_set();
        // Even with all writes never and destructive disabled, T1 reads remain.
        let cfg = cfg_with_overrides(
            GadgetMode::Never,
            GadgetMode::Never,
            GadgetMode::Never,
            false,
        );
        let tools = reg.build_allowed_tools(&cfg);
        assert!(tools.contains(&"wiki.read".to_string()));
        assert!(tools.contains(&"web.search".to_string()));
        assert!(tools.contains(&"infra.list_nodes".to_string()));
    }

    #[test]
    fn build_allowed_tools_wiki_write_auto_included() {
        let reg = registry_with_full_set();
        let cfg = cfg_with_overrides(
            GadgetMode::Never,
            GadgetMode::Auto,
            GadgetMode::Never,
            false,
        );
        let tools = reg.build_allowed_tools(&cfg);
        assert!(tools.contains(&"wiki.write".to_string()));
        // infra.deploy_model is still Never.
        assert!(!tools.contains(&"infra.deploy_model".to_string()));
    }

    #[test]
    fn build_allowed_tools_wiki_write_never_omitted() {
        let reg = registry_with_full_set();
        let cfg = cfg_with_overrides(GadgetMode::Auto, GadgetMode::Never, GadgetMode::Auto, false);
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
        let cfg = cfg_with_overrides(GadgetMode::Ask, GadgetMode::Ask, GadgetMode::Ask, false);
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
        let cfg = cfg_with_overrides(GadgetMode::Auto, GadgetMode::Auto, GadgetMode::Auto, false);
        let tools = reg.build_allowed_tools(&cfg);
        // wiki.delete is T3 — must NOT appear when enabled=false.
        assert!(!tools.contains(&"wiki.delete".to_string()));
    }

    #[test]
    fn build_allowed_tools_t3_enabled_includes_destructive() {
        let reg = registry_with_full_set();
        let cfg = cfg_with_overrides(GadgetMode::Auto, GadgetMode::Auto, GadgetMode::Auto, true);
        let tools = reg.build_allowed_tools(&cfg);
        assert!(tools.contains(&"wiki.delete".to_string()));
    }

    #[test]
    fn build_allowed_tools_output_is_sorted_and_deduped() {
        let reg = registry_with_full_set();
        let cfg = cfg_with_overrides(GadgetMode::Auto, GadgetMode::Auto, GadgetMode::Auto, false);
        let tools = reg.build_allowed_tools(&cfg);
        let mut sorted = tools.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(tools, sorted);
    }

    #[test]
    fn resolve_write_mode_by_prefix() {
        let mut cfg = AgentConfig::default();
        cfg.gadgets.write.default_mode = GadgetMode::Ask;
        cfg.gadgets.write.wiki_write = GadgetMode::Auto;
        cfg.gadgets.write.infra_write = GadgetMode::Never;
        cfg.gadgets.write.scheduler_write = GadgetMode::Auto;
        cfg.gadgets.write.provider_mutate = GadgetMode::Never;

        assert!(matches!(
            resolve_write_mode("wiki.write", &cfg),
            GadgetMode::Auto
        ));
        assert!(matches!(
            resolve_write_mode("infra.deploy_model", &cfg),
            GadgetMode::Never
        ));
        assert!(matches!(
            resolve_write_mode("scheduler.schedule_job", &cfg),
            GadgetMode::Auto
        ));
        assert!(matches!(
            resolve_write_mode("provider.rotate_key", &cfg),
            GadgetMode::Never
        ));
        // Unrecognized prefix uses default_mode.
        assert!(matches!(
            resolve_write_mode("weird.tool", &cfg),
            GadgetMode::Ask
        ));
    }
}
