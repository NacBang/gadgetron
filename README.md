# Gadgetron

A self-hosted control plane for GPU server fleets — log analysis, fault triage, and ad-hoc ops, with an AI assistant (**Penny**) sharing the same surface as the human operator.

```
   user / Penny  ──►  ┌──────────────┐  ──►  registered hosts (SSH + sudo)
                      │   Gateway    │
                      │   + Bundles  │  ──►  PostgreSQL  (TimescaleDB + pgvector)
                      └──────────────┘
                            │
                            ▼
                       /web  (Next.js)
```

## What it does

- **Server fleet inventory** — register hosts via SSH, capture stable identity (`machine_id`, DMI UUID), auto-merge re-registrations, IP/alias edits.
- **GPU + system telemetry** — DCGM-rich GPU stats (util, temp, VRAM, ECC, throttle reasons), CPU / RAM / network / NVMe SMART. 1 Hz background poller writes to a TimescaleDB hypertable; UI reads live + sparklines.
- **Log analyzer** — periodic incremental scan of `dmesg` / `journalctl` / `auth.log` per host. Rule-based regex classifier covers ~20 common kernel/service patterns; **Penny** is the LLM fallback for anything "Error"-shaped that didn't match. Findings get severity, summary, cause, suggested remediation, and a comments thread (operators + Penny both post).
- **Penny** — agent built on Claude Code that reads the same wiki / server / log surface a human does. Calls **gadgets** (MCP tools) for everything it needs. Write-tier gadgets sit behind an `Ask` policy bucket so Penny proposes, the operator approves.
- **Per-host shell** — `server.bash` with mandatory UI confirm dialog, optional `sudo` (NOPASSWD installed during host bootstrap). One escape hatch instead of an MCP entry per command.

## Quick start

```bash
# 1. Postgres (TimescaleDB + pgvector)
docker build -t gadgetron-pgvector-timescale:pg16 images/pgvector-timescale
docker run -d --name gadgetron-pg \
    -p 5432:5432 \
    -e POSTGRES_USER=gadgetron \
    -e POSTGRES_PASSWORD=secret \
    -e POSTGRES_DB=gadgetron_demo \
    gadgetron-pgvector-timescale:pg16

# 2. Local config
cp gadgetron.example.toml gadgetron.toml
$EDITOR gadgetron.toml      # provider keys, OAuth client id, etc.

cp .env.template .env
$EDITOR .env                # database URL, bootstrap admin password, OAuth secret if enabled

# 3. Build + launch (wraps env loading + pid handling)
cargo build --release -p gadgetron-cli
./scripts/launch.sh --bg
```

Browse `http://localhost:18080/web` — sign in with Google or paste an API key minted via `gadgetron key create`.

`./scripts/launch.sh --status` / `--stop` / `--rebuild` for the rest of the lifecycle.

## Core concepts

| Term         | Meaning                                                                                   |
|--------------|-------------------------------------------------------------------------------------------|
| **Bundle**   | A self-contained Cargo crate under `bundles/` shipping gadgets + (optionally) a UI surface. Examples: `server-monitor`, `log-analyzer`. |
| **Gadget**   | An MCP tool that Penny (or the UI via workbench actions) can call. Tier-classified: `Read` (auto), `Write` (per-bucket policy), `Destructive` (off by default). |
| **Plug**     | A core-facing extension point a bundle can hook into without touching the gateway crate.  |
| **Penny**    | The agent. Default runtime: Claude Code CLI + Claude Opus. Replaceable via the `agent.brain` config section. |

Authority: [ADR-P2A-10 — Bundle / Plug / Gadget terminology](docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md).

## Repository layout

```
crates/
  gadgetron-cli/        # `gadgetron serve | mcp | key | …`
  gadgetron-gateway/    # HTTP + SSE + auth + CSP + workbench actions
  gadgetron-core/       # Shared types, agent traits, config
  gadgetron-penny/      # Penny runtime, gadget registry, MCP stdio
  gadgetron-knowledge/  # Wiki + search + ingest
  gadgetron-xaas/       # Multi-tenant ops: users, sessions, conversations,
                        # billing ledger, audit log
  gadgetron-web/        # Embedded Next.js UI (build.rs runs `next build`)
bundles/
  server-monitor/       # Inventory, SSH bootstrap, stats collector,
                        # background poller, server.* gadgets
  log-analyzer/         # Scanner, rule engine, LLM classifier,
                        # findings + comments stores, loganalysis.* gadgets
  document-formats/     # PDF / docx / etc. ingestion
  gadgetron-core/       # First-party knowledge bundle (wiki + web search)
docs/                   # Design (`design/phase*/`), ADRs, manual
scripts/
  launch.sh             # Foreground / background / status / stop / rebuild
  e2e-harness/          # PR gate (boots full stack + curl assertions)
```

## Documentation

- [`docs/00-overview.md`](docs/00-overview.md) — product narrative
- [`docs/INDEX.md`](docs/INDEX.md) — reading guide
- [`docs/design/phase2/`](docs/design/phase2/) — active design surface
- [`docs/adr/`](docs/adr/) — architecture decision records
- [`docs/manual/`](docs/manual/) — operator manual
- [`CLAUDE.md`](CLAUDE.md) — agent collaboration guide (gstack, graphify, PR gate)

## Contributing

Run the E2E harness before opening a PR:

```bash
./scripts/e2e-harness/run.sh           # full (~2-3 min warm)
./scripts/e2e-harness/run.sh --quick   # skip cargo test (~30s)
```

See [`scripts/e2e-harness/README.md`](scripts/e2e-harness/README.md) for the gate table and the "how to add a gate" pattern.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
