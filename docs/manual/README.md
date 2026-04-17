# Gadgetron Operator Manual

Gadgetron is a self-hosted Rust-native OpenAI-compatible gateway with optional Phase 2A assistant and collaboration features. It fronts OpenAI, Anthropic, Gemini, Ollama, vLLM, and SGLang providers, and can expose the Penny assistant runtime plus the embedded Web UI when configured.

This manual tracks the operator-facing surface on workspace trunk (`0.2.0`). The historical Phase 1 snapshot remains tagged as `v0.1.0-phase1`; versioning policy is documented in `docs/process/06-versioning-policy.md`.

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
| [penny.md](penny.md) | **Phase 2A**: Penny 협업 에이전트 런타임 (Claude Code + 위키 + SearXNG) — 설치, 설정, 프라이버시 고지, 트러블슈팅 |
| [web.md](web.md) | **Phase 2A**: Gadgetron Web UI — `http://localhost:8080/web` 채팅 UI 설정, Origin 격리, 키 회전, 헤드리스 빌드 |
| [evaluation.md](evaluation.md) | **Phase 2A**: Penny 평가 하네스 — 시나리오 기반 자동 검증 (`eval/run_eval.py`), SearXNG 설정, CI 게이트 연동 |

---

## Current Operator Surface

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
- Gemini provider — request/response adaptation implemented
- vLLM provider — tested end-to-end against a live vLLM instance
- SGLang provider — tested end-to-end; supports `reasoning_content` field for reasoning models (e.g. GLM-5.1)
- TUI dashboard (`gadgetron serve --tui`) — 3-column layout (Nodes/Models/Requests), live gateway data via broadcast channel, `metrics_middleware` request forwarding, graceful shutdown with 5-second audit drain
- CLI flags: `--config`, `--bind`, `--tui`, `--no-db`, `--provider` (priority: CLI > env > config file > built-in default)
- `gadgetron tenant create --name <name>` and `gadgetron tenant list`
- `gadgetron key create --tenant-id <uuid>` for persistent keys
- `gadgetron key create` for no-db/local development convenience
- `gadgetron key list --tenant-id <uuid>` and `gadgetron key revoke --key-id <uuid>`
- `gadgetron init` — generate an annotated `gadgetron.toml`
- `gadgetron doctor` — check configuration, database connectivity, provider reachability, and `/health`
- `gadgetron mcp serve` — stdio MCP server used by the Penny subprocess bridge and available for manual smoke tests
- `penny` model registration when `gadgetron.toml` contains a valid `[knowledge]` section
- Embedded Web UI at `/web` when built with the default `web-ui` feature and `[web].enabled = true`
- `gadgetron-testing` crate — `FakeLlmProvider` and `FailingProvider` for use in unit and integration tests

**Stubbed (HTTP 501):**
- `GET /api/v1/nodes`
- `POST /api/v1/models/deploy`
- `DELETE /api/v1/models/{id}`
- `GET /api/v1/models/status`
- `GET /api/v1/usage`
- `GET /api/v1/costs`

**Not yet implemented:**
- Node management CLI subcommands
- PostgreSQL-backed quota enforcement
- Audit log PostgreSQL persistence
- Full TUI keyboard navigation and scrolling
- Docker image (future)
- `gadgetron penny ...` convenience subcommands such as `penny init`
- Interactive approval flow for agent write/destructive tools (deferred to Phase 2B)
