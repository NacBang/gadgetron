# Quickstart: local demo with PostgreSQL + `demo.sh`

This is the canonical local operator path for Gadgetron trunk. The supported local loop is:

- `./demo.sh build`
- `./demo.sh start`
- `./demo.sh status`
- `./demo.sh logs`
- `./demo.sh stop`

The demo path assumes a PostgreSQL server with the `vector` extension available. A plain PostgreSQL image without `pgvector` is not sufficient for the current knowledge-backed runtime.

---

## Step 1 — Start a pgvector-enabled PostgreSQL

Follow [installation.md §Step 4 PostgreSQL setup](installation.md#step-4-postgresql-setup) (Ubuntu) or [installation.md §Step 5 PostgreSQL setup](installation.md#step-5-postgresql-setup) (macOS) to start the `pgvector/pgvector:pg16` container. Then wait until PostgreSQL is ready:

```sh
docker exec gadgetron-pgvector pg_isready -U gadgetron -d gadgetron_demo
```

Expected output: `localhost:5432 - accepting connections`

---

## Step 2 — Build the release binary

```sh
git clone https://github.com/NacBang/gadgetron.git
cd gadgetron
./demo.sh build
```

`./demo.sh build` compiles `gadgetron-cli` in release mode and prepares the binary that `./demo.sh start` will run.

---

## Step 3 — Generate a baseline config, then enable a provider

Generate an annotated `gadgetron.toml`:

```sh
./target/release/gadgetron init --yes
```

Then edit `gadgetron.toml` and enable at least one provider. The fastest success path is OpenAI:

```toml
[server]
bind = "127.0.0.1:8080"

[providers.openai]
type = "openai"
api_key = "${OPENAI_API_KEY}"
models = ["gpt-4o-mini"]
```

If you want the full assistant surface (`penny` model + `/web`), add the canonical `[agent]`, `[agent.brain]`, and `[knowledge]` blocks from [penny.md](penny.md). Do not rely on legacy `[penny]` examples.

---

## Step 4 — Export runtime environment

```sh
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@127.0.0.1:5432/gadgetron_demo"
export OPENAI_API_KEY="sk-your-openai-key"
```

If you are using a self-hosted provider such as vLLM or Ollama, export only the variables your chosen provider block requires.

Also export the admin-bootstrap password (used by Step 4.5):

```sh
export GADGETRON_ADMIN_PW="pick-a-real-password-here"
```

The env var **name** is referenced from `gadgetron.toml` in Step 4.5. Choose any name you like (the examples use `GADGETRON_ADMIN_PW`); keep the value in your shell / secret manager, never in the config file.

---

## Step 4.5 — Configure first-admin bootstrap

Every pg-backed `gadgetron serve` run checks the `users` table at startup. On a **fresh database** the table is empty and serve will **hard-fail** unless `[auth.bootstrap]` is set in `gadgetron.toml`. The block is the only supported path to create the first admin without hand-crafting SQL (ISSUE 14 TASK 14.2 / v0.5.7).

Open `gadgetron.toml` from Step 3 and add:

```toml
[auth.bootstrap]
admin_email         = "admin@example.com"
admin_display_name  = "Initial Admin"
admin_password_env  = "GADGETRON_ADMIN_PW"     # NAME of the env var set in Step 4
```

Three rules to remember:

- `admin_password_env` holds the env var **name**, not the password. The gateway reads `std::env::var("GADGETRON_ADMIN_PW")` at startup → argon2id PHC hash → `users.password_hash`.
- If the named env var is unset or empty, startup fails with `bootstrap requires $GADGETRON_ADMIN_PW to be set`. Set it before `./demo.sh start`.
- On subsequent restarts, `[auth.bootstrap]` is ignored with a warn log (the users table is no longer empty). Remove the block on the next deploy to keep secrets out of the config file.

`--no-db` deployments skip this entirely — there is no users table to check. See [configuration.md §`[auth.bootstrap]`](configuration.md#authbootstrap) for the full behavior matrix.

---

## Step 5 — Start the demo and verify health

```sh
./demo.sh start
./demo.sh status
./demo.sh logs
```

`status` should report a running process and an `OK` health check. If startup fails with a `vector` extension message, your PostgreSQL server is reachable but does not provide `pgvector`; replace it with a pgvector-enabled server and retry.

For a live tail:

```sh
./demo.sh logs -f
```

---

## Step 6 — Create a tenant and API key

The server must be running for migrations and key management.

```sh
./target/release/gadgetron tenant create --name "my-team"
./target/release/gadgetron key create --tenant-id <tenant_uuid>
```

The second command prints the raw `gad_live_...` key once to **stderr** (SEC-M7: prevents accidental capture in scripts that pipe stdout). Use that Bearer token for API and Web UI access. For the full CLI surface (scopes, revocation, user / team management, reindex, doctor) see [cli.md](cli.md).

---

## Step 7 — Send the first request

```sh
curl -s http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer gad_live_replace_me" \
  -d '{
    "model": "gpt-4o-mini",
    "messages": [{"role": "user", "content": "Say hello in one sentence."}],
    "stream": false
  }' | jq .
```

If you enabled Penny, `GET /v1/models` should also include `penny`, and `http://127.0.0.1:8080/web` should serve the embedded Web UI (chat shell).

**Try the browser wiki workbench.** Since 0.2.0 the same `[knowledge]` config also serves a browser-driven wiki CRUD UI at `http://127.0.0.1:8080/web/wiki` (standalone URL) or as the "Wiki" left-rail tab inside `/web`. Click a seed page in the left list to exercise the read → edit → save loop — `wiki-read` fetches content, the Markdown renders inline (react-markdown + remark-gfm), a toast confirms on save. E2E Gate 11d drives this same loop in a real Chromium under the harness. See [web.md §/web/wiki](web.md#web-wiki--브라우저-워크벤치-wiki-crud) for the full UI-to-action mapping.

**Try the operator dashboard.** Since 0.2.7 `/web/dashboard` ships as a third sibling tab (Chat / Wiki / Dashboard) with tenant-scoped live tiles for chat, direct-action, and Penny tool planes — backed by `GET /usage/summary` for the 24-hour rollup and a `/events/ws` WebSocket for real-time `ChatCompleted` events. See [web.md §`/web/dashboard`](web.md#webdashboard--operator-observability-issue-4--v027). Gates 7k.3 + 11f cover the shape and the page render.

---

## Step 8 — Stop the demo

```sh
./demo.sh stop
```

---

## Notes

- `./demo.sh start` auto-rebuilds the release binary if tracked source files are newer than `target/release/gadgetron`, unless you explicitly set `GADGETRON_DEMO_SKIP_BUILD=1`.
- `./demo.sh start` also checks the target PostgreSQL and enables `CREATE EXTENSION vector` automatically when the server provides it but the current database has not enabled it yet.
- For install prerequisites, see [installation.md](installation.md).
- For Web UI operation, see [web.md](web.md).
- For Penny-specific config, see [penny.md](penny.md).
