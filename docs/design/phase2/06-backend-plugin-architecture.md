# 06 — Backend Plugin Architecture

> **Status**: Draft v1 (2026-04-16) — user-directed via session discussion
> **Author**: PM (Claude)
> **Parent**: `docs/design/phase2/00-overview.md`, `docs/design/phase2/01-knowledge-layer.md`, `docs/design/phase2/05-knowledge-semantic.md`
> **Drives**: P2B workstream "plugin-ai-infra extraction"
> **Scope**: Separate the AI-infrastructure management functionality from the core Kairos/knowledge/gateway stack, enabling multiple backends (AI infra, CI/CD, accounting, ...) to plug into the same collaboration hub.
> **Relationship to ADR-P2A-07**: Complementary. ADR-P2A-07 confirms "지식 레이어 core는 도메인 비종속" — this document makes the functional layer match that principle.

---

## Table of Contents

1. Problem & principle
2. Three-tier architecture
3. `BackendPlugin` trait
4. Knowledge ↔ Function separation
5. Lifecycle: enable / disable / uninstall
6. Crate boundary changes
7. Frontmatter & wiki integration
8. Cross-domain query flow (the JARVIS value)
9. Migration plan (P2A → P2A+ → P2B)
10. Open questions
11. Out of scope

---

## 1. Problem & principle

### Problem

Gadgetron's current crate layout mixes **backend-agnostic** infrastructure (gateway, Kairos, knowledge, web UI) with **AI-infrastructure-specific** code (provider adapters, scheduler, node monitor, model catalog) in a single monolithic binary. This works for Phase 2A's single-backend scope, but:

- **ADR-P2A-07 explicitly states** the knowledge layer is domain-agnostic. Today's AI-infra backend is a use case, not the system's identity.
- Adding a second backend (CI/CD, calendar, accounting, ...) would currently require editing core crates.
- Uninstalling AI-infra code is architecturally impossible — it is entangled.
- `gadgetron-web`, `gadgetron-kairos`, `gadgetron-knowledge` are already designed to be backend-neutral; only the binary composition is monolithic.

### Principle

> **Knowledge is core. Capabilities are pluggable.**

One shared wiki (the JARVIS memory). Many pluggable backends (the JARVIS capabilities).

Follows directly from ADR-P2A-07's domain-agnostic framing + `05-knowledge-semantic.md` hybrid semantic search (which enables natural cross-domain retrieval).

---

## 2. Three-tier architecture

```
┌───────────────────────────────────────────────────────────────────┐
│                🟦 CORE (backend-agnostic)                          │
│                "Kairos의 집 + 협업 기판"                            │
│  ┌────────────────────────────────────────────────────────────┐   │
│  │  gadgetron-core                                             │   │
│  │   · AppConfig, GadgetronError, LlmProvider trait            │   │
│  │   · NEW: BackendPlugin trait (§3)                            │   │
│  │   · NEW: PluginRegistry                                     │   │
│  └────────────────────────────────────────────────────────────┘   │
│  ┌──────────────────┐  ┌──────────────────┐  ┌────────────────┐   │
│  │ gadgetron-gateway│  │ gadgetron-router │  │ gadgetron-xaas │   │
│  │ HTTP + SSE + auth│  │ LLM 라우팅(범용)  │  │ tenants/audit/ │   │
│  └──────────────────┘  └──────────────────┘  │ postgres pool  │   │
│                                              │ + pgvector     │   │
│  ┌─────────────────────────────────────────┐ └────────────────┘   │
│  │ gadgetron-kairos                        │                      │
│  │   · Claude Code 서브프로세스 드라이버    │                      │
│  │   · McpToolRegistry (plugins register here)                    │
│  │   · Kairos 페르소나 system prompt                               │
│  └─────────────────────────────────────────┘                      │
│  ┌─────────────────────────────────────────┐                      │
│  │ gadgetron-knowledge                     │   ← JARVIS의 "기억"   │
│  │   · wiki (md + git + frontmatter)       │                      │
│  │   · chunking + embedding + pgvector     │                      │
│  │   · 하이브리드 검색 (RRF fusion)         │                      │
│  │   · wiki_registry (P2B: Private/Team/Public/Plugin)            │
│  └─────────────────────────────────────────┘                      │
│  ┌─────────────────────────────────────────┐                      │
│  │ gadgetron-web (UI)                      │                      │
│  │   · MCP 툴은 런타임에 동적 발견          │                      │
│  │   · 플러그인 추가 시 UI는 자동 대응      │                      │
│  └─────────────────────────────────────────┘                      │
└───────────────────────────────────────────────────────────────────┘
                            ▲
                            │ BackendPlugin trait
                            │ (MCP tools + seed pages + LLM providers)
                            │
┌───────────────────────────┴───────────────────────────────────────┐
│                  🟨 PLUGINS (domain-specific)                      │
│  ┌─────────────────────┐  ┌────────────┐  ┌──────────────────┐    │
│  │ plugin-ai-infra     │  │ plugin-cicd│  │ plugin-accounting│    │
│  │  (현재의 AI 스택)    │  │   (미래)    │  │    (미래)        │    │
│  │                     │  │            │  │                  │    │
│  │ · provider 어댑터    │  │ · build    │  │ · invoices       │    │
│  │ · scheduler          │  │   hooks    │  │ · reports        │    │
│  │ · node monitor       │  │ · deploy   │  │                  │    │
│  │ · 모델 카탈로그       │  │   webhooks │  │                  │    │
│  │                     │  │            │  │                  │    │
│  │ MCP tools:          │  │ MCP tools: │  │ MCP tools:       │    │
│  │  gpu.list, model.*, │  │  build.*,  │  │  invoice.*,      │    │
│  │  scheduler.stats    │  │  deploy.*  │  │  report.generate │    │
│  │                     │  │            │  │                  │    │
│  │ Seed pages:         │  │ ...        │  │ ...              │    │
│  │  ai-infra/getting-* │  │            │  │                  │    │
│  └─────────────────────┘  └────────────┘  └──────────────────┘    │
└───────────────────────────────────────────────────────────────────┘
                            ▲
                            │
┌───────────────────────────┴───────────────────────────────────────┐
│                🟩 USER-FACING SURFACES                             │
│  ┌─────────────────┐  ┌──────────────┐  ┌───────────────────┐     │
│  │ Web UI (/web)   │  │ CLI          │  │ OpenAI API (/v1)  │     │
│  │ 채팅 + 설정      │  │ gadgetron *  │  │ 외부 클라이언트    │     │
│  └─────────────────┘  └──────────────┘  └───────────────────┘     │
└───────────────────────────────────────────────────────────────────┘
```

The three tiers communicate through:

- **Core → Plugins**: `AppConfig` read-only views (subsection per plugin), `McpToolRegistryBuilder` (plugins add tools), LLM provider map (plugins add providers).
- **Plugins → Core**: `BackendPlugin::initialize()` registers everything the plugin needs.
- **Surfaces ← Plugins**: Tools are discovered via the MCP registry; plugins don't directly know about surfaces.

---

## 3. `BackendPlugin` trait

### Definition (Rust, P2A+ scaffold)

```rust
// crates/gadgetron-core/src/plugin/mod.rs

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::AppConfig;
use crate::provider::LlmProvider;

/// Deterministic marker for how a plugin wants its seeded knowledge treated
/// when the operator disables or uninstalls the plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisableBehavior {
    /// Leave seed pages where they are. Default. JARVIS-style: knowledge
    /// outlives capability.
    KeepKnowledge,
    /// Move seed pages to `_archived/<plugin_name>/` on disable. Operator
    /// can move them back manually.
    ArchiveKnowledge,
    /// Ask the operator via the runtime prompt layer (CLI/web UI).
    PromptOperator,
}

/// A markdown page a plugin wants to seed into the wiki on first enable.
/// The page is subject to the same security pipeline as any `wiki.write`.
pub struct SeedPage {
    /// Relative path inside the wiki root. Example: "ai-infra/getting-started.md"
    pub path: String,
    /// Full page content including TOML frontmatter. Frontmatter MUST set:
    ///   source = "plugin_seed"
    ///   plugin = "<plugin name>"
    ///   plugin_version = "<plugin version>"
    pub content: String,
    /// If true, overwrite an existing page with the same path. Default false.
    pub overwrite: bool,
}

/// Builder handed to plugins during `initialize`. The plugin uses this to
/// express what it wants to register. The core owns the actual registration
/// targets (router, MCP registry, etc.) — plugins never touch them directly.
pub struct PluginContext<'a> {
    pub config: &'a AppConfig,
    pub plugin_config: &'a toml::Value, // the `[plugins.<name>]` subsection
    // Private handles to registries — exposed via methods on this context:
    // fn mcp_registry_mut(&mut self) -> &mut McpToolRegistryBuilder
    // fn register_llm_provider(&mut self, name: String, p: Arc<dyn LlmProvider>)
    // fn register_http_routes(&mut self, router: axum::Router<AppState>)
    // fn seed_pages(&mut self, pages: Vec<SeedPage>)
    // (exact surface finalized during P2A+ implementation)
}

pub trait BackendPlugin: Send + Sync + 'static {
    /// Stable machine identifier. Used as key in `[plugins.<name>]` config,
    /// frontmatter `plugin` field, and seed page prefix. Must be kebab-case.
    fn name(&self) -> &str;

    /// Semver version of this plugin build. Used for seed-upgrade decisions.
    fn version(&self) -> &str;

    /// One-line human-readable description shown in `gadgetron plugins list`.
    fn description(&self) -> &str;

    /// Called once at `gadgetron serve` startup (and on explicit enable).
    /// Plugin registers its MCP tools, LLM providers, HTTP routes, seed
    /// pages, etc. through the provided context.
    fn initialize(&mut self, ctx: &mut PluginContext<'_>) -> crate::error::Result<()>;

    /// Called when the plugin is disabled at runtime (config change or admin
    /// command). Plugin tears down any long-lived resources (subprocess pools,
    /// background tasks, etc.). Default no-op.
    fn shutdown(&mut self) -> crate::error::Result<()> {
        Ok(())
    }

    /// How this plugin wants its seeded knowledge handled on disable.
    /// Default: keep.
    fn on_disable(&self) -> DisableBehavior {
        DisableBehavior::KeepKnowledge
    }
}
```

### Config surface

Add to `gadgetron.toml`:

```toml
[plugins]
# Ordered list of plugins to enable at startup. Built-in plugins (compiled
# into the binary) are available by name; external plugins (P2C+) are
# loaded by path or registry reference.
enabled = ["ai-infra"]

# Each plugin gets its own subsection. Free-form TOML — the plugin itself
# defines its schema and validates on initialize().
[plugins.ai-infra]
# Example: which providers the ai-infra plugin should register.
# (Currently these live at top-level [providers] — will migrate in P2B.)

[plugins.ai-infra.nodes]
# Moved from top-level [[nodes]] during P2B extraction.
```

P2A+ backward compatibility: if `[plugins]` section is absent, the built-in ai-infra plugin is enabled by default with the legacy top-level `[providers]`, `[[nodes]]`, etc. as its config.

---

## 4. Knowledge ↔ Function separation

This is the user's direct question: "지식과 기능이 연동되어 있는데 어떻게 관리?"

### Three layers, explicitly

| Layer | Where it lives | Plugin dependency |
|-------|----------------|-------------------|
| **Shared memory** (wiki, hybrid search, embedding) | core `gadgetron-knowledge` | **None**. Forever core. |
| **Knowledge content** (GPU runbooks, CI incident records, ...) | wiki markdown files (git) | **Metadata only** — `tags`, `type`, `plugin` frontmatter fields identify domain |
| **Capabilities** (GPU ops, CI triggers, ...) | plugin MCP tools | **Yes**. Remove plugin = remove capability |

### Decoupling rule

Knowledge **content** survives plugin removal. Only the **capability to act** on that content goes away. This mirrors how a person's notes about a tool still read sensibly after the tool is uninstalled — the notes remain, only the ability to run the tool is gone.

### Seed pages: the plugin's knowledge contribution

When a plugin enables for the first time, it can inject "starter docs" into the wiki:

```markdown
---
source = "plugin_seed"
plugin = "ai-infra"
plugin_version = "0.2.0"
tags = ["ai-infra", "getting-started"]
type = "runbook"
created = 2026-04-16T00:00:00Z
updated = 2026-04-16T00:00:00Z
---

# Deploying a new model to the cluster

1. Pick a node with enough VRAM (see `gpu.list` tool).
2. ...
```

These pages are full citizens of the wiki. The operator can edit them, and after edit `source` becomes `"user"` (or stays `"plugin_seed"` with `source_modified_by = "user"` — decision deferred).

---

## 5. Lifecycle: enable / disable / uninstall

### Event matrix

| Event | Wiki content | MCP tools | pgvector index |
|-------|--------------|-----------|----------------|
| Plugin **enable** (first time) | seed pages injected (frontmatter `source="plugin_seed"`) | registered | auto chunking/embedding via existing write path |
| Plugin **re-enable** (version bump) | seed pages re-injected only if `plugin_version > existing.plugin_version` (decision: opt-in via plugin) | re-registered | updated on rewrite |
| Plugin **disable** (runtime toggle) | unchanged | unregistered | unchanged |
| Plugin **uninstall** (binary rebuilt without plugin) | based on `on_disable()`: keep OR move to `_archived/<plugin>/` | fully gone | reindex eventually reflects archive path |
| Manual edit of seed page | md reflects edit | no change | reindex catch-up |
| `git pull` of another operator's wiki changes | md reflects changes | no change | reindex catch-up |

### Archive flow (when `on_disable() == ArchiveKnowledge`)

```
SELECT page_name FROM wiki_pages
WHERE frontmatter->>'plugin' = $plugin_name;
```

Then for each page:
1. Read file from `<wiki_path>/<page>`
2. Write to `<wiki_path>/_archived/<plugin_name>/<page>`
3. Append frontmatter update: `archived = true`, `archived_at = now`, `archived_reason = "plugin disabled"`
4. Git commit: `"archive: <plugin_name> (plugin disabled)"`
5. `DELETE FROM wiki_pages WHERE frontmatter->>'plugin' = $plugin_name` + reindex new archived paths

---

## 6. Crate boundary changes

### Current (after PR #31 on main)

```
crates/
├── gadgetron-core
├── gadgetron-gateway
├── gadgetron-kairos
├── gadgetron-knowledge
├── gadgetron-web
├── gadgetron-xaas
├── gadgetron-router
├── gadgetron-provider      ← AI-infra-specific
├── gadgetron-scheduler     ← AI-infra-specific
├── gadgetron-node          ← AI-infra-specific
└── gadgetron-cli
```

### Target (P2B end)

```
crates/
├── gadgetron-core          (BackendPlugin trait + PluginRegistry here)
├── gadgetron-gateway
├── gadgetron-kairos
├── gadgetron-knowledge     (ADR-P2A-07 pgvector + embedding)
├── gadgetron-web
├── gadgetron-xaas          (tenants/audit only; model-catalog moved)
├── gadgetron-router
└── gadgetron-cli           (plugin registry wiring, built-in plugins compiled in)

plugins/                    (new top-level directory)
└── plugin-ai-infra/        (one crate per plugin)
    ├── src/
    │   ├── lib.rs          (impl BackendPlugin)
    │   ├── provider/       (moved from gadgetron-provider)
    │   ├── scheduler/      (moved from gadgetron-scheduler)
    │   ├── node/           (moved from gadgetron-node)
    │   ├── catalog/        (moved from gadgetron-xaas)
    │   └── mcp_tools/      (new: adapters exposing MCP tools for the above)
    ├── seed_pages/
    │   ├── getting-started.md
    │   ├── runbooks/h100-boot-failure.md
    │   └── ...
    └── Cargo.toml
```

`gadgetron-provider`, `-scheduler`, `-node` cease to exist as top-level crates in P2B. Their modules move under `plugins/plugin-ai-infra/src/`.

### Linking model (P2A+ → P2B)

- **P2A+**: Plugins are compiled-in (static linking). `gadgetron-cli` depends on each plugin crate and registers them explicitly in `main()`. Simplest. No dynamic loading complexity.
- **P2B+ (optional)**: Explore dynamic loading via `cdylib` + `libloading`, or out-of-process plugins via stdio IPC (like MCP itself). Not required for the second-plugin milestone.

---

## 7. Frontmatter & wiki integration

See `docs/design/phase2/05-knowledge-semantic.md §4` for the complete schema including the `plugin` and `plugin_version` fields added in this document's context.

Key new patterns:

- **`source = "plugin_seed"`** identifies seeded knowledge.
- **`plugin = "<name>"`** anchors uninstall/archive filtering.
- **`plugin_version = "<semver>"`** drives seed-refresh decisions.
- Seed pages live anywhere in the wiki tree. **Convention** is `<plugin_name>/...` but not enforced. This matches the ADR-P2A-07 "convention-over-schema" principle.

---

## 8. Cross-domain query flow (the JARVIS value)

The user's vision is JARVIS. The architectural payoff of "one shared wiki + many pluggable capabilities" shows up in queries like:

> "지난주 H100 부팅 실패랑 이번 CI 배포 실패가 관련 있는 것 같은데 확인해봐."

Flow:

1. **Kairos receives the query.** Model reasons about domains involved: ai-infra + cicd.
2. **Hybrid search** (one call, both domains):
   - Semantic: embedding of "부팅 실패 배포 시기" matches both ai-infra incident pages AND cicd deploy-log pages.
   - Keyword: "H100", "deploy", "CI" hit both.
   - RRF fuses → ranked list across domains.
3. **Tool calls** (plugin-specific):
   - `mcp__ai-infra__gpu.stats` (ai-infra plugin)
   - `mcp__cicd__deploy.log` (cicd plugin, future)
   - Kairos calls both sequentially based on what the wiki pages reference.
4. **Synthesis.** Kairos combines retrieved knowledge + live data in the user's language.
5. **Knowledge capture.** If the conclusion is new, `wiki.write` to `decisions/cross/<date>-gpu-vs-ci.md` with `tags = ["ai-infra", "cicd"]`.

This is **impossible** with namespaced-per-plugin wikis. The shared wiki is the architectural linchpin.

---

## 9. Migration plan

### P2A (current, DONE)

- Kairos, knowledge, gateway, web all shipped as one binary.
- AI-infra code sits in separate crates but statically linked into binary.
- PR #31 on main delivered the demo-capable integration.

### P2A+ (proposed, ~1 week after ADR-P2A-07 implementation)

- **Add `BackendPlugin` trait + `PluginRegistry` to `gadgetron-core`.** No crate moves yet.
- **`gadgetron-cli::main`** wraps existing AI-infra wiring in a `BuiltInAiInfraPlugin` that impls `BackendPlugin`.
- **`[plugins]` config section** added. `enabled = ["ai-infra"]` default. Legacy flat config still accepted.
- **CLI**: `gadgetron plugins list` / `gadgetron plugins status` subcommands.
- No functional change for users. Architecture gains the seam.

### P2B (after ADR-P2A-07 and plugin seam)

- **Extract**: move `gadgetron-provider`, `-scheduler`, `-node`, AI-specific parts of `-xaas` into `plugins/plugin-ai-infra/`.
- **Seed pages**: write initial ai-infra getting-started + key runbooks as plugin seed content.
- **Second plugin** as proof: pick a small one (e.g., `plugin-filesystem` exposing `fs.read`, `fs.search` MCP tools; or `plugin-calendar`). Validates the seam isn't ai-infra-shaped.
- **Multi-wiki registry** (`WikiScope::{Private, Team, Public, Plugin { owner }}`) lands here.

### P2C

- **Multi-tenant** tenant_id ACL on wiki scopes.
- **External plugins** (dynamic loading or out-of-process IPC).
- **Cross-tenant knowledge sharing** policies.

---

## 9.5. Kairos brain ↔ ai-infra provider seam (Sprint B1 prep)

User-flagged gap (2026-04-16 session): 
> Kairos의 endpoint와 ai-infra provider는 별개로 관리가 되어야 할 것 같은데, 맞나?

### Current state

Two parallel concepts in `gadgetron.toml` that don't yet cross-reference:

| Concept | Where | What it controls |
|---|---|---|
| Kairos brain | `[agent.brain]` | Which LLM endpoint Claude Code reasons through |
| LLM providers | `[providers.*]` | What raw models `/v1/chat/completions` exposes to callers |

`BrainMode` today: `ClaudeMax | ExternalAnthropic | ExternalProxy | GadgetronLocal` — none of them reference `[providers.*]`. To point Kairos at a local vLLM, the operator currently has to run a separate LiteLLM-style proxy so the provider's OpenAI-compat endpoint is rewrapped as Anthropic-compat.

### Proposed addition

```rust
pub enum BrainMode {
    ClaudeMax,
    ExternalAnthropic,
    ExternalProxy,
    GadgetronLocal,
    UseProvider,  // NEW
}

pub struct BrainConfig {
    pub mode: BrainMode,
    pub provider: Option<String>,  // NEW — name of [providers.<name>] to use as brain
    // ... existing fields ...
}
```

Config usage:

```toml
[agent.brain]
mode = "use_provider"
provider = "anthropic-haiku"

[providers.anthropic-haiku]
type = "anthropic"
api_key = "env:ANTHROPIC_API_KEY"
models = ["claude-3-5-haiku-20241022"]
```

### Validation rules (to add)

- **V23**: `brain.mode = "use_provider"` ⇒ `brain.provider` is Some.
- **V24**: `brain.provider` name exists in `providers`.
- **V25**: Referenced provider's `type` is `"anthropic"` (or future compatible types). Other types require `external_proxy` + an operator-run translation proxy.

### Plugin-architecture implication

When `ai-infra` becomes a plugin (P2B), LLM providers move into `plugin-ai-infra`. The core Kairos (still in the binary) resolves the `brain.provider` reference via the generic `LlmProvider` registry maintained by core. The plugin contributes entries; Kairos consumes one.

This is the **core principle**: Kairos is core (identity travels with the product), ai-infra providers are plugin-scoped (domain-specific). The brain-provider seam is the bridge.

### Why deferred

Implementing this requires:
- Provider config type discrimination (today only `anthropic` type could be a brain; `vllm`/`ollama`/`openai` need a translation layer).
- Config validation wiring through `AppConfig::validate()`.
- `spawn.rs` brain resolution: lookup provider → extract endpoint + auth → set `ANTHROPIC_BASE_URL` + `ANTHROPIC_API_KEY` env.

Estimated ~200 lines including tests. Scheduled for Sprint B3 after ADR-P2A-07 (semantic wiki + DB required) lands. Postgres dependency of B2 is the higher-priority unblock.

### Acceptance criteria (when implemented)

- `[agent.brain] mode = "use_provider", provider = "foo"` where `[providers.foo] type = "anthropic"` works end-to-end.
- Non-anthropic provider types fail validation with a clear message pointing at `external_proxy`.
- `docs/manual/kairos.md` gains a section "Using an ai-infra provider as Kairos's brain".

---

## 10. Open questions

1. **Dynamic vs static linking (P2B+ decision)** — static is simpler, dynamic enables third-party plugins. Defer to after second-plugin extraction.
2. **Plugin config validation** — each plugin owns its `[plugins.<name>]` schema validation. Should the core do sanity checks (e.g., `enabled` list matches compiled-in plugins)? Propose: yes, at startup.
3. **Plugin version / core version compatibility** — P2B+. Semver compat matrix or capability-flag negotiation.
4. **Seed page update semantics** — if operator has edited a seeded page, and the plugin upgrades, do we overwrite? Propose: never overwrite operator edits; emit `gadgetron wiki audit`-style warning.
5. **LLM provider plugins** — the ai-infra plugin currently contributes LLM providers (vllm, sglang, etc.). Is this a plugin concern or a separate concept? Propose: plugins CAN register providers via `PluginContext::register_llm_provider`, but Kairos itself (embedded in core) is NOT a plugin. Kairos is part of the core identity.
6. **Observability conventions** — plugin-emitted traces/metrics should carry `plugin = "<name>"` span attribute. Standardize in core.

---

## 11. Out of scope

- **Not a package manager.** P2A-P2C do not include plugin installation/discovery/marketplace. Plugins are compiled-in crates the operator chooses at build time.
- **Not a sandbox.** Plugins run in the same process with full access. No capability-based security. Revisit in P3.
- **Not an API gateway for non-Kairos clients.** Plugins expose MCP tools for Kairos; they may optionally expose HTTP routes under `/api/v1/plugins/<name>/*`, but the primary surface is Kairos.

---

## 12. Review provenance

- User direction (2026-04-16 session): "구조상 Gadgetron의 AI 인프라 관리 기능은 바이너리에서 분리해야할 것 같죠? 백엔드 플러그인으로." → confirmed direction.
- Follow-up: "지식이랑 기능이랑 연동이 되어 있는데 이것이 어떻게 관리가 될까요" → §4 answered.
- Follow-up: "그런데 결국엔 아이언맨의 JAVIS를 만들꺼에요" → framing anchor for "one shared memory" architecture.
- ADR-P2A-07 cross-reference: semantic wiki makes cross-domain retrieval performant, which is the load-bearing assumption of §8.

---

*End of 06-backend-plugin-architecture.md draft v1.*
