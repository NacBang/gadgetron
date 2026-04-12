# Gadgetron Operator Manual

Gadgetron is a Rust-native API gateway that presents an OpenAI-compatible HTTP interface in front of one or more LLM providers (OpenAI, Anthropic, Ollama, vLLM, SGLang). It handles authentication, per-tenant quota enforcement, request routing, and audit logging. It is designed to be self-hosted.

This manual covers the Sprint 1-6 implementation state (Gadgetron v0.1.0, Rust edition 2021, `rust-version = "1.80"`).

---

## Manual pages

| Page | What it covers |
|------|---------------|
| [installation.md](installation.md) | Prerequisites, build from source, Docker (future) |
| [configuration.md](configuration.md) | Environment variables, `gadgetron.toml` reference, provider setup |
| [quickstart.md](quickstart.md) | Zero to first chat completion in 5 minutes |
| [tui.md](tui.md) | Terminal dashboard: layout, key bindings, color scheme, live gateway connection (`--tui`) |
| [api-reference.md](api-reference.md) | Every endpoint: method, path, auth, request/response, error codes |
| [auth.md](auth.md) | API key format, how auth works, scope system |
| [troubleshooting.md](troubleshooting.md) | Common errors and their fixes |

---

## What Gadgetron is and is not (as of Sprint 6)

**Implemented and working:**
- `POST /v1/chat/completions` — non-streaming and SSE streaming, backed by real LLM providers
- `GET /v1/models` — lists models from configured providers
- `GET /health` — unconditional liveness probe
- `GET /ready` — PostgreSQL pool health check (200 healthy / 503 unhealthy)
- Bearer token authentication backed by PostgreSQL
- Per-tenant scope enforcement (`OpenAiCompat`, `Management`, `XaasAdmin`)
- In-memory quota enforcement (daily ceiling check)
- Structured audit log (written to tracing; PostgreSQL batch-insert is Sprint 2+)
- Automatic PostgreSQL schema migrations on startup
- vLLM provider — tested end-to-end against a live vLLM instance
- SGLang provider — tested end-to-end; supports `reasoning_content` field for reasoning models (e.g. GLM-5.1)
- TUI dashboard (`gadgetron serve --tui`) — 3-column layout (Nodes/Models/Requests), live gateway data via broadcast channel, `metrics_middleware` request forwarding, graceful shutdown with 5-second audit drain
- CLI flags: `--config`, `--bind`, `--tui` (priority: CLI > env > config file > built-in default)
- `gadgetron-testing` crate — `FakeLlmProvider` and `FailingProvider` for use in unit and integration tests

**Stubbed (HTTP 501):**
- `GET /api/v1/nodes`
- `POST /api/v1/models/deploy`
- `DELETE /api/v1/models/{id}`
- `GET /api/v1/models/status`
- `GET /api/v1/usage`
- `GET /api/v1/costs`

**Not yet implemented:**
- CLI subcommands other than `gadgetron serve` (tenant/key management, node management)
- PostgreSQL-backed quota enforcement (Sprint 2)
- Audit log PostgreSQL persistence (Sprint 2+)
- TUI keyboard navigation and scrolling (Sprint 7)
- Docker image (future)
- Gemini provider (Phase 1 Week 6+)
