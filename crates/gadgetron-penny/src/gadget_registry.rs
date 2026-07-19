//! `GadgetRegistry` — the MCP tool dispatch table for Penny.
//!
//! # Lifecycle (builder/freeze pattern)
//!
//! 1. `main()` constructs `GadgetRegistryBuilder::new()` and calls
//!    `register()` for each concrete `GadgetProvider` implementation
//!    (e.g. `KnowledgeToolProvider`).
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
//! - The agent CANNOT register new providers at runtime. Making the
//!   registry type-system-level immutable post-startup enforces this.
//!   A compromised provider cannot call `registry.register(...)`
//!   because that method doesn't exist on the frozen type.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use gadgetron_core::agent::config::{AgentConfig, GadgetMode, GadgetsConfig};
use gadgetron_core::agent::tools::{
    ensure_tool_name_allowed, DynamicGadgetSurface, GadgetDispatchContext, GadgetError,
    GadgetProvider, GadgetResult, GadgetSchema, GadgetTier,
};
use gadgetron_core::audit::GadgetMetadata;
use gadgetron_core::knowledge::AuthenticatedContext;
use gadgetron_core::workbench::{
    ApprovalRequest, ApprovalResumeStrategy, ApprovalState, ApprovalStore,
};
use uuid::Uuid;

/// Default tenant id used by the in-process approvals path. Must match
/// the value the UI's `loganalysis-list` etc. uses so a Penny-created
/// approval shows up in the operator's `GET /approvals/pending` list.
const DEFAULT_TENANT_ID: &str = "00000000-0000-0000-0000-000000000001";

/// Upper bound on how long a Penny dispatch waits for a human to click
/// approve / deny. After this elapses the call returns
/// `GadgetError::Denied` with reason "approval timed out".
const APPROVAL_WAIT: Duration = Duration::from_secs(120);
/// Polling interval inside the wait loop. SSE push is the eventual
/// upgrade; 1 s polling is fine for the operator-attended demo.
const APPROVAL_POLL: Duration = Duration::from_secs(1);

/// Configuration for routing every recognized child tool call back to the
/// parent gateway. The parent owns the live catalog, policy, Review, audit,
/// and provider dispatch boundary.
#[derive(Clone, Debug)]
pub struct ForwardConfig {
    /// Base URL of the parent gateway, e.g. `http://127.0.0.1:18080`.
    pub base_url: String,
    /// Bearer token used to authenticate the forwarded
    /// `/v1/tools/{name}/invoke` POST.
    pub auth_token: String,
}

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
    /// `allowed_names` set used by `dispatch()` for L3 defense-in-depth.
    /// A caller that bypasses
    /// `build_allowed_tools` — for example a direct `gadgetron gadget serve`
    /// consumer — cannot reach a `Never`/`Ask`-mode tool because
    /// `dispatch()` checks the precomputed set before routing to the
    /// provider. Runtime mode changes are applied by `reconfigure()`,
    /// which atomically swaps the allowed/ask sets and the live gadget
    /// mode snapshot used for new Penny subprocesses.
    ///
    /// If two providers register tools with the same namespaced name,
    /// the later-registered one wins in the dispatch map — but the
    /// `all_schemas` vec retains both entries so operators see the
    /// duplicate in `/v1/tools`. The test
    /// `duplicate_tool_name_last_wins_in_dispatch` locks this in.
    pub fn freeze(self, cfg: &AgentConfig) -> GadgetRegistry {
        self.freeze_inner(cfg, None, None, Vec::new())
    }

    /// Same as `freeze` but threads an `ApprovalStore` so Ask-mode
    /// Write tools can route through the operator-approval path
    /// instead of being silently filtered out. `None` keeps the
    /// approval-less behavior (Ask is treated as Never at dispatch time).
    pub fn freeze_with_approvals(
        self,
        cfg: &AgentConfig,
        approval_store: Option<Arc<dyn ApprovalStore>>,
    ) -> GadgetRegistry {
        self.freeze_inner(cfg, approval_store, None, Vec::new())
    }

    /// Freeze a child registry that forwards every recognized tool to the
    /// parent HTTP boundary instead of running a local provider.
    pub fn freeze_with_forwarding(
        self,
        cfg: &AgentConfig,
        forward: ForwardConfig,
    ) -> GadgetRegistry {
        self.freeze_inner(cfg, None, Some(forward), Vec::new())
    }

    /// Freeze a child-side registry with an explicit snapshot of schemas that
    /// exist in the parent gateway but not in this process. These schemas are
    /// discoverable through MCP and every invocation is forwarded to the
    /// parent, where authentication, policy, approval, lifecycle, and audit
    /// are applied again. Unknown names are never forwarded implicitly.
    pub fn freeze_with_forwarding_and_schemas(
        self,
        cfg: &AgentConfig,
        forward: ForwardConfig,
        schemas: Vec<GadgetSchema>,
    ) -> Result<GadgetRegistry, GadgetError> {
        let mut seen = HashSet::new();
        for schema in &schemas {
            let category = schema.name.split('.').next().unwrap_or("");
            ensure_tool_name_allowed(&schema.name, category)?;
            if !seen.insert(schema.name.clone()) {
                return Err(GadgetError::InvalidArgs(format!(
                    "duplicate forwarded gadget schema: {}",
                    schema.name
                )));
            }
        }
        Ok(self.freeze_inner(cfg, None, Some(forward), schemas))
    }

    fn freeze_inner(
        self,
        cfg: &AgentConfig,
        approval_store: Option<Arc<dyn ApprovalStore>>,
        forward: Option<ForwardConfig>,
        forwarded_schemas: Vec<GadgetSchema>,
    ) -> GadgetRegistry {
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
        // The child can load the same local provider that the parent exposes.
        // Local ownership wins for exact-name overlap; only schemas unavailable
        // in this process become forward-only capabilities.
        let forwarded_schemas: BTreeMap<String, GadgetSchema> = forwarded_schemas
            .into_iter()
            .filter(|schema| !by_tool_name.contains_key(&schema.name))
            .map(|schema| (schema.name.clone(), schema))
            .collect();
        for schema in forwarded_schemas.values() {
            let category = schema.name.split('.').next().unwrap_or("").to_string();
            tool_metadata.insert(
                schema.name.clone(),
                GadgetMetadata {
                    tier: schema.tier.into(),
                    category,
                },
            );
        }
        // Precompute the allowed-names set. A tool is in the set iff
        // `tool_is_enabled(schema, cfg, has_ask_handler)` — Ask-mode
        // tools are allowed when EITHER an approval store or a
        // parent-forwarding seam is wired, because dispatch can route
        // them through the approval flow in either case. Without
        // either, the legacy "Ask collapses to Never" behavior holds.
        let has_ask_handler = approval_store.is_some() || forward.is_some();
        let allowed_names: HashSet<String> = all_schemas
            .iter()
            .chain(forwarded_schemas.values())
            .filter(|schema| tool_is_enabled(schema, cfg, has_ask_handler))
            .map(|schema| schema.name.clone())
            .collect();
        // Tools whose effective mode is Ask. Carried separately so
        // `dispatch` can decide between (a) call provider directly or
        // (b) create approval + wait + call provider on approve, or
        // (c) forward to the parent gateway.
        let ask_names: HashSet<String> = all_schemas
            .iter()
            .chain(forwarded_schemas.values())
            .filter(|schema| {
                matches!(schema.tier, GadgetTier::Write)
                    && matches!(resolve_write_mode(&schema.name, cfg), GadgetMode::Ask)
            })
            .map(|schema| schema.name.clone())
            .collect();
        let forward_client = forward.as_ref().map(|_| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(150))
                .build()
                .expect("reqwest client must build")
        });
        GadgetRegistry {
            by_tool_name,
            all_schemas: Arc::from(all_schemas.into_boxed_slice()),
            forwarded_schemas: Arc::new(forwarded_schemas),
            gadgets_config: Arc::new(ArcSwap::new(Arc::new(cfg.gadgets.clone()))),
            allowed_names: Arc::new(ArcSwap::new(Arc::new(allowed_names))),
            ask_names: Arc::new(ArcSwap::new(Arc::new(ask_names))),
            tool_metadata: Arc::new(tool_metadata),
            dynamic_surface: Arc::new(RwLock::new(None)),
            approval_store,
            forward,
            forward_client,
        }
    }
}

impl Default for GadgetRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Dispatch table. Cheap to clone (internal `Arc`s). The operator-
/// allowed / Ask-mode sets live behind `ArcSwap`, so a runtime
/// config update (Side Panel → Tool Modes) can atomically swap in a
/// new allowlist without stopping the server.
#[derive(Clone)]
pub struct GadgetRegistry {
    by_tool_name: HashMap<String, Arc<dyn GadgetProvider>>,
    all_schemas: Arc<[GadgetSchema]>,
    /// Parent-advertised schemas unavailable in this process. Kept separate
    /// from local providers so dispatch can only route these exact names over
    /// the authenticated callback seam.
    forwarded_schemas: Arc<BTreeMap<String, GadgetSchema>>,
    /// Live gadget mode snapshot. Workbench updates swap this together
    /// with the allow/ask sets so new Penny subprocesses can forward
    /// the same policy into their stdio MCP child.
    gadgets_config: Arc<ArcSwap<GadgetsConfig>>,
    /// Precomputed set of tool names whose tier × mode resolves to
    /// "operator-allowed" under the current config. Used by
    /// `dispatch()` for L3 defense-in-depth. Tools NOT in this set
    /// are rejected with `GadgetError::Denied` before the provider is
    /// invoked. Replaced atomically by `reconfigure`.
    allowed_names: Arc<ArcSwap<HashSet<String>>>,
    /// Tools whose mode resolves to `Ask`. Listed in `allowed_names`
    /// (so Penny sees them and can call them) but routed through the
    /// `approval_store` before reaching the provider. Replaced
    /// atomically by `reconfigure`.
    ask_names: Arc<ArcSwap<HashSet<String>>>,
    /// Denormalized `(tier, category)` per tool name. Used by the
    /// Penny audit emitter in `session.rs::drive` to fill
    /// `ToolCallCompleted` events without walking the provider list
    /// on the hot path.
    tool_metadata: Arc<HashMap<String, GadgetMetadata>>,
    /// Enabled external Bundle surface. The provider set remains immutable;
    /// only Core's trusted lifecycle manager may replace this single generic
    /// adapter, whose own signed capability snapshot changes on enable/disable.
    dynamic_surface: Arc<RwLock<Option<Arc<dyn DynamicGadgetSurface>>>>,
    /// Approval persistence — when an Ask-mode tool is invoked, the
    /// dispatch path creates a pending request here, polls until
    /// resolution (or timeout), and only then runs the provider.
    /// `None` in legacy/test wirings; in that case Ask-mode tools are
    /// rejected with `GadgetError::Denied` to fail closed.
    approval_store: Option<Arc<dyn ApprovalStore>>,
    /// Parent-forwarding seam used by the `gadgetron gadget serve`
    /// grandchild. When `Some`, every recognized local or explicit
    /// forward-only tool POSTs to `<base_url>/v1/tools/{name}/invoke` —
    /// the parent there owns lifecycle, policy, approval, and audit.
    /// Mutually exclusive with `approval_store` in practice (the
    /// parent uses approval_store, the grandchild uses forward).
    forward: Option<ForwardConfig>,
    /// reqwest client reused for forward dispatches. `Some` iff
    /// `forward.is_some()`. Build cost is ~ms; reuse keeps connection
    /// pooling working across rapid back-to-back tool calls.
    forward_client: Option<reqwest::Client>,
}

impl GadgetRegistry {
    /// All registered tool schemas, flattened. Includes duplicates
    /// (see `freeze` notes). Callers that need unique names should
    /// dedupe on `schema.name`.
    pub fn all_schemas(&self) -> Vec<GadgetSchema> {
        let mut schemas = self.all_schemas.to_vec();
        schemas.extend(self.forwarded_schemas.values().cloned());
        if let Some(surface) = self.dynamic_surface() {
            schemas.extend(surface.all_schemas());
        }
        schemas
    }

    /// Number of distinct tool names in the dispatch map.
    pub fn len(&self) -> usize {
        self.all_schemas()
            .into_iter()
            .map(|schema| schema.name)
            .collect::<HashSet<_>>()
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_tool_name.is_empty()
            && self.forwarded_schemas.is_empty()
            && self
                .dynamic_surface()
                .is_none_or(|surface| surface.all_schemas().is_empty())
    }

    /// Attach the one Core-owned dynamic adapter. Domain Bundles never receive
    /// this registry and cannot mutate the attachment themselves.
    pub fn attach_dynamic_surface(&self, surface: Arc<dyn DynamicGadgetSurface>) {
        *self
            .dynamic_surface
            .write()
            .expect("dynamic Gadget surface lock poisoned") = Some(surface);
    }

    fn dynamic_surface(&self) -> Option<Arc<dyn DynamicGadgetSurface>> {
        self.dynamic_surface
            .read()
            .expect("dynamic Gadget surface lock poisoned")
            .clone()
    }

    /// Build the `--allowed-tools` list passed to `claude -p`.
    /// Filters the full schema set per `AgentConfig`:
    ///
    /// - `GadgetTier::Read` → always included (V1 forces read = Auto)
    /// - `GadgetTier::Write` → included iff the matching subcategory is not
    ///   `GadgetMode::Never`. The subcategory → tool mapping is based on
    ///   the tool name prefix:
    ///   - `wiki.*` → `write.wiki_write`
    ///   - `infra.*` → `write.infra_write` (deferred tools, not yet live)
    ///   - `scheduler.*` → `write.scheduler_write` (deferred, not yet live)
    ///   - `provider.*` → `write.provider_mutate` (deferred, not yet live)
    ///   - anything else → `write.default_mode`
    /// - `GadgetTier::Destructive` → included iff `destructive.enabled = true`
    ///   (always false under V5). Under Path 1 this branch is
    ///   effectively dead code — the filter never emits a T3 tool — but
    ///   the logic is in place for when approval flow lands.
    ///
    /// Output is sorted by tool name for deterministic test snapshots.
    pub fn build_allowed_tools(&self, cfg: &AgentConfig) -> Vec<String> {
        let has_ask_handler = self.approval_store.is_some() || self.forward.is_some();
        let mut out: Vec<String> = self
            .all_schemas()
            .into_iter()
            .filter(|schema| tool_is_enabled(schema, cfg, has_ask_handler))
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
        self.all_schemas()
            .into_iter()
            .filter(|schema| self.schema_is_allowed(schema))
            .map(|schema| schema.name)
            .collect::<HashSet<_>>()
            .len()
    }

    /// True if the tool name is operator-allowed under the current
    /// config. Used by tests + the L3 gate in `dispatch`.
    pub fn is_tool_allowed(&self, name: &str) -> bool {
        if self.by_tool_name.contains_key(name) {
            return self.allowed_names.load().contains(name);
        }
        if let Some(schema) = self.forwarded_schemas.get(name) {
            return self.schema_is_allowed(schema);
        }
        self.dynamic_surface()
            .and_then(|surface| {
                surface
                    .all_schemas()
                    .into_iter()
                    .find(|schema| schema.name == name)
            })
            .is_some_and(|schema| self.schema_is_allowed(&schema))
    }

    /// Recompute `allowed_names` + `ask_names` from the live schemas
    /// against `cfg` and atomically swap them in. Called by the
    /// `PATCH /workbench/agent/modes` handler so an operator flipping
    /// a bucket from `Auto`→`Ask` in the Side Panel takes effect on
    /// the NEXT dispatch without restarting the server.
    ///
    /// Does NOT restart running Penny subprocesses — those keep the
    /// `--allowed-tools` list they were spawned with. New subprocesses
    /// pick up the new list the next time Penny respawns.
    pub fn reconfigure(&self, cfg: &AgentConfig) {
        let has_ask_handler = self.approval_store.is_some() || self.forward.is_some();
        let allowed: HashSet<String> = self
            .all_schemas
            .iter()
            .chain(self.forwarded_schemas.values())
            .filter(|schema| tool_is_enabled(schema, cfg, has_ask_handler))
            .map(|schema| schema.name.clone())
            .collect();
        let ask: HashSet<String> = self
            .all_schemas
            .iter()
            .chain(self.forwarded_schemas.values())
            .filter(|schema| {
                matches!(schema.tier, GadgetTier::Write)
                    && matches!(resolve_write_mode(&schema.name, cfg), GadgetMode::Ask)
            })
            .map(|schema| schema.name.clone())
            .collect();
        self.allowed_names.store(Arc::new(allowed));
        self.ask_names.store(Arc::new(ask));
        self.gadgets_config.store(Arc::new(cfg.gadgets.clone()));
    }

    /// Snapshot of the current gadget modes after any Workbench
    /// reconfiguration. Returned by value so callers can splice it into
    /// a request-local `AgentConfig` without holding a live reference.
    pub fn current_gadgets_config(&self) -> GadgetsConfig {
        (*self.gadgets_config.load_full()).clone()
    }

    /// Cheap `Arc` clone of the `(tool_name → GadgetMetadata)` snapshot
    /// used by `gadgetron-penny::session::drive` + the stream-level
    /// audit emitter to fill `ToolCallCompleted` events.
    pub fn tool_metadata_snapshot(&self) -> Arc<HashMap<String, GadgetMetadata>> {
        let Some(surface) = self.dynamic_surface() else {
            return self.tool_metadata.clone();
        };
        let mut metadata = (*self.tool_metadata).clone();
        for schema in surface.all_schemas() {
            let category = schema.name.split('.').next().unwrap_or("").to_string();
            metadata.insert(
                schema.name,
                GadgetMetadata {
                    tier: schema.tier.into(),
                    category,
                },
            );
        }
        Arc::new(metadata)
    }

    fn schema_is_allowed(&self, schema: &GadgetSchema) -> bool {
        if self.by_tool_name.contains_key(&schema.name) {
            return self.allowed_names.load().contains(&schema.name);
        }
        let gadgets = self.gadgets_config.load();
        tool_is_enabled_for_gadgets(
            schema,
            &gadgets,
            self.approval_store.is_some() || self.forward.is_some(),
        )
    }

    /// Dispatch a tool call to the provider that owns it.
    ///
    /// **L3 gate (defense-in-depth)**: this method re-checks the
    /// operator config even though `build_allowed_tools`
    /// already filtered the names Claude Code sees in `--allowed-tools`.
    /// A caller that does not use the command-level filter — for example
    /// a direct `gadgetron gadget serve` stdio consumer — cannot reach a
    /// `Never`/`Ask`-mode tool because the precomputed `allowed_names` set
    /// is consulted BEFORE the provider is invoked.
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
        self.dispatch_inner(None, name, args).await
    }

    pub async fn dispatch_with_context(
        &self,
        context: GadgetDispatchContext,
        name: &str,
        args: serde_json::Value,
    ) -> Result<GadgetResult, GadgetError> {
        self.dispatch_inner(Some(context), name, args).await
    }

    async fn dispatch_inner(
        &self,
        context: Option<GadgetDispatchContext>,
        name: &str,
        args: serde_json::Value,
    ) -> Result<GadgetResult, GadgetError> {
        let (target, is_allowed, requires_approval) =
            if let Some(provider) = self.by_tool_name.get(name) {
                (
                    DispatchTarget::Static(provider.clone()),
                    self.allowed_names.load().contains(name),
                    self.ask_names.load().contains(name),
                )
            } else if let Some(schema) = self.forwarded_schemas.get(name) {
                let gadgets = self.gadgets_config.load();
                let has_ask_handler = self.forward.is_some();
                let allowed = tool_is_enabled_for_gadgets(schema, &gadgets, has_ask_handler);
                let ask = matches!(schema.tier, GadgetTier::Write)
                    && matches!(
                        resolve_write_mode_for_gadgets(&schema.name, &gadgets),
                        GadgetMode::Ask
                    );
                (DispatchTarget::Forwarded, allowed, ask)
            } else {
                let surface = self
                    .dynamic_surface()
                    .ok_or_else(|| GadgetError::UnknownGadget(name.to_string()))?;
                let schema = surface
                    .all_schemas()
                    .into_iter()
                    .find(|schema| schema.name == name)
                    .ok_or_else(|| GadgetError::UnknownGadget(name.to_string()))?;
                let gadgets = self.gadgets_config.load();
                let has_ask_handler = self.approval_store.is_some() || self.forward.is_some();
                let allowed = tool_is_enabled_for_gadgets(&schema, &gadgets, has_ask_handler);
                let ask = matches!(schema.tier, GadgetTier::Write)
                    && matches!(
                        resolve_write_mode_for_gadgets(&schema.name, &gadgets),
                        GadgetMode::Ask
                    );
                (DispatchTarget::Dynamic(surface), allowed, ask)
            };
        // Agent subprocess children with a parent callback never execute a
        // local provider. The parent owns the live catalog, versioned policy,
        // approval and audit boundary for Read/Auto as well as mutations.
        if let (Some(forward), Some(client)) = (self.forward.as_ref(), self.forward_client.as_ref())
        {
            return forward_tool_call(client, forward, name, args).await;
        }

        let policy_authorized = context
            .as_ref()
            .is_some_and(|context| context.policy_authorized);
        if !is_allowed && !policy_authorized {
            return Err(GadgetError::Denied {
                reason: format!("tool '{name}' disabled by operator config"),
            });
        }

        // Forward-only schemas never execute locally, including Read/Auto
        // tools. The parent gateway rechecks the live catalog and policy, so a
        // Bundle disabled after this child took its snapshot fails closed.
        if matches!(&target, DispatchTarget::Forwarded) {
            return Err(GadgetError::Denied {
                reason: format!("forward-only tool '{name}' has no authenticated parent callback"),
            });
        }

        // Legacy in-process Ask gate. Common-policy authorized calls already
        // completed Review at the parent boundary and do not enter here.
        if requires_approval
            && !policy_authorized
            && !context
                .as_ref()
                .is_some_and(|context| context.approval_granted)
        {
            let Some(store) = self.approval_store.as_ref() else {
                return Err(GadgetError::Denied {
                    reason: format!(
                        "tool '{name}' requires operator approval but no approval store is wired"
                    ),
                });
            };
            let approved_args =
                wait_for_approval(store.as_ref(), context.as_ref(), name, args).await?;
            return dispatch_target(target, context, name, approved_args).await;
        }
        dispatch_target(target, context, name, args).await
    }
}

enum DispatchTarget {
    Static(Arc<dyn GadgetProvider>),
    Dynamic(Arc<dyn DynamicGadgetSurface>),
    Forwarded,
}

async fn dispatch_target(
    target: DispatchTarget,
    context: Option<GadgetDispatchContext>,
    name: &str,
    args: serde_json::Value,
) -> Result<GadgetResult, GadgetError> {
    match target {
        DispatchTarget::Static(provider) => match context.as_ref() {
            Some(context) => provider.call_with_context(context, name, args).await,
            None => provider.call(name, args).await,
        },
        DispatchTarget::Dynamic(surface) => match context {
            Some(context) => {
                surface
                    .dispatch_gadget_with_context(context, name, args)
                    .await
            }
            None => surface.dispatch_gadget(name, args).await,
        },
        DispatchTarget::Forwarded => Err(GadgetError::Execution(format!(
            "forward-only tool '{name}' reached local dispatch"
        ))),
    }
}

/// Create a pending `ApprovalRequest` for `(tool, args)` and poll the
/// store every `APPROVAL_POLL` until resolution or `APPROVAL_WAIT`
/// elapses. Returns the original args on approve (forwarded verbatim
/// to the provider), `GadgetError::Denied` on deny / timeout.
async fn wait_for_approval(
    store: &dyn ApprovalStore,
    context: Option<&GadgetDispatchContext>,
    tool_name: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, GadgetError> {
    let actor = AuthenticatedContext::system();
    let mut actor = actor;
    actor.tenant_id = context
        .and_then(|context| Uuid::parse_str(&context.tenant_id).ok())
        .unwrap_or_else(|| Uuid::parse_str(DEFAULT_TENANT_ID).unwrap_or(Uuid::nil()));
    if let Some(actor_id) = context.and_then(|context| Uuid::parse_str(&context.actor_id).ok()) {
        actor.api_key_id = actor_id;
        actor.real_user_id = Some(actor_id);
    }
    let id = Uuid::new_v4();
    let req = ApprovalRequest::new_pending(
        id,
        &actor,
        tool_name.to_string(),
        Some(tool_name.to_string()),
        args.clone(),
    )
    .with_resume_strategy(ApprovalResumeStrategy::WaitingCaller);
    store
        .create(req)
        .await
        .map_err(|e| GadgetError::Execution(format!("approval store create: {e}")))?;
    let deadline = Instant::now() + APPROVAL_WAIT;
    loop {
        if Instant::now() >= deadline {
            return Err(GadgetError::Denied {
                reason: format!(
                    "approval timed out after {}s — operator did not respond in time",
                    APPROVAL_WAIT.as_secs()
                ),
            });
        }
        tokio::time::sleep(APPROVAL_POLL).await;
        let cur = store
            .get(id)
            .await
            .map_err(|e| GadgetError::Execution(format!("approval store get: {e}")))?;
        match cur.state {
            ApprovalState::Pending => continue,
            ApprovalState::Approved => return Ok(args),
            ApprovalState::Denied => {
                let reason = cur
                    .deny_reason
                    .map(|r| format!(": {r}"))
                    .unwrap_or_default();
                return Err(GadgetError::Denied {
                    reason: format!("operator denied {tool_name}{reason}"),
                });
            }
        }
    }
}

/// Forward a recognized child tool call to the parent gateway. The parent's
/// `/v1/tools/{name}/invoke` handler applies the common policy, runs Review
/// when required, dispatches the provider, and returns the tool result.
///
/// The 150 s reqwest client timeout sits a hair above the 120 s
/// `APPROVAL_WAIT` so a parent timeout always fires first with a
/// clean `denied: approval timed out` instead of a network error.
async fn forward_tool_call(
    client: &reqwest::Client,
    forward: &ForwardConfig,
    tool_name: &str,
    args: serde_json::Value,
) -> Result<GadgetResult, GadgetError> {
    let url = format!(
        "{}/v1/tools/{}/invoke",
        forward.base_url.trim_end_matches('/'),
        tool_name
    );
    let resp = client
        .post(&url)
        .bearer_auth(&forward.auth_token)
        .json(&args)
        .send()
        .await
        .map_err(|e| GadgetError::Execution(format!("forward POST {url}: {e}")))?;
    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| GadgetError::Execution(format!("forward decode {url}: {e}")))?;
    if !status.is_success() {
        let msg = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| body.to_string());
        // Map known error codes back to `GadgetError` variants so the
        // grandchild's `gadget_error_as_tool_result` mapper produces
        // the same `denied: ...` text Penny already knows how to render.
        let code = body
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        return Err(match code {
            "mcp_denied_by_policy" => GadgetError::Denied { reason: msg },
            "mcp_unknown_tool" => GadgetError::UnknownGadget(tool_name.to_string()),
            _ => {
                GadgetError::Execution(format!("forward {url} returned {}: {msg}", status.as_u16()))
            }
        });
    }
    // `/v1/tools/{name}/invoke` uses the Core snake_case shape. Accept the
    // older camelCase spelling as a compatibility fallback for mixed-version
    // parent/child upgrades.
    let content = body
        .get("content")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let is_error = body
        .get("is_error")
        .or_else(|| body.get("isError"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(GadgetResult { content, is_error })
}

/// `GadgetModeReconfigurer` impl so the `PATCH /workbench/agent/modes`
/// handler (living in the gateway crate, which cannot depend on penny)
/// can rebuild the operator-config sets through a trait object instead
/// of a concrete `Arc<GadgetRegistry>`.
impl gadgetron_core::agent::tools::GadgetModeReconfigurer for GadgetRegistry {
    fn reconfigure(&self, cfg: &gadgetron_core::agent::AgentConfig) {
        GadgetRegistry::reconfigure(self, cfg);
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

    async fn dispatch_gadget_with_context(
        &self,
        context: GadgetDispatchContext,
        name: &str,
        args: serde_json::Value,
    ) -> Result<GadgetResult, GadgetError> {
        self.dispatch_with_context(context, name, args).await
    }
}

/// Schema-discovery seam consumed by the gateway's MCP `/v1/tools`
/// endpoint. Returns an owned `Vec` so the caller is insulated from
/// the registry's internal `Arc<[GadgetSchema]>` storage — the cost is
/// one clone per listing (O(schemas), acceptable for a discovery
/// endpoint that runs at single-digit QPS).
impl gadgetron_core::agent::tools::GadgetCatalog for GadgetRegistry {
    fn all_schemas(&self) -> Vec<GadgetSchema> {
        self.all_schemas()
    }

    fn policy_metadata(&self, name: &str) -> Option<gadgetron_core::policy::GadgetPolicyMetadata> {
        if let Some(schema) = self
            .all_schemas
            .iter()
            .chain(self.forwarded_schemas.values())
            .find(|schema| schema.name == name)
        {
            return Some(gadgetron_core::policy::GadgetPolicyMetadata::from_schema(
                schema,
            ));
        }
        self.dynamic_surface()?.policy_metadata(name)
    }
}

/// Determine whether a tool should appear in `--allowed-tools`.
///
/// `Ask` mode is now operator-allowed when the registry carries an
/// `ApprovalStore` — dispatch routes the call through approval first.
/// Without a store the legacy "Ask collapses to Never" behavior holds
/// so old wirings stay safe.
fn tool_is_enabled(schema: &GadgetSchema, cfg: &AgentConfig, has_approval_store: bool) -> bool {
    tool_is_enabled_for_gadgets(schema, &cfg.gadgets, has_approval_store)
}

fn tool_is_enabled_for_gadgets(
    schema: &GadgetSchema,
    gadgets: &GadgetsConfig,
    has_approval_store: bool,
) -> bool {
    match schema.tier {
        GadgetTier::Read => true,
        GadgetTier::Write => {
            let mode = resolve_write_mode_for_gadgets(&schema.name, gadgets);
            match mode {
                GadgetMode::Auto => true,
                GadgetMode::Ask => has_approval_store,
                GadgetMode::Never => false,
            }
        }
        GadgetTier::Destructive => gadgets.destructive.enabled,
    }
}

/// Map a `GadgetTier::Write` tool name to the config subcategory that
/// controls its mode.
fn resolve_write_mode(name: &str, cfg: &AgentConfig) -> GadgetMode {
    resolve_write_mode_for_gadgets(name, &cfg.gadgets)
}

fn resolve_write_mode_for_gadgets(name: &str, gadgets: &GadgetsConfig) -> GadgetMode {
    let w = &gadgets.write;
    let prefix = name.split('.').next().unwrap_or("");
    match prefix {
        "wiki" => w.wiki_write,
        "infra" => w.infra_write,
        "scheduler" => w.scheduler_write,
        "provider" => w.provider_mutate,
        _ => w.namespace_mode(prefix).unwrap_or(w.default_mode),
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

    struct TestDynamicSurface {
        seen_context: std::sync::Mutex<Option<GadgetDispatchContext>>,
    }

    struct TestContextProvider {
        seen_context: std::sync::Mutex<Option<GadgetDispatchContext>>,
    }

    struct PanicApprovalStore;

    #[async_trait]
    impl gadgetron_core::workbench::ApprovalStore for PanicApprovalStore {
        async fn create(
            &self,
            _request: gadgetron_core::workbench::ApprovalRequest,
        ) -> Result<
            gadgetron_core::workbench::ApprovalRequest,
            gadgetron_core::workbench::ApprovalError,
        > {
            panic!("an already-approved request must not create a second approval")
        }

        async fn get(
            &self,
            _id: Uuid,
        ) -> Result<
            gadgetron_core::workbench::ApprovalRequest,
            gadgetron_core::workbench::ApprovalError,
        > {
            panic!("an already-approved request must not poll a second approval")
        }

        async fn mark_approved(
            &self,
            _id: Uuid,
            _approver: &AuthenticatedContext,
        ) -> Result<
            gadgetron_core::workbench::ApprovalRequest,
            gadgetron_core::workbench::ApprovalError,
        > {
            panic!("an already-approved request must not resolve a second approval")
        }

        async fn mark_denied(
            &self,
            _id: Uuid,
            _approver: &AuthenticatedContext,
            _reason: Option<String>,
        ) -> Result<
            gadgetron_core::workbench::ApprovalRequest,
            gadgetron_core::workbench::ApprovalError,
        > {
            panic!("an already-approved request must not resolve a second approval")
        }

        async fn list_pending(
            &self,
            _tenant_id: Uuid,
        ) -> Result<
            Vec<gadgetron_core::workbench::ApprovalRequest>,
            gadgetron_core::workbench::ApprovalError,
        > {
            panic!("an already-approved request must not list a second approval")
        }
    }

    impl gadgetron_core::agent::tools::GadgetCatalog for TestDynamicSurface {
        fn all_schemas(&self) -> Vec<GadgetSchema> {
            vec![GadgetSchema {
                name: "external.inspect".into(),
                tier: GadgetTier::Read,
                description: "inspect an enabled external Bundle".into(),
                input_schema: json!({"type": "object"}),
                idempotent: Some(true),
            }]
        }
    }

    #[async_trait]
    impl gadgetron_core::agent::tools::GadgetDispatcher for TestDynamicSurface {
        async fn dispatch_gadget(
            &self,
            name: &str,
            _args: serde_json::Value,
        ) -> Result<GadgetResult, GadgetError> {
            Err(GadgetError::Denied {
                reason: format!("{name} requires context"),
            })
        }

        async fn dispatch_gadget_with_context(
            &self,
            context: GadgetDispatchContext,
            _name: &str,
            args: serde_json::Value,
        ) -> Result<GadgetResult, GadgetError> {
            *self.seen_context.lock().unwrap() = Some(context);
            Ok(GadgetResult {
                content: args,
                is_error: false,
            })
        }
    }

    #[async_trait]
    impl GadgetProvider for TestContextProvider {
        fn category(&self) -> &'static str {
            "knowledge"
        }

        fn gadget_schemas(&self) -> Vec<GadgetSchema> {
            vec![GadgetSchema {
                name: "wiki.search".to_string(),
                tier: GadgetTier::Read,
                description: "search contextual knowledge".to_string(),
                input_schema: json!({"type": "object"}),
                idempotent: Some(true),
            }]
        }

        async fn call(
            &self,
            _name: &str,
            _args: serde_json::Value,
        ) -> Result<GadgetResult, GadgetError> {
            Err(GadgetError::Denied {
                reason: "context required".to_string(),
            })
        }

        async fn call_with_context(
            &self,
            context: &GadgetDispatchContext,
            _name: &str,
            args: serde_json::Value,
        ) -> Result<GadgetResult, GadgetError> {
            *self.seen_context.lock().unwrap() = Some(context.clone());
            Ok(GadgetResult {
                content: args,
                is_error: false,
            })
        }
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

    #[tokio::test]
    async fn dynamic_surface_updates_discovery_and_preserves_dispatch_context() {
        let registry = GadgetRegistryBuilder::new().freeze(&default_cfg());
        let surface = Arc::new(TestDynamicSurface {
            seen_context: std::sync::Mutex::new(None),
        });
        registry.attach_dynamic_surface(surface.clone());

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.all_schemas()[0].name, "external.inspect");
        assert_eq!(
            registry.build_allowed_tools(&default_cfg()),
            vec!["external.inspect"]
        );
        assert!(registry
            .dispatch("external.inspect", json!({}))
            .await
            .is_err());

        let context = GadgetDispatchContext::new("tenant-42", "actor-7", "request-9")
            .with_scopes(["management".to_string()]);
        let result = registry
            .dispatch_with_context(
                context.clone(),
                "external.inspect",
                json!({"subject": "live"}),
            )
            .await
            .unwrap();
        assert_eq!(result.content, json!({"subject": "live"}));
        assert_eq!(*surface.seen_context.lock().unwrap(), Some(context));
        assert!(registry
            .tool_metadata_snapshot()
            .contains_key("external.inspect"));
    }

    #[tokio::test]
    async fn static_provider_receives_authenticated_dispatch_context() {
        let provider = Arc::new(TestContextProvider {
            seen_context: std::sync::Mutex::new(None),
        });
        let mut builder = GadgetRegistryBuilder::new();
        builder.register(provider.clone()).unwrap();
        let registry = builder.freeze(&default_cfg());
        let context = GadgetDispatchContext::new("tenant-42", "actor-7", "request-9");

        let result = registry
            .dispatch_with_context(context.clone(), "wiki.search", json!({"query": "lesson"}))
            .await
            .unwrap();

        assert_eq!(result.content, json!({"query": "lesson"}));
        assert_eq!(*provider.seen_context.lock().unwrap(), Some(context));
    }

    #[tokio::test]
    async fn approved_workbench_context_bypasses_only_the_duplicate_ask_gate() {
        let mut builder = GadgetRegistryBuilder::new();
        builder
            .register(Arc::new(
                TestProvider::new("knowledge").with_tool("wiki.write", GadgetTier::Write),
            ))
            .unwrap();
        let cfg = cfg_with_overrides(GadgetMode::Auto, GadgetMode::Ask, GadgetMode::Auto, false);
        let registry = builder.freeze_with_approvals(&cfg, Some(Arc::new(PanicApprovalStore)));
        let context = GadgetDispatchContext::new("tenant-42", "actor-7", "approval-9")
            .with_approval_granted();
        let result = registry
            .dispatch_with_context(context, "wiki.write", json!({"content": "approved"}))
            .await
            .unwrap();
        assert_eq!(result.content, json!({"ok": true}));
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

    // ---- L3 defense-in-depth ----

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
        // direct `gadgetron gadget serve` stdio consumer) could reach a
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
        // Ask === Never. The L2 build_allowed_tools filter already
        // excludes Ask; the L3 gate must match so the two sources of
        // truth can never drift.
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

    #[tokio::test]
    async fn common_policy_authorization_bypasses_legacy_mode_without_bypassing_registration() {
        let mut builder = GadgetRegistryBuilder::new();
        builder
            .register(Arc::new(
                TestProvider::new("knowledge").with_tool("wiki.write", GadgetTier::Write),
            ))
            .unwrap();
        let cfg = cfg_with_overrides(
            GadgetMode::Never,
            GadgetMode::Never,
            GadgetMode::Never,
            false,
        );
        let registry = builder.freeze(&cfg);
        let context = GadgetDispatchContext::new("tenant-42", "actor-7", "request-9")
            .with_policy_authorized();
        let result = registry
            .dispatch_with_context(context.clone(), "wiki.write", json!({}))
            .await
            .unwrap();
        assert_eq!(result.content, json!({"ok": true}));
        assert!(matches!(
            registry
                .dispatch_with_context(context, "ghost.write", json!({}))
                .await,
            Err(GadgetError::UnknownGadget(_))
        ));
    }

    #[tokio::test]
    async fn parent_callback_receives_read_tools_even_when_a_local_provider_exists() {
        use axum::{http::HeaderMap, routing::post, Json, Router};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/v1/tools/wiki.read/invoke",
            post(
                |headers: HeaderMap, Json(args): Json<serde_json::Value>| async move {
                    assert_eq!(
                        headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok()),
                        Some("Bearer child-token")
                    );
                    Json(serde_json::json!({
                        "content": {"source": "parent", "args": args},
                        "is_error": false
                    }))
                },
            ),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut builder = GadgetRegistryBuilder::new();
        builder
            .register(Arc::new(
                TestProvider::new("knowledge").with_tool("wiki.read", GadgetTier::Read),
            ))
            .unwrap();
        let registry = builder.freeze_with_forwarding(
            &default_cfg(),
            ForwardConfig {
                base_url: format!("http://{address}"),
                auth_token: "child-token".into(),
            },
        );
        let result = registry
            .dispatch("wiki.read", json!({"name": "home"}))
            .await
            .unwrap();
        assert_eq!(result.content["source"], "parent");
        assert_eq!(result.content["args"]["name"], "home");
        server.abort();
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
        // Tier + Mode rule: T2 `Write` — `Auto` or `Never` per
        // subcategory. `Ask` is logged as a startup warning and treated
        // as `Never` (no approval flow to resolve it).
        //
        // Any tool whose mode resolves to `Ask` MUST NOT appear in
        // `--allowed-tools`, otherwise Claude Code sees it as an
        // auto-runnable tool and invokes it without the approval card
        // that the current build does not implement.
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
    fn reconfigure_updates_current_gadget_modes_snapshot() {
        let mut cfg = AgentConfig::default();
        cfg.gadgets
            .write
            .namespace_modes
            .insert("example-domain".into(), GadgetMode::Ask);
        let reg = registry_with_full_set_cfg(&cfg);

        let mut next = cfg.clone();
        next.gadgets
            .write
            .namespace_modes
            .insert("example-domain".into(), GadgetMode::Auto);
        reg.reconfigure(&next);

        assert_eq!(
            reg.current_gadgets_config()
                .write
                .namespace_mode("example-domain"),
            Some(GadgetMode::Auto)
        );
    }

    #[test]
    fn resolve_write_mode_by_prefix() {
        let mut cfg = AgentConfig::default();
        cfg.gadgets.write.default_mode = GadgetMode::Ask;
        cfg.gadgets.write.wiki_write = GadgetMode::Auto;
        cfg.gadgets.write.infra_write = GadgetMode::Never;
        cfg.gadgets.write.scheduler_write = GadgetMode::Auto;
        cfg.gadgets.write.provider_mutate = GadgetMode::Never;
        cfg.gadgets
            .write
            .namespace_modes
            .insert("example-domain".into(), GadgetMode::Auto);

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
        assert!(matches!(
            resolve_write_mode("example-domain.mutate", &cfg),
            GadgetMode::Auto
        ));
        // Unrecognized prefix uses default_mode.
        assert!(matches!(
            resolve_write_mode("weird.tool", &cfg),
            GadgetMode::Ask
        ));
    }
}
