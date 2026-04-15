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

# 3. Create tenant + API key
cargo run -- tenant create --name dev-team
# Copy the printed tenant UUID into:
cargo run -- key create --tenant-id <tenant-uuid>

# Optional: local development without PostgreSQL-backed auth
# cargo run -- key create

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

| Document | Status |
|----------|--------|
| [Platform Architecture](docs/architecture/platform-architecture.md) | Draft (Phase C review pending) |
| [XaaS Phase 1](docs/design/xaas/phase1.md) | Approved (4 rounds, 23 fixes) |
| [Gateway Wire-up](docs/design/gateway/wire-up.md) | Draft (Round 0 self-check; Round 1+ pending) |
| [Core Types](docs/design/core/types-consolidation.md) | Round 3 Approved |
| [Testing Harness](docs/design/testing/harness.md) | Round 2 retry |

`docs/manual/*` documents the current Phase 1 binary. `docs/design/phase2/*` documents planned Kairos work and is not implemented yet.

## Sprint Progress

| Sprint | Scope | Tests |
|--------|-------|-------|
| 1 | Core types (GadgetronError, Secret, TenantContext, ProcessState FSM) | 35 |
| 2 | XaaS (PostgreSQL schema, PgKeyValidator, QuotaEnforcer, AuditWriter) | 17 |
| 3 | Gateway wire-up (IntoResponse, middleware, SSE) | 14 |
| 4 | vLLM + SGLang provider activation | included above |
| 5 | E2E harness, criterion benchmarks, TUI wiring | included above |
| 6 | TUI live hardening (broadcast channel, /ready PG check, graceful shutdown) | included above |
| **Total** | | **~100 passed** |

Current workspace state is larger than the historical sprint table above: the repository has 200+ tests today, and full E2E coverage requires PostgreSQL to be available.

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
