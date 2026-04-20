# Installation

Gadgetron runs on **Linux** (Ubuntu 22.04+) and **macOS** (13+). This guide covers a fresh-system install and aligns to the current canonical local runtime: PostgreSQL with `pgvector` plus the repo-local `./demo.sh` operator loop.

---

## Ubuntu 22.04 (from scratch)

### Step 1: System packages

```bash
sudo apt-get update
sudo apt-get install -y \
  curl build-essential pkg-config libssl-dev \
  git ca-certificates \
  postgresql postgresql-client
```

### Step 2: Rust toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env
rustc --version   # workspace MSRV is 1.80; the repo pins 1.94.0 via rust-toolchain.toml and rustup will auto-install it on first cargo build
cargo --version
```

### Step 3: Clone and build

```bash
git clone https://github.com/NacBang/gadgetron.git
cd gadgetron
./demo.sh build
```

Build time: ~3-5 minutes on a cold cache. The binary is at `target/release/gadgetron`.

### Step 4: PostgreSQL setup

```bash
# Recommended local path: run pgvector in Docker
docker run -d \
  --name gadgetron-pgvector \
  -e POSTGRES_USER=gadgetron \
  -e POSTGRES_PASSWORD=secret \
  -e POSTGRES_DB=gadgetron_demo \
  -p 5432:5432 \
  pgvector/pgvector:pg16
```

If you use a host-installed PostgreSQL instead of Docker, install the matching `pgvector` extension package for your distribution and verify that `CREATE EXTENSION vector` succeeds in the target database. Without `pgvector`, the current knowledge-backed runtime will fail during migrations.

### Step 5: Verify

```bash
# Check binary
./target/release/gadgetron --help

# Run tests
cargo test --workspace --lib
```

### Step 6: First run

```bash
./target/release/gadgetron init --yes
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@127.0.0.1:5432/gadgetron_demo"
./demo.sh start
./demo.sh status
./demo.sh logs
```

`quickstart.md` covers the provider block you need in `gadgetron.toml`, tenant/API-key creation, and the first request path. `./demo.sh stop` shuts the local demo down. See **"What success looks like"** below (after the macOS section) for representative output of each `demo.sh` command — line skeletons are verbatim from `demo.sh`; PID values, filesystem paths, and URLs are runtime-substituted from your checkout.

---

## macOS (from scratch)

### Step 1: Homebrew

If not installed:
```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

### Step 2: System packages

```bash
brew install postgresql@16 git
```

### Step 3: Rust toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env
rustc --version   # workspace MSRV is 1.80; the repo pins 1.94.0 via rust-toolchain.toml and rustup will auto-install it on first cargo build
```

### Step 4: Clone and build

```bash
git clone https://github.com/NacBang/gadgetron.git
cd gadgetron
./demo.sh build
```

### Step 5: PostgreSQL setup

```bash
docker run -d \
  --name gadgetron-pgvector \
  -e POSTGRES_USER=gadgetron \
  -e POSTGRES_PASSWORD=secret \
  -e POSTGRES_DB=gadgetron_demo \
  -p 5432:5432 \
  pgvector/pgvector:pg16
```

If you prefer a Homebrew PostgreSQL instead of Docker, install a matching `pgvector` extension locally and verify that the target database can execute `CREATE EXTENSION vector`.

### Step 6: Verify

```bash
./target/release/gadgetron --help
cargo test --workspace --lib
```

### Step 7: First run

```bash
./target/release/gadgetron init --yes
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@127.0.0.1:5432/gadgetron_demo"
./demo.sh start
./demo.sh status
./demo.sh logs
```

### What success looks like

All demo state lives under `.gadgetron/demo/` inside the repo. The log is always at `.gadgetron/demo/gadgetron.log` (NOT `.gadgetron/state/...`).

The blocks below show representative output. Field **labels and prefixes** (`PID:`, `Status:`, `Health:`, `Hint:`, etc.) are verbatim from `demo.sh`. **Values** after the label — numeric PIDs, absolute filesystem paths, port-bound URLs, `launchctl` service IDs — are runtime-substituted and will differ on your checkout. The examples use `/path/to/gadgetron/` as a stand-in for your repo root.

**`./demo.sh start` — Linux / nohup path:**

```
Demo started.
  PID: 48213
  URL: http://127.0.0.1:8080/web
  Health: http://127.0.0.1:8080/health
  Log: /path/to/gadgetron/.gadgetron/demo/gadgetron.log
  DB: postgresql:///gadgetron_demo
```

The `URL:` line points at the `/web` chat shell. The same origin also serves (a) the browser wiki workbench at `<URL>/wiki` — "Wiki" left-rail tab inside the chat shell (ISSUE A.2, 0.2.3+), and (b) the operator dashboard at `<URL>/dashboard` — "Dashboard" left-rail tab (ISSUE 4, 0.2.7+). See [web.md §/web/wiki](web.md#web-wiki--브라우저-워크벤치-wiki-crud) and [web.md §`/web/dashboard`](web.md#webdashboard--operator-observability-issue-4--v027) for the full UIs.

**`./demo.sh start` — macOS / launchctl path** (no PID line — supervised by `launchd`):

```
Demo started.
  URL: http://127.0.0.1:8080/web
  Health: http://127.0.0.1:8080/health
  Log: /path/to/gadgetron/.gadgetron/demo/gadgetron.log
  Launchd: gui/501/com.gadgetron.demo
  DB: postgresql:///gadgetron_demo
```

If `start` prints `Server exited during startup. Recent log output:` (Linux/nohup path) or `LaunchAgent exited during startup. Recent log output:` (macOS/launchctl path) followed by 40 tailed log lines, the process died before the `/health` probe responded — read the tail and consult `troubleshooting.md`.

**`./demo.sh status` — healthy:**

```
Config: /path/to/gadgetron/gadgetron.toml
Bind:   127.0.0.1:8080
DB:     postgresql:///gadgetron_demo
Log:    /path/to/gadgetron/.gadgetron/demo/gadgetron.log
Status: running
PID:    48213
Health: ok (http://127.0.0.1:8080/health)
Web:    http://127.0.0.1:8080/web
```

**`./demo.sh status` — degraded (health probe failed):** the `Status:` line still prints `running` or `launchctl loaded`, but instead of `Health: ok` you get:

```
Health: unavailable (http://127.0.0.1:8080/health)
Hint:   PostgreSQL on postgresql:///gadgetron_demo is missing pgvector
```

The `Hint:` line only appears when the log contains the string `extension "vector" is not available`. No hint + `Health: unavailable` means the cause is something else — tail the log.

**`./demo.sh status` — stopped:** one of these script-defined variants (the parenthetical reason varies; the PID value is runtime-substituted):

```
Status: stopped (no PID file)
Status: stopped (stale PID file: 48213)
Status: stopped (launchctl job not loaded)
```

**`./demo.sh logs`** — tails the last `GADGETRON_DEMO_TAIL_LINES` (default 80) lines of `.gadgetron/demo/gadgetron.log`. Use `./demo.sh logs -f` to follow. If the file doesn't exist, `logs` exits non-zero with `Log file not found: <log-path>` (the path portion is the runtime-resolved `LOG_FILE` value) — this usually means `start` has never been invoked from this checkout.

---

## Requirements summary

| Component | Minimum version | Install command |
|-----------|----------------|-----------------|
| Rust | 1.80 MSRV (repo pins 1.94.0 via `rust-toolchain.toml`) | `rustup` (see above) |
| PostgreSQL | 16 recommended | `apt install postgresql` / `brew install postgresql@16` |
| `pgvector` | must match the PostgreSQL major version | Docker `pgvector/pgvector:pg16` or your distro's pgvector package |
| OpenSSL dev | any | `apt install libssl-dev` (Ubuntu only; macOS includes it) |
| C compiler | any | `apt install build-essential` / Xcode CLT (macOS) |

Gadgetron does not require a GPU. GPU support is used only by the node-management subsystem. The gateway runs on any host that can reach PostgreSQL and your LLM providers.

---

## Install binary system-wide (optional)

```bash
sudo cp target/release/gadgetron /usr/local/bin/gadgetron
gadgetron --help
```

---

## Headless build (no Web UI)

The default Gadgetron build includes the Web UI (`gadgetron-web` crate compiled into the binary via the `web-ui` Cargo feature on `gadgetron-gateway`, on by default). To produce a headless binary for API-only deployments (or for build environments without Node.js), disable default features:

```bash
cargo build --release --no-default-features -p gadgetron-cli
```

This turns off `gadgetron-cli`'s `default = ["web-ui"]`, which in turn disables the `web-ui` feature on `gadgetron-gateway` transitively and skips the `gadgetron-web` crate's `build.rs` entirely (no `npm` invocation required at build time). `gadgetron-cli` does not define a standalone `headless` feature — `--no-default-features` alone is the correct invocation.

**Verify**:

```bash
./target/release/gadgetron serve &
curl -I http://localhost:8080/web/   # HTTP/1.1 404 Not Found expected
curl -sf http://localhost:8080/v1/models -H "Authorization: Bearer $KEY"  # API still works
```

The `/web/*` subtree is not registered and returns the generic 404 — no indication that `gadgetron-web` was compiled out (DX-W-NB4, information hiding for probe attempts).

**Build requirements for default (with Web UI)**:

| Component | Minimum version | Install |
|---|---|---|
| Node.js | 20.19.0 (pinned via `crates/gadgetron-web/web/.nvmrc`) | `nvm install 20.19.0` / `brew install node@20` |
| npm | bundled with Node 20 (npm 10+) | — |

If `npm` is not on PATH when building the default profile and you do NOT want the Web UI, set `GADGETRON_SKIP_WEB_BUILD=1` to embed a fallback `index.html` that displays "Gadgetron Web UI unavailable" — or use `--no-default-features` above for a clean build. The canonical local path remains `./demo.sh build` / `start`; `start` auto-rebuilds the release binary when tracked source files are newer.

Related: `docs/manual/web.md` (Web UI setup), `docs/design/phase2/03-gadgetron-web.md §20` (feature flag topology).

---

## Docker

Docker support is planned for a future sprint. No official image has been published yet.

## Production deployment

### 1. systemd service unit

Use a dedicated system account and keep runtime data out of `/etc`.

1. Create the system user:

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin gadgetron
```

2. Create the config and data directories:

```bash
sudo install -d -m 0755 /etc/gadgetron
sudo install -d -o gadgetron -g gadgetron -m 0755 /var/lib/gadgetron
sudo install -d -o gadgetron -g gadgetron -m 0755 /var/lib/gadgetron/wiki
```

3. Use this file layout:

- `/etc/gadgetron/gadgetron.toml`, config file
- `/etc/gadgetron/gadgetron.env`, `EnvironmentFile` holding secrets
- `/var/lib/gadgetron/wiki`, `wiki_path`, use an absolute path because `wiki_path` resolves relative to the config file directory, not the current working directory

4. Write the unit file at `/etc/systemd/system/gadgetron.service`:

> **Note**: systemd already uses `SIGTERM` as the default stop signal, so the unit below does not override `KillSignal`.

```ini
[Unit]
Description=Gadgetron service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=gadgetron
Group=gadgetron
EnvironmentFile=/etc/gadgetron/gadgetron.env
WorkingDirectory=/var/lib/gadgetron
ExecStart=/usr/local/bin/gadgetron serve --config /etc/gadgetron/gadgetron.toml
ExecStartPost=/usr/bin/curl -fsS http://127.0.0.1:8080/ready
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

5. Write `/etc/gadgetron/gadgetron.toml` with an absolute wiki path:

```toml
[server]
bind = "127.0.0.1:8080"

[paths]
wiki_path = "/var/lib/gadgetron/wiki"
```

6. Write `/etc/gadgetron/gadgetron.env` with the runtime secrets and log level:

```dotenv
GADGETRON_DATABASE_URL=postgres://gadgetron:secret@127.0.0.1:5432/gadgetron
OPENAI_API_KEY=sk-...
GADGETRON_ADMIN_PW=change-me
RUST_LOG=info
```

7. Reload systemd and start the service:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now gadgetron
sudo systemctl status gadgetron
```

8. Read the startup logs if the unit does not come up:

```bash
sudo journalctl -u gadgetron -n 200 --no-pager
```

> **Warning**: do NOT add `ExecReload=/bin/kill -HUP` unless you intend to trigger a catalog reload, not a process restart. For restarts, use `systemctl restart gadgetron`.

> **Warning**: `wiki_path` in `gadgetron.toml` resolves relative to the config file directory. Use an absolute path, for example `/var/lib/gadgetron/wiki`, to avoid landing the wiki inside `/etc/gadgetron/`.

### 2. Nginx TLS termination

Put the public TLS endpoint and the Gadgetron routes on the same host. `/web/`, `/v1/`, and `/api/` should all terminate on one origin and forward to the same upstream.

1. Install Nginx and place this server block in your site config:

```nginx
server {
    listen 443 ssl;
    server_name example.com;

    ssl_certificate /etc/ssl/certs/example.fullchain.pem;
    ssl_certificate_key /etc/ssl/private/example.key.pem;

    proxy_cookie_flags ~ secure;

    location /web/ {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Real-IP $remote_addr;
    }

    location /v1/ {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Real-IP $remote_addr;
    }

    location /api/ {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Real-IP $remote_addr;

        # Required for /api/v1/web/workbench/events/ws (WebSocket
        # upgrade). Nginx 400s the handshake without these three.
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }
}
```

2. Validate the config and reload Nginx:

```bash
sudo nginx -t
sudo systemctl reload nginx
```

3. Verify that the public endpoint can reach the readiness probe:

```bash
curl -fsS https://example.com/ready
```

`proxy_cookie_flags ~ secure;` requires Nginx 1.19.8 or newer and appends `Secure` to proxied cookies.

> **Note**: if you run an older Nginx, either rewrite `Set-Cookie` with a `proxy_hide_header Set-Cookie` plus `add_header Set-Cookie ...` pattern, or rely on a site-wide `Strict-Transport-Security` header and have the application emit `Secure` itself.

> **Note**: `X-Forwarded-For` is forwarded for future use. The gateway does not parse it today.

> **Note**: no CORS headers are needed at Nginx. All routes are served from the same origin.

### 3. Caddy TLS termination

Caddy can terminate TLS and provision certificates automatically. Put the public domain name in the site address and reverse proxy to the local Gadgetron listener.

1. Write a `Caddyfile` like this:

```caddyfile
example.com {
	reverse_proxy 127.0.0.1:8080 {
		header_up X-Forwarded-Proto https
		header_up X-Forwarded-For {remote_host}
		header_down Set-Cookie "(?i)^(.+)$" "$1; Secure"
	}
}
```

2. Reload Caddy:

```bash
sudo systemctl reload caddy
```

3. Verify that Caddy can reach the upstream:

```bash
curl -fsS https://example.com/ready
```

If you already append cookie flags in the application, remove the `header_down` rewrite to avoid duplicate attributes.

> **Note**: Caddy provisions and renews TLS certificates automatically when the site address is a real public domain and the server is reachable on ports 80 and 443.

> **Note**: Caddy 2's `reverse_proxy` handles the WebSocket upgrade for `/api/v1/web/workbench/events/ws` automatically (no extra directive needed), unlike the explicit `proxy_http_version 1.1; Upgrade; Connection "upgrade";` triplet required by Nginx.

### 4. Health probe pattern

Use the shallow probes for orchestration and reserve the authenticated bootstrap path for smoke tests.

| Endpoint | Meaning | Use |
|---|---|---|
| `GET /health` | Liveness, unconditional `200` | Kubernetes `livenessProbe`, load balancer health check |
| `GET /ready` | Readiness, `200` when the PostgreSQL pool is up, `503` otherwise | Kubernetes `readinessProbe` |
| `GET /api/v1/web/workbench/bootstrap` | Deeper smoke test, requires Bearer auth | Authenticated monitoring only, not Kubernetes probes |

1. Check liveness:

```bash
curl -fsS http://127.0.0.1:8080/health
```

2. Check readiness:

```bash
curl -fsS http://127.0.0.1:8080/ready
```

3. Run the authenticated smoke test:

```bash
curl -fsS \
  -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:8080/api/v1/web/workbench/bootstrap
```

4. Use the shallow endpoints in Kubernetes:

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 8080
  initialDelaySeconds: 5
  periodSeconds: 10

readinessProbe:
  httpGet:
    path: /ready
    port: 8080
  initialDelaySeconds: 2
  periodSeconds: 5
```

> **Note**: `ExecStartPost` in the systemd unit above already blocks service startup until `/ready` returns `200`.

### 5. Backup and restore

#### 5.1 State inventory

Gadgetron's persistent state spans three storage areas.

**PostgreSQL tables** (all require backup — none are cache or ephemeral):

| Domain | Tables |
|---|---|
| Identity | `tenants`, `users`, `teams`, `team_members`, `user_sessions`, `api_keys` |
| Quota / billing | `quota_configs`, `billing_events` |
| Audit | `audit_log`, `action_audit_events`, `tool_audit_events` |
| Knowledge | `wiki_pages`, `wiki_chunks`, `activity_events`, `knowledge_candidates`, `candidate_decisions` |

> **Warning** The `wiki_chunks` table stores vector embeddings in a column that requires the `pgvector` extension (`CREATE EXTENSION vector`). Any restore target that does not have this extension installed will fail during migration replay. Install the extension package (e.g. `postgresql-16-pgvector` on Debian/Ubuntu) on the target host before restoring.

**Filesystem state**:

- **Wiki git repo** — path set by `[knowledge] wiki_path` in `gadgetron.toml`. Every page write auto-commits with message `auto-commit: <path> <iso8601-utc>`. Back up the full directory including `.git/`, or push to a backup remote.
- **Bundles directory** — path set by `[web] bundles_dir` (optional). Contains `bundle.toml` manifests for operator-installed bundles. Include in backup if the key is configured.

#### 5.2 Daily backup (no downtime required)

`pg_dump -Fc` produces a consistent snapshot without locking writers. Git operations are atomic per file. Both are safe to run while `gadgetron serve` is live.

```sh
# 1. PostgreSQL dump in custom format, compression level 6
pg_dump -Fc -Z 6 "" > /backup/gadgetron-$(date +%Y%m%d-%H%M%S).dump

# 2. Wiki git repo (choose one)
#    a) tar the full directory (simpler, no remote needed)
tar -czf /backup/wiki-$(date +%Y%m%d-%H%M%S).tar.gz \
  -C "$(dirname "$wiki_path")" "$(basename "$wiki_path")"
#    b) push to a backup remote (preferred when a remote is configured)
git -C "$wiki_path" push

# 3. Bundles directory (only when [web] bundles_dir is set)
tar -czf /backup/bundles-$(date +%Y%m%d-%H%M%S).tar.gz \
  -C "$(dirname "$bundles_dir")" "$(basename "$bundles_dir")"
```

Schedule with a systemd timer or cron. Retain at least seven daily dumps.

#### 5.3 Cold backup for bit-perfect consistency

Stop the server before dumping when you need a byte-exact snapshot, for example before a major schema migration or a schema-drift investigation.

```sh
systemctl stop gadgetron
pg_dump -Fc -Z 6 "$GADGETRON_DATABASE_URL" > /backup/gadgetron-cold-$(date +%Y%m%d-%H%M%S).dump
tar -czf /backup/wiki-cold-$(date +%Y%m%d-%H%M%S).tar.gz \
  -C "$(dirname "$wiki_path")" "$(basename "$wiki_path")"
systemctl start gadgetron
```

Downtime is typically under 30 seconds for databases up to a few gigabytes.

#### 5.4 Restore and post-restore validation

**Requirements on the target host**: PostgreSQL 16, the `pgvector` extension package installed, and a reachable Gadgetron config at `/etc/gadgetron/gadgetron.toml`.

**Restore sequence**:

```sh
# 1. Create the target database (pgvector server package must already be
#    installed at the cluster / container level — pgvector/pgvector:pg16
#    ships it; Debian/Ubuntu hosts need `postgresql-NN-pgvector`).
createdb gadgetron

# 2. Install the vector extension INSIDE the target database.
#    Extensions are per-database in PostgreSQL, so this must run against
#    `gadgetron`, not `postgres`. Without it, pg_restore step 3 fails on
#    the `CREATE EXTENSION vector` statement in the dump.
psql -d gadgetron -c "CREATE EXTENSION IF NOT EXISTS vector"

# 3. Restore the dump
pg_restore --if-exists --clean -d gadgetron /backup/gadgetron-*.dump

# 4. Restore the wiki repo
mkdir -p "$wiki_path"
tar -xzf /backup/wiki-*.tar.gz -C "$(dirname "$wiki_path")"

# 5. Restore bundles (only if [web] bundles_dir is configured)
tar -xzf /backup/bundles-*.tar.gz -C "$(dirname "$bundles_dir")"

# 6. Start gadgetron
systemctl start gadgetron
```

**Post-restore validation**:

Gadgetron has no automated consistency checker for post-restore state. Run the following checks manually.

```sh
# Connectivity and config
gadgetron doctor --config /etc/gadgetron/gadgetron.toml

# Wiki page state (stale pages, missing frontmatter)
gadgetron wiki audit --config /etc/gadgetron/gadgetron.toml
```

Check for foreign-key orphans that CASCADE rules might have left behind:

```sql
-- api_keys pointing to deleted users
SELECT k.id, k.user_id FROM api_keys k
  LEFT JOIN users u ON k.user_id = u.id
  WHERE u.id IS NULL AND k.user_id IS NOT NULL;

-- user_sessions pointing to deleted users (CASCADE should prevent this)
SELECT s.id, s.user_id FROM user_sessions s
  LEFT JOIN users u ON s.user_id = u.id
  WHERE u.id IS NULL;

-- team_members pointing to missing teams or users
SELECT m.team_id, m.user_id FROM team_members m
  LEFT JOIN teams t ON m.team_id = t.id
  LEFT JOIN users u ON m.user_id = u.id
  WHERE t.id IS NULL OR u.id IS NULL;

-- audit_log actor coverage
SELECT COUNT(*) FILTER (WHERE actor_user_id IS NULL) AS null_actor,
       COUNT(*) AS total
FROM audit_log;
```

A clean restore shows zero rows for the first three queries. A non-zero `null_actor` count in the last query is expected for system-generated events.

### 6. Upgrade and rolling deploy

**Upgrade model.** Each Gadgetron release bundles up to three things that move together: the binary, `gadgetron.toml` additions, and a `migrations/` directory in-tree at `crates/gadgetron-xaas/migrations/`. On `gadgetron serve` boot the binary runs `sqlx::migrate!(...)` against the configured pool (`crates/gadgetron-cli/src/main.rs:761`, `:799`, `:1428`); sqlx tracks applied migrations in a `_sqlx_migrations` table so the call is idempotent — re-running a start against an already-migrated schema is a no-op. Migrations are **forward-only** on trunk — there is no `down.sql`, so schema rollback = PITR restore (see §5).

#### 6.1 In-place upgrade (single node, minute-scale downtime)

Acceptable for internal deployments where a 30–90 second `/health` outage is fine.

```sh
# 1. Snapshot the current state (in case the new binary fails or a migration
#    misbehaves). Reuse the §5.2 daily backup recipe — skip only if the last
#    scheduled dump is younger than ~1 hour.
sudo -u postgres pg_dump -Fc gadgetron > "/backup/pre-upgrade-$(date -u +%Y%m%dT%H%M%SZ).dump"

# 2. Fetch and build the target revision. Keep the old binary on disk until
#    post-start verification passes — the rollback recipe needs it.
cd /opt/gadgetron
sudo -u gadgetron git fetch origin
sudo -u gadgetron git checkout <target-tag-or-sha>
sudo -u gadgetron cargo build --release

# 3. Stop the old process. systemd stops are graceful: the gateway drains
#    in-flight chat + 5-second audit flush (see §1 systemd unit).
sudo systemctl stop gadgetron

# 4. Atomically swap the binary. Avoid `cp` over a running target — the
#    `install` invocation is rename-based so /usr/local/bin/gadgetron stays
#    consistent if anything reads it mid-swap.
sudo install -m 755 /opt/gadgetron/target/release/gadgetron /usr/local/bin/gadgetron

# 5. Start. The first boot runs any new migrations before opening the listen
#    socket — no need to run a separate `sqlx migrate` step.
sudo systemctl start gadgetron

# 6. Confirm the new version started and migrations applied.
/usr/local/bin/gadgetron --version
curl -fsS http://localhost:8080/health
psql "$GADGETRON_DATABASE_URL" -tAc \
  "SELECT version, description FROM _sqlx_migrations ORDER BY version DESC LIMIT 5"
```

The last `psql` should show the newest migration files from the target revision's `crates/gadgetron-xaas/migrations/` — if the top entry is still the previous version's latest, migrations didn't run (pool wiring issue or server didn't actually restart).

#### 6.2 Rolling upgrade (behind a load balancer)

For zero-downtime deployments with two or more Gadgetron instances behind a TCP or HTTP LB. The schema-compatibility window is small but real: v(N) and v(N+1) must both run against the v(N+1) schema for the overlap period when one node has upgraded and the other hasn't.

**Preflight rule**: only safe when the target migration is **additive** (new table, new nullable column, new index). Column drops, type changes, and NOT-NULL additions without a default BREAK the old-binary pods reading the schema — those releases require §6.3 drain-all instead. The commit message for each migration calls out which kind it is; when in doubt, read the `.sql` file.

```sh
# Per-node, one at a time:

# 1. Mark this node unhealthy so the LB stops sending new traffic. If using
#    the §4.2 authenticated smoke probe as an LB health check, flip to the
#    "drain" response via a flag file or control endpoint — or just force
#    /health to 503 by stopping. Real-world: most LBs check every 2-5s, so
#    wait for 2 probe intervals before step 2.
touch /var/lib/gadgetron/DRAIN   # consumed by your LB's custom health check
sleep 10

# 2. Stop the old binary. Drain period covered by systemd TimeoutStopSec.
sudo systemctl stop gadgetron

# 3. Swap + start (same as §6.1 steps 4-5). The first node of the batch
#    triggers the migration; subsequent nodes see idempotent no-ops.
sudo install -m 755 /opt/gadgetron/target/release/gadgetron /usr/local/bin/gadgetron
sudo systemctl start gadgetron

# 4. Post-start verification. Wait for /health and /ready BOTH to return 200
#    (the 503 window on /ready during pgvector pool warm-up is typically
#    under 2 seconds but spikes on slow disks).
until curl -fsS http://localhost:8080/health && curl -fsS http://localhost:8080/ready; do
  sleep 1
done

# 5. Re-advertise to the LB.
rm /var/lib/gadgetron/DRAIN

# 6. Repeat steps 1-5 on the next node only after this one is serving traffic
#    for at least one LB probe cycle — premature rollover leaves zero healthy
#    nodes if the new binary has a cold-start bug.
```

**During the window when node-A is on v(N+1) and node-B is still on v(N)**, both talk to the v(N+1) schema. v(N) writes to tables it doesn't know about are impossible (old code doesn't reference new tables); v(N) reads from widened columns are fine as long as the widening was additive — a new nullable column is invisible to v(N)'s `SELECT col_a, col_b` projections.

#### 6.3 Drain-all upgrade (for breaking schema changes)

When the release notes mark a migration as breaking (column drop, non-null backfill, type change), take every node down before migrating:

```sh
# 1. Backup (same as §6.1 step 1 — mandatory here, not optional).
# 2. Drain LB (all nodes).
# 3. Stop all gadgetron instances.
# 4. Upgrade binary on ONE node and start — this node runs the migration.
# 5. Verify migration applied and /health is 200.
# 6. Upgrade + start remaining nodes (no re-migration; _sqlx_migrations now records the run).
# 7. Un-drain LB.
```

The all-nodes-down window is usually 1-2 minutes dominated by the migration itself. Pre-benchmark long-running migrations against a backup-restored staging copy to bound the downtime — a `CREATE INDEX` on a 10M-row table can take minutes; a `CLUSTER` command scales with table size.

#### 6.4 Rollback (downgrade binary)

Rollback is always a binary swap paired with (sometimes) a schema restore. The decision tree:

- **New binary crashes on start but no migration landed** (common with a bad config-validator change): reinstate the previous binary — `sudo install -m 755 /usr/local/bin/gadgetron.prev /usr/local/bin/gadgetron && sudo systemctl start gadgetron`. Keep a copy at `gadgetron.prev` as part of the §6.1 swap so this is always available for one-step rollback.
- **New binary started and a migration applied, but runtime is broken**: the migration is already committed and is forward-only. Rolling back the binary alone works IF the previous version is schema-forward-compatible with the new migration (the additive case from §6.2). If not, restore the §6.1 pre-upgrade backup following the §5.4 recipe, then install the previous binary.
- **Migration itself failed mid-way**: sqlx wraps each `.sql` file in a transaction, so a failed migration rolls back its own partial state — the `_sqlx_migrations` row is NOT inserted. The next start will retry the same migration. If the migration can never succeed (bad SQL or pre-existing data incompatible), fix the `.sql` file and redeploy, OR skip the migration by inserting its version into `_sqlx_migrations` manually (`INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time) VALUES (...)` — only under vendor guidance; use with care).

#### 6.5 Configuration file changes

`gadgetron.toml` additions are additive by design — older binaries ignore fields they don't know. Removals are rare and are called out in the release notes with a minimum-supported-binary floor. After editing `gadgetron.toml`:

- **Full reload** requires a restart. `gadgetron serve` reads the config once at startup; there is no SIGHUP for the TOML path.
- **Catalog-only reload** (`[web].catalog_path` / `[web].bundles_dir` pointee changes): `POST /api/v1/web/workbench/admin/reload-catalog` or `kill -HUP <pid>` — see [api-reference.md §POST /admin/reload-catalog](api-reference.md). No process restart needed.
- **Provider-table changes** (new `[providers.x]` block): full restart. Live-add of a provider is NOT supported on trunk.

---

## Troubleshooting install issues

| Problem | Fix |
|---------|-----|
| `rustc: command not found` after install | Run `source $HOME/.cargo/env` or restart your shell |
| `error: linker cc not found` | Install `build-essential` (Ubuntu) or Xcode CLT: `xcode-select --install` (macOS) |
| `openssl/ssl.h: No such file` | Install `libssl-dev` (Ubuntu): `sudo apt install libssl-dev` |
| `could not connect to server` (PostgreSQL) | Start your pgvector-capable PostgreSQL and verify the URL in `GADGETRON_DATABASE_URL` |
| `createdb: database creation failed` | Ensure your user has PG superuser role: `sudo -u postgres createuser -s $USER` |
| `extension "vector" is not available` | Your PostgreSQL server does not provide `pgvector`; use `pgvector/pgvector:pg16` or install the matching pgvector package locally |
| `cargo build` timeout or OOM | Ensure at least 4 GB RAM and 2 GB disk for compilation |
