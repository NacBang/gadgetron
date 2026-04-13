# Gadgetron

Rust-native GPU/LLM orchestration platform with sub-millisecond P99 gateway overhead.

## Features

- **OpenAI-compatible API** — drop-in `/v1/chat/completions` with SSE streaming
- **6 LLM providers** — OpenAI, Anthropic, Gemini, Ollama, vLLM, SGLang
- **6 routing strategies** — RoundRobin, CostOptimal, LatencyOptimal, QualityOptimal, Fallback, Weighted
- **GPU-aware scheduling** — VRAM bin-packing, NUMA topology, MIG support
- **Multi-tenant platform** — API key auth, per-tenant quota (i64 cents), audit logging
- **Single binary** — `gadgetron serve` runs the full stack

## Quick Start

```bash
# Prerequisites: Docker, an OpenAI API key

# 1. Start PostgreSQL
docker run -d --name gadgetron-db \
  -e POSTGRES_USER=gadgetron -e POSTGRES_PASSWORD=gadgetron -e POSTGRES_DB=gadgetron \
  -p 5432:5432 postgres:16

# 2. Start Gadgetron (run once to apply migrations, then Ctrl-C)
export OPENAI_API_KEY="sk-your-key"
export GADGETRON_DATABASE_URL="postgresql://gadgetron:gadgetron@localhost:5432/gadgetron"
cargo run -- serve

# 3. Create tenant + API key via SQL
# Note: CLI key management (`gadgetron key create`) is coming in a future sprint.
# For now, insert directly into PostgreSQL. See docs/manual/quickstart.md Step 4
# for the full instructions including key hashing.
docker exec -i gadgetron-db psql -U gadgetron -d gadgetron <<'SQL'
INSERT INTO tenants (id, name, status)
VALUES ('00000000-0000-0000-0000-000000000001', 'dev-team', 'Active');

-- Replace key_hash with: echo -n 'gad_live_YOUR_SECRET' | sha256sum | cut -d' ' -f1
INSERT INTO api_keys (tenant_id, prefix, key_hash, kind, scopes, name)
VALUES (
  '00000000-0000-0000-0000-000000000001',
  'gad_live',
  'PASTE_YOUR_64_CHAR_SHA256_HASH_HERE',
  'live',
  ARRAY['OpenAiCompat'],
  'dev-key'
);
SQL

# 4. Send first request
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer gad_live_..." \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Hello!"}]}'
```

## Architecture

```
gadgetron (single binary)
├── gadgetron-core       — shared types, traits, errors (leaf crate)
├── gadgetron-provider   — 6 LLM provider adapters
├── gadgetron-router     — 6 routing strategies + MetricsStore
├── gadgetron-gateway    — axum HTTP server + Tower middleware + SSE
├── gadgetron-scheduler  — VRAM bin-packing + LRU eviction
├── gadgetron-node       — NodeAgent + GPU ResourceMonitor
├── gadgetron-xaas       — auth, tenant, quota, audit (PostgreSQL)
├── gadgetron-testing    — mocks, fakes, test harnesses
├── gadgetron-tui        — Ratatui terminal dashboard
└── gadgetron-cli        — CLI entry point
```

## Development

```bash
# Run all tests
cargo test --workspace

# Check formatting + lints
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Security scan
cargo audit
cargo deny check licenses bans advisories
```

## Design Documents

**Approved / Implemented — Phase 1** (backing code merged to `main`):

| Document | Status |
|----------|--------|
| [Platform Architecture v1](docs/architecture/platform-architecture.md) | Approved (7300+ lines, 4 rounds + Phase C consolidation) |
| [Core Types Consolidation](docs/design/core/types-consolidation.md) | Round 3 Approved — Implemented |
| [XaaS Phase 1](docs/design/xaas/phase1.md) | Round 3 Approved — Implemented |
| [Testing Harness](docs/design/testing/harness.md) | Round 3 Approved — Implemented |
| [Gateway Wire-up (Sprint 3)](docs/design/gateway/wire-up.md) | Implemented (code-led; retroactive review log) |
| [Sprint 4 — vLLM + SGLang](docs/design/provider/sprint4-provider-activation.md) | Implemented |
| [Sprint 5 — E2E + bench + TUI](docs/design/testing/sprint5-e2e-bench-tui.md) | Implemented |
| [Sprint 6 — TUI live hardening](docs/design/hardening/sprint6-tui-live-hardening.md) | Implemented |
| [Sprint 7 — CLI usability + no-db](docs/design/usability/sprint7-cli-init-nodb.md) | Implemented |
| [Sprint 8 — CLI/bench/TUI/Gemini](docs/design/polish/sprint8-cli-bench-tui-gemini.md) | Implemented |
| [Sprint 9 — Node process lifecycle](docs/design/node/sprint9-process-lifecycle.md) | Implemented |

**Phase 2 — Draft v3** (Round 2 cross-review addressed, implementation not started):

| Document | Status |
|----------|--------|
| [Phase 2 Overview (하방/상방 reframe)](docs/design/phase2/00-overview.md) | Draft v3 (Round 2 reviewed by 4 agents) |
| [Knowledge Layer spec](docs/design/phase2/01-knowledge-layer.md) | Draft v3 (`gadgetron-knowledge`) |
| [Kairos Agent Adapter spec](docs/design/phase2/02-kairos-agent.md) | Draft v3 (`gadgetron-kairos`) |
| [Phase 2A WBS](docs/design/phase2/03-p2a-wbs.md) | Approved WBS (4-week breakdown + gates) |

ADRs: [P2A-01 allowed-tools enforcement](docs/adr/ADR-P2A-01-allowed-tools-enforcement.md), [P2A-02 dangerously-skip-permissions risk](docs/adr/ADR-P2A-02-dangerously-skip-permissions-risk-acceptance.md), [P2A-03 SearXNG privacy](docs/adr/ADR-P2A-03-searxng-privacy-disclosure.md).

## Sprint Progress — Phase 1 (complete)

| Sprint | Scope | Commit |
|:---:|---|---|
| 1 | Core types (`GadgetronError`, `Secret`, `TenantContext`, `ProcessState` FSM) | `6060b32` |
| 2 | XaaS (PostgreSQL schema, `PgKeyValidator`, `QuotaEnforcer`, `AuditWriter`) | rolled into Sprint 3 |
| 3 | Gateway wire-up (`IntoResponse`, middleware chain, SSE) | `6c80faa` |
| 4 | vLLM + SGLang provider activation | `d53aa60` |
| 5 | E2E harness (7 scenarios) + criterion benchmarks + TUI wiring | `106966e` / `f6a2b0d` |
| 6 | TUI live gateway, clap CLI, `/ready` PG check, graceful shutdown | `b06511b` |
| 7 | CLI usability (`tenant` / `key` / `init` / `--no-db` / `doctor`) | `d7da479` |
| 8 | CLI completion, Criterion benches, TUI nav, Gemini provider | `4170891` |
| 9 | NodeAgent process lifecycle + Scheduler deploy + eviction | `02218b9` |
| hotfix | PRs #7–#10 (streaming latency, 401/413 OpenAI shape, TTY pre-check, manual sync) | `5655e5d`..`345f433` |
| P1.5 | Phase 2 design docs + ADRs + Phase 2 error variants in core | `59859b1`..`97bcf75` |

**Test inventory** (source attributes): **213** `#[test]` / `#[tokio::test]` across the workspace — 158 unit (in-crate `#[cfg(test)]`), 48 gateway/middleware, 7 e2e integration scenarios. See `cargo test --workspace` for the definitive runner count.

**Benchmarks**: `gadgetron-gateway` criterion suite — `auth_cache.rs`, `middleware_chain.rs`, `router.rs`. Phase 1 manual QA: P99 resolve 65 ns, middleware chain 50 µs, 20–15000× SLO headroom.

## Team

PM-led 10-agent architecture:

| Agent | Domain |
|-------|--------|
| chief-architect | Core types, cross-crate consistency |
| gateway-router-lead | HTTP gateway, routing, SSE |
| inference-engine-lead | Provider adapters, protocol translation |
| gpu-scheduler-lead | VRAM scheduling, NVML, MIG |
| xaas-platform-lead | Multi-tenancy, billing, audit |
| devops-sre-lead | Deployment, CI/CD, observability |
| ux-interface-lead | TUI, Web dashboard |
| qa-test-architect | Test strategy, mocks, benchmarks |
| security-compliance-lead | Threat modeling, OWASP, compliance |
| dx-product-lead | CLI UX, error messages, documentation |

## License

MIT
