# ADR-P2A-10 — Extension terminology: Bundle / Plug / Gadget

| Field | Value |
|---|---|
| **Status** | ACCEPTED (amended same-day — see §Amendment) |
| **Date** | 2026-04-18 |
| **Author** | PM (Claude), user-directed via session discussion |
| **Parent** | `docs/design/phase2/04-mcp-tool-registry.md` v2, `docs/design/phase2/06-backend-plugin-architecture.md` v1, D-20260418-01 |
| **Amends** | 04 v2 (`McpToolProvider` → `GadgetProvider`), 06 v1 (`BackendPlugin` → `Bundle`), 11 v0 (extractor registration path), 07 v1 (bundle-server naming) |
| **Blocks** | All P2B bundle work, `gadgetron install` CLI, `bundle.toml` manifest schema |

---

## Amendment — 2026-04-18 (same-day, before implementation drift)

The first version of this ADR adopted **Bundle / `Driver` / Gadget** (legacy name quoted). After external review (codex-chief-advisor MAJOR finding: `Driver` carries kernel / JDBC / ODBC baggage that implies hardware-compatibility contracts we do not mean), the core-facing axis was renamed from `Driver` to **Plug**. Rationale:

- `Plug` cleanly expresses the electrical / mechanical "fits into a port" metaphor — which is exactly what a Rust trait implementation does against a core-defined trait.
- `Plug` is not overloaded in mainstream Rust, systems, or ML vocabularies.
- `Plug` + `Gadget` reads as a consistent workbench metaphor (plugs into sockets, gadgets in Penny's hand).
- `gadgetron plug list` is one short word — operator CLI stays terse.

All references to "Driver" / "drivers" in the first version of this document have been rewritten to `Plug` / `plugs` in the body below. The file was renamed from `ADR-P2A-10-bundle-driver-gadget-terminology.md` to `ADR-P2A-10-bundle-plug-gadget-terminology.md`. D-entry `D-20260418-05` records the amendment. Downstream artifacts (`docs/architecture/glossary.md`, CLI subcommand `gadgetron plug`, `BundleContext::plugs` field) reflect the final naming.

---

## Context

Gadgetron's existing extension vocabulary is a single word — **"plugin"** — that unintentionally mashes three distinct concerns into one namespace:

1. **`BackendPlugin` trait** (`docs/design/phase2/06-backend-plugin-architecture.md §3`) — a Rust entry-point trait whose `initialize(&mut PluginContext)` method registers Rust trait implementations (`LlmProvider`, `Extractor`, `BlobStore`, `EntityKind`), HTTP routes, and seed pages. These are consumed by the **core-facing runtime / service layer** (gateway, wiki, and Bundle-owned services such as the `ai-infra` router/scheduler).
2. **`McpToolProvider` trait** (`docs/design/phase2/04-mcp-tool-registry.md v2 §3`) — a Rust trait whose impls produce MCP tool schemas (`wiki.write`, `web.search`, future `graph.query_graph`). These are consumed by **Penny** as agent tools.
3. **External utilities** (e.g., [graphify](https://github.com/safishamsi/graphify), Whisper, PaddleOCR) — out-of-process services that expose MCP tools via stdio/HTTP. Not covered by either trait today.

Calling all three "plugins" causes recurring ambiguity in review:

- "Does `plugin-ai-infra` add plugin tools to Penny?" — yes (MCP tools) **and** no (the LlmProvider adapters are invisible to Penny).
- "Can we write a plugin for PDF extraction?" — yes (as an `Extractor` implementation registered by a `BackendPlugin`), but that plugin does not appear in Penny's toolbox; it's consumed internally by `wiki.import`.
- "Is graphify a plugin?" — uncertain; it exposes MCP tools but does not implement any Rust trait.

The core vs. Penny distinction is load-bearing — it determines performance characteristics (hot path vs. agent loop), license constraints (single-binary vs. optional install), and security boundary (in-process vs. subprocess isolation). One word cannot carry three meanings.

Additionally, "single Rust binary" is a property of **Gadgetron core**, not of the ecosystem. External utilities can and should dock onto core to expand capability. The current terminology does not differentiate core capability extension from agent tool provisioning, which muddles the architectural conversation about where to put a new capability.

## Decision

Adopt a three-term extension vocabulary. Each term names one concern, has one consumer, and maps to one implementation concept.

### The three terms

| Term | Consumer | Interface | Runtime |
|---|---|---|---|
| **Bundle** | The operator (install/enable/disable target) | Manifest + entry-point trait | Rust crate or external distribution (pip / npm / docker / binary URL) |
| **Plug** | **Core-facing runtime / service layer** — gateway, wiki, embedding pipeline, and Bundle-owned services such as the `ai-infra` router/scheduler | Rust trait implementation (`impl LlmProvider for …`) | Must be in-process Rust (compile-time or dylib) |
| **Gadget** | **Penny** — LLM agent | MCP tool schema (JSON) + dispatcher | In-core Rust / In-Bundle Rust / external subprocess / external HTTP / wasm |

### Relationship

A **Bundle** is the unit of distribution. Each Bundle provides **zero or more Plugs** (core-facing Rust trait implementations) and **zero or more Gadgets** (Penny-facing MCP tools). A Bundle may provide only Plugs (e.g., an S3 `BlobStore` Plug), only Gadgets (e.g., graphify's 7 graph query Gadgets), or both (e.g., `ai-infra` provides `LlmProvider` Plugs + `gpu.*` Gadgets).

Ownership note: "core-facing" here describes which side of the interface consumes the Plug, not crate ownership. Canonical ownership remains `gateway` in core; `router`, `provider`, and `scheduler` under the `ai-infra` Bundle; and `node` split across `server` / `gpu` / `ai-infra` per D-20260418-01.

### Core vocabulary mapping to Rust symbols

| Concept | Rust symbol |
|---|---|
| Bundle entry-point trait | `Bundle` (was `BackendPlugin`) |
| Context passed to `Bundle::install` | `BundleContext` (was `PluginContext`) |
| Registry of installed Bundles | `BundleRegistry` (was `PluginRegistry`) |
| Trait for Gadget supply | `GadgetProvider` (was `McpToolProvider`) |
| Registry of active Gadgets | `GadgetRegistry` (was `McpToolRegistry`) |
| Builder form pre-freeze | `GadgetRegistryBuilder` (was `McpToolRegistryBuilder`) |
| Gadget permission tier enum | `GadgetTier` (was `Tier`) |
| Gadget invocation mode enum | `GadgetMode` (was `ToolMode`) |
| Penny's stdio MCP server | `gadget_server` module (was `mcp_server`) |

### Plug surface inside `BundleContext`

The Plug registration path becomes explicit — callers see which Plug axis they are touching:

```rust
// Was (06 v1 §3):
ctx.register_extractor(Arc::new(PdfExtractor::new()));
ctx.register_llm_provider(name, Arc::new(OpenAi::new(cfg)));

// Becomes:
ctx.plugs.extractors.register(Arc::new(PdfExtractor::new()));
ctx.plugs.llm_providers.register(name, Arc::new(OpenAi::new(cfg)));
ctx.plugs.blob_stores.register(name, Arc::new(S3BlobStore::new(cfg)));
ctx.plugs.embedders.register(Arc::new(OpenAiCompatEmbedder::new(cfg)));
ctx.plugs.entity_kinds.register(spec);
ctx.plugs.schedulers.register(name, Arc::new(VramBinPackScheduler::new(cfg)));
ctx.plugs.http_routes.mount("/api/v1/gpu", router);
```

Gadget registration stays routed through a dedicated handle:

```rust
ctx.gadgets_mut().register(Arc::new(GpuListGadget::new(nvml.clone())));
ctx.gadgets_mut().register(Arc::new(ModelLoadGadget::new(scheduler.clone())));
```

### CLI

The operator-facing CLI uses the three terms in descending specificity:

```sh
gadgetron bundle install <name>        # install a Bundle
gadgetron bundle enable <name> --scope <scope>
gadgetron bundle list                  # installed Bundles
gadgetron bundle info <name>           # Plugs + Gadgets this Bundle provides
gadgetron bundle search <query>        # catalog lookup
gadgetron bundle disable <name>
gadgetron bundle uninstall <name>

gadgetron plug list                  # all Plugs registered to core ports
gadgetron plug info <port>           # which Plugs fill a given port (e.g. LlmProvider)

gadgetron gadget list                  # all Gadgets Penny can see
gadgetron gadget info <name>           # Gadget schema + source Bundle
gadgetron gadget call <name> --args '{…}'  # debug invocation

gadgetron install <name>               # alias for `gadgetron bundle install <name>`
```

The `gadgetron install <name>` form is the intended everyday invocation; `bundle install` is the canonical long form for docs and scripts.

### Config schema

`gadgetron.toml` uses a `[bundles.<name>]` namespace for Bundle-specific configuration:

```toml
[bundles.ai-infra]
nvml_poll_interval_s = 5

[bundles.ai-infra.gadgets]
# Gadget-level overrides (tier, mode) — documented in 04 v3
"gpu.list"    = { tier = "read",        mode = "auto" }
"model.load"  = { tier = "destructive", mode = "ask"  }

[bundles.graphify]
# Bundle-level config for the external runtime
workdir = "/var/gadgetron/bundles/graphify"
```

The `[plugins.*]` root section is renamed to `[bundles.*]`.

### Bundle manifest file

External Bundles ship a `bundle.toml` at their package root:

```toml
[bundle]
name        = "graphify"
version     = "0.4.21"
license     = "MIT"
homepage    = "https://github.com/safishamsi/graphify"

[bundle.runtime]
kind        = "subprocess"
entry       = "graphify-mcp"
transport   = "mcp-stdio"

[bundle.install]
method      = "pip"
spec        = "graphifyy[all]>=0.4.21"

[bundle.plugs]
# empty — graphify does not provide core Plugs

[bundle.gadgets]
namespace   = "graph"
```

In-Cargo Rust Bundles provide their manifest at compile time via `Bundle::manifest() -> BundleManifest`; no `bundle.toml` file is required.

## Alternatives considered

### Alt 1 — `Dock` + `Plugin`

Operator's initial framing, revised twice during session. Rejected on:

- **`Dock`** inverts natural semantics (a "dock" is where external things plug in, not the core's own extension points) and triggers Docker confusion on first read.
- **`Plugin`** is overloaded (VS Code, WordPress, Obsidian, ChatGPT, Figma, IntelliJ) and collides with existing `BackendPlugin` Rust symbol.

### Alt 2 — `Port` + `Gadget`

Hexagonal-architecture-faithful. `Port` precisely names the trait that core defines; `Adapter` names the implementation a Bundle provides. Rejected on:

- `Port` triggers network-port confusion without glossary support.
- Two-word terminology ("LlmProvider Port, OpenAi Adapter") is verbose in docs and diagrams.
- Engineers unfamiliar with Cockburn's hexagonal pattern need a detour to grok it.

Kept as the **documentation sub-pattern** for advanced readers — the glossary notes that a Plug trait defines a Port and Plug implementations are Adapters.

### Alt 3 — Keep `BackendPlugin` / `McpToolProvider` and add docs to clarify

Zero rename cost but preserves the underlying ambiguity indefinitely. Every future design doc must re-explain which kind of "plugin" it means. Rejected on compounding-confusion grounds — the cost of rename is bounded (~3.5 engineer-days, estimated §Consequences) and paid once; the cost of ambiguity is paid every design review forever.

### Alt 4 — Two separate trait entry points (`PlugProvider` + `GadgetProvider`, no unified Bundle trait)

Removes the need for a single entry-point trait. Rejected on:

- Most Bundles supply both surfaces — requiring two trait implementations per Bundle doubles boilerplate.
- Lifecycle (install / enable / disable / uninstall) is Bundle-level; without a unified trait the lifecycle has no anchor.
- Manifest ownership becomes ambiguous.

### Alt 5 — No Bundle term; name distribution units by content

"`graphify` is a Gadget package"; "`blob-s3` is a Plug package". Rejected on:

- Mixed-content Bundles (like `ai-infra`) have no natural category.
- Version, license, audit attribution, config-section ownership all live at distribution-unit granularity and need a term.
- CLI verb (`gadgetron install …`) needs a noun; without Bundle we end up silently inventing one.

### Alt 6 — `Kit` instead of `Bundle`

"Gadget Kit" pairs pleasantly. Rejected on: "AI-infra Kit" feels forced for mixed-content units, and `Bundle` is more discoverable in Rust ecosystem vocabulary.

## Consequences

### Immediate (before P2B implementation opens)

1. **Rename PR landed** — Rust symbols, doc filenames, doc body terminology all updated per §`Rename scope` below. Executed as six small PRs to keep review tractable.
2. **`docs/architecture/glossary.md` authored** — single source of truth for Bundle / Plug / Gadget definitions plus Port / Adapter sub-pattern. All design docs link to it on first use.
3. **Crate layout reorganized** — canonical ownership is reclassified before the directory move fully lands: `gadgetron-provider`, `gadgetron-router`, and `gadgetron-scheduler` belong to the `ai-infra` Bundle surface, and the engine-facing part of `gadgetron-node` joins that migration cluster while `node` as a whole still splits across `server` / `gpu` / `ai-infra`. During the transition, the current top-level crates remain in `crates/`; canonical core crates are `gadgetron-core`, `gadgetron-gateway`, `gadgetron-penny`, `gadgetron-knowledge`, `gadgetron-xaas`, `gadgetron-web`, `gadgetron-cli`, `gadgetron-tui`, and `gadgetron-testing`.
4. **Design docs refreshed** — `04-mcp-tool-registry.md` → `04-gadget-registry.md`; `06-backend-plugin-architecture.md` → `06-bundle-architecture.md`; `07-plugin-server.md` → `07-bundle-server.md`. Body updated to the new vocabulary. Section numbers preserved where possible to minimize cross-reference churn.
   **Implementation note (2026-04-18):** `07-plugin-server.md` → `07-bundle-server.md` rename landed. `04-mcp-tool-registry.md` and `06-backend-plugin-architecture.md` filesystem renames were intentionally deferred: each file carries a `"legacy filename:"` header note and a canonical-vocabulary banner in lieu of a disk rename, avoiding churn across 20+ cross-references. This is tracked and closed in `docs/reviews/document-consistency-sweep-2026-04-18.md` (C-4). Filenames may be updated in a later cycle when cross-reference tooling is available.
5. **Agent roster updated** — `docs/process/00-agent-roster.md` and persona files in `docs/agents/*.md` replace "plugin" usage with the appropriate term.

### Rename scope (exhaustive)

**Rust symbols** (touched in all `crates/` and `bundles/`):

| Old | New |
|---|---|
| `BackendPlugin` | `Bundle` |
| `PluginContext<'a>` | `BundleContext<'a>` |
| `PluginRegistry` | `BundleRegistry` |
| `McpToolProvider` | `GadgetProvider` |
| `McpToolRegistry` | `GadgetRegistry` |
| `McpToolRegistryBuilder` | `GadgetRegistryBuilder` |
| `ToolSchema` | `GadgetSchema` |
| `ToolResult` | `GadgetResult` |
| `Tier` | `GadgetTier` |
| `ToolMode` | `GadgetMode` |
| `gadgetron-penny::mcp_server` | `gadgetron-penny::gadget_server` |
| `gadgetron-penny::registry` | `gadgetron-penny::gadget_registry` |
| `McpError` (module `gadgetron-core::agent::tools`) | `GadgetError` (module `gadgetron-core::gadget`) |

**Unchanged** (not misnamed, or domain-neutral):

- `DisableBehavior` — applies to Bundles, name is already concern-neutral
- `SeedPage` — applies to Bundles, name is already concern-neutral
- `LlmProvider`, `Extractor`, `BlobStore`, `EntityKind`, `Scheduler`, `EmbeddingProvider` — these are the Plug traits themselves; each is a domain-specific Plug type, so the individual names stay
- `gadgetron mcp serve` stdio subcommand — renamed to `gadgetron gadget serve` (MCP is the wire protocol; Gadget is the payload)

**Config**:

| Old | New |
|---|---|
| `[plugins.<name>]` | `[bundles.<name>]` |
| `[plugins.<name>.tools]` | `[bundles.<name>.gadgets]` |
| `[agent.tools]` | `[agent.gadgets]` |

**CLI**:

- New noun subcommands: `gadgetron bundle|plug|gadget …`
- Alias: `gadgetron install` = `gadgetron bundle install`
- Renamed: `gadgetron mcp serve` → `gadgetron gadget serve` (with a 1-release deprecation shim that prints a migration notice and continues to work)

**Directories**:

```
plugins/              →  bundles/
plugins/plugin-ai-infra/           →  bundles/ai-infra/
plugins/plugin-cicd/               →  bundles/cicd/
plugins/plugin-server/             →  bundles/server/
plugins/plugin-document-formats/   →  bundles/document-formats/
plugins/plugin-web-scrape/         →  bundles/web-scrape/
```

Current top-level crate paths (`crates/gadgetron-*`) remain during the transition. Canonical ownership, however, follows D-20260418-01: `crates/gadgetron-provider`, `crates/gadgetron-router`, and `crates/gadgetron-scheduler` migrate into `bundles/ai-infra/`, while `crates/gadgetron-node` splits across `bundles/server/`, `bundles/gpu/`, and `bundles/ai-infra/`. That migration is a separate PR and is **not** part of this rename (it is functional refactoring that happened to get deferred by the plugin boundary reshuffle in D-20260418-01).

**Design docs**:

| Old | New |
|---|---|
| `docs/design/phase2/04-mcp-tool-registry.md` | `04-gadget-registry.md` |
| `docs/design/phase2/06-backend-plugin-architecture.md` | `06-bundle-architecture.md` |
| `docs/design/phase2/07-plugin-server.md` | `07-bundle-server.md` |
| (new) | `docs/design/phase2/12-external-gadget-runtime.md` |
| (new) | `docs/architecture/glossary.md` |

### Deferred

- **External Gadget runtime spec (subprocess / http / container / wasm)** — full design doc (`12-external-gadget-runtime.md`) is deferred to immediately after this rename lands; it replaces the `DockedUtility` draft from session discussion.
- **Graphify as the external-runtime pilot Bundle** — sequenced after `12-external-gadget-runtime.md` lands.

### Forward compatibility

- `Bundle::manifest()` and `bundle.toml` are versioned (`bundle.manifest_version = 1`). Future additions go behind a version bump.
- The `gadgetron mcp serve` → `gadgetron gadget serve` deprecation shim stays **two releases (v0.3 and v0.4), removed in v0.5**. Extended from the original one-release window after security-compliance-lead review identified operator integration points (systemd units, `.mcp.json`, Docker `CMD`, shell aliases) that need a quarterly change window to migrate. On every `gadgetron mcp serve` invocation the shim prints a `tracing::warn!` naming the replacement command and the ADR reference.

> **Amendment 2026-04-20 (observed outcome)**: v0.5.0 through v0.5.5 shipped without the shim being removed — the `McpCmd::Serve` branch in `crates/gadgetron-cli/src/main.rs` still dispatches to `handle_gadget_serve` with a `tracing::warn!(legacy_command = "gadgetron mcp serve", replacement = "gadgetron gadget serve", ...)` on invocation. Removal is deferred to a later release, tracked as a CLI follow-up. Intent preserved — operators were given the two-release migration window and are now getting a third; deprecation message is unchanged and still points at the replacement command. When the shim is finally removed, the CLI's own wording at `main.rs:444` ("will be removed in v0.5") must be updated to reflect the actual shipping release at that time.
- Config section `[plugins.*]` → `[bundles.*]` is rewritten transparently at `AppConfig::load` time with a `tracing::warn!` per moved section (same pattern as the `[penny]` → `[agent.brain]` migration in ADR-P2A-06). Conflict policy: if both `[plugins.<name>]` and `[bundles.<name>]` are present, the migration returns `GadgetronError::Config` with the actionable wording at `crates/gadgetron-core/src/config.rs:313-317`. Same rule for `[agent.tools]` vs `[agent.gadgets]`.
- **Error code wire freeze**: `GadgetError::error_code()` returns the `mcp_*` code family (`mcp_unknown_tool`, `mcp_denied_by_policy`, `mcp_rate_limited`, `mcp_approval_timeout`, `mcp_invalid_args`, `mcp_execution_failed`). These are persisted in `tool_audit_events.error_code` and consumed by downstream SIEM / BI. The Rust type rename `McpError` → `GadgetError` is safe but the string table is wire-frozen. Regression guard: `gadget_error_codes_are_wire_frozen` test in `agent/tools.rs`.
- **Audit DB column freeze**: `tool_audit_events.tool_name TEXT` column name is wire-frozen. The Rust field `GadgetCallCompleted.gadget_name` is mapped to this column by the P2B DB writer's explicit SQL. Do NOT add a schema migration to rename the column to `gadget_name`.

## References

- Operator session 2026-04-18 (this discussion)
- `docs/architecture/glossary.md` (authored alongside this ADR)
- Cockburn, Alistair. "Hexagonal Architecture" (2005) — Port/Adapter sub-pattern
- Model Context Protocol specification — [modelcontextprotocol.io](https://modelcontextprotocol.io) (wire-level reference for Gadgets)
- [graphify](https://github.com/safishamsi/graphify) — motivating example of an external-Gadget Bundle
