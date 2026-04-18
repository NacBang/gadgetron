# ADR-P2A-10-ADDENDUM-01 — Bundle / Plug / Gadget RBAC granularity

| Field | Value |
|---|---|
| **Status** | ACCEPTED — rev5 (W3 kickoff 5-agent synthesis + knowledge-layer priority reframing 2026-04-18) |
| **Date** | 2026-04-18 (v1 / rev2 / rev3 / rev4 / rev5) |
| **Author** | security-compliance-lead (v1), PM-directed team synthesis (rev2-rev4), PM integration of W3 kickoff 5-agent review + user knowledge-first directive (rev5) |
| **Parent** | ADR-P2A-10 (bundle-plug-gadget-terminology) |
| **Amends** | ADR-P2A-10 §Decision — adds per-Plug enablement axis, `requires_plugs` cascade, external Gadget runtime RBAC, `bundle info` CLI shape, `GadgetronBundlesHome` resolver |
| **Blocks** | `docs/design/phase2/12-external-gadget-runtime.md` (pending), `bundle.toml` schema v1 freeze, `gadgetron bundle info` CLI output, `gadgetron-core::bundle` trait scaffold |
| **Synthesis round** | 6 agents 2026-04-18 (security, chief-architect, xaas, devops, dx, qa) + codex-chief-advisor external validator. Convergence matrix in §Team synthesis log. |

---

## Revision history

- **v1** (2026-04-18, security-compliance-lead) — initial addendum: 3-axis RBAC, 5 external-runtime enforcement points, 3 open questions for PM.
- **rev2** (2026-04-18, team synthesis) — PM questions resolved, 4 new blockers surfaced by cross-team review incorporated, `requires_plugs` promoted from deferred to P2B-alpha ship, `GadgetronBundlesHome` resolver added, `tenant_overrides` reserved schema, `admin_detail` leak-safety, `PlugId` newtype / `#[must_use] RegistrationOutcome` Rust shape.
- **rev3** (2026-04-18, round-2 team feedback integration) — xaas: `tenant_workdir_quota.last_synced_at` column + FD-open check step 3b in cascade + P2B admin annotation on `tenant_overrides`. codex-chief-advisor: `requires_plugs` completeness lint, enforcement floors 6 (Resource ceilings) + 7 (Egress policy), `admin_detail` choke-point named + extended to `Denied`/`Execution` variants + regression test, `tenant_overrides` upgraded to `warn!` + CFG-045 operator ack gate, `GadgetronBundlesHome` tier-resolution logging, timing-oracle threat noted in STRIDE.
- **rev4** (2026-04-18, W2 kickoff team synthesis) — 4-agent review of §4 trait shape prior to W2 implementation. codex-chief-advisor 3 MAJORs integrated: (1) `BundleRegistry` is **metadata-only**, live `dyn Bundle` values are dropped after `install` returns — never stored as `Vec<Arc<dyn Bundle>>`; (2) `RegistrationOutcome::SkippedByAvailability` carries `missing: Vec<PlugId>` for operator debugging; (3) field-form access (`ctx.plugs.*` / `ctx.gadgets.*`) standardized across ADR + glossary, method-form (`gadgets_mut()`) no longer used. security-compliance-lead 6 W2 deliverables added as trait-freeze conditions: panic isolation (`catch_unwind`), duplicate-id rejection, `register()` log field redaction, `let _` audit-completeness guarantee, `CoreAuditEvent::PlugSkippedByConfig` structured variant (Gate 1 MUST-LAND schema freeze preview), Bundle trait rustdoc trust constraints (P2B in-tree only, audit target operator-only, in-core full-trust).
- **rev5** (2026-04-18, W3 kickoff team synthesis + **knowledge-layer priority reframing**) — 5-agent review (chief-architect / devops / qa / xaas / codex) for W3, with user directive overriding scope: "지식 레이어를 빠르게 만들어 테스트하면서 다른 기능을 구현". Codex 5 MAJORs + chief-architect `#[non_exhaustive]` locks integrated. Most significantly, **W3 is not a single PR and not purely Bundle plumbing** — it is split into a DAG and re-sequenced so knowledge-layer end-to-end testability lands first. See §W3 scope (rewritten) + §Gadget trait decision + §install_all signature policy + §check-bundles tolerance policy.

---

## Context

ADR-P2A-10 established that a Bundle is the operator's install/enable/disable target and that a Bundle may provide zero-or-more Plugs (core-facing Rust trait impls) and zero-or-more Gadgets (Penny-facing MCP tools). Codex raised BLOCK 3 against that framing on 2026-04-18:

> A Bundle can contain a harmless Plug and a destructive Gadget, which means enablement and permissions cannot safely live at Bundle granularity. Subprocess/HTTP Gadget runtimes also leave unanswered whose credentials, workdir, filesystem view, tenant identity, and audit principal execute the action; the rename makes that coupling easier to ignore.

The `ai-infra` Bundle is the concrete counterexample: it provides both `LlmProvider` Plugs (infrastructure wiring no operator would toggle piecemeal) **and** the `model.load` Gadget, which is `Destructive`-tier and can pin a GPU for hours. "Install ai-infra" today is one RBAC decision covering two unrelated trust decisions.

ADR-P2A-10 §Config already provides a per-Gadget tier/mode override table, so the Gadget axis has a per-item control. The Plug axis has no equivalent. Codex sentence 2 is a separate class: subprocess / HTTP / wasm Gadget runtimes do not inherit in-process safety invariants automatically.

---

## Decision

### §1. Three-axis RBAC model

Coarsest to finest:

1. **Bundle enablement** (scope-aware) — `gadgetron bundle enable <name> --scope <global|tenant|user>`. When a Bundle is disabled, no Plugs/Gadgets register. Unchanged from ADR-P2A-10.

2. **Per-Gadget mode override** (already in ADR-P2A-10 §Config) — Bundle enabled, specific Gadget muted/demoted:

   ```toml
   [bundles.ai-infra.gadgets]
   "model.load" = { mode = "never" }
   "gpu.list"   = { mode = "auto", tier = "read" }
   ```

3. **Per-Plug enable/disable** (NEW) — Bundle enabled, specific Plug not registered with the router:

   ```toml
   [bundles.ai-infra.plugs]
   "anthropic-llm" = { enabled = false }    # bundle loads; AnthropicLlmProvider not in router
   "openai-llm"    = { enabled = true }     # explicit (same as default)
   ```

   Default on Bundle enable: every Plug registers (opt-out). No `"ask"` / `"auto"` state on Plug axis — Plugs are hot-path infrastructure, no user-visible prompt surface.

### §2. Scope of Plug enablement — per-deployment v1, `tenant_overrides` schema reserved

**Decision** (team synthesis: security + chief-architect + devops + dx + qa majority, xaas minority flagged):

- **P2B-alpha / P2B-beta**: per-deployment enforcement. Operator has one `gadgetron.toml` per deployment; Helm/CI manages per-environment via separate `values-staging.yaml` / `values-prod.yaml` or env injection. Plug selection happens at daemon-init and is immutable over request lifetime → no router lock contention, no per-request authorization axis, no P99 regression.
- **P2B schema reserved field**: `bundle.toml` v1 and `gadgetron.toml` parser **accept** `[bundles.<name>.plugs.<plug>.tenant_overrides]` keys, store them in `AppConfig`, but **do not act on them**. Parser emits a `tracing::warn!(target: "gadgetron_config", "tenant_overrides reserved — enforcement deferred to P2C per ADR-P2A-10-ADDENDUM-01 §2")` — upgraded from `info!` per codex round-2 — and **requires explicit operator acknowledgement** via `[features] tenant_plug_overrides_accepted_as_reserved = true`. Startup rejects with `CFG-045 — tenant_overrides stanza present but not acknowledged as P2B-reserved` if the stanza is set without the feature toggle. Rationale: a silent no-op on data-residency configuration can masquerade as a compliance breach ("tenant A thought anthropic-llm was disabled, it wasn't"). The operator must acknowledge that the P2B daemon does not enforce the stanza.
- **`gadgetron bundle info <name>` annotation for reserved overrides**: admin-visible output of the `TENANT OVERRIDES` table (§6) explicitly suffixes each row with `(reserved — not enforced until P2C)` when running on a P2B release. A tenant admin cannot be surprised by "I configured it but nothing happened".
- **P2C**: router resolves the active tenant's override table at request time using `AuthenticatedContext.tenant_id` per `08-identity-and-users.md`. Schema:

   ```toml
   [bundles.ai-infra.plugs."anthropic-llm"]
   enabled = true                           # deployment-wide default (rev2 §2 active)

   [bundles.ai-infra.plugs."anthropic-llm".tenant_overrides]  # reserved in P2B, enforced in P2C
   "tenant-a" = { enabled = false }         # data residency: OpenAI-only
   "tenant-b" = { enabled = true }
   ```

**xaas minority position preserved**: the multi-tenant data-residency scenario (Tenant A OpenAI-only vs Tenant B Anthropic-only on shared daemon) is real and needs this in P2C. The deferred schema avoids a rebuild-the-plane-in-flight migration later.

### §3. `requires_plugs` cascade — ships in P2B-alpha

**Decision** (team synthesis: devops + chief-architect + qa majority, security + dx flagged "could defer but OK"):

The previously-deferred `requires_plugs` field **ships in P2B-alpha**. devops' SRE argument won: without explicit cascade, operators see "Gadget missing from Penny's toolbox, no log trail" when a dependency is disabled — the exact class of "archaeological dig through tracing" ticket this mechanism prevents. Cost is ~1 engineer-day.

**Schema** (in `bundle.toml`):

```toml
[bundle.gadgets."model.load"]
requires_plugs = ["anthropic-llm"]    # if the plug is skipped, the gadget is skipped too
```

**Rust type** (chief-architect):

```rust
pub struct BundleManifest {
    pub name:           String,
    pub version:        semver::Version,
    pub plugs:          Vec<PlugId>,
    pub gadgets:        Vec<GadgetManifestEntry>,
    pub requires_plugs: HashMap<GadgetName, Vec<PlugId>>,   // per-Gadget, not flat
}
```

**Enforcement** — check at registration, not per-invocation. A Gadget whose required Plug is disabled is not registered; `GadgetRegistry::list()` does not include it. Registration-time `tracing::warn!` emits the cascade reason:

```
warn[BUNDLE-031]: gadget "model.load" skipped — required plug "anthropic-llm" is not registered
  Bundle "ai-infra" declares: gadgets."model.load".requires_plugs = ["anthropic-llm"]
  The plug "anthropic-llm" was disabled by config or its bundle is not enabled.
  "model.load" will not appear in Penny's toolbox for this session.
  To restore: re-enable the "anthropic-llm" plug in [bundles.ai-infra.plugs] or
  remove the requires_plugs constraint from bundle.toml if the dependency is stale.
```

**Bundle-author responsibility** — explicit cascade is a developer-experience improvement. Runtime fail-closed (a registered Gadget that internally calls a missing Plug) remains the safety net: `GadgetError::Execution { reason: "required_plug_not_registered: <plug_id>" }` + audit event. Both paths coexist; `requires_plugs` catches the issue at startup, fail-closed catches runtime surprises.

**`requires_plugs` completeness lint — ships in P2B-alpha** (codex round-2 MAJOR). The cascade is a convention unless enforced. A `cargo xtask check-bundles` tool (gated in CI) statically analyzes every Bundle crate:

- Scans every `ctx.plugs.<port>.get(<plug_id>)` and `ctx.require_plugs(&[...])` callsite inside the Bundle source.
- Cross-references the set of called `PlugId`s against the union of `bundle.toml` `requires_plugs` maps for every Gadget in that Bundle.
- Fails CI if any Plug called by Bundle code is not declared in at least one Gadget's `requires_plugs` (or the Bundle itself declares the Plug as non-optional via the top-level `plugs` list).

This catches the "author wrote `requires_plugs = []` but the Gadget actually calls a Plug" class that would otherwise silently fall through to runtime fail-closed. The tool runs in the same CI stage as `cargo clippy` and is a release gate. Shipping this in P2B-alpha — not deferred — because without it `requires_plugs` is a politeness layer that can silently lie.

### §4. Enforcement point + Rust shape

Single enforcement point in `Bundle::install(&mut BundleContext)`. `BundleContext` reads the per-Bundle config at construction and exposes typed predicates. chief-architect's four mandatory changes from round 1 synthesis are incorporated:

**A. `PlugId` newtype** (not `&str` everywhere):

```rust
// gadgetron-core::bundle::id
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlugId(pub(crate) Arc<str>);   // Arc<str> → per-call clone is free

impl PlugId {
    pub fn new(s: impl Into<Arc<str>>) -> Result<Self, PlugIdError> {
        // kebab-case validation, length bounds, no "::" etc.
    }
    pub fn as_str(&self) -> &str { &self.0 }
}

pub type GadgetName = Arc<str>;   // same pattern, separate type for misuse resistance
```

**B. `BundleContext` API surface**:

```rust
pub struct BundleContext<'a> {
    pub config:      &'a AppConfig,
    pub bundle:      &'a BundleDescriptor,          // name, version, manifest_version
    pub plugs:       PlugHandles<'a>,
    pub gadgets:     GadgetHandles<'a>,
    pub seed_pages:  SeedPageBuffer<'a>,
}

impl BundleContext<'_> {
    pub fn bundle_name(&self) -> &str            { &self.bundle.name }
    pub fn is_plug_enabled(&self, p: &PlugId) -> bool { /* cached BTreeMap */ }
    pub fn require_plugs(&self, ps: &[PlugId]) -> Result<(), MissingPlugs> { ... }
    pub fn is_plug_enabled_by_name(&self, s: &str) -> bool { /* parse+lookup */ }
}
```

**C. `#[must_use] RegistrationOutcome`** (not `Result` — skip is policy, not error):

```rust
#[must_use = "ignoring a RegistrationOutcome hides whether the plug was actually wired"]
pub enum RegistrationOutcome {
    Registered,
    SkippedByConfig,                                 // [bundles.<>.plugs.<>] enabled = false
    SkippedByAvailability { missing: Vec<PlugId> },  // rev4: carries missing IDs per codex MAJOR 2
}

impl<T: ?Sized> PlugRegistry<'_, T> {
    pub fn register(&mut self, id: PlugId, plug: Arc<T>) -> RegistrationOutcome {
        if !self.ctx.is_plug_enabled(&id) {
            tracing::info!(
                target: "gadgetron_audit",
                bundle = self.ctx.bundle_name(),
                plug = id.as_str(),
                axis = "llm_provider",
                "plug_skipped_by_config"
            );
            return RegistrationOutcome::SkippedByConfig;
        }
        self.inner.insert(id, plug);
        RegistrationOutcome::Registered
    }
}
```

`#[must_use]` gives Bundle authors compile-time nudge without `?` ceremony. Discard with `let _ = ...` when intentional.

**D. Core-internal `pub(crate)` discipline**: the registry's inner map is `pub(crate) within gadgetron-core::bundle`. Bundle crates consume `BundleContext` (a core type) and never reach into `config::*` or `registry.inner` directly. Coupling direction stays D-12-compliant: core→core for mechanism, Bundle→core for call site.

**E. `BundleRegistry` is metadata-only** (rev4, codex MAJOR 1): `BundleRegistry` stores **`BundleDescriptor` + registered Plug/Gadget inventory + install status only**. Live `dyn Bundle` values are **not** stored — they are constructed once at startup, `install()` is called sequentially, and the `Box<dyn Bundle>` is dropped after `install` returns. Do **not** use `Vec<Arc<dyn Bundle>>` — that makes the `&mut self` mutability contract incoherent (Arc::get_mut only works at refcount-1) and creates the first-reinstall-forces-supersedes failure mode. Any state that must outlive `install` lives in the registered services (the `Arc<T>` stored in each `PlugRegistry<T>`), not in the Bundle object itself.

**F. `Bundle::install` panic isolation** (rev4, security-compliance-lead W2 deliverable #1): `BundleRegistry::install_all(&self, bundles: Vec<Box<dyn Bundle>>)` invokes each `bundle.install(&mut ctx)` inside `std::panic::catch_unwind(AssertUnwindSafe(...))`. A panicking Bundle is recorded as `PlugStatusKind::RegistrationFailed { reason: "panic: <msg>" }` in `BundleRegistry`; other Bundles continue to install. A panicking Bundle does NOT terminate the daemon — that would be a DoS trivially triggered by any mis-built Bundle.

**G. Duplicate-id rejection** (rev4, security-compliance-lead W2 deliverable #2): `BundleRegistry::install_all` checks `BundleDescriptor.name` uniqueness before invoking `install`. A duplicate name returns `BundleError::Install("bundle already installed: {id}")`. Re-installing the same Bundle would shadow/overwrite previous Plug registrations and break the audit trail — explicit rejection is correct.

**H. `register()` log-field redaction** (rev4, security-compliance-lead W2 deliverable #3): the `tracing::info!` emitted inside `PlugRegistry::register` MUST carry only `bundle`, `plug` (id string), and `axis`. The `plug: Arc<T>` argument itself MUST NEVER be logged via `Debug`, `{:?}`, or field binding. Rationale: `T` may embed secrets (provider API keys). Regression test `register_log_contains_only_id_bundle_axis` asserts the JSON field whitelist.

### §5. External Gadget runtime RBAC — five enforcement points + runtime metadata

Codex BLOCK 3 sentence 2 is answered at spec level; wire protocol deferred to `docs/design/phase2/12-external-gadget-runtime.md`. The floor contractual points below **must not** be relaxed by doc 12.

For any Gadget whose `[bundle.runtime].kind` is `subprocess`, `http`, `container`, or `wasm`:

1. **Credentials** — runtime receives caller's `AuthenticatedContext` via `PluginInvocation` per doc 10 D5-a (strict inheritance). Never daemon credentials. Payload carries `caller_user_id`, `tenant_id`, `role`, `teams`, `request_id`, plus caller-scoped provider API key. Out-of-band runtime credentials (graphify's Neo4j connection, etc.) come from operator's per-tenant secret store.
2. **Workdir** — tenant-scoped at `GadgetronBundlesHome::tenant_workdir(tenant_id, bundle_name)` (see §7 for resolver). Subprocess `current_dir`; container/wasm single mount. **Canonicalize check** before `Command::current_dir(workdir)`: resolved path must still sit under the expected tenant root; symlink escape → `GadgetronError::Penny::GadgetIntegrity` + abort.
3. **Filesystem view** — no direct FS access outside workdir. Wiki reads via `WikiReadView` in `PluginInvocation` (caller-scoped per 08/09/10). Blob reads via `BlobReadView`. Write-class views gated on Gadget tier — a `Read`-tier external Gadget receives read views only. `TMPDIR=$GADGETRON_WORKDIR/.tmp` set in spawn env (not host `/tmp`).
4. **Tenant identity** — `tenant_id` rides with `AuthenticatedContext` and is not spoofable. Subprocess/container: `GADGETRON_TENANT_ID` env set by core + invocation payload re-asserts; mismatch → abort. HTTP: `tenant_id` in loopback-token-signed envelope.
5. **Audit principal** — every external Gadget call emits `GadgetCallCompleted` with `owner_id`, `tenant_id`, `runtime_kind`, `runtime_bundle`, `runtime_version`, `conversation_id`, `claude_session_uuid`. Audit principal is **always** the caller; runtime is labeled metadata, never the subject.
6. **Resource ceilings** (codex round-2 MAJOR) — spawn fails closed if no ceiling is declared. Linux: `RLIMIT_AS` (virtual memory), `RLIMIT_NOFILE` (open FDs), `RLIMIT_CPU` (seconds), plus a cgroup v2 `memory.max` under `gadgetron.slice/tenants/<tenant>/bundles/<bundle>.scope`. macOS dev: `ulimit -v/-n/-t` equivalents, best-effort. `bundle.toml` declares per-runtime limits (`[bundle.runtime.limits] memory_mb = 2048`, `open_files = 256`, `cpu_seconds = 300`) — operator `gadgetron.toml` can tighten via `[bundles.<name>.runtime.limits]` overrides. Missing declaration at both levels → `GadgetronError::Config("bundle requires runtime.limits for external runtime")`, daemon refuses to start the Bundle. Rationale: a graphify or Whisper subprocess can fork-bomb or OOM the host without this. The §5 five-point floor was identity+FS; ceiling 6 is resource-exhaustion DoS defense.
7. **Egress policy** (codex round-2 MAJOR) — external runtime network access is default-deny. `bundle.toml` declares explicit allowlist (`[bundle.runtime.egress] allow = ["api.anthropic.com:443", "api.openai.com:443"]`). Linux enforcement: network namespace per external runtime + nftables rules bound to the allowlist. Container: same, inside the container network. Subprocess without namespace support (macOS dev): app-layer proxy fallback, `HTTPS_PROXY=<gadgetron-egress-proxy>` env injected, proxy enforces allowlist + logs every connection attempt. `bundle.toml` omission → default-deny empty allowlist, runtime has no network. Operator can override in `gadgetron.toml` per-bundle. Every blocked attempt emits `tracing::warn!(target: "gadgetron_audit", ..., "external_runtime_egress_blocked")` + audit row. Rationale: without this, a compromised Bundle exfiltrates tenant data unchecked — the identity enforcement on ceiling 1 names WHO the request is from, but without egress policy the confused-deputy attack is open.

**Runtime metadata persistence** (xaas round 1 gap resolution):

The existing `tool_audit_events` migration `20260416000001` has no columns for `runtime_kind`, `runtime_bundle`, `runtime_version`. wire-frozen `tool_name` column must not be overloaded. Resolution — **additive migration** `20260418000001_external_runtime_meta.sql`:

```sql
-- ADR-P2A-10-ADDENDUM-01 §5 runtime metadata
ALTER TABLE tool_audit_events
  ADD COLUMN external_runtime_meta JSONB NULL;

CREATE INDEX tool_audit_events_external_runtime_kind_idx
  ON tool_audit_events ((external_runtime_meta->>'kind'))
  WHERE external_runtime_meta IS NOT NULL;
```

Populated only when `runtime.kind != InCore`. In-core Gadgets write `NULL`. Rationale (xaas): `category` column overloading (`<category>/<runtime_kind>/...`) corrupts existing BI dashboards that filter on plain category values. JSONB is additive; existing rows still readable; existing queries unchanged.

**Leak-safety — `admin_detail: Option<String>`** (xaas, extended in rev3 per codex round-2 MAJOR):

```rust
pub enum GadgetError {
    UnknownGadget(String),
    Denied { reason: String, admin_detail: Option<String> },      // rev3: extended
    RateLimited { gadget: String, remaining: u32, limit: u32 },
    ApprovalTimeout { secs: u64 },
    InvalidArgs(String),
    Execution { reason: String, admin_detail: Option<String> },   // rev3: extended
    GadgetNotAvailable {
        gadget: String,
        reason: String,                 // shown to non-admin callers ("This tool is not available.")
        admin_detail: Option<String>,   // admin-only — names the disabled Plug
    },
}
```

rev3 extends `admin_detail` to `Denied` and `Execution` per codex finding — any variant whose `reason` could name a Plug / tenant / provider is a potential disclosure channel. A single audit of every variant at rev3 locks the pattern.

**Redaction choke-point** — one function, named and regression-tested. The rendering site is `gadgetron-gateway::error::render_gadget_error_for_caller(err: &GadgetError, ctx: &AuthenticatedContext) -> RenderedError`. It consults `ctx.role` and appends `admin_detail` only for `Role::Admin` / `Role::Owner`. **No other call site** may format a `GadgetError` for caller display; Penny's stream formatter, gateway HTTP response, and audit emitter all go through this function. Regression test `gadget_not_available_hides_admin_detail_from_non_admin` in `crates/gadgetron-gateway/tests/error_redaction.rs` — asserts non-admin render omits `admin_detail`; admin render includes it; every variant of `GadgetError` with an `admin_detail` field is exercised in the matrix. Any PR that introduces a new `admin_detail`-bearing variant without updating this test fails CI.

### §6. `gadgetron bundle info` CLI output

**Team-converged format** (dx proposal, devops' distinction between config-disabled vs runtime-failed adopted, xaas's per-tenant column admin-gated):

```
Bundle: ai-infra  v0.3.1  [enabled]
Source: bundles/ai-infra  (Rust-native, in-process)

PLUGS (4)
  NAME             PORT          STATUS
  openai-llm       LlmProvider   registered
  vllm             LlmProvider   registered
  anthropic-llm    LlmProvider   disabled-by-config (bundles.ai-infra.plugs.anthropic-llm)
  vram-lru-sched   Scheduler     registration-failed [nvml init: GPU not found]

GADGETS (3)
  NAME             TIER         MODE
  gpu.list         read         auto
  model.load       destructive  ask  (requires_plugs: anthropic-llm [unsatisfied])
  scheduler.stats  read         auto

Use `gadgetron plug info <port>` to see which Plugs fill each core port.
Use `gadgetron gadget info <name>` for Gadget schema + invocation details.
```

**Status values** (3 distinct):

- `registered` — in the router, callable
- `disabled-by-config (<toml-key>)` — `enabled = false` in `gadgetron.toml`; names the exact key
- `registration-failed [reason]` — `Bundle::install` attempted registration, inner error; error included inline

**Admin-only extension** (rev2 per xaas, rev3 annotation per xaas round-2 + codex round-2) — when `tenant_overrides` rows exist and caller has `admin` role, an additional `TENANT OVERRIDES` table appears with explicit P2B-reserved annotation:

```
TENANT OVERRIDES  (P2B: parsed but not enforced — enforcement activates in P2C per ADR-P2A-10-ADDENDUM-01 §2)
  PLUG             TENANT         STATUS
  anthropic-llm    tenant-a       disabled (reserved — not enforced until P2C)
  anthropic-llm    tenant-b       enabled (default)
```

The header annotation and per-row `(reserved — not enforced until P2C)` suffix prevent silent misconfiguration — a tenant admin who sets an override in P2B sees exactly why it isn't effective.

**`--json` flag** (qa) — structured form for CI/automation and for regression assertions. Avoids golden-file brittleness on column-width changes.

**`BundleRegistry` query API** (chief-architect):

```rust
pub struct PlugStatus {
    pub id:      PlugId,
    pub port:    &'static str,           // "LlmProvider", "Scheduler", ...
    pub status:  PlugStatusKind,
}

pub enum PlugStatusKind {
    Registered,
    DisabledByConfig { toml_key: String },
    RegistrationFailed { reason: String },
}

impl BundleRegistry {
    pub fn list_plugs(&self, bundle: &BundleId) -> Vec<PlugStatus>;
    pub fn list_gadgets(&self, bundle: &BundleId) -> Vec<GadgetStatus>;
}
```

### §7. `GadgetronBundlesHome` path resolver (devops blocker)

v1's `~/.gadgetron/tenants/<tenant_id>/bundles/<bundle>/workdir/` is unsafe in container/K8s deployments. distroless non-root images may have `$HOME = /home/nonroot` with no persistent mount, or `$HOME` unset → `"/"`, causing silent corruption at root FS. Resolver with priority chain:

```rust
// gadgetron-core::bundle::home (new module)
pub fn resolve_bundles_home(cfg: &AppConfig) -> Result<PathBuf, HomeError> {
    // 1. Explicit config override — recommended for all container deployments
    if let Some(p) = &cfg.bundles.workdir_root {
        return Self::validate_writable(p);
    }
    // 2. Env var — for CI/Helm env: injection without config rebuild
    if let Ok(p) = std::env::var("GADGETRON_BUNDLES_HOME") {
        return Self::validate_writable(&PathBuf::from(p));
    }
    // 3. Data-dir convention
    if let Ok(p) = std::env::var("GADGETRON_DATA_DIR") {
        return Self::validate_writable(&PathBuf::from(p).join(".gadgetron"));
    }
    // 4. Legacy $HOME — only when resolves writable AND not "/"
    let home = dirs::home_dir().ok_or(HomeError::NoHome)?;
    if home.as_os_str() == "/" {
        return Err(HomeError::RootHomeRefused);  // fail-closed
    }
    Self::validate_writable(&home.join(".gadgetron"))
}

pub fn tenant_workdir(tenant_id: &TenantId, bundle_name: &str) -> PathBuf {
    resolve_bundles_home(...).join("tenants").join(tenant_id.to_string()).join("bundles").join(bundle_name).join("workdir")
}
```

**Startup fails (not warn)** if no resolvable writable path. Silent fallback to `/gadgetron/...` is rejected — a tenant writing to host root is a security misconfiguration, not a recoverable edge case.

**Tier-resolution logging** (codex round-2 MINOR) — at startup, after `resolve_bundles_home` succeeds, emit `tracing::info!(target: "gadgetron_config", tier = "config_override|env_GADGETRON_BUNDLES_HOME|env_GADGETRON_DATA_DIR|home_dir", resolved_path = %path.display(), "bundles_home resolved")`. SRE debugging "why does staging write under `/data/...` but prod writes under `~/.gadgetron`" becomes a single log grep.

**Helm values addition** (devops):

```yaml
gadgetron:
  bundlesWorkdirRoot: "/data/gadgetron/bundles"   # maps to config [bundles] workdir_root
```

**Env var translation rule** (devops) — nested Plug config keys:

```
GADGETRON_BUNDLE_<BUNDLE>_PLUG_<PLUG>_ENABLED=false
```

Hyphens in names → `__` (double underscore). Example: `GADGETRON_BUNDLE_AI__INFRA_PLUG_ANTHROPIC__LLM_ENABLED=false`.

### §8. Quota & cleanup for tenant workdir (xaas; rev3 additions per xaas round-2)

Workdir per-tenant quota enforced at OS level (Linux `xfs_quota` / `overlayfs size=`; macOS dev `diskutil quota`). Daemon records usage in `tenant_workdir_quota`. Runtime spawn fails if cap exceeded — **fail closed at spawn time**, not post-execution.

**Quota ledger schema** (rev3 — `last_synced_at` added per xaas round-2, detects stale counters after daemon crash):

```sql
CREATE TABLE tenant_workdir_quota (
    tenant_id      UUID        NOT NULL PRIMARY KEY REFERENCES tenants(id),
    bytes_used     BIGINT      NOT NULL DEFAULT 0 CHECK (bytes_used >= 0),
    bytes_cap      BIGINT      NOT NULL CHECK (bytes_cap > 0),
    last_synced_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

`last_synced_at` enables (a) a "quota sync is stale" alert when any row has not been touched in > 1h, (b) a startup reconciliation shortcut — daemon re-scans only tenants whose timestamp is older than a configurable threshold, avoiding the O(n) re-scan of every tenant on every boot.

**Tenant deletion cascade** (rev3 — step 3b added per xaas round-2, closes open-FD window):

1. Mark tenant `is_active = false` in DB.
2. Enqueue `WorkdirPurgeJob(tenant_id)` background task.
3. Job sends `SIGTERM` to all subprocesses under the tenant's workdir → 5s grace → `SIGKILL`.
3b. **Assert no open FDs under workdir** using `/proc/<daemon_pid>/fd` scan (Linux) or `lsof` equivalent (macOS). If any FD still points inside the workdir, abort purge, emit `ERROR` audit event, and re-enqueue the job with a 30s backoff. Rationale: the daemon itself may hold FDs into the workdir (log tails, mmap'd model weights, open sockets). Deleting with live FDs leaves ghost files on filesystems where unlinked-but-open files are inaccessible from the FS but still consume quota.
4. `std::fs::remove_dir_all(workdir)` → delete `tenant_workdir_quota` row.

Order matters: kill subprocesses, verify no daemon FDs, then delete. This is fail-closed — one step 3b failure re-enqueues; infinite 30s backoff is acceptable because tenant deletion is a rare operation.

Cross-tenant leak threats:
- Symlink escape: canonicalize check (§5).
- Hardlink: mount workdir with `nosuid,nodev,noexec` on Linux; canonicalize check fallback on macOS.
- Temp file collision: `TMPDIR` points inside workdir, not host `/tmp`.

### §9. Observability (devops)

**Prometheus metrics**:

```
# gauge — current plugs registered per bundle
gadgetron_plugs_active{bundle, plug, port, status}    # status: "registered" | "disabled_by_config"

# counter — registration events per daemon startup
gadgetron_plug_registrations_total{bundle, plug, outcome}   # outcome: "registered" | "skipped_by_config" | "skipped_by_availability" | "registration_failed"

# counter — requires_plugs cascade skips
gadgetron_gadget_cascade_skips_total{bundle, gadget, missing_plug}

# counter — external runtime call invocations
gadgetron_external_gadget_calls_total{bundle, gadget, runtime_kind, outcome}

# histogram — external runtime call latency
gadgetron_external_gadget_duration_seconds{bundle, gadget, runtime_kind}
```

**Log levels per event** (`target: "gadgetron_audit"` unless stated):

| Event | Level |
|---|---|
| Plug registered | `DEBUG` |
| Plug skipped by config (`enabled=false`) | `INFO` |
| Plug registration failed | `ERROR` |
| Gadget cascade skip (`requires_plugs` unsatisfied) | `WARN` |
| Unknown Plug name in config | `WARN` (`target: "gadgetron_config"`) |
| `tenant_overrides` detected but P2B reserved | `INFO` (`target: "gadgetron_config"`) |
| External runtime call | `INFO` (per call) + `WARN`/`ERROR` on failure |
| External runtime auth mismatch | `ERROR` (always) + audit row |
| Bundle disabled (all plugs drop) | `INFO` |

**Dashboard row** — "Disabled Plugs by Bundle": `sum(gadgetron_plugs_active == 0) by (bundle)` stacked bar; alert threshold any bundle where `registration-failed` count > 0 at startup.

### §10. CLI error messages (dx)

Operator typos and misconfigurations — three canonical cases:

**CFG-042 — Unknown Plug name**:
```
error[CFG-042]: unknown plug "openai-llmm" in [bundles.ai-infra.plugs]

  --> gadgetron.toml:14:1
  [bundles.ai-infra.plugs.openai-llmm]
  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  The name "openai-llmm" is not registered by any installed Bundle.

  Known plugs in bundle "ai-infra": openai-llm, anthropic-llm, vllm, vram-lru-sched
  Run `gadgetron plug list --bundle ai-infra` for the full list.

  Gadgetron started, but this config line has no effect. Fix the typo
  or remove the stanza to silence this warning.
```

(Warn-not-error per v1 §Migration — upgraded Bundle may drop a Plug; daemon still starts.)

**CFG-043 — Plug exists but in a different Bundle**:
```
error[CFG-043]: plug "openai-llm" is not provided by bundle "graphify"

  --> gadgetron.toml:22:1
  [bundles.graphify.plugs.openai-llm]
  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  "openai-llm" is a valid plug name, but it is registered by bundle "ai-infra",
  not "graphify". Per-plug config must be placed under the Bundle that owns it.

  Move this stanza to [bundles.ai-infra.plugs.openai-llm] to take effect.
  Run `gadgetron bundle info openai-llm --find` to locate a plug by name.
```

**CFG-044 — Bundle disabled, per-Plug override attempted**:
```
error[CFG-044]: per-plug config on disabled bundle "ai-infra" has no effect

  --> gadgetron.toml:14:1
  [bundles.ai-infra.plugs.anthropic-llm]
  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  Bundle "ai-infra" is disabled. When a Bundle is disabled, no Plugs or Gadgets
  register — per-plug enable/disable settings are ignored.

  To apply per-plug config, first enable the bundle:
    gadgetron bundle enable ai-infra
  Or remove the [bundles.ai-infra.plugs.*] stanzas if you intend to keep
  the bundle disabled.
```

### §11. CLI command scope

Per-Plug enable/disable stays **config-file-only for P2B-alpha / P2B-beta**. No runtime `gadgetron plug enable/disable` command that mutates `gadgetron.toml`. Rationale (dx): config-file single source of truth, git-diffable paper trail, no sidecar state, no edit-race with operator's editor. One convenience: `gadgetron plug disable ai-infra anthropic-llm --dry-run` prints the TOML stanza for the operator to paste (no mutation).

Revisit in P2C if operator UX pain becomes evident.

---

## Migration

**Additive. No breaking change.**

- Existing `[bundles.<name>]` sections keep working.
- New `[bundles.<name>.plugs.<plug_name>]` subsection is optional — missing means enabled.
- `AppConfig::load` validates:
  - Unknown Plug name → `tracing::warn!(target: "gadgetron_config", ..., "unknown_plug_reference")` (not hard error). Rationale: Bundle version drift.
  - `tenant_overrides` keys present → `tracing::info!(target: "gadgetron_config", ..., "tenant_overrides reserved — enforcement deferred to P2C")`.
  - Bundle disabled + per-Plug override → CFG-044 hard error (clearly misconfigured).
- **One additive DB migration** — `20260418000001_external_runtime_meta.sql` adds `external_runtime_meta JSONB` column. `tool_audit_events.tool_name` wire-frozen.
- `gadgetron bundle info <name>` output shape gains Plugs table + optional admin-only Tenant Overrides table (§6).

---

## Open questions — resolved in rev2

All three v1 open questions resolved by team synthesis 2026-04-18:

1. ~~Scope of Plug enablement — per-deployment or per-scope?~~ — **resolved §2**: per-deployment for P2B, `tenant_overrides` reserved schema for P2C.
2. ~~`bundle info` column naming?~~ — **resolved §6**: `NAME | PORT | STATUS` 3-col format; STATUS 3 states; admin-only `TENANT OVERRIDES` extension; `--json` flag.
3. ~~`requires_plugs` cascade ship in P2B-alpha or slide to beta?~~ — **resolved §3**: ships in P2B-alpha.

---

## Consequences

- **Codex BLOCK 3 closed.** Per-Plug axis + explicit external runtime enforcement points + `admin_detail` leak-safety. Re-validation by codex-chief-advisor in Team synthesis log.
- **Rust type discipline locked.** `PlugId` newtype + `#[must_use] RegistrationOutcome` + per-Gadget `requires_plugs` HashMap — chief-architect signed off round 2.
- **One additive DB migration** — `external_runtime_meta JSONB`. `tool_audit_events.tool_name` column remains wire-frozen (ADR-P2A-10 §Forward compatibility).
- **Container/K8s safe path resolution** — `GadgetronBundlesHome` resolver, fail-closed on `$HOME == "/"` edge.
- **P2B test coverage** — 13 new tests across `gadgetron-core`, `gadgetron-gateway`, `gadgetron-testing`:
  - `unknown_plug_in_config_emits_warn_not_hard_error`
  - `plug_disabled_by_config_is_not_registered`
  - `is_plug_enabled_returns_correct_tristate`
  - `bundle_disabled_takes_precedence_over_plug_override`
  - `external_gadget_call_propagates_authenticated_context`
  - `bundle_plug_toml_subsection_parses_with_defaults`
  - `requires_plugs_missing_disables_gadget_with_warn`
  - `tenant_overrides_parsed_as_reserved_no_op_p2b` (rev2)
  - `bundles_home_resolver_fail_closed_on_root_home` (rev2)
  - `tenant_overrides_without_ack_toggle_refuses_startup_cfg_045` (NEW rev3, per codex MINOR 5)
  - `requires_plugs_completeness_lint_fails_on_undeclared_plug_call` (NEW rev3, per codex MAJOR 1)
  - `external_runtime_spawn_fails_without_resource_ceilings` (NEW rev3, per codex MAJOR 2)
  - `gadget_not_available_hides_admin_detail_from_non_admin` (NEW rev3, per codex MAJOR 3 — matrix over all admin_detail-bearing variants)
  - PBT: `is_plug_enabled_is_pure_function_of_config` + `authenticated_context_survives_serialization_roundtrip`
  - Harness additions: `FakeBundle`, `FakePlugRegistry`, `FakeTenantContext`, `FakeSubprocessRuntime`, `tracing_test::subscriber` capture helper
  - Effort: ~6 engineer-days (v1: 4, rev2: +1 for `tenant_overrides`/`GadgetronBundlesHome`, rev3: +1 for 4 new tests + `cargo xtask check-bundles` scaffold)
- **Observability surface** — 5 Prometheus metrics + 9 log levels + 1 dashboard panel (§9).
- **`gadgetron plug enable/disable` CLI** — deferred to P2C; `--dry-run` TOML printer added as convenience in P2B-alpha.
- **Doc 12 contract floor** — 5 enforcement points + runtime metadata persistence + workdir canonicalize check are floors, not choices.

---

## Team synthesis log (2026-04-18)

Round 1: 6 agents parallel reviewed v1 addendum and returned positions. Round 2: team convergence meeting — this rev2 document incorporates all round 1 inputs.

### Convergence matrix (rev2)

| Item | security | chief-architect | xaas | devops | dx | qa | codex | Resolution |
|---|---|---|---|---|---|---|---|---|
| Per-Plug enable axis | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Ship — §1 |
| Per-deployment P2B / tenant_overrides P2C | ✅ | ✅ | 🟡¹ | ✅ | ✅ | ✅ | ✅ | Ship — §2 (xaas minority preserved) |
| `tenant_overrides` ack gate + `warn!` + CFG-045 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 🟢 | rev3 ship — §2 |
| `requires_plugs` ship P2B-alpha | 🟡² | ✅ | ✅ | 🟢 | ✅ | ✅ | ✅ | Ship — §3 |
| `requires_plugs` completeness lint | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 🟢 | rev3 ship — §3 |
| `PlugId` newtype + `#[must_use]` | ✅ | 🟢 | ✅ | ✅ | ✅ | ✅ | ✅ | Ship — §4 |
| `admin_detail` leak-safety | 🟢 | ✅ | 🟢 | ✅ | ✅ | ✅ | ✅ | Ship — §5 |
| `admin_detail` extended to `Denied`/`Execution` + choke-point | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 🟢 | rev3 ship — §5 |
| Enforcement floor 6: Resource ceilings | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 🟢 | rev3 ship — §5 |
| Enforcement floor 7: Egress policy | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 🟢 | rev3 ship — §5 |
| JSONB runtime metadata column | ✅ | ✅ | 🟢 | ✅ | ✅ | ✅ | ✅ | Ship — §5 |
| `GadgetronBundlesHome` resolver | ✅ | ✅ | ✅ | 🟢 | ✅ | ✅ | ✅ | Ship — §7 |
| `GadgetronBundlesHome` tier-resolution log | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 🟢 | rev3 ship — §7 |
| `tenant_workdir_quota.last_synced_at` | ✅ | ✅ | 🟢 | ✅ | ✅ | ✅ | ✅ | rev3 ship — §8 |
| Deletion cascade step 3b (FD-open check) | ✅ | ✅ | 🟢 | ✅ | ✅ | ✅ | ✅ | rev3 ship — §8 |
| 3-col `bundle info` + --json | ✅ | ✅ | ✅ | 🟢 | 🟢 | ✅ | ✅ | Ship — §6 |
| Error codes CFG-042/043/044 | ✅ | ✅ | ✅ | ✅ | 🟢 | ✅ | ✅ | Ship — §10 |
| config-file-only CLI scope | ✅ | ✅ | ✅ | ✅ | 🟢 | ✅ | ✅ | Ship — §11 |
| Timing-oracle padding in `render_gadget_error_for_caller` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 🟢 | rev3 ship — STRIDE I |

Legend: 🟢 = originated this proposal. ✅ = approved. 🟡 = approved with minority note preserved.
¹ xaas flagged that deferring `tenant_overrides` enforcement to P2C means real multi-tenant XaaS blocked until P2C — minority position preserved in §2.
² security preferred deferring `requires_plugs` to P2B-beta; concurred with majority since fail-closed remains the safety net.

### STRIDE re-pass (rev2)

security-compliance-lead confirms no severity-HIGH finding introduced by rev2/rev3:
- **S** Spoofing — unchanged; tenant_id not spoofable via §5 enforcement.
- **T** Tampering — `admin_detail` blocks Plug-name disclosure to non-admin (rev3 extended to `Denied`/`Execution`), new migration is additive + audited.
- **R** Repudiation — improved; cascade skip audit event closes additional silent-disable gap.
- **I** Information disclosure — reduced; `admin_detail` redacted through single choke-point (`render_gadget_error_for_caller`) with regression test. **Residual MINOR** (codex round-2 noted): timing oracle on `GadgetNotAvailable` — non-admin attacker iterating Gadget names can distinguish "exists but denied" from "doesn't exist / cascade-skipped" by response-time difference, enabling Gadget-name enumeration. Attacker must already be authenticated + generate many requests. Mitigation: the `render_gadget_error_for_caller` function pads its work with a constant-time `tokio::time::sleep` to equalize response time across variants. Ship the padding in P2B-alpha with a benchmark-based tuning knob. Not a blocker but tracked.
- **D** Denial of service — **improved via rev3 §5 floors 6+7** (resource ceilings, egress policy); intended fail-closed on Plug unavailability; external runtime now has RLIMIT + egress default-deny + quota + cgroup memory cap.
- **E** Elevation — unchanged; external runtime inherits caller AuthenticatedContext, not daemon creds. rev3 §3 `requires_plugs` completeness lint prevents Bundle-author trust failure from turning into runtime confusion.

Net-positive for security posture vs v1. rev3 explicitly addresses 3 codex MAJORs (lint, ceilings+egress, admin_detail extension) and 3 MINORs (tenant_overrides ack, timing oracle note, resolver tier logging).

### codex-chief-advisor validation (round 2 + round 3)

**Round 2 validation of rev2**: **BLOCK 3 CLOSED** (verdict letter issued). Per-Plug axis + enforcement point specificity + `admin_detail` + runtime-metadata-not-overloading-tool_name all resolve the original concerns. 3 MAJOR + 3 MINOR residual concerns raised for rev3 integration, none promoted to new blocker.

**Round 3 integration in this document**: all 6 codex residual concerns addressed in rev3 sections —
- MAJOR 1 (`requires_plugs` correctness lint) → §3 `cargo xtask check-bundles` CI gate
- MAJOR 2 (resource ceilings + egress policy) → §5 floors 6 + 7
- MAJOR 3 (`admin_detail` choke-point + extension) → §5 `render_gadget_error_for_caller` + `Denied`/`Execution` variants + regression test
- MINOR 4 (timing oracle) → STRIDE I note + constant-time padding in P2B-alpha
- MINOR 5 (`tenant_overrides` ack) → §2 `warn!` upgrade + CFG-045 + feature toggle
- MINOR 6 (resolver tier logging) → §7 `tracing::info!(tier = …, resolved_path = …)`

rev3 is the final shape for P2B-alpha opening. No further external round required.

---

## W3 scope — rev5 rewrite (knowledge-first DAG)

**Principle (user directive, 2026-04-18)**: 지식 레이어를 빠르게 end-to-end 로 돌려서 테스트하면서 다른 기능(Bundle 인프라 확장) 을 점진적으로 구현한다. W3 를 추상적 registry plumbing 중심으로 진행하지 않고, **실제 사용자 가치(wiki.import RAW ingestion 경로 + E2E 검증)** 가 먼저 land 되도록 재프레이밍.

### §W3 split DAG

W3 는 단일 PR 이 아니다. 의존 DAG:

```
                    W2 merged (bundle trait)
                           │
         ┌─────────────────┼─────────────────┐
         │                 │                 │
     W3-KL-1           W3-BUN-1          W3-XAAS
 (지식 E2E             (rev5 non_         (migrations
  wiki.import          exhaustive         000001/000002
  + Extractor          locks + cascade    no deps)
  + PDF extractor)     resolver)
         │                 │                 │
     W3-KL-2               │             W3-XAAS-B
 (Penny RAG               │             (CoreAuditEventWriter
  system prompt            │              + WorkdirPurgeJob)
  + citations)             │
         │             W3-BUN-2              │
         │         (populated GadgetHandles  │
         │          + additional plug axes)  │
         │                 │                 │
         └─────────────────┴─────────────────┘
                           │
                     W3-DEVOPS
              (xtask check-bundles (warn)
               + CI test-cpu/integration split
               + Helm + Prometheus)
                           │
                    P2B-alpha release tag
                    (2 MUST-LAND gates verified)
```

**critical path = W3-KL-1 → W3-KL-2 → P2B-alpha tag** (user directive). Other PRs land in parallel as they don't block E2E knowledge layer validation.

### §W3-KL-1 scope (highest priority, land first)

- `plugins/plugin-document-formats/` 스켈레톤 (markdown extractor only in first cut; PDF/docx/pptx via feature gates follow)
- `Extractor` trait (core-facing Plug) + registration into `BundleContext.plugs.extractors`
- `IngestPipeline::import(bytes, content_type, opts)` in `gadgetron-knowledge::ingest`
- `wiki.import` Gadget (new entry in `KnowledgeGadgetProvider`)
- End-to-end integration test: markdown RAW → Extractor → chunking → frontmatter → wiki write → pgvector embed → semantic search retrieval — all on a testcontainers postgres harness
- 예상 ~900 LOC + 6-8 tests

### §Gadget trait decision (rev5, codex MAJOR 3)

**`Gadget` trait 은 추가하지 않는다. `GadgetProvider` 유지.** `GadgetProvider::gadget_schemas()` + `GadgetProvider::call(name, args)` 패턴이 이미 "one provider = many gadgets" 시나리오를 지원하고, 새로 `trait Gadget` 을 item 단위로 도입하면 Bundle 저자가 이중 trait 구현 + JSON schema 중복을 감수해야 함. `GadgetHandles<'a>` 는 category 단위 handle 을 `Arc<dyn GadgetProvider>` 로 holding. Glossary §Gadget 에 해당 문구 유지.

### §BundleRegistry::install_all signature policy (rev5, codex MAJOR 5)

W2 가 `install_all(config, bundles)` 를 freeze 했으나 Gate 1 MUST-LAND 를 위해 W3-XAAS-B 에서 `install_all(config, bundles, sink: Arc<dyn CoreAuditEventSink>)` 로 확장 가능. 마지막 positional 파라미터 추가는 **rev5 에서 명시적으로 허용** — non-sink 호출자는 `NoopCoreAuditEventSink::new_arc()` 전달 pattern 사용. supersedes D-entry 별도 불필요 (이 문단 자체가 변경 승인).

### §check-bundles tolerance policy (rev5, codex MAJOR 4)

W3-DEVOPS 가 `cargo xtask check-bundles` 를 ship 할 때 기본 모드는 **`--warn-only`**. `--deny` 플래그는 존재하지만 CI 는 호출하지 않음. `gadgetron_xtask_check_bundles_warnings_total{bundle, kind}` Prometheus 카운터로 3 sprint 관찰 후 false-positive 0 확인되면 `--deny` 기본값 전환. 전환은 별도 PR + D-entry + dx 의 operator changelog 공지.

### §`#[non_exhaustive]` 확정 (rev5, chief-architect)

다음 타입들은 W3-BUN-1 첫 커밋에서 `#[non_exhaustive]` 적용 (향후 필드/variant 추가가 non-breaking 이 되도록):

- `PlugHandles<'a>` — 추가 Plug 축 (Extractor / BlobStore / Scheduler / EmbeddingProvider / EntityKind / HTTP routes) 로 확장
- `GadgetHandles<'a>` — category 별 handle (knowledge / infra / cicd / …) 로 확장
- `BundleRegistry` — W3 에서 cascade resolver + CoreAuditEventSink 필드 추가
- `PlugStatusKind` — W3 에서 `SkippedByAvailability { missing_plugs }` 케이스 추가 가능

### §W3 cfg-gated callsite policy (rev5, qa)

`#[cfg(feature = "...")]` 로 gated 된 `ctx.plugs.<port>.get(id)` callsite 는 W3-DEVOPS `xtask check-bundles` 에서 warn-only 로 처리 (hard-fail 안 함). W4 hardening 단계에서 policy 확정. fixture `tests/fixtures/cfg_gated/` 은 W3 에 포함하되 assertion 은 exit 0 + stderr warn 패턴.

## References

- ADR-P2A-10 `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md` (parent)
- Codex review v1 2026-04-18 — BLOCK 3 (external chief-advisor finding, session archive)
- Codex validation rev2 2026-04-18 — BLOCK 3 closed
- D-20260418-04 `docs/process/04-decision-log.md` — Bundle / Plug / Gadget trinity decision
- D-20260418-05 `docs/process/04-decision-log.md` — Driver → Plug rename amendment
- D-20260418-06 `docs/process/04-decision-log.md` — Team synthesis rev2 (this document)
- `docs/design/phase2/10-penny-permission-inheritance.md` §D5-a — `AuthenticatedContext` strict inheritance
- `docs/design/phase2/04-gadget-registry.md` §6 L3 — per-dispatch mode re-check gate
- `docs/design/phase2/06-bundle-architecture.md` §Config — `[bundles.<name>.gadgets]` override table
- ADR-P2A-06 Stabilization sprint item 1 — `ToolCallCompleted` audit schema (extended by `GadgetCallCompleted` per §5)
- `docs/architecture/glossary.md` — `Bundle`, `Plug`, `Gadget`, `PlugId`, `GadgetronBundlesHome` definitions (updated rev2)
