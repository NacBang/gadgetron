# Gadgetron Operator Manual

Gadgetron is a Rust-native API gateway that presents an OpenAI-compatible HTTP interface in front of one or more LLM providers (OpenAI, Anthropic, Ollama). It handles authentication, per-tenant quota enforcement, request routing, and audit logging. It is designed to be self-hosted.

This manual covers the Sprint 1-3 implementation state (Gadgetron v0.1.0, Rust edition 2021, `rust-version = "1.80"`).

---

## Manual pages

| Page | What it covers |
|------|---------------|
| [installation.md](installation.md) | Prerequisites, build from source, Docker (future) |
| [configuration.md](configuration.md) | Environment variables, `gadgetron.toml` reference, provider setup |
| [quickstart.md](quickstart.md) | Zero to first chat completion in 5 minutes |
| [api-reference.md](api-reference.md) | Every endpoint: method, path, auth, request/response, error codes |
| [auth.md](auth.md) | API key format, how auth works, scope system |
| [troubleshooting.md](troubleshooting.md) | Common errors and their fixes |

---

## What Gadgetron is and is not (as of Sprint 1-3)

**Implemented and working:**
- `POST /v1/chat/completions` — non-streaming and SSE streaming
- `GET /v1/models` — lists models from configured providers
- `GET /health`, `GET /ready` — health probes
- Bearer token authentication backed by PostgreSQL
- Per-tenant scope enforcement (`OpenAiCompat`, `Management`, `XaasAdmin`)
- In-memory quota enforcement (daily ceiling check)
- Structured audit log (written to tracing; PostgreSQL batch-insert is Sprint 2+)
- Automatic PostgreSQL schema migrations on startup

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
- Docker image (future)
- Gemini, vLLM, SGLang providers (Phase 1 Week 6+)
