# Round 3 — chief-architect Review — `docs/design/phase2/04-mcp-tool-registry.md` v1

| Field | Value |
|---|---|
| **Reviewer** | chief-architect |
| **Review round** | Round 3 — Rust idiom + cross-crate type consistency + D-12 crate seam + implementation determinism |
| **Doc under review** | `docs/design/phase2/04-mcp-tool-registry.md` Draft v1 (PM-authored 2026-04-14) |
| **Date** | 2026-04-14 |
| **Decision anchors** | D-12 (crate boundary), D-13 (`GadgetronError`), D-20260411-10 (`Scope` enum), D-20260412-02 (implementation determinism), D-20260414-04 (Agent-Centric), ADR-P2A-05 |
| **Predecessors** | Round 1.5 dx-product-lead (`round-1_5-dx-product-lead-mcp-registry-v1.md`, BLOCK), Round 2 qa-test-architect (`round-2-qa-test-architect-mcp-registry-v1.md`, BLOCK) |
| **Sibling docs already Round-3 cleared** | `00-overview.md` v3, `01-knowledge-layer.md` v3, `02-kairos-agent.md` v3, `03-gadgetron-web.md` v2.1 |
| **Load-bearing** | This trait is the *stable plugin seam* for P2A → P2B → P2C → P3. Every change to `McpToolProvider` breaks downstream crates, so this Round 3 gate is higher-stakes than the siblings. |
| **Verdict** | **BLOCK** |

---

## Executive summary

The document is architecturally sound at the decision level — the 3-tier × 3-mode matrix, the dashmap+oneshot approval flow, and the namespace reservation are all defensible. However, the text-level spec has **seven** concrete architectural defects (CA-MCP-B1…B7) that prevent a TDD implementer from writing code directly from the doc. Five of them are crate-seam / type-inconsistency issues that would surface immediately on `cargo check`. Two are correctness defects in the `ApprovalRegistry` lifetime model that would ship a latent bug.

The doc was authored *after* an initial scaffolding of `gadgetron-core::agent::{config,tools}` landed in commit `b6b314d`. Several claims in the doc ("NEW module", exact trait shape, `#[derive(...)]` list) are already contradicted by the code on main. The doc is therefore simultaneously forward-spec (describing a future `gadgetron-kairos` crate that doesn't exist) and backward-spec (describing `gadgetron-core::agent::tools` that *does* exist but differs). Both halves need to be reconciled.

I found 7 blockers, 6 majors, and 7 minors. QA (`QA-MCP-B1/B2`) and dx (`DX-MCP-B1…B5`) already cover the test-surface and operator-UX gaps; my findings are disjoint and focus on the Rust type system + crate seam + determinism rails.

**v0.1.x → v0.2.0 migration path for `[kairos]` → `[agent.brain]` is entirely unimplemented in code** despite being claimed in §11 — this is the single highest-impact finding (CA-MCP-B1), because it forms a direct cross-doc contradiction with `02-kairos-agent.md` v3 which the PM already Round-3 cleared.

---

## v1 blocker verification from siblings

| Doc | Prior blocker | Status for this review |
|---|---|---|
| 00-overview v3 `LlmProvider` seam | APPROVED | Unchanged; this doc correctly does NOT add `KairosProvider` as a new trait path; the agent-centric pivot keeps the router provider map as the dispatch table, with `McpToolProvider` a separate plugin seam beneath it. Good. |
| 01-knowledge v3 `WikiError` taxonomy | APPROVED | This doc does not touch `WikiError`; however see **CA-MCP-B3** below — `McpError` does not compose into `GadgetronError`, which creates a parallel error universe. |
| 02-kairos v3 `[kairos]` config shape | APPROVED | **This is now contradicted** by §11 of this doc. See **CA-MCP-B1**. |
| 03-gadgetron-web v2.1 `WebConfig` placement | APPROVED | Unchanged — this doc adds `AgentConfig` as a peer field in `AppConfig`, consistent with how `WebConfig` was added. `AppConfig::agent` has already landed at `crates/gadgetron-core/src/config.rs:21`. Good. |

---

## BLOCKER findings

### CA-MCP-B1 — §11 `[kairos]` → `[agent.brain]` migration is pure prose; the `AppConfig` loader has zero support for field aliasing and `02-kairos-agent.md` v3 (Round-3 cleared) directly contradicts this spec

**Type / module under scrutiny**: `gadgetron_core::config::AppConfig::load` + `gadgetron_core::agent::config::{AgentConfig, BrainConfig}` + `02-kairos-agent.md §10` TOML example

**Defect**:

The doc §11 reads:
> The Kairos session builder (§5 of `02-kairos-agent.md`) already constructs the subprocess env from `KairosConfig.claude_base_url` / `KairosConfig.claude_model`. The only change in P2A is that these fields are now populated from `[agent.brain]` instead of `[kairos]` (rename + migration in the config loader; legacy `[kairos]` fields accepted with deprecation warning through P2B).

Three independent problems:

1. **The loader has no aliasing.** `crates/gadgetron-core/src/config.rs:181` calls `toml::from_str(&content)` against a strongly-typed `AppConfig` struct. `AppConfig` has NO `kairos` field, no `#[serde(alias = "kairos")]`, no `#[serde(flatten)]` into a deprecated section, and no post-deserialize migration hook. Exactly what happens today if an operator supplies `[kairos] claude_binary = "claude"` is: `toml::from_str` **silently ignores** the entire unknown table (toml crate default behavior is to accept unknown top-level keys in untagged structs). The operator gets a running server with `[agent.brain].mode = "claude_max"` silently, losing their `claude_base_url` override. That is a **silent behavior regression** from v0.1.x — the worst possible outcome for a user upgrading.

2. **`02-kairos-agent.md` v3 §10 (lines 982–988) still prescribes the canonical `[kairos]` schema** with fields `claude_binary`, `claude_base_url`, `claude_model`, `request_timeout_secs`, `max_concurrent_subprocesses`. Env overrides are listed as `GADGETRON_KAIROS_CLAUDE_BINARY` etc. None of these exist in the new `[agent]` / `[agent.brain]` schema in this doc §4. The two docs are now **mutually contradictory canonical TOML examples**. Since 02-kairos-agent.md v3 is Round-3 cleared, fixing this doc alone is not sufficient — the PM must also patch 02-kairos-agent.md v3 §10 or explicitly mark it superseded.

3. **The field mapping is incomplete even as prose.** §11 names only `claude_base_url` / `claude_model` as migrating, but `[kairos]` also has `claude_binary` (→ `[agent].binary`?), `request_timeout_secs` (→ where? `[agent].request_timeout_secs` does not exist in the new schema at all), and `max_concurrent_subprocesses` (→ ditto, dropped?). An operator reading §11 cannot construct a correct migration.

**Exact fix** (this must land in v2 of this doc AND a companion patch to `02-kairos-agent.md`):

1. Add a new subsection **§11.1 "v0.1.x → v0.2.0 field migration table"** with every v0.1.x field, its v0.2.0 destination (or "removed, see §X"), and the exact behavior when the v0.1.x field is present:

   | v0.1.x `[kairos]` | v0.2.0 destination | Behavior when v0.1.x field present |
   |---|---|---|
   | `claude_binary` | `[agent].binary` | Populate `agent.binary` if `[agent].binary` absent, emit `tracing::warn!(field = "kairos.claude_binary", replacement = "agent.binary", "deprecated Phase 2A field — will be removed in Phase 2C")` |
   | `claude_base_url` | `[agent.brain].external_base_url` + set `mode = "external_proxy"` | Same pattern |
   | `claude_model` | dropped — agent cannot pick brain model via this field any longer (see §14); emit an ERROR-level warning telling the operator to move to `[agent.brain]` |
   | `request_timeout_secs` | `[agent].request_timeout_secs` (NEW field — add to §4 schema) | Populate + deprecation warn |
   | `max_concurrent_subprocesses` | `[agent].max_concurrent_subprocesses` (NEW field — add to §4 schema) | Populate + deprecation warn |

2. Specify the loader mechanism. The only safe mechanism given that `AppConfig` is a strongly-typed struct is:
   - Parse the TOML to `toml::Value` first (untyped).
   - If `root.kairos` is present, extract its fields, move them under `root.agent.brain` (with the mapping above), and emit one deprecation warning per moved field.
   - Then serialize the `Value` back to a string and deserialize into `AppConfig`.
   - Test: `v0_1_x_kairos_config_loads_with_deprecation_warning` lives in `crates/gadgetron-core/src/config.rs` tests and asserts every moved field round-trips.

3. Patch `02-kairos-agent.md` v3 §10 to cross-reference this doc and add "**Superseded in P2A by `docs/design/phase2/04-mcp-tool-registry.md §4` — the `[kairos]` section is read for backward compatibility only, with deprecation warnings.**" at the top of the TOML example.

Without this fix, a v0.1.x operator running `gadgetron serve` after upgrading will experience either (a) silent regression to defaults, or (b) a panic at the `subprocess().env(ANTHROPIC_BASE_URL, ...)` site when `agent.brain.external_base_url` is empty. Both are P0 upgrade-path bugs.

---

### CA-MCP-B2 — `McpToolRegistry` and `AgentToolRegistry` are both referenced as real types but neither is defined; the dispatch table owner is unspecified

**Type / module under scrutiny**: `McpToolRegistry` (§2 L84, §13 L657, §14 L694), `AgentToolRegistry` (§6 L355)

**Defect**:

The doc uses two different names for what must be the same type:
- §2 L84 "Providers are registered via `McpToolRegistry::register(Box::new(...))`"
- §6 L355 "`AgentToolRegistry::build_allowed_tools(&AgentConfig)` produces the `--allowed-tools` list"
- §13 L657 "`McpToolRegistry::register` has a hardcoded check"
- §14 L694 "`McpToolRegistry::register(Box::new(XxxProvider::new(...)))` at startup"

Neither `McpToolRegistry` nor `AgentToolRegistry` is declared anywhere in the doc as a struct with fields. `crates/gadgetron-core/src/agent/tools.rs` (already landed) has the trait but no registry struct; the helper `ensure_tool_name_allowed(name, category) -> Result<(), McpError>` exists instead of the `assert!`-in-a-loop shown in §13 L657.

Three concrete gaps:

1. **Which crate does it live in?** The trait is in `gadgetron-core::agent::tools`. The registry must be either (a) in core (problematic: core would hold an `Arc<dyn McpToolProvider>` collection and have runtime state, violating the D-12 principle that core holds only types and pure functions), (b) in `gadgetron-kairos` (natural home — matches `ApprovalRegistry`), or (c) in `gadgetron-gateway` (natural home of the dispatch table). Pick one.

2. **What is its fields layout?** The doc only shows `McpToolRegistry::register(Box::new(...))`, leaving the reader to guess. It must be explicitly defined, e.g.:
   ```rust
   pub struct McpToolRegistry {
       providers: Vec<Box<dyn McpToolProvider>>,
       /// Flat lookup: fully-qualified tool name → provider index in `providers`.
       /// Built once at startup after all providers are registered.
       by_tool_name: HashMap<String, usize>,
       /// Cached flat `ToolSchema` list — the source of truth for `--allowed-tools`.
       all_schemas: Vec<ToolSchema>,
   }
   ```

3. **Ownership semantics**. `ApprovalRegistry` is `Arc<DashMap<...>>` internally (so it clones cheaply); is `McpToolRegistry` a `Arc<McpToolRegistry>` owned by `AppState`? Is `register(&mut self, ...)` a mutation phase followed by a `fn freeze(self) -> Arc<FrozenRegistry>`? The doc never specifies the transition from mutable-startup-phase to immutable-serving-phase. This is the exact architectural concern I flagged on `02-kairos-agent` round-2 — builder pattern vs. mutex. Pick one and write it down.

**Exact fix**:

1. Rename all references to a single name. `McpToolRegistry` is the stronger name (matches `McpToolProvider`).
2. Add a new **§2.1 "`McpToolRegistry` — the dispatch table"** with the full struct definition, the `register` / `build_allowed_tools` / `dispatch` methods, the crate that owns it, and the builder-freeze pattern:
   ```rust
   // crates/gadgetron-kairos/src/registry.rs  (or gadgetron-gateway::mcp::registry)
   pub struct McpToolRegistryBuilder {
       providers: Vec<Box<dyn McpToolProvider>>,
   }

   pub struct McpToolRegistry {
       by_tool_name: HashMap<String, Arc<dyn McpToolProvider>>,
       all_schemas: Arc<[ToolSchema]>,  // cheap to clone for sse events
   }

   impl McpToolRegistryBuilder {
       pub fn register(&mut self, provider: Box<dyn McpToolProvider>) -> Result<(), McpError> {
           for schema in provider.tool_schemas() {
               ensure_tool_name_allowed(&schema.name, provider.category())?;
           }
           self.providers.push(provider);
           Ok(())
       }
       pub fn freeze(self) -> McpToolRegistry { /* build lookup table, return Arc-able value */ }
   }

   impl McpToolRegistry {
       pub fn build_allowed_tools(&self, cfg: &AgentConfig) -> Vec<String> { ... }
       pub async fn dispatch(&self, name: &str, args: Value) -> Result<ToolResult, McpError> { ... }
   }
   ```
3. State the lifecycle in a short paragraph: "the builder is mutable, lives in `main()` until all `register` calls complete, then is consumed by `.freeze()` into an immutable `Arc<McpToolRegistry>` that is cloned into `AppState` and the kairos session builder."

Without this, `#10` (MCP server) cannot begin coding because the dispatch target is undefined.

---

### CA-MCP-B3 — `McpError` has no `From` impl into `GadgetronError`; it is a parallel error universe that cannot surface through the gateway's error path

**Type / module under scrutiny**: `McpError` (`gadgetron-core/src/agent/tools.rs:110`), `GadgetronError` (`gadgetron-core/src/error.rs:120`)

**Defect**:

`GadgetronError` is the single user-facing error path per D-13. Every error that leaves a crate boundary must be convertible into it. `McpError` has 6 variants (`UnknownTool`, `Denied`, `RateLimited`, `ApprovalTimeout`, `InvalidArgs`, `Execution`). The doc never shows `impl From<McpError> for GadgetronError` and there is no hint about how these errors reach the HTTP response.

Consider the call chain under `ask`-mode denial:
1. Claude Code calls `gadgetron mcp serve` via stdio.
2. `mcp serve` dispatches to `McpToolRegistry::dispatch` which calls `provider.call()` which may return `Err(McpError::Denied)`.
3. `mcp serve` serializes the `McpError` into a `tool_result { is_error: true, content: ... }` block and returns it to Claude Code.

OK so far — within the MCP stdio transport, `McpError` stays `McpError`. But:

4. `McpError::ApprovalTimeout` can also arise at the `Kairos session` level (the approval receiver times out inside the `KairosProvider::chat_stream` call, which is running in the router dispatch inside the axum handler).
5. The handler must convert this to an HTTP response. `KairosProvider` returns `Result<BoxedStream, GadgetronError>`. The inner `ApprovalTimeout` must become a `GadgetronError` variant — but no variant exists for it.

The obvious candidates are `GadgetronError::StreamInterrupted { reason }` (lossy — loses the error code) or a new `GadgetronError::Kairos { kind: KairosErrorKind::ToolApprovalTimeout { secs } }` variant (adds a new variant to `KairosErrorKind`, which is `#[non_exhaustive]` so non-breaking).

**Exact fix**:

1. Add an explicit **§10.1 "`McpError` → `GadgetronError` conversion"** section with the full table:

   | `McpError` variant | `GadgetronError` target | HTTP code | error_code |
   |---|---|---|---|
   | `UnknownTool(_)` | `GadgetronError::Kairos { kind: KairosErrorKind::ToolError { code: "mcp_unknown_tool" }, ... }` | 500 | `kairos_tool_error` |
   | `Denied { reason }` | `GadgetronError::Kairos { kind: KairosErrorKind::ToolDenied { reason }, ... }` | 403 | `kairos_tool_denied` |
   | `RateLimited { .. }` | `GadgetronError::Kairos { kind: KairosErrorKind::ToolRateLimited { .. }, ... }` | 429 | `kairos_tool_rate_limited` |
   | `ApprovalTimeout { secs }` | `GadgetronError::Kairos { kind: KairosErrorKind::ToolApprovalTimeout { secs }, ... }` | 504 | `kairos_tool_approval_timeout` |
   | `InvalidArgs(_)` | `GadgetronError::Kairos { kind: KairosErrorKind::ToolInvalidArgs { reason }, ... }` | 400 | `kairos_tool_invalid_args` |
   | `Execution(_)` | `GadgetronError::Kairos { kind: KairosErrorKind::ToolExecution { reason }, ... }` | 500 | `kairos_tool_execution_failed` |

2. Add the 6 new variants to `KairosErrorKind` in `crates/gadgetron-core/src/error.rs`. Since `KairosErrorKind` is `#[non_exhaustive]` this is additive. Extend `error_code`, `error_message`, `http_status_code` in the same file and bump the variant count test `all_fourteen_variants_exist` (currently 14) — wait, actually the variant count is counting `GadgetronError` variants, not `KairosErrorKind`, so 14 stays; only `KairosErrorKind` grows.

3. Add the `impl From<McpError> for GadgetronError { ... }` block explicitly in §10.1. Put it in `crates/gadgetron-core/src/agent/tools.rs` (needs `use crate::error::{GadgetronError, KairosErrorKind}`) or split it out so `tools.rs` stays layer-clean. Per D-12 and my round-2 review of 02-kairos-agent: `From<X> for GadgetronError` conversions should live in `error.rs` as a dedicated `mod conversions` module.

Without this, a `Denied` error in the `KairosProvider::chat_stream` path will force the implementer to invent an ad-hoc conversion (probably `GadgetronError::StreamInterrupted { reason: format!("{e}") }`) which loses structured information and fails the existing `kairos_agent_error_message_does_not_contain_stderr` test pattern (since `McpError::Execution(reason)` might carry subprocess output that needs redaction).

---

### CA-MCP-B4 — Tool name vs. category vs. config field naming is inconsistent: "infrastructure" / "infra" / "infra_write" / "knowledge" / "wiki.*"; §2's invariant "`name.starts_with(self.category())`" is violated by the doc's own examples

**Type / module under scrutiny**: `McpToolProvider::category()` / `ToolSchema::name` / `[agent.tools.write].*` field names

**Defect**:

§2 L111-113 says:
> The registry validates the name matches this provider's category before calling; implementers may assume `name.starts_with(self.category())`.

But the doc's own tables use:
- Category `"knowledge"`, tool names `wiki.read`, `wiki.get`, `wiki.search`, `wiki.write`, `web.search` (§14 L686) — none start with `"knowledge"`
- Category `"infrastructure"` (§2 L95), tool names `infra.list_nodes` etc. (§14 L688), SSE event `"category": "infrastructure"` (§7 L489), config field `infra_write` (§4 L288) — three different spellings for one concept
- Category `"scheduler"`, tool names `scheduler.schedule_job` (§14) — this one is consistent

This is both a correctness defect (the registry pre-validation `name.starts_with(self.category())` will reject every single `KnowledgeToolProvider` tool) and a naming-hygiene defect (operators configuring `infra_write = "ask"` have to mentally link that to `"infrastructure"` category and `infra.*` tool prefix, three different strings).

Three reconciliation options:

**Option A** — tool prefix == category (strictest, matches the documented invariant):
- Rename category `"knowledge"` → keep; rename tools `wiki.*` → `knowledge.wiki.*`, `web.search` → `knowledge.web.search`. Rename category `"infrastructure"` → keep; rename tools `infra.*` → `infrastructure.*`. This is consistent but verbose.

**Option B** — drop the invariant (looser):
- Replace `name.starts_with(self.category())` with `name.split('.').next() == first_segment_of_category_table(self.category())` — a more complex mapping table. Not recommended (adds indirection).

**Option C** — separate "category" (broad: `"knowledge"` / `"infrastructure"`) from "prefix" (per-tool: `"wiki"` / `"infra"`), with multiple prefixes per category:
- Add a `tool_prefix: &'static str` field on `ToolSchema` separate from the category.
- Change the invariant to `schema.tool_prefix` must be an allowed prefix for `provider.category()`, maintained by a second const table in `gadgetron-core::agent::tools`.
- This preserves the existing tool names while being explicit.

**Exact fix**:

Pick Option A (most Rust-idiomatic and matches the documented invariant) and apply a consistent rename across:
1. §2 reserved-categories list (L95).
2. §7 SSE event example (L488-489).
3. §14 roadmap table (L686-691).
4. `crates/gadgetron-core/src/agent/tools.rs` docstring (L32 — reserved categories comment).
5. `[agent.tools.write]` config fields in §4 (L287-290) — `wiki_write` → `knowledge_wiki_write` OR keep `wiki_write` as a semantic-alias field that maps to the `knowledge.wiki.write` tool (explicit mapping table required).

If Option A is too verbose, pick Option C and add the `tool_prefix` field. Either way, reconcile the three spellings so the invariant holds and a new provider author can start from `gadgetron-core::agent::tools` and not hit contradictions.

Also: §4 config field `infra_write` — but the §14 roadmap table lists `infra.list_nodes`, `infra.get_gpu_util` etc. as **T1 Read** (implied — they are list/get operations). Only `infra.deploy_model`, `infra.hot_reload_config`, `infra.set_routing_strategy` are T2 Write. But `infra_write` config is a single knob covering "all T2 infra tools." Under the new category system, this means the config field name doesn't map cleanly to any tool prefix — `infra_write` is a subcategory label, not a namespace. Document this explicitly: add a subsection in §4 or §5 that defines "a *subcategory* is a cluster of related T2 tools grouped for config purposes; subcategories do NOT form a level in the tool name hierarchy."

---

### CA-MCP-B5 — `ApprovalRegistry::decide` + `await_decision` contain a double-remove race; `pending.remove(&id)` is called on both the success path and the timeout path, and the non-winner drops an already-sent oneshot sender

**Type / module under scrutiny**: `ApprovalRegistry::{decide, await_decision}` (§7 L444-470)

**Defect**:

The code is:

```rust
pub fn decide(&self, id: ApprovalId, decision: ApprovalDecision) -> Result<(), ApprovalError> {
    let (_, pending) = self.pending.remove(&id).ok_or(ApprovalError::NotFound)?;
    pending.tx.send(decision).map_err(|_| ApprovalError::ChannelClosed)?;
    Ok(())
}

pub async fn await_decision(
    &self,
    id: ApprovalId,
    rx: oneshot::Receiver<ApprovalDecision>,
) -> ApprovalDecision {
    match tokio::time::timeout(self.timeout, rx).await {
        Ok(Ok(decision)) => decision,
        Ok(Err(_)) => ApprovalDecision::Timeout, // channel closed
        Err(_) => {
            let _ = self.pending.remove(&id);
            ApprovalDecision::Timeout
        }
    }
}
```

Three correctness problems:

1. **Double-Timeout**: If `decide(id, Allow)` runs concurrently with the `tokio::time::timeout` firing, the following interleaving is possible on a multi-thread runtime:
   - t=0: `enqueue` inserts `id` → map.
   - t=59.999s: client POSTs to `/v1/approvals/{id}`. `decide` calls `self.pending.remove(&id)` → gets `Some(pending)`. It calls `pending.tx.send(Allow)`.
   - t=60.000s: `tokio::time::timeout` deadline fires *before* the `tx.send` has delivered into `rx`. The `rx` future resolves to `Err(_)` (channel closed — because `tx` was moved into `decide`'s local binding which is about to be dropped). The `Ok(Err(_))` arm fires: **returns `ApprovalDecision::Timeout` even though the user clicked Allow**.
   - This is not a hypothetical — `tokio::time::timeout` uses `poll` semantics; a send that has completed on one worker thread may not be observed by the waiter on another thread until the next scheduler tick, and the timeout can fire between those two points.

2. **Orphan lingering**: In the timeout branch, `self.pending.remove(&id)` is called. But `decide()` may also have already called `self.pending.remove(&id)`. DashMap's `remove` is atomic and returns `None` on the second call, so the `let _ =` pattern is safe — but the first-mover semantics are inverted: if `decide` wins the race, the `Ok(Err(_))` arm fires (because `tx` was consumed and dropped by `decide` → channel closed from the receiver's view after the decision was sent). So the user's `Allow` turns into `Timeout`.

3. **No audit distinction**: in the `Ok(Err(_))` arm the doc says "channel closed". But a "channel closed" in the above timing race is **actually a user-clicked-Allow that was lost**. The audit log (§10) will record `ToolApprovalTimeout` when the real ground truth is `ToolApprovalGranted`. This is a **silently-wrong audit entry** — worst kind, because the operator sees "tool timed out, user didn't click" when the user actually did click Allow.

**Exact fix**:

Change `PendingApproval` to hold `Arc<tokio::sync::Notify>` + `Arc<Mutex<Option<ApprovalDecision>>>` instead of the oneshot, OR use a different coordination primitive where the `decide` side writes the decision into shared state *before* signalling. Minimal-change fix:

```rust
use std::sync::Arc;
use tokio::sync::Notify;

pub struct PendingApproval {
    // ...
    decision: Arc<Mutex<Option<ApprovalDecision>>>,
    notify: Arc<Notify>,
}

impl ApprovalRegistry {
    pub fn decide(&self, id: ApprovalId, decision: ApprovalDecision) -> Result<(), ApprovalError> {
        let pending = self
            .pending
            .get(&id)
            .ok_or(ApprovalError::NotFound)?;
        let mut slot = pending.decision.lock().unwrap();
        if slot.is_some() {
            return Err(ApprovalError::AlreadyDecided);
        }
        *slot = Some(decision);
        pending.notify.notify_one();
        Ok(())
    }

    pub async fn await_decision(&self, id: ApprovalId) -> ApprovalDecision {
        let Some(pending) = self.pending.get(&id).map(|r| r.clone_inner()) else {
            return ApprovalDecision::Timeout; // never enqueued
        };
        tokio::select! {
            _ = pending.notify.notified() => {
                let slot = pending.decision.lock().unwrap();
                self.pending.remove(&id);
                slot.clone().unwrap_or(ApprovalDecision::Timeout)
            }
            _ = tokio::time::sleep(self.timeout) => {
                // Race: decide may have won between notify and here.
                // Check slot one last time.
                let slot = pending.decision.lock().unwrap();
                let final_decision = slot.clone().unwrap_or(ApprovalDecision::Timeout);
                self.pending.remove(&id);
                final_decision
            }
        }
    }
}
```

Key property: **the decision is written to shared state before signalling**, so the "am I a winner?" check (`slot.is_some()`) after the timeout is authoritative. The timeout branch no longer assumes timeout — it re-reads the slot.

Add a new `ApprovalError::AlreadyDecided` variant to distinguish from `NotFound`.

Add the test **`enqueue_decide_and_timeout_race_does_not_produce_double_timeout`** in `crates/gadgetron-kairos/tests/approval_flow.rs` using `tokio::time::pause` + `advance` to deterministically interleave:
```rust
let (id, rx) = reg.enqueue(...);
let h = tokio::spawn(async move { reg_clone.await_decision(id).await });
// Advance to 1ms before timeout.
tokio::time::advance(Duration::from_millis(59_999)).await;
// Call decide at exactly the edge.
reg.decide(id, ApprovalDecision::Allow).unwrap();
// Advance 2ms past the timeout.
tokio::time::advance(Duration::from_millis(2)).await;
assert_eq!(h.await.unwrap(), ApprovalDecision::Allow);  // NOT Timeout
```

This is a load-bearing correctness fix — without it, the audit log is wrong in the exact corner case operators care about most (approval edge timing).

---

### CA-MCP-B6 — `approval_timeout_secs` default 60s can outlive the parent chat SSE request; no cleanup when the client drops the SSE stream (browser tab closed, network drop)

**Type / module under scrutiny**: `ApprovalRegistry::await_decision` lifetime vs. `KairosProvider::chat_stream` lifetime vs. the incoming `axum::response::sse::Sse` stream from the browser

**Defect**:

The approval flow spans three lifetimes:

1. **Browser SSE stream** — lives as long as the browser keeps the tab open to `POST /v1/chat/completions` with `stream=true`. If the user closes the tab, the axum handler's `Sse` stream future is dropped.
2. **`KairosProvider::chat_stream`** — returns a `BoxStream<'static, Result<ChatChunk, GadgetronError>>`. This stream owns the subprocess handle (via `ClaudeCodeSession`) and is driven forward when the axum handler polls the SSE. When the SSE is dropped, the stream is dropped, which drops the session, which SIGTERMs claude.
3. **`ApprovalRegistry::await_decision`** — is called from **inside** the chat_stream during a tool call. It `rx.await`s up to 60s.

Now consider: user opens the tab, kairos invokes `wiki.write` in `ask` mode. The SSE emits `gadgetron.approval_required`. User closes the tab. Expected behavior:

- Browser SSE drops.
- Axum handler future is dropped.
- BUT: the subprocess has already sent the MCP tool call and is blocked reading the response. The `KairosProvider::chat_stream` future is dropped mid-flight, **dropping the `await_decision` future**. The `rx` future is dropped. The oneshot sender is still held in `DashMap`. The sender will never deliver — cleanup only happens when the timeout fires 60s later.
- During those 60s, the entry lingers in the map. The approval_id is visible to other request paths (a replay attack could POST to `/v1/approvals/{id}` with the stale id and resolve it after the parent request is dead — no one cares, but this is a resource leak).

Additionally, **the claude subprocess is SIGTERMed** (via `Drop for ClaudeCodeSession`) — but it had an MCP call in flight, so the subprocess may have already exited by the time `await_decision` even begins. The MCP call never completes on the stdio side. Whether the subprocess is still alive is indeterminate from the approval registry's view.

This is not a correctness bug — just a resource leak + stale audit — but the doc is silent on it.

**Exact fix**:

Add a new subsection **§7.1 "Lifetime contract"** specifying:

1. `PendingApproval` stores a `request_id: Uuid` (already does — §7 L380) and this request_id must match the top-level `/v1/chat/completions` request.
2. When the SSE stream is dropped (axum handler future cancelled), a `Drop` impl on a `ChatRequestGuard` held by the handler calls `ApprovalRegistry::cancel_for_request(request_id)`:
   ```rust
   pub fn cancel_for_request(&self, request_id: Uuid) {
       self.pending.retain(|_, p| p.request_id != request_id);
   }
   ```
3. `cancel_for_request` is invoked in the handler's cleanup path (either `Drop` on a per-request guard that holds `Arc<ApprovalRegistry> + request_id`, or a tokio spawn that awaits `CancellationToken::cancelled()` bound to the request scope).
4. Audit event: emit `ToolApprovalCancelled { approval_id, reason: "request_dropped" }` — add a sixth variant to `ToolAuditEvent` in §10.

Also: the doc §6 (L339) says `approval_timeout_secs < 10 || > 600` — range. Consider also clamping at the **server request timeout** from `[server].request_timeout_ms` (30_000ms default): if the SSE cannot live longer than 30s, then a 60s approval timeout **guarantees** the double-cleanup path above will fire on every timeout. Add a V15 validation rule: `approval_timeout_secs * 1000 <= server.request_timeout_ms`.

Note: V15 interacts with `[server].request_timeout_ms` which is per-request. If the server timeout is a "max inactivity" idle timeout rather than a total-duration timeout, then SSE streams can live longer than 30s (they reset the idle timer on each event via heartbeat). The §7 heartbeat clause says "`: keepalive\n\n` every 15 seconds" — consistent with an idle timer. If that's the model, the V15 rule should instead be `approval_timeout_secs * 1000 <= 2 * heartbeat_interval_secs * 1000` to guarantee at least one heartbeat fires before the timeout. Specify which model the server uses and then add the right validation rule.

---

### CA-MCP-B7 — Cargo feature gate hierarchy is unowned: `default = ["web-ui", "agent-read"]` collides with the gateway's existing `default = ["web-ui"]`, and `agent-read` / `agent-write` / `agent-destructive` are not located in any crate

**Type / module under scrutiny**: `Cargo.toml [features]` block in §6 L340-351

**Defect**:

The doc §6 declares:
```toml
[features]
default = ["web-ui", "agent-read"]
agent-read = []
agent-write = ["agent-read"]
agent-destructive = ["agent-write"]
infra-tools = ["agent-write"]
scheduler-tools = ["agent-write"]
slurm = ["scheduler-tools"]
k8s = ["scheduler-tools"]
```

But:

1. **There is no owner crate specified.** The doc says "Headless/read-only builds strip write tools from the binary at compile time" (L353), which implies the features live on the binary crate — `gadgetron-cli`. But the features include `agent-read` etc. which must propagate to `gadgetron-core::agent::tools` (to `#[cfg(feature = ...)]`-gate provider `is_available()` impls) or to `gadgetron-knowledge` (to gate `KnowledgeToolProvider::tool_schemas` content).

2. **`default = ["web-ui", ...]` already exists on `gadgetron-gateway` as `default = ["web-ui"]`** (`crates/gadgetron-gateway/Cargo.toml:29`). If features are added to gateway, the default becomes `default = ["web-ui", "agent-read"]` — OK additive. But `gadgetron-cli` has no `[features]` block at all today. If the features live on `gadgetron-cli`, then CLI's `default` must activate the downstream gateway feature via `gadgetron-gateway = { workspace = true, features = ["web-ui"] }` explicitly.

3. **Provider `is_available()` cannot be feature-gated in a library-stable way.** The trait method `is_available(&self) -> bool` can return `cfg!(feature = "agent-write")` — but `cfg!` is evaluated in the crate where the impl is defined, so if `KnowledgeToolProvider` is in `gadgetron-knowledge`, the cfg evaluates in *knowledge*'s feature set, not the CLI's. To reach the CLI's feature set, the CLI must pass a runtime boolean into the provider constructor, OR the CLI re-exports a feature forward to knowledge via `gadgetron-knowledge = { workspace = true, features = ["agent-write"] }`. The doc never shows this plumbing.

4. **`grep -r kill_process target/release/gadgetron returns no matches`** (§6 L353) — this only holds if the `kill_process` tool is compile-time gated behind `agent-destructive`. That requires either (a) the entire provider impl for the tool is `#[cfg(feature = "agent-destructive")]`, meaning `tool_schemas()` returns different lists depending on compile flags, or (b) the tool lives in a separate crate (`gadgetron-cluster` — which won't exist until P3). Pick one and document it.

**Exact fix**:

1. Add **§6.1 "Feature owner and propagation map"** with the crate that owns each feature:

   | Feature | Owner crate | Propagates to |
   |---|---|---|
   | `agent-read` | `gadgetron-cli` | `gadgetron-knowledge/agent-read`, `gadgetron-gateway/agent-read` |
   | `agent-write` | `gadgetron-cli` | `gadgetron-knowledge/agent-write` (includes `wiki.write`), `gadgetron-gateway/agent-write` |
   | `agent-destructive` | `gadgetron-cli` | `gadgetron-knowledge/agent-destructive` (enables future destructive tools), `gadgetron-gateway/agent-destructive` |
   | `infra-tools` | `gadgetron-cli` | (P2C) `gadgetron-infra/*` |
   | `scheduler-tools` | `gadgetron-cli` | (P3) `gadgetron-scheduler-tools/*` |
   | `slurm` | `gadgetron-cli` | `gadgetron-scheduler-tools/slurm` |
   | `k8s` | `gadgetron-cli` | `gadgetron-scheduler-tools/k8s` |

2. The `gadgetron-cli` `[features]` block is then:
   ```toml
   [features]
   default = ["web-ui", "agent-read"]
   web-ui = ["gadgetron-gateway/web-ui"]
   agent-read = ["gadgetron-knowledge/agent-read", "gadgetron-gateway/agent-read"]
   agent-write = ["agent-read", "gadgetron-knowledge/agent-write", "gadgetron-gateway/agent-write"]
   agent-destructive = ["agent-write", "gadgetron-knowledge/agent-destructive", "gadgetron-gateway/agent-destructive"]
   infra-tools = ["agent-write"]
   scheduler-tools = ["agent-write"]
   slurm = ["scheduler-tools"]
   k8s = ["scheduler-tools"]
   ```
   (Gateway and core need corresponding `[features]` blocks with the same feature names as no-op passthroughs or with actual `#[cfg(feature = ...)]` guards.)

3. `gadgetron-core::agent::tools` does **not** receive any feature gates — the trait and types are always compiled. Only concrete provider implementations in leaf crates (`gadgetron-knowledge`, `gadgetron-infra`, `gadgetron-scheduler-tools`) carry the feature gates on their `fn tool_schemas` return.

4. Add the test `headless_build_strips_write_tools` in `crates/gadgetron-cli/tests/` that builds with `--no-default-features --features web-ui` and asserts `nm` / `grep` against the binary does not show any write tool names. This was implicitly promised at L353 and needs a concrete test spec.

Without this, the feature gate cannot be implemented — every attempt will hit a "which crate owns this feature?" question.

---

## MAJOR findings

### CA-MCP-M1 — `Scope::AgentAutoApproveT2` is named in ADR-P2A-05 §(d) but absent from this doc §15.10 and §9; the decision log only ratifies `AgentApproval`

**Location**: §9 L550 (`Auth`), §15.10 L711 (resolved decisions), vs. ADR-P2A-05 §(d) L79

ADR-P2A-05 §(d) L79:
> API SDK 소비자는 `AgentAutoApproveT2` scope 로 T2 자동 승인 가능; T3 은 여전히 사람이 필요

This doc resolves only `Scope::AgentApproval` — a single scope. But ADR-P2A-05 specifies **two** distinct scopes:
- `AgentApproval` — normal browser client scope, OR explicitly-granted headless manual approval
- `AgentAutoApproveT2` — headless SDK scope that implicitly grants T2 approvals without a card

This matters because the Round 1.5 dx review (DX-MCP-B4) **also** flagged the missing headless path — the two findings reinforce each other. Decide explicitly:

**Option A**: ADR-P2A-05 was aspirational, and the PM decided to drop `AgentAutoApproveT2` during authoring of this doc. Update ADR-P2A-05 §(d) to strike the mention, and add a note to §15.10 "Resolved decisions" line 11: `Scope::AgentAutoApproveT2 deferred to P2C pending headless-client spec (see DX-MCP-B4)`.

**Option B**: Both scopes are needed. Add `Scope::AgentAutoApproveT2` to §15.10 and §9 auth requirements:
- §9 L550: `Auth`: `Scope::OpenAiCompat` + (`Scope::AgentApproval` OR `Scope::AgentAutoApproveT2`)
- §9 semantics: when the caller has `AgentAutoApproveT2` but not `AgentApproval`, T2-tier cards are auto-resolved `Allow`; T3-tier cards still require a manual UI click (so the call fails with a clear error).
- Add a test `post_approvals_id_with_auto_approve_t2_allows_t2_but_not_t3`.

Both scopes need corresponding entries in `as_str()` match in `context.rs`:
```rust
Self::AgentApproval => "agent_approval",
Self::AgentAutoApproveT2 => "agent_auto_approve_t2",
```
(The existing `as_str` uses snake_case — new variants must follow.)

---

### CA-MCP-M2 — `ToolAuditEvent` has no declared parent enum and no declared home crate; the Phase 1 audit surface is not referenced

**Location**: §10 L565-599, D-20260411-10 M6 audit reference in ADR L172

§10 L563 says "New variants in the gateway audit event enum" — but there is no existing gateway audit event enum. I searched `crates/gadgetron-gateway/src/**` and found no `pub enum AuditEvent` or similar. The "M6 tools_called audit" referenced in ADR-P2A-05 L172 is a Phase-2 addition to whatever audit path already exists — but Phase 1 has **no** structured audit enum, just `sqlx` writes to `audit_log` rows via ad-hoc SQL.

Two concrete gaps:

1. **Is `ToolAuditEvent` a new standalone enum, or an extension of a Phase 1 parent?** If standalone, where is the `fn write_audit_event(&AuditStore, ToolAuditEvent) -> Result<()>` surface, and how does it compose with Phase 1 `audit_log.row_id` foreign keys? If an extension, name the parent enum and show the added variants.

2. **Which crate owns it?** Options:
   - `gadgetron-kairos::audit` — natural because kairos owns the approval flow
   - `gadgetron-gateway::audit` — natural because gateway owns `/v1/approvals/{id}` dispatch
   - `gadgetron-xaas::audit` — matches the Phase 1 audit subsystem

**Fix**: Add a one-paragraph clarification to §10 opening: "`ToolAuditEvent` is a new standalone enum in `gadgetron-xaas::audit` (co-located with Phase 1 audit writes). It is persisted via the existing `audit_log` table with a new `event_type` column value `"tool_*"`. Schema migration: add SQL migration `NNNN_tool_audit_events.sql` in `crates/gadgetron-xaas/migrations/`." If the crate placement is different, say so explicitly — but the reader must not have to guess.

---

### CA-MCP-M3 — `PendingApproval::category: &'static str` but `ApprovalRegistry::enqueue` takes `category: &'static str` from a dynamic call site — this is only sound if the category string is always a const literal

**Location**: §7 L382 `category: &'static str`, §7 L417 `category: &'static str` parameter

`McpToolProvider::category() -> &'static str` (§2 L101) — the trait enforces `&'static`. So all concrete provider implementations must return a `&'static str`, i.e., a string literal or a `const`. This is correct and matches my own advice: "don't allocate a `String` for a value that's known at compile time."

But the doc doesn't say why the `&'static` bound is on the trait. A naive provider author could write:

```rust
impl McpToolProvider for DynamicProvider {
    fn category(&self) -> &'static str {
        Box::leak(self.category_name.clone().into_boxed_str())  // silent leak per call
    }
}
```

...and the code would compile. This is a subtle Rust footgun: the `&'static` bound does not prevent memory leaks from `Box::leak`, which is the "easy fix" a junior dev reaches for.

**Fix**: Add a doc comment on `McpToolProvider::category()` stating explicitly:
```rust
/// MUST return a string literal (`&'static str`) constructed at compile time.
/// Using `Box::leak` to satisfy the lifetime bound is an anti-pattern that
/// causes an unbounded memory leak per call.
///
/// Good: `fn category(&self) -> &'static str { "knowledge" }`
/// Bad:  `fn category(&self) -> &'static str { Box::leak(self.name.clone()) }`
fn category(&self) -> &'static str;
```

Also add a lint (rustc or clippy) test that catches `Box::leak` inside `impl McpToolProvider`... actually clippy doesn't have such a lint. A `#[deny(clippy::mem_forget)]` / `#[deny(clippy::leaking_memory)]` at the trait module level is the best we can do.

Lower priority than the blockers but worth documenting while we have the ergonomic decision on the table.

---

### CA-MCP-M4 — `ToolSchema::name: String` forces per-startup allocation across all providers; the doc's own `tool_schemas()` contract says "called once at startup" but the implementation still pays for String clones on each call

**Location**: §2 L130 `pub name: String`, §2 L105 contract "Called once at startup. The registry caches the result"

Given the contract "called once at startup, registry caches", each provider's `tool_schemas` call path is:

```rust
for provider in &providers {
    for schema in provider.tool_schemas() {
        registry.all_schemas.push(schema);  // moves the ToolSchema
    }
}
```

Each `ToolSchema` has `name: String` (+ `description: String`, `input_schema: serde_json::Value`). A minimal provider with 5 tools allocates 5 × `String::from("knowledge.wiki.read")` etc., plus 5 × description strings, at every call. Since the call is once at startup, this is not a hot path — fine.

BUT: the doc L127 says `McpToolRegistry::dispatch` takes `name: &str` and **matches it against the cached schemas** (§2 L111 "The registry pre-validates that `name.starts_with(self.category())`"). The dispatch path is per-tool-call, which **is** hot. If the registry keeps `all_schemas: Vec<ToolSchema>` and looks up by `schema.name == incoming_name`, that's a linear scan per call. If instead the registry keeps `by_tool_name: HashMap<String, ...>`, the lookup allocates only per call (to hash the incoming `&str`), which is acceptable.

The fix is a single sentence in §2.1 (see CA-MCP-B2 fix): "`McpToolRegistry` keeps a `HashMap<String, Arc<dyn McpToolProvider>>` keyed by fully-qualified tool name, built at startup from all schemas flattened. Per-call lookup is `hashmap.get(name)` which is `O(1)`."

Secondary: consider whether `name: String` should be `name: Cow<'static, str>` to allow static providers to pass `&'static str` literals without cloning. This is a micro-opt not worth pursuing at the trait level — the stable plugin interface is worth more than the 5 × 16-byte allocations per startup.

---

### CA-MCP-M5 — `Tier::Destructive` + `DestructiveToolsConfig::enabled = false` + `WriteToolsConfig::default_mode = "never"` are three different ways to say "this tool does not exist"; pick one canonical representation and make the others map to it

**Location**: §3 L205-207 (tier defaults), §4 L297 (`enabled = false`), §4 L284 (`default_mode = "never"`)

Three code paths to "block a tool":

1. Tool author declares `Tier::Destructive` on the schema → default `enabled = false` → tool is omitted from `--allowed-tools`.
2. Operator sets `[agent.tools.write].default_mode = "never"` → all T2 tools without explicit subcategory override are omitted from `--allowed-tools`.
3. Operator sets `[agent.tools.destructive].enabled = false` → all T3 tools are omitted from `--allowed-tools`.

Plus the runtime check:
- `L3 MCP server gate` (§6 L357) re-checks the mode on every call and returns `McpError::Denied { reason }` if the mode is `never`.

Question: under path (1) + (3), if the operator **did not** set `enabled = true` but a T3 tool somehow reaches the MCP server, what is the `reason` string on `McpError::Denied`? The doc doesn't say. Path (2) does not add it either. The `build_allowed_tools` function is supposed to filter them out at Layer 2, so Layer 3 should never see them... unless Claude Code is buggy and calls a tool it wasn't told about. In that case the L3 denial fires. What reason?

**Fix**: Add a reason-code table in §6 or a new `McpError::Denied::Reason` sub-enum:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenialReason {
    ToolDisabledByFeatureGate,      // compile-time: Cargo feature is off
    ToolDisabledByConfig,            // L2: [agent.tools.*] = "never" or enabled=false
    ToolRequiresApprovalAndDenied,   // L4: user clicked Deny
    ToolRateLimited,                 // L4: T3 budget exhausted
    ToolExtraConfirmationMismatch,   // L4: extra_confirmation token wrong
    ReservedNamespaceViolation,      // §13: provider tried to register agent.*
}

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("denied by policy: {reason:?}")]
    Denied { reason: DenialReason, detail: Option<String> },
    // ...
}
```

This solves three problems at once:
- Agent can `match` on the reason instead of parsing a string.
- Audit log `ToolApprovalDenied.reason: String` becomes `reason: DenialReason` — enum, not string, at the persistence layer.
- `McpError::Denied { reason }` is no longer a bare `String` that might leak details.

The doc §9 L586 already uses a string-enum-ish format for the audit:
> `reason: String, // "user_clicked_deny" | "rate_limit_exceeded" | "timeout" | "extra_confirmation_mismatch"`

This is stringly-typed, which is fine for JSON serialization but brittle for internal matching. Promote it to a real enum with `#[serde(rename_all = "snake_case")]`.

Related DX finding: Round 1.5 DX-MCP-M1 already flagged that `McpError::Denied::reason` is never populated concretely. The fix here is stronger — make it typed.

---

### CA-MCP-M6 — `gadgetron-kairos` crate is referenced in §7, §10, §16 but does not exist in the workspace; every file path starting with `crates/gadgetron-kairos/` is a ghost path

**Location**: §7 L366 (`crates/gadgetron-kairos/src/approval.rs`), §16 L727 (`crates/gadgetron-kairos/tests/approval_flow.rs`), ADR-P2A-05 `gadgetron-kairos` references, workspace `Cargo.toml`

`Cargo.toml:3-16` lists workspace members; `gadgetron-kairos` is not one. `crates/` directory (`ls` output) confirms — 12 crates, no `gadgetron-kairos`. Yet the design doc, the ADR, and `02-kairos-agent.md` v3 all write file paths under `crates/gadgetron-kairos/`.

This is a scaffolding gap, not a design defect per se. But:

1. **No `Cargo.toml` has ever been drafted** for this crate. What does it depend on? (Must include `gadgetron-core`, `dashmap`, `tokio`, `uuid`, `thiserror`, `async-trait`, probably `serde` and `serde_json`.) Does `dashmap` need to be added as a workspace dep for this crate? (Actually `dashmap = "6"` is already in workspace deps at root `Cargo.toml:80`, good.)

2. **Ordering**: `#10` (MCP server) and `#13-#15` (Kairos session/stream/provider) need this crate to exist. The TDD order for P2A must start with `crates/gadgetron-kairos/Cargo.toml` creation + workspace member addition.

3. **Cross-doc impact**: `02-kairos-agent.md` v3 is Round-3 cleared but assumes `gadgetron-kairos` exists. That doc's Round 2 review (round2-chief-architect.md) explicitly says "`ClaudeCodeSession`... `config: Arc<KairosConfig>` (config sharing fine)" — citing a struct that lives in the absent crate.

**Fix**: Before this doc can land, add a **§0.1 "Crate scaffolding prerequisites"** section specifying:

1. `gadgetron-kairos` must be added as a workspace member in root `Cargo.toml` at line 16 (after `gadgetron-knowledge`).
2. Add `gadgetron-kairos = { path = "crates/gadgetron-kairos" }` to root `[workspace.dependencies]` block.
3. Create `crates/gadgetron-kairos/Cargo.toml` with dependencies:
   ```toml
   [package]
   name = "gadgetron-kairos"
   version.workspace = true
   edition.workspace = true
   license.workspace = true

   [dependencies]
   gadgetron-core = { workspace = true }
   tokio = { workspace = true }
   dashmap = { workspace = true }
   serde = { workspace = true }
   serde_json = { workspace = true }
   async-trait = { workspace = true }
   thiserror = { workspace = true }
   uuid = { workspace = true }
   chrono = { workspace = true }
   tracing = { workspace = true }
   futures = { workspace = true }

   [dev-dependencies]
   tokio = { workspace = true, features = ["test-util", "macros", "rt-multi-thread"] }
   ```
4. Specify the module layout: `src/{lib.rs, session.rs, stream.rs, provider.rs, config.rs, approval.rs, redact.rs}` — cross-referencing `02-kairos-agent.md §4-§5` for the first three modules, and this doc §7 for `approval.rs`.
5. The scaffolding commit is a prerequisite for any `#10`-`#15` code PR.

This is not strictly this doc's responsibility — it's PM-level scaffolding — but the doc is the most visible place that demands the crate exists. A one-paragraph "prerequisites" note here unblocks the entire P2A TDD start.

---

## MINOR findings

### CA-MCP-N1 — `Tier` derives `Hash` in `crates/gadgetron-core/src/agent/tools.rs:84` but §2 L144 omits `Hash` from the derive list

The code on main (`tools.rs:84`) is:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
```
The doc §2 L144 shows:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
```

The code is correct (the `Hash` is needed to use `Tier` as a HashMap key for the dispatch table). The doc must be patched to match.

### CA-MCP-N2 — `ToolSchema.idempotent` uses `#[serde(default, skip_serializing_if = "Option::is_none")]` in the landed code but §2 L141 only shows `#[serde(default)]`

Same direction — code has the `skip_serializing_if`, doc doesn't. Minor presentation bug, patch the doc.

### CA-MCP-N3 — `McpError::error_code()` exists in `crates/gadgetron-core/src/agent/tools.rs:131-141` but is absent from §2

The landed impl has a stable machine-readable error code surface:
```rust
impl McpError {
    pub fn error_code(&self) -> &'static str { ... }
}
```
Mirrors `GadgetronError::error_code()` pattern. The doc should show this explicitly because `ToolAuditEvent::ToolApprovalDenied.reason: String` (§10 L585) should be populated with `err.error_code()` when the denial is from a code path, not a user click. Without showing the function, the implementer has to rediscover the pattern.

### CA-MCP-N4 — §13 L657-672 uses `assert!` for reserved-namespace enforcement; the landed `ensure_tool_name_allowed` helper returns `Result<(), McpError>`, which is better

The doc shows a panic-based enforcement:
```rust
for tool in provider.tool_schemas() {
    let name = &tool.name;
    assert!(
        !name.starts_with("agent.") && ...,
        "tool {name} is in the reserved 'agent.*' namespace (ADR-P2A-05 §14)"
    );
}
```

The code on main (`tools.rs:173-191`) uses a graceful `Result`-returning helper:
```rust
pub fn ensure_tool_name_allowed(name: &str, category: &str) -> Result<(), McpError> { ... }
```

Prefer the Result-returning version — `assert!` aborts the process and makes CI runs harder to diagnose. Update §13 to use the helper and move the test `reserved_agent_namespace_is_rejected` accordingly (which already exists in `tools.rs:198`, also named the same — so this nit is really just a doc-code sync).

### CA-MCP-N5 — `Scope::AgentApproval` serde representation is unspecified; the existing `Scope` enum preserves PascalCase at the serde layer but `as_str()` uses snake_case

**Location**: §15.10 `Scope::AgentApproval` mention

`crates/gadgetron-core/src/context.rs` already has:
- serde: PascalCase (`"OpenAiCompat"`, `"Management"`, `"XaasAdmin"`) — NO `rename_all` attribute
- `as_str()`: snake_case (`"openai_compat"`, `"management"`, `"xaas_admin"`)

The doc doesn't say which representation `AgentApproval` should use at the serde layer. Since the existing enum lacks `rename_all`, the new variant will serialize as `"AgentApproval"` by default — **not** what most JWT / API-key stores expect. The `as_str()` method should return `"agent_approval"`.

**Fix**: Explicitly specify in §15.10:
> `Scope::AgentApproval` serializes as `"AgentApproval"` (PascalCase) via serde default (consistent with existing variants — no `rename_all` is applied). `Scope::AgentApproval.as_str()` returns `"agent_approval"` (consistent with existing variants' snake_case `as_str` output).

Alternatively, convert the entire `Scope` enum to `#[serde(rename_all = "snake_case")]` — but that's a serialization breaking change for existing API keys and must go through the decision log. Do not do this.

Also applies to `Scope::AgentAutoApproveT2` if CA-MCP-M1 is resolved as Option B: `as_str` returns `"agent_auto_approve_t2"`.

### CA-MCP-N6 — `async fn call(&self, ...)` returns `Result<ToolResult, McpError>` but the trait derives from `#[async_trait]`; document the vtable-allocation footprint for future readers

The trait uses `#[async_trait]` which desugars `async fn call(...)` to `fn call(...) -> Pin<Box<dyn Future + Send + '_>>`. This allocates a `Box` per call. At the MCP tool call rate (low — agent-driven, ~1-10 per request), this is a non-issue. But a future reader might wonder if we should wait for `type_alias_impl_trait` / RFC 3668 to remove the allocation.

Add a comment:
```rust
/// Dispatch a tool call.
///
/// This trait uses `#[async_trait]` which incurs a single `Box<dyn Future>`
/// allocation per call. This is acceptable because tool calls are driven by
/// the agent (bounded by LLM rate — typically < 100/sec) and the per-call
/// cost is dominated by the provider work (wiki write, shell exec). Moving
/// to native AFIT (async-fn-in-trait) requires `dyn` support, which stabilizes
/// in rustc 1.85+ — revisit once workspace MSRV permits.
async fn call(&self, name: &str, args: serde_json::Value) -> Result<ToolResult, McpError>;
```

The current workspace MSRV is 1.80 (root `Cargo.toml:22`). AFIT-in-dyn-trait is not yet stable in 1.80. So `#[async_trait]` is the correct choice today — document the reason.

### CA-MCP-N7 — `AgentConfig::validate(&self, providers: &HashMap<String, ProviderConfig>)` takes a `&HashMap` instead of a `&impl Fn(&str) -> bool`; leaks core's internal map type into the validation API

**Location**: `crates/gadgetron-core/src/agent/config.rs:61-66` + §5 validation rules V10

The landed code:
```rust
pub fn validate(&self, providers: &std::collections::HashMap<String, crate::config::ProviderConfig>) -> Result<()>
```

This couples `AgentConfig::validate` to the full `ProviderConfig` enum just to ask "does this provider name exist?" for V10. A leaner signature:

```rust
pub fn validate(&self, provider_exists: impl Fn(&str) -> bool) -> Result<()>
```

...gives the same V10 check with no dependency on `ProviderConfig` details.

This is genuinely a minor nit (the current code compiles and the dependency is acyclic: config.rs → agent/config.rs, where agent/config.rs imports from parent config.rs). Rust-idiom-wise, pushing the decision boundary closer to the call site is cleaner. Not blocking — call out as a cleanup at the next refactor window.

---

## Crate boundary diff from D-12

D-12 table (`docs/reviews/pm-decisions.md` §67-89) lists Phase 1 types only. This doc introduces the following new `gadgetron-core` types:

| New type | Crate | File | Justification (per D-12 principle) |
|---|---|---|---|
| `McpToolProvider` (trait) | `gadgetron-core` | `src/agent/tools.rs` | ✅ Crosses `gadgetron-knowledge` + `gadgetron-kairos` + `gadgetron-infra` + `gadgetron-scheduler-tools` + `gadgetron-cluster` — shared plugin interface, must live in core |
| `ToolSchema` / `Tier` / `ToolResult` / `McpError` | `gadgetron-core` | `src/agent/tools.rs` | ✅ Associated types of `McpToolProvider`; must live with the trait |
| `AgentConfig` / `BrainConfig` / `BrainMode` / `BrainShimConfig` / `ToolsConfig` / `ToolMode` / `WriteToolsConfig` / `DestructiveToolsConfig` / `ExtraConfirmation` | `gadgetron-core` | `src/agent/config.rs` | ✅ Config types owned by `AppConfig`; standard D-12 pattern matches `WebConfig` placement |
| `Scope::AgentApproval` (+ `AgentAutoApproveT2` if CA-MCP-M1 resolved as Option B) | `gadgetron-core` | `src/context.rs` | ✅ Extension of existing `#[non_exhaustive]` enum; D-20260411-10 Phase 2 expansion clause covers this |
| `McpToolRegistry` / `McpToolRegistryBuilder` (after CA-MCP-B2 fix) | `gadgetron-kairos` or `gadgetron-gateway` | TBD | ⚠ **Must be decided** — not yet in D-12 |
| `ApprovalRegistry` / `PendingApproval` / `ApprovalDecision` / `ApprovalError` | `gadgetron-kairos` | `src/approval.rs` | ✅ Kairos-owned approval state; not used by anyone else; D-12 principle (used-by-one-crate → lives-in-that-crate) |
| `ToolAuditEvent` (after CA-MCP-M2 fix) | `gadgetron-xaas::audit` (recommended) or `gadgetron-gateway::audit` | TBD | ⚠ **Must be decided** — not yet in D-12 |

**Recommended D-12 update** (after this doc clears Round 3): append the above table entries to `docs/reviews/pm-decisions.md` §D-12 crate boundary table, same format as the existing 19 entries.

No D-12 violations detected — every new type has a defensible home. The two TBD rows are not violations, just gaps this doc must close.

---

## Determinism findings (per D-20260412-02)

Per "구현 결정론적" rule — items where implementation behavior is not uniquely specified and a TDD implementer would have to guess:

### DET-1 — §6 L355 `AgentToolRegistry::build_allowed_tools(&AgentConfig)` has no signature or return type

What does it return? `Vec<String>`? `&[String]`? Owned or borrowed? What is the *order* of the returned list (Claude Code cares about `--allowed-tools` order for disambiguation)? What if two providers register the same tool name — error? last-wins? If a tool is `Tier::Read` in mode `auto`, is it in the list? (Yes, obviously.) If a tool is `Tier::Write` in mode `ask`, is it in the list? (Also yes — the card flow triggers at runtime, not at `--allowed-tools` filter time.) If a tool is `Tier::Write` in mode `never`, is it in the list? (No — explicitly stated §3 L215.) Specify the exact predicate as pseudocode in §6.

### DET-2 — §7 `ApprovalRegistry::enqueue` returns `(ApprovalId, oneshot::Receiver<ApprovalDecision>)` — the receiver is returned outside the registry; who owns it?

§7 L439 returns `(id, rx)` as a tuple, which implies the caller owns the receiver. But then §7 L463 has `self.pending.remove(&id)` inside `await_decision`, which means the registry also holds a reference. The double-ownership is tractable (the receiver is separate from the map) but must be explicitly documented: "the caller awaits the receiver; the map holds only the sender (`tx`). Dropping the receiver while the sender is held by the map is safe — the `tx.send()` on `decide` returns `Err(_)` which maps to `ApprovalError::ChannelClosed`."

See also CA-MCP-B5 — the double-ownership is the root cause of the race condition.

### DET-3 — §8 L516 `tier: 'write' | 'destructive'` — where is T1 `'read'`?

The frontend interface type omits `'read'`. Reason: T1 `read` tools are always `auto` mode and never generate an approval card, so the ApprovalCard will never see a T1 tool. That's correct. But the exclusion should be called out in the interface comment:
```ts
// T1 Read tools are always auto — they never generate an ApprovalCard.
// The 'read' variant is deliberately absent from this union.
tier: 'write' | 'destructive';
```

### DET-4 — §10 L601-604 retention policy is stated in days but not in columns; is it enforced by a cron, by a DB trigger, or by the application?

"Read (T1) — 30 days default; Write (T2) — 90 days minimum; Destructive (T3) — 365 days minimum, excluded from any purge_audit_log operation."

Who enforces this? The `purge_audit_log` reference hints at an existing function, but none exists in the Phase 1 codebase. Phase 1 `audit_log` table has no `tier` column. A retention policy that spans tiers requires the `ToolAuditEvent` to persist its tier in a `audit_log.tier` column — that schema change has to be specified here. Add:

> §10.1: the `audit_log` table gains a `tier TEXT NULL` column (migration `NNNN_audit_log_tier.sql`). Existing rows have NULL tier (pre-P2A). Retention job `SELECT id FROM audit_log WHERE tier IN ('read') AND created_at < NOW() - INTERVAL '30 days'` + delete. The job runs... [cron? background task?] — specify.

### DET-5 — §16 test list uses tests names that reference `tokio::time::pause` but no assertion helpers are specified

`enqueue_and_decide_allow_unblocks_receiver` — does this use `tokio::time::pause`? If yes, list the assertion primitive (`tokio::test(start_paused = true)` attribute, `tokio::time::advance`). If no, how does it avoid flakiness on CI? This was also flagged by QA (QA-MCP-B1) — double-confirming.

---

## Action Items (for v2 doc revision)

| ID | Severity | Location | Owner | Fix |
|---|---|---|---|---|
| CA-MCP-B1 | BLOCKER | §11 + `02-kairos-agent.md §10` | PM | Add §11.1 field migration table + specify loader mechanism + patch `02-kairos-agent.md §10` with supersession note |
| CA-MCP-B2 | BLOCKER | §2 L84, §6 L355, §13 L657, §14 L694 | PM | Add §2.1 `McpToolRegistry` struct definition + rename `AgentToolRegistry` → `McpToolRegistry` globally + specify builder-freeze lifecycle |
| CA-MCP-B3 | BLOCKER | §10 (new §10.1) + `error.rs` | chief-architect | Add `impl From<McpError> for GadgetronError` with full variant table + extend `KairosErrorKind` with 6 new variants |
| CA-MCP-B4 | BLOCKER | §2 L90/95, §4 L288, §7 L489, §14 L686-691 | PM | Pick Option A (rename tool prefixes to match category) or Option C (add `tool_prefix` field) and apply consistently |
| CA-MCP-B5 | BLOCKER | §7 L444-470 | chief-architect | Rewrite `ApprovalRegistry::{decide,await_decision}` with `Notify`-based coordination + shared decision slot + add double-race test |
| CA-MCP-B6 | BLOCKER | §7 (new §7.1) | chief-architect | Add lifetime contract section + `cancel_for_request` API + `ChatRequestGuard` drop + new `ToolApprovalCancelled` audit variant + V15 validation rule |
| CA-MCP-B7 | BLOCKER | §6 L340-351 (new §6.1) | chief-architect | Add feature owner + propagation map + concrete `gadgetron-cli/Cargo.toml` feature block + `headless_build_strips_write_tools` test spec |
| CA-MCP-M1 | MAJOR | §9 L550, §15.10, ADR-P2A-05 §(d) | PM | Decide Option A (drop `AgentAutoApproveT2`) or Option B (add both scopes); update scope list, auth table, and `as_str()` |
| CA-MCP-M2 | MAJOR | §10 L563 | PM | Name the parent enum + owner crate; add migration spec for `audit_log.tier` column |
| CA-MCP-M3 | MAJOR | `tools.rs` doc comments | chief-architect | Add doc-comment warning against `Box::leak` pattern for `category()` return |
| CA-MCP-M4 | MAJOR | §2 (new §2.1) | chief-architect | Specify `McpToolRegistry.by_tool_name: HashMap<String, Arc<dyn McpToolProvider>>` for `O(1)` dispatch |
| CA-MCP-M5 | MAJOR | §6 L357 + §10 L585 | chief-architect | Replace string reason with `DenialReason` enum; promote audit `reason: String` to typed enum |
| CA-MCP-M6 | MAJOR | §0 (new §0.1) | PM | Add "crate scaffolding prerequisites" with workspace member add + Cargo.toml draft for `gadgetron-kairos` |
| CA-MCP-N1 | MINOR | §2 L144 | PM | Add `Hash` to `Tier` derive list to match landed code |
| CA-MCP-N2 | MINOR | §2 L141 | PM | Add `skip_serializing_if` to `idempotent` serde attribute |
| CA-MCP-N3 | MINOR | §2 end | PM | Show `McpError::error_code()` method in the `McpError` example |
| CA-MCP-N4 | MINOR | §13 L657-672 | PM | Replace `assert!` with `ensure_tool_name_allowed` helper (already landed in `tools.rs:173`) |
| CA-MCP-N5 | MINOR | §15.10 | PM | Specify `Scope::AgentApproval` PascalCase serde + `"agent_approval"` as_str |
| CA-MCP-N6 | MINOR | §2 L113 | PM | Add `#[async_trait]` allocation rationale + MSRV note in doc comment |
| CA-MCP-N7 | MINOR | `config.rs:61` | chief-architect | Consider `impl Fn(&str) -> bool` instead of `&HashMap<String, ProviderConfig>` — not blocking |

---

## Rubric §3 checklist (chief-architect Round 3)

| Item | Result | Notes |
|---|---|---|
| Rust idiom (zero-cost where it matters) | PARTIAL | `&'static str` on category is zero-cost; `ToolSchema.name: String` is not — see M4 |
| `From` impls complete | FAIL | `From<McpError> for GadgetronError` missing — CA-MCP-B3 |
| `'static` bounds justified | PASS | `McpToolProvider: Send + Sync + 'static` is correct for dyn-trait storage |
| `#[non_exhaustive]` on public enums | MIXED | `Tier` is NOT non-exhaustive (intentional — adding a new tier is a major release concern); `McpError` IS non-exhaustive-by-default via `thiserror` enum; `ApprovalDecision` is NOT non-exhaustive (intentional — fixed 3-variant state machine). Document these intentions. |
| Trait object safety | PASS | All trait methods take `&self`, no `Self: Sized` bounds, no generic methods — `dyn McpToolProvider` is object-safe |
| Workspace dep hygiene | PASS | No new deps added by the doc; `dashmap` already workspace |
| Crate seam (D-12) | FAIL (2 TBD rows) | `McpToolRegistry` and `ToolAuditEvent` crate placement unspecified |
| Implementation determinism (D-20260412-02) | FAIL | See DET-1..DET-5 |
| Cross-doc consistency | FAIL | `02-kairos-agent.md` v3 directly contradicted; `Scope::AgentAutoApproveT2` ADR mismatch |
| `GadgetronError` taxonomy extension plan | FAIL | `KairosErrorKind` must gain 6 variants for McpError interop; not specified |

**Pass: 4 / 10. Fail: 5 / 10. Mixed: 1 / 10.**

---

## Verdict

**BLOCK**

The architectural decisions are sound. The text-level spec has 7 concrete blockers that prevent TDD-start. None of them are mortal — each has an explicit, ~1-page fix in this review — but all 7 must land in a v2 revision before any `#10` code PR. In particular:

- **CA-MCP-B1** is the highest-priority fix because it creates a direct contradiction with an already-Round-3-cleared sibling (`02-kairos-agent.md` v3) and risks a silent v0.1.x → v0.2.0 upgrade regression.
- **CA-MCP-B5** is the highest-risk-if-missed fix because it ships a subtle correctness bug (timer-edge double-Timeout) that will only fire in production under load — exactly when the audit log matters most.
- **CA-MCP-B6** is the highest-impact-on-UX fix because a browser tab close = 60s dangling approval is a very real operator experience.

The Round 1.5 dx review and Round 2 qa review already cover the operator-UX and test-surface gaps. My findings are disjoint and additive — after v2 addresses all 7 blockers plus integrates the dx (5 blockers) and qa (2 blockers) findings, this doc is ready for a v2 Round-3 pass.

I will **re-review** against v2 with the following gate: all 7 CA-MCP-B* must be resolved + the 5 ghost references (`McpToolRegistry`/`AgentToolRegistry`/`KnowledgeToolProvider`/`gadgetron-kairos` crate/audit enum parent) must all point to concrete types in concrete crates with concrete Cargo.toml dependencies.

---

*End of Round 3 chief-architect review of `docs/design/phase2/04-mcp-tool-registry.md` v1. 2026-04-14.*
