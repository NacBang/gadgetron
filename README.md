# Gadgetron

A self-hosted control plane for GPU server fleets — inventory, telemetry, log analysis, and an AI assistant (**Penny**) for cluster operators.

## Quick start

### 1. Prerequisites

- Rust 1.80+ (`rustup install stable`)
- Docker (for the Postgres container)
- Node.js 20+ and npm 10+ (for the embedded web UI build) — or skip with `GADGETRON_SKIP_WEB_BUILD=1`

### 2. Database — Postgres with TimescaleDB + pgvector

Build the image and start the container:

```bash
docker build -t gadgetron-pgvector-timescale:pg16 images/pgvector-timescale
docker run -d --name gadgetron-pg -p 5432:5432 \
    -e POSTGRES_USER=gadgetron \
    -e POSTGRES_PASSWORD=secret \
    -e POSTGRES_DB=gadgetron_demo \
    gadgetron-pgvector-timescale:pg16
```

Schema migrations run automatically on first server boot — no manual `psql` setup needed.

### 3. Environment variables (`.env`)

```bash
cp .env.template .env
```

Edit `.env`:

| Variable | Required | Notes |
|---|---|---|
| `GADGETRON_DATABASE_URL` | yes | `postgres://gadgetron:secret@127.0.0.1:5432/gadgetron_demo` (matches the Docker setup above). |
| `GADGETRON_ADMIN_PW` | yes | First-admin password. **Change this** before first boot — it's read once when the `users` table is empty. |
| `GADGETRON_GOOGLE_CLIENT_SECRET` | conditional | Required **only if** `[auth.google]` is enabled in `gadgetron.toml`. Leave the placeholder if you don't use Google sign-in. |

`.env` is gitignored. Never commit it.

### 4. Server config (`gadgetron.toml`)

```bash
cp gadgetron.example.toml gadgetron.toml
```

Minimum config to run a working server:

```toml
[server]
bind = "0.0.0.0:18080"

[router.default_strategy]
type = "round_robin"

[auth.bootstrap]
admin_email = "admin@example.com"
admin_display_name = "Admin"
admin_password_env = "GADGETRON_ADMIN_PW"
```

Optional sections (uncomment as you go):

- `[providers.*]` — your upstream LLM endpoints (vLLM, SGLang, OpenAI, Anthropic, Ollama, Gemini)
- `[agent]` + `[agent.brain]` + `[knowledge]` — Penny (the AI assistant); requires the Claude Code CLI separately
- `[auth.google]` — Sign in with Google (also set `GADGETRON_GOOGLE_CLIENT_SECRET` in `.env`)
- `[web]` — embedded web UI behavior (bundle dir, API base path)

`gadgetron.toml` is gitignored — your endpoints and absolute binary paths stay local.

### 5. Build + launch the web server

```bash
cargo build --release -p gadgetron-cli
./scripts/launch.sh --bg
```

The launcher loads `.env`, starts `gadgetron serve` in the background, and writes a PID file.

| Command | Effect |
|---|---|
| `./scripts/launch.sh` | Foreground, tail logs to terminal |
| `./scripts/launch.sh --bg` | Background, log to `/tmp/gadgetron-serve.log` |
| `./scripts/launch.sh --status` | Health probe + recent log lines |
| `./scripts/launch.sh --stop` | Stop the background instance |
| `./scripts/launch.sh --rebuild` | `cargo build --release` then restart |

For a headless / API-only build (no Node.js required):

```bash
GADGETRON_SKIP_WEB_BUILD=1 cargo build --release -p gadgetron-cli --no-default-features
```

### 6. Sign in

Open **http://localhost:18080/web** in a browser. Sign in with the admin email from `gadgetron.toml` (`auth.bootstrap.admin_email`) and the password from `.env` (`GADGETRON_ADMIN_PW`).

To mint an API key for headless clients:

```bash
# Full mode (PostgreSQL): pass the tenant UUID
./target/release/gadgetron key create --tenant-id <TENANT_UUID>

# No-db mode: tenant id is not required
./target/release/gadgetron key create
```

The raw key is printed once and never stored. Default scope is `OpenAiCompat`; pass `--scope <CSV>` to override.

### Optional — enable Penny (AI assistant)

Penny requires the [Claude Code CLI](https://docs.anthropic.com/claude/claude-code). Install it, run `claude login`, then uncomment the `[agent]`, `[agent.brain]`, and `[knowledge]` blocks in `gadgetron.toml` and restart with `./scripts/launch.sh --rebuild`.

## License

Source-available under the [PolyForm Noncommercial License 1.0.0](LICENSE).

- **Noncommercial use** (personal, research, education, evaluation) is free of charge.
- **Commercial use** requires a separate license — contact **jungho@manycoresoft.co.kr**.

See [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE) for the full terms.

---

© 2026 ManyCoreSoft Co., Ltd.
