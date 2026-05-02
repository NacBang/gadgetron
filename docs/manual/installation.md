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

If this machine will run `server-add` with `password_bootstrap`, also install
`sshpass` on the Gadgetron host:

```bash
brew install sshpass
```

`password_bootstrap` uses `sshpass` only for the one-time password SSH login
that installs a generated monitoring key on the target host. The `key_path`
and `key_paste` registration modes do not require it.

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
| `sshpass` | optional | Required only for `server-add` `password_bootstrap`; `apt install sshpass` / `brew install sshpass` |

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

#### 5.5 Wiki git operations

The wiki is a local git repo at `[knowledge].wiki_path`. Every `wiki.write` / `wiki.rename` / `wiki.delete` commits automatically when `wiki_autocommit = true` (default). That makes the repo a first-class persistence layer: tar backups (§5.2) capture state but not the collaboration shape — the shape comes from pushing to a remote and having multiple writers pull. The recipes below cover daily git ops on the wiki beyond the tar snapshot path.

**Understand what's in the repo.** `wiki_path` carries:
- `.md` pages at any nesting depth (operator-authored + Penny-authored + seed)
- `_archived/<YYYY-MM-DD>/` directories from `wiki.delete` soft-deletes
- `.git/` with the full commit history + config + per-commit `wiki_git_author` identity

**What's NOT in the repo** — the pgvector `wiki_chunks` index. That's a read-through cache; `gadgetron reindex` rebuilds it from the `.md` state at any point.

##### Push to a remote (backup + collaboration)

```sh
# One-time: add a remote pointing at the backup/collab server.
cd "$wiki_path"
sudo -u gadgetron git remote add origin git@internal-git:gadgetron/team-wiki.git
sudo -u gadgetron git push -u origin main   # or whatever branch name autocommit used

# Recurring: push after each wiki_autocommit — wire as a systemd timer.
cat > /etc/systemd/system/gadgetron-wiki-push.service <<'UNIT'
[Unit]
Description=Push gadgetron wiki to remote
After=network-online.target

[Service]
Type=oneshot
User=gadgetron
WorkingDirectory=/var/lib/gadgetron/wiki
ExecStart=/usr/bin/git push origin main
UNIT

cat > /etc/systemd/system/gadgetron-wiki-push.timer <<'TIMER'
[Unit]
Description=Push gadgetron wiki every 5 minutes

[Timer]
OnBootSec=2min
OnUnitActiveSec=5min

[Install]
WantedBy=timers.target
TIMER

sudo systemctl enable --now gadgetron-wiki-push.timer
```

Branch name is whatever `git init` defaulted to when `wiki_autocommit` first fired (usually `main` or `master` depending on the host's git version). Check with `git branch --show-current` in `wiki_path`.

##### Pull external edits in

If another server + Gadgetron instance pushes to the same remote, pull their changes into the local repo. `gadgetron serve` reads from the filesystem lazily, so a pull that only adds files doesn't require a restart — but the pgvector index won't see new pages until `gadgetron reindex` runs.

```sh
# 1. Pull (in a systemd timer or manually)
sudo -u gadgetron git -C /var/lib/gadgetron/wiki pull --ff-only origin main

# 2. Reindex so semantic search sees the new pages
sudo -u gadgetron /usr/local/bin/gadgetron reindex \
  --config /etc/gadgetron/gadgetron.toml
```

**`--ff-only`** is critical — it refuses to auto-merge divergent histories and surfaces the conflict for manual resolution. The conflict resolution recipe is below.

##### Divergent histories (conflict resolution)

When two Gadgetron instances both wrote to the same page between pull cycles, `--ff-only` refuses to merge and prints `Not possible to fast-forward, aborting.`. You'll get a conflict on the `.md` file.

```sh
cd "$wiki_path"

# 1. See the diverging commits
git log --oneline --graph --all -20

# 2. Attempt the merge. It will stop with conflict markers in the
#    divergent .md file(s).
sudo -u gadgetron git pull --no-ff origin main

# 3. Resolve each conflicted file by editing. Conflict markers are
#    standard <<<<<<< / ======= / >>>>>>>. The wiki is Markdown —
#    conflict markers are NOT legal Markdown and will break rendering,
#    so resolve before any reader encounters the page.
vim some-page.md

# 4. Mark resolved + complete the merge
sudo -u gadgetron git add some-page.md
sudo -u gadgetron git commit --no-edit

# 5. Re-push the merged head
sudo -u gadgetron git push origin main

# 6. Reindex to refresh pgvector
sudo -u gadgetron /usr/local/bin/gadgetron reindex \
  --config /etc/gadgetron/gadgetron.toml
```

**Preventing conflicts in the first place** — route all wiki writes through a single Gadgetron instance. The wiki is not designed for concurrent multi-master writes; the git repo makes conflict resolution possible but operationally expensive. For teams with multiple Gadgetron deployments, promote one as the write-primary and point others at it via `[agent]` + `[knowledge]` config (replicate the TOML, not the wiki state).

##### Recovering from `.git` corruption

Symptom: `wiki.write` fails with `wiki_conflict` (see `manual/penny.md §트러블슈팅`) even when no other writer is active, and `git status` in `wiki_path` returns `fatal: bad object HEAD`.

Fastest recovery path: `tar` backup from §5.2 contains the full `.git/` tree, so restore + resume:

```sh
# 1. Stop gadgetron to release wiki file locks
sudo systemctl stop gadgetron

# 2. Move the broken wiki aside (don't delete until you've verified
#    the restore — corrupt .git might still have salvageable objects)
sudo -u gadgetron mv /var/lib/gadgetron/wiki /var/lib/gadgetron/wiki.broken

# 3. Restore the most recent tar backup (§5.2 recipe output)
sudo -u gadgetron tar -xzf /backup/wiki-20260420-120000.tar.gz \
  -C /var/lib/gadgetron

# 4. Verify the restored git state
sudo -u gadgetron git -C /var/lib/gadgetron/wiki status

# 5. If a remote exists, fetch + reset to the remote head — this
#    pulls in any writes that happened AFTER the backup timestamp.
sudo -u gadgetron git -C /var/lib/gadgetron/wiki fetch origin
sudo -u gadgetron git -C /var/lib/gadgetron/wiki reset --hard origin/main

# 6. Start + reindex
sudo systemctl start gadgetron
sudo -u gadgetron /usr/local/bin/gadgetron reindex \
  --config /etc/gadgetron/gadgetron.toml

# 7. If the restore verified successfully, remove the broken copy
sudo -u gadgetron rm -rf /var/lib/gadgetron/wiki.broken
```

**No-backup recovery**: if neither a tar backup nor a remote exists, salvage the `.md` files directly — they're plain Markdown on disk regardless of `.git/` state. Copy them out, `rm -rf wiki/.git`, `git init` + `git add` + initial commit, and resume. History is lost but content survives.

##### Wiki audit + reindex interplay after git ops

`gadgetron wiki audit` scans for stale pages (`_archived/` older than 90 days by default) and pages missing frontmatter. Run after every bulk git operation — merge commits may bring in `.md` files without the frontmatter shape the workbench expects:

```sh
sudo -u gadgetron /usr/local/bin/gadgetron wiki audit \
  --config /etc/gadgetron/gadgetron.toml
```

`gadgetron reindex` is the companion: it rebuilds the pgvector `wiki_chunks` index from the current `.md` filesystem state. Default is incremental (diffs against the index); use `--full` after a `git reset --hard` or tar restore where the index may be ahead of the actual filesystem.

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

### 7. Observability integration

Gadgetron does not expose a Prometheus `/metrics` scrape endpoint on trunk — metrics-style monitoring is assembled from the structured tracing log stream plus a handful of HTTP endpoints. The sections below cover what the binary emits, how to ship it somewhere queryable, and what to alert on.

#### 7.1 What the binary emits natively

**Tracing targets** (structured JSON logs when `RUST_LOG` is set to non-default and a JSON subscriber is installed; plain text otherwise). Every call site uses `tracing::<level>!(target: "<name>", ...)` — filter by target rather than by file path. Current targets on trunk (verified via `grep -r 'target: "' crates/`):

| Target | What it emits |
|---|---|
| `gadgetron_audit` | Chat audit events (one line per completed `/v1/chat/completions`). `RUST_LOG=gadgetron_audit=info` is the minimum to observe chat traffic. |
| `gadgetron_config` | Startup config summary + config reload events. Errors here are fatal during boot. |
| `config_migration` | v0.1.x `[penny]` → `[agent.brain]` per-field deprecation warnings. See also `cli_deprecation`. |
| `penny_audit` | Penny tool-call audit events (the `ToolCallCompleted` sink — persists to `tool_audit_events` when the pool is wired). |
| `penny_subprocess` | Claude Code subprocess lifecycle (spawn / exit / timeout / signal). |
| `penny_stream` | Penny streaming event transformation. Noisy at `debug`; keep at `warn` in prod. |
| `penny_session` | Claude Code session resumption + token budget. |
| `penny_shared_context` / `penny_shared_context.inject` | Pre-chat context injection (wiki seed pages, agent instructions). |
| `knowledge_service`, `knowledge_semantic`, `knowledge_config` | Knowledge layer reads, pgvector index state, config validation. |
| `llm_wiki_store`, `wiki_search`, `wiki_audit`, `wiki_frontmatter`, `wiki_seed` | Wiki-specific subsystems. |
| `agent_config` | `[agent]` / `[agent.brain]` validation at startup + reload. |
| `cli_deprecation` | CLI verb deprecations (`gadgetron mcp serve` → `gadget serve`). Safe to route to WARN-level alerts. |
| `home`, `penny_home`, `node` | Home-directory creation + node-registry events. |

**HTTP surface**:

| Endpoint | Auth | Purpose |
|---|---|---|
| `GET /health` | none | Unconditional liveness (200 on live process). |
| `GET /ready` | none | PostgreSQL pool health (200 healthy / 503 unhealthy). Use for load-balancer readiness probes. |
| `GET /api/v1/web/workbench/usage/summary` | `OpenAiCompat` | 24h tenant-scoped chat roll-up — counts, tokens, costs, latency percentiles. The closest Gadgetron has to a "metrics" endpoint today; scrape it on a cron. |
| `GET /api/v1/web/workbench/events/ws` | `OpenAiCompat` (query-token fallback for browsers) | WebSocket live activity stream — `ChatCompleted` + `ToolCallCompleted` frames as they publish. |
| `GET /api/v1/web/workbench/admin/audit/log` | `Management` | Tenant-pinned audit reads, filterable by `actor_user_id` + `since` + `limit`. |
| `GET /api/v1/web/workbench/admin/billing/events` | `Management` | Tenant-pinned billing ledger reads (chat + tool + action). |

**Postgres-side observability** (query directly from monitoring):

```sql
-- Applied migrations (upgrade success signal)
SELECT version, description, installed_on, success
FROM _sqlx_migrations ORDER BY version DESC LIMIT 5;

-- Recent auth failures (401 rate)
SELECT COUNT(*) FROM audit_log
WHERE status != 'ok' AND timestamp > NOW() - INTERVAL '5 minutes';

-- Per-tenant 24h chat counts (sanity vs /usage/summary)
SELECT tenant_id, COUNT(*) AS chats, SUM(cost_cents) AS cents
FROM audit_log WHERE timestamp > NOW() - INTERVAL '24 hours'
GROUP BY tenant_id ORDER BY chats DESC LIMIT 10;
```

#### 7.2 Shipping logs

Gadgetron writes to stdout/stderr. Route the stream to your aggregator of choice — the systemd unit in §1 already captures both into the journal.

**journald → Loki (recommended for small / single-cluster deployments)**:

```yaml
# promtail.yml — scrape the systemd journal and tag gadgetron lines
scrape_configs:
  - job_name: journal
    journal:
      json: false
      max_age: 12h
      labels:
        job: systemd-journal
    relabel_configs:
      - source_labels: ['__journal__systemd_unit']
        target_label: unit
    pipeline_stages:
      - match:
          selector: '{unit="gadgetron.service"}'
          stages:
            - regex:
                expression: '^(?P<ts>[0-9T:.Z-]+)\s+(?P<level>[A-Z]+)\s+(?P<target>[a-z_.]+):\s+(?P<msg>.*)$'
            - labels:
                level:
                  target:
```

Sample LogQL queries after that ships:

```logql
# Chat audit rows per minute (should trend with your request volume)
sum by (tenant_id) (rate({unit="gadgetron.service",target="gadgetron_audit"}[1m]))

# 401 surge detector (>10 failures in 5m)
sum(count_over_time({unit="gadgetron.service"} |= "401" [5m]))

# Deprecation warnings (flag before a minor-bump release)
{unit="gadgetron.service",target="cli_deprecation"}
```

**stdout → Fluentd / ELK / CloudWatch**: any log-forwarder that reads systemd journal or stdout works; Gadgetron's output is plain text unless the operator configures a `tracing_subscriber::fmt::json()` layer (feature not exposed via TOML on trunk — requires a code tweak to `crates/gadgetron-cli/src/telemetry.rs`).

#### 7.3 Alerting signals

Seven signals that catch most real production failures. Each row gives the signal, a sample rule, and the fix path.

| Signal | Rule (LogQL-ish) | Typical cause / fix |
|---|---|---|
| Process died | `/health` returns connection-refused for 30s | systemd would restart per `Restart=on-failure`; page if restart loop detected. |
| Database unhealthy | `/ready` returns 503 for 60s | pgvector container exited, connection-pool exhaustion, network partition. `psql "$GADGETRON_DATABASE_URL" -c '\l'` from the gadgetron host to confirm. |
| Migration failure | `_sqlx_migrations.success = false` OR last row `installed_on` is older than the deploy | sqlx rolls back failed migrations — re-deploy or hand-apply per §6.4. |
| 401 surge | `rate({target="gadgetron_audit",status="error"}[5m]) > baseline×10` | Brute-force attempt or client regression — cross-check `api_keys.revoked_at` for legitimate keys, `audit_log.api_key_id = nil` for anonymous 401s. |
| 429 pressure | `/admin/billing/events` showing bursts near `quota_configs.daily_used_cents` limits | Quota too tight for observed load — tune `[quota_rate_limit]` per `configuration.md §Production tuning recipes`. |
| Penny subprocess stuck | `{target="penny_subprocess"}` entries without a matching completion for > `request_timeout_secs` | Claude Code subprocess hung — check `ps -ef \| grep claude`, verify `[agent].request_timeout_secs` isn't higher than the reverse-proxy read timeout. |
| Disk growth | `audit_log` / `billing_events` / `tool_audit_events` row count growing with no pruning cron | Apply `auth.md §Audit log retention and tenant lifecycle` pruning recipe. |

#### 7.4 What's not provided

- **No Prometheus `/metrics` endpoint** — if you need pull-based scrape, run an adapter (e.g. `mtail` over the journal) or submit a PR to add one. Pull-based metrics are tracked as a P2C observability ISSUE, not scheduled.
- **No pre-packaged Grafana dashboard** — the LogQL/SQL recipes above are the primitives; operators assemble panels to taste.
- **No built-in trace export (OTel)** — `tracing` is captured but not forwarded to OTLP today. Adding an `opentelemetry-otlp` exporter in `telemetry.rs` is a one-commit addition for operators who need distributed traces.

### 8. Post-deploy acceptance smoke test

`gadgetron doctor` covers pre-boot sanity (config + DB + provider reachability + `/health`). It does NOT prove end-to-end correctness — `/v1/chat/completions` round-trips, scope enforcement, billing + audit persistence. Run the acceptance script below after initial install, after every §6 upgrade, after every §5.4 restore, and after any `gadgetron.toml` change that edited providers or scopes.

The script returns non-zero on the first failure — wire it into your deploy pipeline's post-cutover gate, or run manually as a 30-second sanity check.

```sh
#!/usr/bin/env bash
# save as scripts/smoke.sh, chmod +x

set -euo pipefail

GAD="${GAD:-http://127.0.0.1:8080}"
KEY_USER="${KEY_USER:?set to an OpenAiCompat-scope gad_live_... key}"
KEY_MGMT="${KEY_MGMT:?set to a Management-scope gad_live_... key}"
MODEL="${MODEL:-gpt-4o-mini}"   # or whatever you configured
FAILED=0

check() { printf "  %-48s" "$1:"; }
ok()    { echo "PASS"; }
fail()  { echo "FAIL — $1"; FAILED=$((FAILED+1)); }

echo "Gadgetron acceptance smoke @ $GAD"
echo

# --- 1. Liveness + readiness --------------------------------------------------
check "GET /health returns 200"
curl -fsS "$GAD/health" >/dev/null && ok || fail "gadgetron process is not live"

check "GET /ready returns 200"
curl -fsS "$GAD/ready" >/dev/null && ok || fail "database pool is unhealthy"

# --- 2. Model discovery -------------------------------------------------------
check "GET /v1/models includes $MODEL"
MODELS=$(curl -fsS "$GAD/v1/models" -H "Authorization: Bearer $KEY_USER" | jq -r '.data[].id')
echo "$MODELS" | grep -qx "$MODEL" && ok || fail "configured model '$MODEL' not surfaced by any provider"

# --- 3. Auth rejection sanity ------------------------------------------------
check "wrong key returns 401"
STATUS=$(curl -s -o /dev/null -w '%{http_code}' "$GAD/v1/chat/completions" \
  -H "Authorization: Bearer gad_live_definitely_not_real_key_0000000" \
  -H 'Content-Type: application/json' \
  -d '{"model":"'"$MODEL"'","messages":[{"role":"user","content":"hi"}]}')
[ "$STATUS" = "401" ] && ok || fail "expected 401, got $STATUS"

check "OpenAiCompat key on /admin returns 403"
STATUS=$(curl -s -o /dev/null -w '%{http_code}' \
  "$GAD/api/v1/web/workbench/admin/billing/events?limit=1" \
  -H "Authorization: Bearer $KEY_USER")
[ "$STATUS" = "403" ] && ok || fail "scope enforcement broken — expected 403, got $STATUS"

# --- 4. Chat round-trip (non-streaming) --------------------------------------
check "POST /v1/chat/completions returns 200 + content"
RESP=$(curl -fsS "$GAD/v1/chat/completions" \
  -H "Authorization: Bearer $KEY_USER" \
  -H 'Content-Type: application/json' \
  -d '{"model":"'"$MODEL"'","messages":[{"role":"user","content":"reply only with the word OK"}],"max_tokens":8}')
CONTENT=$(echo "$RESP" | jq -r '.choices[0].message.content // empty')
[ -n "$CONTENT" ] && ok || fail "empty response content — upstream provider issue; see RUST_LOG=gadgetron_provider=debug"

# --- 5. Streaming round-trip --------------------------------------------------
check "POST /v1/chat/completions stream reaches [DONE]"
STREAM=$(curl -fsS -N "$GAD/v1/chat/completions" \
  -H "Authorization: Bearer $KEY_USER" \
  -H 'Content-Type: application/json' \
  -d '{"model":"'"$MODEL"'","messages":[{"role":"user","content":"one word"}],"max_tokens":4,"stream":true}')
echo "$STREAM" | grep -q "^data: \[DONE\]" && ok || fail "stream truncated — upstream hung or provider returned SSE error"

# --- 6. Persistence sanity ---------------------------------------------------
# The chat above should have produced an audit_log row AND a billing_events row.
# Wait briefly for fire-and-forget writes to settle.
sleep 2

check "audit_log grew by ≥1 row in the last minute"
AUDIT_COUNT=$(psql "$GADGETRON_DATABASE_URL" -tAc \
  "SELECT COUNT(*) FROM audit_log WHERE timestamp > NOW() - INTERVAL '1 minute'")
[ "${AUDIT_COUNT:-0}" -ge 1 ] && ok || fail "audit writer not persisting — check RUST_LOG=gadgetron_audit"

check "billing_events grew by ≥1 chat row in the last minute"
BILL_COUNT=$(psql "$GADGETRON_DATABASE_URL" -tAc \
  "SELECT COUNT(*) FROM billing_events WHERE event_kind = 'chat' AND created_at > NOW() - INTERVAL '1 minute'")
[ "${BILL_COUNT:-0}" -ge 1 ] && ok || fail "billing enforcer not persisting — check the PgQuotaEnforcer pool wiring"

# --- 7. Optional: Penny registration ----------------------------------------
# Uncomment if your deployment expects penny in /v1/models
# check "Penny registered (model=penny available)"
# echo "$MODELS" | grep -qx penny && ok || fail "[knowledge] invalid; see RUST_LOG=knowledge_config"

# --- 8. Optional: cookie-session login (ISSUE 15/16) ------------------------
# Uncomment + fill if you want to smoke the cookie-auth path. Requires
# a provisioned user + password; see manual/multiuser.md §2.
# USER_EMAIL="smoke@example.com"; USER_PW="..."
# check "POST /api/v1/auth/login returns 200 + cookie"
# CODE=$(curl -sS -c /tmp/smoke.jar -o /dev/null -w '%{http_code}' \
#   -H 'Content-Type: application/json' \
#   -d "{\"email\":\"$USER_EMAIL\",\"password\":\"$USER_PW\"}" \
#   "$GAD/api/v1/auth/login")
# [ "$CODE" = "200" ] && ok || fail "cookie-session login broken"

echo
if [ "$FAILED" -gt 0 ]; then
  echo "FAIL — $FAILED check(s) failed"; exit 1
else
  echo "OK — all acceptance checks passed"; exit 0
fi
```

**Wiring into a deploy pipeline** (systemd `ExecStartPost` pattern for automatic post-boot verification):

```ini
# /etc/systemd/system/gadgetron.service.d/acceptance.conf
[Service]
ExecStartPost=/bin/bash -c 'sleep 5 && GAD=http://127.0.0.1:8080 KEY_USER=$GAD_KEY_USER KEY_MGMT=$GAD_KEY_MGMT /usr/local/bin/gadgetron-smoke || systemctl stop gadgetron'
```

The `systemctl stop` on failure gives the LB's `/ready` probe a clear 503 signal and lets `Restart=on-failure` retry a fixed number of times before giving up — preventing a broken deploy from staying up silently.

**What the smoke test does NOT cover** — keep these in your post-cutover manual sweep:

- `/web` + `/web/wiki` + `/web/dashboard` browser render (needs a headless browser; use the harness Playwright gates as the in-repo reference)
- Penny tool-call round-trip (needs `claude` subprocess + MCP; long-lived and has its own `manual/evaluation.md` harness for that)
- Bundle marketplace install/uninstall (`api-reference.md §Bundle operator recipes` covers the 5-step flow)
- SearXNG / web.search path (set `KEY_USER` to a SearXNG-enabled tenant and probe `eval/run_eval.py --scenario web-search-direct-query`)
- Full auth flow including cookie-session + scope-synthesis (only 401/403 smoke in step 3 — the full matrix is in the harness)

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
| `sshpass is not installed on the gadgetron host` | Install `sshpass` on the machine running Gadgetron (`sudo apt-get install sshpass` or `brew install sshpass`), then retry `server-add` `password_bootstrap` |
| `cargo build` timeout or OOM | Ensure at least 4 GB RAM and 2 GB disk for compilation |
