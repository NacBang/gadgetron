# Gadgetron Glossary

> **Status**: canonical. All design docs link here on first use of an extension term.
> **Authority**: ADR-P2A-10 fixes the Bundle / Plug / Gadget trinity.
> **Update policy**: a new term lands here only when a new ADR introduces it.

---

## Core trinity

### Bundle

A **Bundle** is the unit of distribution, versioning, and lifecycle. Operators `install`, `enable`, `disable`, and `uninstall` Bundles. Examples: `ai-infra`, `graphify`, `document-formats`, `blob-s3`.

A Bundle provides:

- **zero or more Plugs** (Rust trait implementations consumed by core), and
- **zero or more Gadgets** (MCP tools consumed by Penny).

A Bundle is either **Rust-native** (compiled into the `gadgetron` binary or a dylib) or **external** (pip / npm / docker / binary URL, installed at runtime into `~/.gadgetron/bundles/<name>/`).

Every Bundle owns:

- a name (kebab-case, globally unique within a Gadgetron deployment)
- a version (semver)
- a license
- an install method (compile-time / pip / npm / docker / binary-url)
- a manifest (compile-time `Bundle::manifest()` for Rust-native; `bundle.toml` file for external)
- a `[bundles.<name>]` config subsection in `gadgetron.toml`
- optional seed pages (markdown written to the wiki on first enable)
- a `DisableBehavior` (what happens to seed pages on disable)

Rust entry-point trait: **`Bundle`**. See `gadgetron-core::bundle`.

### Plug

A **Plug** is a Rust trait implementation consumed by core modules. Plugs are invisible to Penny — they extend the system's capability surface at the type level.

Examples:

- `OpenAiLlmProvider`, `AnthropicLlmProvider`, `VllmLlmProvider` — Plugs for the `LlmProvider` trait, consumed by the router
- `PdfExtractor`, `DocxExtractor`, `WhisperExtractor` — Plugs for the `Extractor` trait, consumed by the wiki import pipeline
- `S3BlobStore`, `AzureBlobStore` — Plugs for the `BlobStore` trait, consumed by the ingestion pipeline
- `OpenAiCompatEmbedder`, `OllamaEmbedder` — Plugs for the `EmbeddingProvider` trait, consumed by the wiki chunk encoder
- `VramBinPackScheduler`, `WeightedLruScheduler` — Plugs for the `Scheduler` trait, consumed by the node scheduler
- `NvmlNodeMonitor` — Plug for the `NodeMonitor` trait, consumed by the node registry
- `GpuEntityKind`, `ModelEntityKind` — Plugs registered against the `EntityKind` registry

Plugs are typed, compile-time (or dylib-loaded), hot-path capable. They run in the same process as core.

Plug registration happens inside `Bundle::install`:

```rust
impl Bundle for AiInfraBundle {
    fn install(&mut self, ctx: &mut BundleContext<'_>) -> Result<()> {
        ctx.plugs.llm_providers.register("openai",   Arc::new(OpenAi::new(&cfg.openai)));
        ctx.plugs.llm_providers.register("vllm",     Arc::new(Vllm::new(&cfg.vllm)));
        ctx.plugs.schedulers.register  ("vram-lru", Arc::new(VramBinPackScheduler::new(&cfg.sched)));
        ctx.plugs.entity_kinds.register(gpu_kind_spec());
        Ok(())
    }
}
```

### Gadget

A **Gadget** is an MCP tool consumed by Penny (the LLM agent). Gadgets appear in Penny's system prompt as callable tools with JSON schema.

Examples:

- `wiki.write`, `wiki.search`, `wiki.import` — core-shipped Gadgets
- `web.search`, `web.fetch` — core Gadgets (web-scrape Bundle supplies the underlying Plug)
- `graph.query_graph`, `graph.god_nodes`, `graph.shortest_path` — graphify Bundle Gadgets
- `gpu.list`, `model.load`, `model.unload`, `scheduler.stats` — ai-infra Bundle Gadgets
- `build.deploy`, `deploy.rollback` — cicd Bundle Gadgets (future)

Every Gadget has:

- a name (dotted, e.g. `namespace.action` — the namespace is declared by the Bundle that supplies it)
- a JSON schema for arguments
- a JSON schema for results
- a **tier** (`GadgetTier::{Read, Write, Destructive}`) — what class of effect the Gadget can cause
- a **mode** (`GadgetMode::{Auto, Ask, Never}`) — policy for this deployment; may be overridden per-tenant
- a **runtime** — one of `InCore`, `InBundle`, `Subprocess`, `Http`, `Wasm` (see §Runtime below)

Rust trait: **`GadgetProvider`**. See `gadgetron-core::gadget`.

Gadget registration happens inside `Bundle::install` via `ctx.gadgets.*` (field form, consistent with `ctx.plugs.*` per ADR-P2A-10-ADDENDUM-01 rev4):

```rust
ctx.gadgets.knowledge.register(Arc::new(WikiListGadget::new(wiki.clone())));
ctx.gadgets.infra.register(Arc::new(GpuListGadget::new(nvml.clone())));
```

`ctx.plugs` and `ctx.gadgets` are both plain fields on `BundleContext`. Borrow-checker permits disjoint simultaneous `&mut` on these sibling fields per standard NLL rules.

---

## Runtime variants (for Gadgets)

A Gadget's `runtime` determines how its invocation is dispatched. The runtime is declared in the Bundle manifest and enforced by `gadgetron-penny::gadget_registry` at freeze time.

| Runtime | Where the code lives | Isolation | Typical use |
|---|---|---|---|
| `InCore` | The `gadgetron` binary itself | Same process, same Arc hierarchy | Core-shipped Gadgets (`wiki.write`) |
| `InBundle` | A Rust-native Bundle compiled into `gadgetron` | Same process, module boundary | `ai-infra` `gpu.list` |
| `Subprocess` | An external process spawned per-call or long-lived | OS process + tenant workdir bind + rlimits | graphify, whisper-mcp, PaddleOCR-mcp |
| `Http` | An external HTTP MCP server | Network boundary + bearer auth | Remote / SaaS Gadgets |
| `Wasm` | A compiled wasm module | `wasmtime` sandbox | Future — stateless tool only |

`Subprocess` is the first external runtime supported; the others arrive as feature gates. Subprocess details (lifecycle, manifest, tenant isolation, audit bridge) are specified in `docs/design/phase2/12-external-gadget-runtime.md`.

---

## Co-occurrence pattern — same capability on both axes

A capability can legitimately appear on **both** axes without duplication of logic. Canonical examples:

- **`scheduler.stats`** — `Scheduler::stats() -> Stats` is a Plug method consumed by the TUI, metrics endpoint, and health checks; `SchedulerStatsGadget` is a ~20-line Gadget that wraps the same call and exposes it to Penny as an MCP tool. One service, two consumers.
- **`gpu.list`** — `NodeMonitor::list_gpus() -> Vec<Gpu>` is a Plug method consumed by the scheduler's VRAM bin-packer; `GpuListGadget` wraps the same call for Penny. Same data, two views.
- **`wiki.import`** — the `Extractor` Plug mechanizes PDF/docx parsing; the `wiki.import` Gadget orchestrates extract → blob → chunk → embed. Plug does the work; Gadget is the agent-facing front door. Not duplication — different layers.

**Pattern rule:** when a capability needs to be visible to both core consumers and Penny, implement the **service as a Plug** (Rust trait + impl), then add a **thin Gadget** that wraps the same service call with JSON schema dispatch. The Gadget should hold no business logic beyond argument validation and serde translation — if the Gadget grows its own logic, that logic belongs in the Plug.

This is standard layering. It is **not** the two-wrappers-of-same-logic anti-pattern Codex warned about in ADR-P2A-10 review (confirmed by chief-architect's 2026-04-18 case-study — no actual duplication in current or planned Gadgetron code).

---

## Sub-pattern: Port and Adapter (for reviewers familiar with hexagonal architecture)

For readers coming from [hexagonal architecture](https://alistair.cockburn.us/hexagonal-architecture/):

- A **Port** is a Rust trait that core defines (e.g., `trait LlmProvider`). It names "what the core needs."
- An **Adapter** is an implementation of a Port (e.g., `struct OpenAiLlmProvider`). It names "who answers the need."

A Bundle's Plug surface is **a set of Adapters for core Ports**. The `Plug` name captures the "fits into a socket" metaphor one level up — an operator thinks of a Plug going into a core capability slot without needing to learn the hexagonal vocabulary. Internally, within `gadgetron-core`, the trait definitions carry the `Port` concept implicitly through the trait bound; `Adapter` stays as an advanced-reader term for trait-level evolution discussions (adding a new Port is a core contract change; adding a new Adapter is a Bundle change).

This sub-pattern is useful when discussing trait evolution (adding a new Port is a core breaking change; adding a new Adapter is a Bundle change).

---

## Related terms

### Penny

The LLM agent. Default implementation is Claude Code CLI + Claude Opus via OAuth (`claude_max`). Penny consumes Gadgets via MCP. Penny does not see Plugs.

Spec: `docs/design/phase2/02-penny-agent.md`.

### Core

The always-present set of Gadgetron crates that ship in the single binary by default:

- `gadgetron-core` — types, traits, config, errors
- `gadgetron-gateway` — HTTP entry, SSE streaming, auth
- `gadgetron-router` — LLM routing strategies
- `gadgetron-penny` — agent runtime, Gadget registry
- `gadgetron-knowledge` — wiki, chunking, embedding, pgvector search
- `gadgetron-xaas` — multi-tenant, billing, audit
- `gadgetron-web` — embedded Web UI
- `gadgetron-cli` — CLI entry points
- `gadgetron-tui` — terminal UI
- `gadgetron-testing` — shared test harness

Everything outside `crates/gadgetron-*` is a Bundle.

### Seed page

A markdown page that a Bundle writes into the wiki on first enable. Used to document the Bundle's tools, seed runbooks, or provide examples. Subject to the same security pipeline as any `wiki.write` (frontmatter validation, path traversal guard, credential BLOCK/AUDIT).

Frontmatter on a seed page:

```toml
source          = "plugin_seed"    # deprecated spelling, renames to "bundle_seed" per ADR-P2A-10 §Rename
plugin          = "ai-infra"       # deprecated, renames to `bundle`
plugin_version  = "0.2.0"          # deprecated, renames to `bundle_version`
```

The frontmatter fields rename follows the same migration pattern as `[penny]` → `[agent.brain]` (transparent rewrite at load time with `tracing::warn!`).

### DisableBehavior

What happens to a Bundle's seed pages when the Bundle is disabled:

- `KeepKnowledge` (default) — pages stay. JARVIS principle: knowledge outlives capability.
- `ArchiveKnowledge` — pages move to `_archived/<bundle_name>/`.
- `PromptOperator` — ask the operator at disable time.

### MCP

**Model Context Protocol** — the wire-level JSON-RPC protocol Gadgets use to communicate with Penny. Gadgets are the Gadgetron concept; MCP is how they are transported.

See: [modelcontextprotocol.io](https://modelcontextprotocol.io).

---

## Anti-patterns (things to avoid saying)

| Avoid | Say instead | Why |
|---|---|---|
| "plugin" | Bundle / Plug / Gadget (pick the one you mean) | Ambiguous — see ADR-P2A-10 §Context |
| "MCP tool" | Gadget | "MCP tool" conflates wire protocol with domain concept |
| "extension" | Bundle (distribution) / Plug (core cap) / Gadget (Penny cap) | Too generic |
| "tool" (alone) | Gadget | Overloaded with CLI tool, build tool, MCP tool |
| "dock" | Bundle (distribution) / external Gadget runtime (hosting) | Discarded per ADR-P2A-10 §Alternatives — semantic inversion + Docker confusion |
| "backend plugin" | Bundle | Legacy term; `BackendPlugin` trait is now `Bundle` |
| "tool provider" | `GadgetProvider` or simply "Bundle that provides Gadget X" | Legacy Rust symbol |

---

## Runtime-time vocabulary (ADR-P2A-10-ADDENDUM-01 rev3)

### PlugId

Newtype wrapper over `Arc<str>` for kebab-case-validated Plug identifiers.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlugId(pub(crate) Arc<str>);
```

Used throughout `BundleContext::plugs.*.register(id, plug)`, `BundleContext::is_plug_enabled(&id)`, `BundleContext::require_plugs(&[id])`, and `bundle.toml` `requires_plugs: HashMap<GadgetName, Vec<PlugId>>`. The `Arc<str>` internal makes per-call clone essentially free (increment ref count); kebab-case validation at `PlugId::new()` catches malformed identifiers at the config-parser boundary. Stringly-typed `&str` parameters accepted only at the config-parse boundary via `BundleContext::is_plug_enabled_by_name(s: &str)`.

### RegistrationOutcome

`#[must_use]` enum returned by every `PlugRegistry::register(...)` call, making silent-skip auditable at compile time.

```rust
#[must_use = "ignoring a RegistrationOutcome hides whether the plug was actually wired"]
pub enum RegistrationOutcome {
    Registered,
    SkippedByConfig,        // per-Plug enable = false
    SkippedByAvailability,  // requires_plugs unsatisfied
}
```

Preferred over `Result<(), PlugDisabled>` because skip is a policy decision (operator asked), not an error path (nothing to recover from). The `#[must_use]` nudge avoids `?` ceremony in Bundle authors' `install` functions while forcing a compile-time acknowledgment.

### GadgetronBundlesHome

Resolver for the root directory where external Bundle tenant workdirs live. Priority chain (first writable wins):

1. `[bundles] workdir_root = "…"` in `gadgetron.toml` (explicit operator override — recommended for container/K8s)
2. `GADGETRON_BUNDLES_HOME` env var
3. `${GADGETRON_DATA_DIR}/.gadgetron`
4. `~/.gadgetron` (legacy — refused if `$HOME == "/"`)

Startup fails closed if no writable path resolves. Resolution tier is logged via `tracing::info!(target: "gadgetron_config", tier = …, resolved_path = …)`.

Spec: ADR-P2A-10-ADDENDUM-01 §7.

### `requires_plugs` cascade

Per-Gadget declaration in `bundle.toml` naming Plugs a Gadget depends on:

```toml
[bundle.gadgets."model.load"]
requires_plugs = ["anthropic-llm"]
```

Rust type: `HashMap<GadgetName, Vec<PlugId>>`. If any listed Plug is not registered (disabled by config or its Bundle is not enabled), the Gadget is not registered; `tracing::warn!` names both the Gadget and the missing Plug. **Enforcement at registration time, not per-invocation**.

**Completeness lint**: `cargo xtask check-bundles` statically verifies every `ctx.plugs.<port>.get(<id>)` callsite inside a Bundle's code is covered by some Gadget's `requires_plugs` declaration. CI gate. See ADR-P2A-10-ADDENDUM-01 §3.

---

## Decision references

- **ADR-P2A-10** — fixes Bundle / Plug / Gadget. All other term decisions downstream of this.
- **ADR-P2A-10-ADDENDUM-01** (rev3) — RBAC granularity; per-Plug axis; external runtime enforcement floors (7 points); `requires_plugs` cascade; `PlugId`, `RegistrationOutcome`, `GadgetronBundlesHome` types; JSONB runtime metadata; `admin_detail` leak-safety.
- **ADR-P2A-05** — agent-centric control plane; introduced the Tool/Tier/Mode model that becomes Gadget/GadgetTier/GadgetMode.
- **ADR-P2A-07** — semantic wiki + pgvector; introduced `EmbeddingProvider` Plug.
- **ADR-P2A-09** — RAW ingestion pipeline; introduced `Extractor`, `BlobStore` Plugs and `wiki.import` Gadget.
- **D-20260418-01** — backend plugin architecture refinement; the "plugin" term survived here and is retired by ADR-P2A-10.
