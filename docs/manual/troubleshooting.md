# Troubleshooting

Each entry describes what you will observe, why it happens, and the exact steps to fix it.

---

## `gadgetron doctor` — automated pre-flight check

Before digging into individual errors, run `gadgetron doctor`. It checks the most common failure points and prints a pass/fail result for each:

```sh
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@localhost:5432/gadgetron"
./target/release/gadgetron doctor
```

`gadgetron doctor` runs 5 checks in order: config file, server port, database, each configured provider, and `/health`. Example output (all checks passing):

```
Gadgetron v0.2.0 — System Check

  [PASS] Config file:      gadgetron.toml found and valid TOML
  [PASS] Server port:      0.0.0.0:8080 available
  [PASS] Database:         GADGETRON_DATABASE_URL configured
  [PASS] Provider openai:  https://api.openai.com reachable in 142ms
  [PASS] /health:          gadgetron is running at http://localhost:8080/health

  All checks passed.
```

Example output (with a failure):

```
Gadgetron v0.2.0 — System Check

  [PASS] Config file:      gadgetron.toml found and valid TOML
  [PASS] Server port:      0.0.0.0:8080 available
  [WARN] Database:         database_url not configured — running in no-db mode
  [FAIL] Provider openai:  https://api.openai.com unreachable — error sending request
  [FAIL] /health:          connection refused at http://localhost:8080/health — run: gadgetron serve

  1 warning(s) found.
  WARN: database_url not configured — running in no-db mode
  2 failure(s) found.
  FAIL: https://api.openai.com unreachable — error sending request
  FAIL: connection refused at http://localhost:8080/health — run: gadgetron serve
```

`gadgetron doctor` exits with status 0 if all checks pass (warnings do not fail the run), or status 2 if any check is `[FAIL]`. CI pre-flight scripts should check for exit code `!= 0` or explicitly `== 2`, not `== 1`.

---

## Server startup errors

### "GADGETRON_DATABASE_URL is not set"

**What happened:** A PostgreSQL-backed CLI command such as `gadgetron tenant create`, `gadgetron key list`, `gadgetron key revoke`, or `gadgetron key create --tenant-id ...` exited with this message.

**Why:** Those commands require a live PostgreSQL database. `gadgetron serve` itself can still run without PostgreSQL in no-db mode.

**Fix:**

```sh
export GADGETRON_DATABASE_URL="postgres://user:password@localhost:5432/gadgetron"
./target/release/gadgetron tenant create --name "my-team"
```

If you intended to run only the gateway locally, use the no-db flow instead:

```sh
./target/release/gadgetron key create
./target/release/gadgetron serve --no-db
```

---

### "failed to connect to PostgreSQL"

**What happened:** `GADGETRON_DATABASE_URL` is set but the server could not open a connection to PostgreSQL within 5 seconds.

**Why:** PostgreSQL is not running, the host/port is wrong, the credentials are wrong, or the database does not exist.

**Fix — verify PostgreSQL is running:**

```sh
pg_isready -h localhost -p 5432
```

Expected: `localhost:5432 - accepting connections`

**Fix — test the connection string directly:**

```sh
psql "$GADGETRON_DATABASE_URL" -c "SELECT 1;"
```

If `psql` fails, the connection string is wrong. Common mistakes:
- Wrong port (default PostgreSQL port is 5432)
- Wrong database name (must exist; `CREATE DATABASE gadgetron;` if needed)
- Wrong password (check `pg_hba.conf` authentication method)

---

### "failed to run database migrations"

**What happened:** The server connected to PostgreSQL but failed to apply the schema migrations.

**Why:** The database user lacks `CREATE TABLE` privileges, or a previous partial migration left the schema in an inconsistent state.

**Fix — verify privileges:**

```sh
psql "$GADGETRON_DATABASE_URL" -c "\du"
```

The Gadgetron database user needs at minimum: `CREATE`, `USAGE` on the schema, and `CONNECT` on the database.

**Fix — grant privileges:**

```sql
GRANT ALL PRIVILEGES ON DATABASE gadgetron TO gadgetron_user;
```

If the schema is partially broken, drop and recreate the database (development only):

```sh
psql -U postgres -c "DROP DATABASE IF EXISTS gadgetron;"
psql -U postgres -c "CREATE DATABASE gadgetron OWNER gadgetron_user;"
```

---

### `penny`가 `/v1/models`에 나타나지 않음

**What happened:** 서버는 기동됐지만 `GET /v1/models` 응답에 `penny`가 없습니다.

**Why:** Penny는 일반 provider와 다르게 `gadgetron.toml`의 `[knowledge]` 섹션이 유효할 때만 런타임에 등록됩니다. `[knowledge]`가 없거나, `wiki_path` 부모 디렉터리가 없거나, 설정 검증에 실패하면 서버는 계속 뜨지만 Penny 등록만 건너뜁니다.

**Fix — 설정 확인:**

```toml
[knowledge]
wiki_path = "./.gadgetron/wiki"
wiki_autocommit = true
wiki_max_page_bytes = 1048576
```

그리고 부모 디렉터리를 미리 만드십시오:

```sh
mkdir -p .gadgetron
```

**Fix — 로그 확인:**

정상 경로에서는 startup log에 `penny: registered`가 남습니다. 실패 경로에서는 `penny: [knowledge] validation failed; skipping` 또는 `failed to register KnowledgeGadgetProvider` 같은 로그가 남습니다.

---

### "failed to load config from gadgetron.toml"

**What happened:** The server found `gadgetron.toml` but could not parse it.

**Why:** The TOML file has a syntax error or an invalid field value.

**Fix:** Validate the TOML syntax:

```sh
# Using Python's tomllib (Python 3.11+):
python3 -c "import tomllib; tomllib.loads(open('gadgetron.toml').read())"

# Or use taplo:
taplo check gadgetron.toml
```

The error message from the server includes the specific field that failed to parse. Look for:
- Mismatched quotes
- Wrong `type` value for a provider
- Missing required fields within a section that is present

---

### "failed to bind to 0.0.0.0:8080"

**What happened:** The server started but could not open the TCP listener.

**Why:** Another process is already using port 8080, or you do not have permission to bind to that address.

**Fix — find what is using the port:**

```sh
lsof -i :8080
```

**Fix — use a different port:**

```sh
GADGETRON_BIND=0.0.0.0:9000 ./target/release/gadgetron
```

Or change `[server].bind` in `gadgetron.toml`.

---

## Request errors

### HTTP 401 Unauthorized — invalid or missing API key

**What you observe:**

```json
{
  "error": {
    "message": "Invalid API key. Verify your API key is correct and has not been revoked.",
    "type": "authentication_error",
    "code": "invalid_api_key"
  }
}
```

**Why:** One of the following:
1. The `Authorization` header is absent from the request
2. The header does not use the `Bearer ` prefix (note the space after Bearer)
3. The key does not start with `gad_`
4. The key is shorter than the minimum length (`gad_` + `live`/`test` + `_` + at least 16 characters)
5. The key hash does not match any active row in `api_keys`
6. The key has been revoked (`revoked_at IS NOT NULL`)

**Fix — check the request header:**

```sh
# Correct format:
curl ... -H "Authorization: Bearer gad_live_your32chartoken00000000000"

# Common mistakes:
# Missing space:    -H "Authorization: Beargad_live_..."
# Wrong prefix:     -H "Authorization: Bearer sk-openai-key"
# Bare token:       -H "Authorization: gad_live_..."
```

**Fix — verify the key exists and is not revoked:**

```sql
SELECT id, tenant_id, prefix, kind, scopes, revoked_at
FROM api_keys
WHERE key_hash = 'your-64-char-sha256-of-key-here';
```

If `revoked_at` is not null, the key is revoked. If no row is found, the key was never inserted or the hash is wrong. Recompute the hash:

```sh
echo -n 'gad_live_your_exact_key_string' | sha256sum | cut -d' ' -f1
```

**Fix — check the tenant is Active:**

```sql
SELECT t.status
FROM tenants t
JOIN api_keys k ON k.tenant_id = t.id
WHERE k.key_hash = 'your-64-char-hash';
```

If `status` is `Suspended` or `Deleted`, the tenant cannot authenticate. Restore the tenant status or create a new tenant.

---

### HTTP 403 Forbidden — wrong scope

**What you observe:**

```json
{
  "error": {
    "message": "Your API key does not have permission for this operation. Check your key's assigned scopes.",
    "type": "permission_error",
    "code": "forbidden"
  }
}
```

**Why:** The API key is valid but lacks the scope required by the route.

| If you requested... | You need scope... |
|--------------------|-------------------|
| `POST /v1/chat/completions` | `OpenAiCompat` |
| `GET /v1/models` | `OpenAiCompat` |
| `GET /api/v1/nodes` | `Management` |
| `POST /api/v1/models/deploy` | `Management` |

**Fix — check the key's current scopes:**

```sql
SELECT scopes FROM api_keys WHERE key_hash = 'your-64-char-hash';
```

**Fix — add the required scope to the key:**

```sql
UPDATE api_keys
SET scopes = array_append(scopes, 'Management')
WHERE key_hash = 'your-64-char-hash';
```

After updating, the next request will use the new scopes (cache TTL is 10 minutes; to take effect immediately, the server must be restarted or the cache entry must expire naturally).

---

### HTTP 429 Quota Exceeded

**What you observe:**

```json
{
  "error": {
    "message": "Your API usage quota has been exceeded. Update quota_configs table to increase limits, or see docs/manual/troubleshooting.md.",
    "type": "quota_error",
    "code": "quota_exceeded"
  }
}
```

**Why:** The tenant's `daily_limit_cents` has been reached. The current quota enforcer is in-memory (`InMemoryQuotaEnforcer`). It checks the `daily_used_cents` value from the `quota_configs` table against the `daily_limit_cents`. If `daily_limit_cents - daily_used_cents <= 0`, requests are rejected.

**Fix — increase the daily limit:**

```sql
UPDATE quota_configs
SET daily_limit_cents = 500000   -- $5,000 USD (values are in cents)
WHERE tenant_id = 'your-tenant-uuid-here';
```

**Fix — reset daily usage (for testing):**

```sql
UPDATE quota_configs
SET daily_used_cents = 0
WHERE tenant_id = 'your-tenant-uuid-here';
```

**Note:** `InMemoryQuotaEnforcer` does not currently write usage back to PostgreSQL — `record_post` only marks the in-memory token as used. The `daily_used_cents` column in the database is not incremented by the current implementation. Until PostgreSQL-backed quota enforcement lands, 429 is triggered only when `daily_limit_cents - daily_used_cents <= 0` at request time (based on whatever value is already in the database when the tenant context is loaded).

---

### GET /ready returns HTTP 503

**What you observe:** `curl http://localhost:8080/ready` returns HTTP 503.

**Why:** The PostgreSQL connection pool health check failed. This means the server either cannot reach PostgreSQL at all, or the pool is fully exhausted (all 20 connections in use and the acquire timeout of 5 seconds was exceeded).

**Fix — verify PostgreSQL is reachable from the server host:**

```sh
pg_isready -h localhost -p 5432
```

Expected: `localhost:5432 - accepting connections`

**Fix — test the connection string directly:**

```sh
psql "$GADGETRON_DATABASE_URL" -c "SELECT 1;"
```

**Fix — check active connections against the pool limit:**

```sql
SELECT count(*) FROM pg_stat_activity
WHERE datname = 'gadgetron';
```

The pool maximum is 20 connections. If your PostgreSQL `max_connections` is lower than 20 (or other applications are consuming connections), the pool cannot fill and the readiness check fails.

**Fix — increase PostgreSQL `max_connections`** (in `postgresql.conf`):

```
max_connections = 100
```

Restart PostgreSQL after changing this value.

**Note:** `GET /health` always returns HTTP 200 regardless of PostgreSQL state. If `/health` is 200 but `/ready` is 503, the gateway process is running but the database is unavailable. Authenticated requests will fail with HTTP 503 (`db_pool_timeout`) until the database is restored.

---

### HTTP 503 Service Unavailable — no providers configured

**What you observe:**

```json
{
  "error": {
    "message": "No suitable provider found for this request. Verify model availability and routing configuration.",
    "type": "invalid_request_error",
    "code": "routing_failure"
  }
}
```

**Why:** The server started with no providers configured (either `gadgetron.toml` has no `[providers]` section, or the file is absent), or all configured providers failed.

**Fix — add a provider to gadgetron.toml:**

```toml
[providers.openai]
type = "openai"
api_key = "${OPENAI_API_KEY}"
models = ["gpt-4o-mini"]
```

Then restart the server. The log line `INFO provider registered name=openai` confirms the provider was loaded.

**Fix — verify the server logs for provider registration:**

```
INFO provider registered name=openai
```

If this log line is absent, the provider config is either missing or failed to load. Check `gadgetron.toml` syntax.

---

### HTTP 502 Bad Gateway — provider error

**What you observe:**

```json
{
  "error": {
    "message": "The upstream LLM provider returned an error. Check provider status and API key validity.",
    "type": "api_error",
    "code": "provider_error"
  }
}
```

**Why:** The configured provider (OpenAI, Anthropic, Ollama) returned an error or is unreachable.

**Fix — test the provider API key directly:**

```sh
# OpenAI
curl -s https://api.openai.com \
  -H "Authorization: Bearer $OPENAI_API_KEY" | jq .error

# Anthropic
curl -s https://api.anthropic.com/v1/models \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" | jq .error
```

**Fix — check the model name is correct:**

The `model` field in the request must match a model ID in the provider's `models` list in `gadgetron.toml`. Use `GET /v1/models` to see what Gadgetron knows about:

```sh
curl -s http://localhost:8080/v1/models \
  -H "Authorization: Bearer gad_live_your_key_here" | jq .
```

---

### HTTP 422 or 400 — malformed request body

**What you observe:** HTTP 422 or HTTP 400 with no Gadgetron error body (axum returns this before the handler runs).

**Why:** The request body is not valid JSON, or required fields (`model`, `messages`) are missing.

**Fix:** Ensure the request has `Content-Type: application/json` and a complete body:

```sh
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer gad_live_your_key_here" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}'
```

---

### HTTP 413 — request body too large

**What you observe:**

```json
{
  "error": {
    "message": "Request body exceeds the 4 MiB limit. Reduce your request size or split it across multiple calls.",
    "type": "invalid_request_error",
    "code": "request_too_large"
  }
}
```

**Why:** The request body is larger than the gateway's 4 MiB (`4_194_304` bytes) hard limit. This limit is enforced **before** authentication, so it applies even to anonymous requests.

Typical causes:
1. Posting a very long conversation (100k+ tokens) without message pruning
2. Attaching large base64-encoded images or binary blobs to `content`
3. Copy-pasting a large file into a `content` field (common in interactive chat UIs)

**Fix — reduce context size:**
Trim the `messages` array or use a summarization pass before sending. For long conversations, keep only the last N turns plus a system message.

**Fix — split the request:**
If you are uploading a large document, chunk it into pieces that each fit under the limit and send them as separate turns.

**The 4 MiB figure is hard-coded** in `crates/gadgetron-gateway/src/server.rs` as `MAX_BODY_BYTES`. If you need a larger limit for a research workload, rebuild with the constant bumped. The 4 MiB default assumes a 128k-token context window at ~4 bytes/token + 8× headroom.

---

### HTTP 404 — `workbench_action_not_found` on `POST /actions/{id}`

**What you observe:**

```json
{
  "error": {
    "message": "action \"<id>\" not found",
    "type": "invalid_request_error",
    "code": "workbench_action_not_found"
  }
}
```

**Why:** Either (a) the `action_id` you posted is not in the catalog, or (b) the catalog has a descriptor with that id but its `required_scope` is not in your key's scope set — in which case the handler returns 404 instead of 403 to avoid leaking existence of scope-gated actions (matches `GET /views/{id}/data`).

**Fix:** Query `GET /api/v1/web/workbench/actions` first; the response lists every descriptor your scope set can invoke. If your target action is missing and you believe you should see it, rotate to a key whose scope set covers the descriptor's `required_scope`. E2E gate 7f locks the public list at five seed ids (`knowledge-search`, `wiki-list`, `wiki-read`, `wiki-write`, `wiki-delete`).

---

### HTTP 409 — `workbench_approval_already_resolved`

**What you observe:**

```json
{
  "error": {
    "message": "approval already resolved (state=approved)",
    "type": "invalid_request_error",
    "code": "workbench_approval_already_resolved"
  }
}
```

on `POST /api/v1/web/workbench/approvals/{approval_id}/approve` or `.../deny`.

**Why:** Approvals are terminal. Once a record moves to `approved` or `denied`, subsequent approve/deny calls are rejected — the store will not re-flip a resolved record. This typically surfaces when:
- A user clicks the approve button twice (common on laggy UIs).
- Two operators race to resolve the same card.
- An automation retried after a network hiccup that actually succeeded.

**Fix:** Read `state` from the error message (or `GET /audit/events?action_id=<id>`) to confirm the final outcome. If the first resolution was correct, nothing to do — the dispatch either already ran (on `approved`) or was refused (on `denied`). If the first resolution was wrong, there is no undo — record a new corrective action and document in the audit trail. E2E Gate 7h.7 exercises this path by asserting the second approve of the same id returns 409.

---

### HTTP 403 — cross-tenant approval forbidden

**What you observe:**

```json
{
  "error": {
    "message": "approval belongs to a different tenant",
    "type": "invalid_request_error",
    "code": "workbench_approval_forbidden"
  }
}
```

**Why:** The authenticated actor's tenant does not match the approval's tenant. This is a hard scope boundary — one tenant cannot approve or deny another tenant's pending actions even if scope would otherwise allow it. The original approval record stays `pending`; nothing about it changes.

**Fix:** Either switch to a key in the owning tenant, or ask that tenant's operator to resolve the approval. If you think this is a mis-configuration (wrong tenant attached to a key), inspect `TenantContext` via the server logs: every request tagged with `target: "scope"` prints `tenant_id = <uuid>` on entry.

---

### HTTP 400 — `approval store is not wired in this build`

**What you observe:**

```json
{
  "error": {
    "message": "approval store is not wired in this build",
    "type": "invalid_request_error",
    "code": "config_error"
  }
}
```

on the approve / deny / cancel endpoints, or similar 400 `audit event query requires Postgres` on `GET /audit/events`.

**Why:** ISSUE 3 / v0.2.6 stores approvals in an in-memory `InMemoryApprovalStore` by default, and writes `action_audit_events` rows to Postgres. When `gadgetron serve` is started without a Postgres pool (no `DATABASE_URL` + the `--no-db` evaluation path), the approval store still wires, but the PG-backed audit writer does not — `GET /audit/events` fails with `config_error`. Approve/deny continue to work against the in-memory store, but the resulting audit rows never reach Postgres, so the query endpoint has nothing to return.

**Fix:** Run with `DATABASE_URL` pointing at a pgvector-enabled Postgres (see `docs/manual/quickstart.md §Postgres setup`). For demo / no-db flows, expect `GET /audit/events` to 400 — use `tracing` logs (`target: "action_audit"`) to see the events that were emitted without being persisted.

---

### HTTP 400 — `tool audit query requires Postgres` (`/audit/tool-events`)

**What you observe:**

```json
{
  "error": {
    "message": "tool audit query requires Postgres (no pool configured)",
    "type": "invalid_request_error",
    "code": "config_error"
  }
}
```

on `GET /api/v1/web/workbench/audit/tool-events`.

**Why:** ISSUE 5 / v0.2.8 (PR #199) replaced the P2A-era `NoopGadgetAuditEventSink` with `run_gadget_audit_writer` backed by Postgres. Under `--no-db` the sink falls back to Noop — events still flow through `target: "penny_audit"` tracing logs, but never reach the `tool_audit_events` table. The query handler 400s rather than silently returning zero rows.

**Fix:** Run with `DATABASE_URL` pointing at a pgvector-enabled Postgres — same fix as for `/audit/events`. For demo / no-db flows, use `tracing` logs (`target: "penny_audit"`) to observe the event stream without persistence. The `/events/ws` WebSocket feed still publishes `ActivityEvent::ToolCallCompleted` from the writer task only when a pool is present, so Dashboard tiles showing "tools" totals will stay at zero under `--no-db`.

---

### HTTP 400 — `usage summary requires Postgres`

**What you observe:**

```json
{
  "error": {
    "message": "usage summary requires Postgres (no pool configured)",
    "type": "invalid_request_error",
    "code": "config_error"
  }
}
```

on `GET /api/v1/web/workbench/usage/summary`.

**Why:** ISSUE 4 / v0.2.7 `/usage/summary` runs three aggregate SQL queries against the three audit tables (`audit_log`, `action_audit_events`, `tool_audit_events`) in parallel via `tokio::join!`. Without a Postgres pool the handler can't issue those queries, so it returns 400 instead of silently returning zeros. This matches the behaviour of `GET /audit/events` (which also 400s without a pool).

**Fix:** Same as the approval-store case above — provision `DATABASE_URL` pointing at a pgvector-enabled Postgres. The `/web/dashboard` page that consumes this endpoint will show its auth gate with an error toast when the underlying request 400s.

---

### `/events/ws` closes immediately with a `{"type":"lag",…}` frame

**What you observe:** WebSocket connects, the server sends a JSON text frame:

```json
{"type":"lag","missed":42,"message":"subscriber lagged; reconnect to resume"}
```

then closes the socket.

**Why:** ISSUE 4 / v0.2.7 `/events/ws` is backed by a bounded `tokio::sync::broadcast::channel`. When your subscriber falls behind the publisher rate (network lag, tab backgrounded, client CPU-starved), the broadcast receiver reports `RecvError::Lagged(N)` — the `N` most recent events were dropped before you got to them. Silently swallowing the drop would hide real overflow; the server instead sends the explicit `lag` frame then closes so the client has a definitive signal.

**Fix:** Reconnect the WebSocket and re-sync baseline via `GET /usage/summary`. Do NOT try to infer "missed" events from the tile deltas — `/usage/summary` provides the authoritative window counters. The `/web/dashboard` page implements this reconnect loop (open WS → on lag frame, re-fetch summary → reopen WS). If lag frames fire repeatedly under low load, investigate publisher-side bursts (many concurrent chat completions) or an undersized channel — the channel capacity is compiled in at `crates/gadgetron-core/src/activity_bus.rs`.

---

### `/events/ws` upgrade returns HTTP 401 from the browser

**What you observe:** Browser `new WebSocket(url)` fails with a 401 during upgrade; console shows `WebSocket connection to 'wss://…/events/ws' failed`.

**Why:** Browsers cannot attach an `Authorization: Bearer …` header to WebSocket upgrades. The gateway's auth middleware therefore accepts a **query-token fallback** scoped ONLY to `/events/ws`: append `?token=gad_live_…` to the URL. If the token is missing, malformed, or belongs to another tenant, the upgrade is rejected with 401 before the protocol switches.

**Fix:** Build the URL with the key appended, e.g. `wss://localhost:8080/api/v1/web/workbench/events/ws?token=gad_live_xxxxx`. The `/web/dashboard` page does this automatically. Server-side, the auth middleware strips `?token=` from the request URI before the request-id and trace lines log, so keys don't appear in `gadgetron.log`. Non-browser clients (curl, websocat, Rust `tokio-tungstenite`) should continue using the `Authorization` header — the query-token fallback is browser-only scaffolding.

---

### `/workbench/activity` returns `{"entries": [], "is_truncated": false}` even after real Penny tool calls

**What you observe:** `GET /api/v1/web/workbench/activity` returns an empty array regardless of traffic — direct-action invocations AND Penny tool calls both ran during your session, but the endpoint reports nothing.

**Why:** This is the **current state**, not a bug. The Penny tool-call + direct-action write paths that feed `/activity` are both live as of v0.2.9:

- Direct-action: `InProcessWorkbenchActionService::invoke` produces `CapturedActivityEvent { origin: UserDirect, kind: DirectAction }` since ISSUE 3 / v0.2.6.
- Penny tool-call: `GadgetAuditEventWriter::with_coordinator()` fans out each tool call to `CapturedActivityEvent { origin: Penny, kind: GadgetToolCall }` since ISSUE 6 / v0.2.9 (PR #201).

But the **read projection** — reading from the coordinator's backing store into `WorkbenchActivityResponse.entries` — has not been wired yet. `InProcessWorkbenchProjection::activity` still returns an empty vector (see [api-reference.md §/workbench/activity](api-reference.md#get-apiv1webworkbenchactivity)). That's the PSL-1 read-path work tracked for a future ISSUE.

**Fix (inspect the captured rows today):**

- `tracing` logs: filter on `target: "action_audit"` for direct-action rows and `target: "penny_audit"` for Penny tool-call rows. The `CapturedActivityEvent` fan-out emits an `INFO`-level span per row so operators can eyeball attribution without a read-side endpoint.
- `/audit/events` (direct-action side) and `/audit/tool-events` (Penny side) both query the durable audit tables directly. If what you want is "which actions ran in the last hour" rather than the activity feed's richer `CapturedActivityEvent` shape, the two audit endpoints are the shipped read path today.
- Dashboard tiles: `GET /usage/summary` + `/events/ws` see the live counts/events. The `/events/ws` WebSocket publishes `ActivityEvent::ChatCompleted` + `::ToolCallCompleted`, which are the broadcast-bus cousins of the durable `CapturedActivityEvent` rows (different path, overlapping signal).

**When does this endpoint start returning rows?** When a future ISSUE wires the coordinator→projection read path. No blocker remains on the write side — both origins produce rows today.

---

### HTTP 404 — `mcp_unknown_tool` on `POST /v1/tools/{name}/invoke`

**What you observe:**

```json
{
  "error": {
    "message": "unknown tool: does.not.exist",
    "type": "invalid_request_error",
    "code": "mcp_unknown_tool"
  }
}
```

**Why:** ISSUE 7 / v0.2.10+ exposes `GET /v1/tools` as the catalog + `POST /v1/tools/{name}/invoke` as dispatch. The 404 surfaces when the `{name}` in the URL is not in the registry, OR the tool exists but is filtered out of the calling key's view (same masking principle as `workbench_action_not_found` — 404 instead of 403 so scope-gated tool existence doesn't leak).

**Fix:** Query `GET /v1/tools` first; it returns the full `{tools: [...], count}` slice the calling key can invoke. If the expected tool is missing, inspect gateway startup logs for `tracing` target `penny_registry` — a tool set to `never` / `ask` mode in `[agent.gadgets.*]` gets dropped from the registry at freeze time. E2E Gate 7i.3 pins the `mcp_unknown_tool` error code for unknown names as part of the MCP error taxonomy.

---

### `/v1/tools/{name}/invoke` returns success but `tool_audit_events` has no row

**What you observe:** External MCP client (claude-code, custom agent) successfully invokes a tool, gets a `{content, is_error:false}` response, but `GET /api/v1/web/workbench/audit/tool-events?tool_name=<x>` returns zero rows.

**Why:** ISSUE 7 TASK 7.3 (PR #207) wires the audit fan-out behind a Postgres pool. Without `DATABASE_URL` (= `--no-db` or unwired demo), the `GadgetAuditEventWriter` falls back to `NoopGadgetAuditEventSink` and the `tool_audit_events` INSERT never happens — the invoke still succeeds end-to-end, but leaves no durable trail.

Parallel failure on the read side: `GET /audit/tool-events` also 400s without a pool (see §HTTP 400 — `tool audit query requires Postgres`).

**Fix:** Configure `DATABASE_URL` pointing at a pgvector-enabled Postgres. For demo / no-db evaluation, inspect `tracing` logs with filter `target: "penny_audit"` to observe the in-memory event stream that would have persisted. External-MCP vs Penny-internal attribution is encoded in `owner_id`: external calls set `Some(api_key_id)`, Penny-internal calls set `None` — Gate 7i.4 asserts this invariant.

---

### HTTP 403 — `scope_required` on `POST /api/v1/web/workbench/admin/reload-catalog`

**What you observe:** Calling the catalog hot-reload endpoint with a workbench / chat API key returns:

```json
{"error": {"code": "scope_required", "message": "..."}}
```

with HTTP 403.

**Why:** ISSUE 8 TASK 8.2 (PR #213 / v0.4.2) added the `/api/v1/web/workbench/admin/` sub-tree for privileged catalog operations. `scope_guard_middleware` gates this prefix at `Scope::Management` — **not** `OpenAiCompat` — because letting workbench users self-reload the catalog would turn the read-side scope masking into a no-op (self-reload lets a caller reshape what they can then see). The rule fires **before** the broader `/api/v1/web/workbench/*` OpenAiCompat rule, so the 403 comes from the scope layer, not from the handler. Harness Gate 7q.2 pins this exact contract.

**Fix:** Use a `Management`-scoped API key. Create one with `gadgetron key create --tenant-id <uuid> --scope management` (the CLI surface accepts the scope name; verify with `gadgetron key list`). See `docs/manual/auth.md` §Scope system for the full scope matrix. The same key works for the rest of the `/api/v1/*` operator surface (`/api/v1/nodes`, `/api/v1/usage`, etc.).

---

### HTTP 400 — `config_error` on `POST /api/v1/web/workbench/admin/reload-catalog`

**What you observe:** The endpoint returns HTTP 400 with a `config_error` body mentioning "catalog reload requires a configured workbench with a descriptor catalog handle" — **even with a Management-scoped key**.

**Why:** The handler reads `state.workbench.descriptor_catalog` — the shared `Arc<ArcSwap<CatalogSnapshot>>` handle wired in TASK 8.1 / PR #211 (originally `Arc<ArcSwap<DescriptorCatalog>>`, widened to `CatalogSnapshot` in TASK 8.3 / PR #214 so the handle carries both catalog and pre-compiled JSON-schema validators). Production `build_workbench` always sets the field, so a 400 here means you are running a headless test fixture or a custom build path that constructs `GatewayWorkbenchService` without threading the ArcSwap handle through. On trunk this is a defensive guard, not a user-facing failure mode.

**Fix:** Start the server through the CLI (`gadgetron serve --config gadgetron.toml`) rather than an in-process test harness. The CLI always calls `build_workbench(...)` which sets the handle. If you are writing an integration test and need the reload endpoint, mirror the pattern in `crates/gadgetron-gateway/tests/web_2b_descriptor_endpoints.rs` that threads `Some(Arc::new(ArcSwap::from_pointee(DescriptorCatalog::seed_p2b().into_snapshot())))` into the service constructor (note the trailing `.into_snapshot()` call — this is the step that compiles validators for every action and bundles them with the catalog into a `CatalogSnapshot`, which is what the ArcSwap expects to hold post-PR #214).

---

### `/admin/reload-catalog` succeeds but `action_count` looks unchanged

**What you observe:** POST `/admin/reload-catalog`, response is `{reloaded:true, source:"seed_p2b", action_count:5, view_count:3, source_path:null}`, but `action_count` matches what was there before the reload — it looks like nothing happened.

**Why:** Expected behavior when `[web] catalog_path` is NOT configured in `gadgetron.toml`. The reload handler's fallback source is the hand-coded `DescriptorCatalog::seed_p2b()` which always produces an identical catalog — the endpoint proves the atomic swap lands, not that the catalog changed. The `source_path: null` field in the response is the confirmation you're on the fallback path. Harness Gate 7q.1 asserts the cross-check invariant: the response `action_count` must equal the live `GET /workbench/actions` count right after, catching the "swap happened but read path still sees old pointer" regression.

**Fix:** Configure `[web] catalog_path = "/absolute/path/to/catalog.toml"` in `gadgetron.toml` (ISSUE 8 TASK 8.4 / v0.4.4, PR #216). After that, edits to the TOML file are picked up on the next `POST /admin/reload-catalog` and the response will carry `source: "config_file"` + `source_path: "<that path>"`. See [`configuration.md`](configuration.md#web) §`[web]` for the full key documentation and the parse-failure guarantee.

**Note on schema validators (TASK 8.3 closed this gap in PR #214).** Through v0.4.2 the endpoint had a known limitation: `InProcessWorkbenchActionService` pre-compiled JSON-schema validators at construction and did NOT rebuild them on reload — safe with `seed_p2b` (schemas identical across reloads) but unsafe for a future file-based source. TASK 8.3 fixed this by introducing `CatalogSnapshot { catalog, validators }` and widening the handle from `Arc<ArcSwap<DescriptorCatalog>>` to `Arc<ArcSwap<CatalogSnapshot>>`. Today a reload runs `DescriptorCatalog::into_snapshot()` which compiles validators for every action in the fresh catalog, then stores the `(catalog, validators)` pair atomically — readers see a consistent generation on every request. If you see action invocations passing the `args` JSON schema after a `catalog_path` TOML edit + reload even though the new TOML rejects that shape, the snapshot-read contract is broken (file a bug citing this recipe + the exact commit of the edit).

---

### HTTP 500 — `config_error` on `/admin/reload-catalog` after editing `catalog_path` TOML

**What you observe:** `[web] catalog_path` is configured, you edit the TOML file, POST `/admin/reload-catalog`, and the endpoint returns HTTP 500 with a `config_error` body that embeds the file path and a parse / IO error message (e.g. `"failed to parse /etc/gadgetron/catalog.toml: expected '=' at line 12"`). `GET /workbench/actions` still returns the PREVIOUS catalog's actions — not the (broken) edit.

**Why:** ISSUE 8 TASK 8.4 (PR #216 / v0.4.4) added a hard guarantee: `DescriptorCatalog::from_toml_file(path)` returns `GadgetronError::Config` on any IO or parse failure, and **the reload handler does NOT replace the running `Arc<ArcSwap<CatalogSnapshot>>` on failure**. A malformed edit cannot take the workbench down — the previously-loaded catalog keeps serving live traffic while you fix the TOML.

**Fix:**

1. Read the exact error message in the response body — it names the path AND the specific serde / IO error (e.g. `"missing field 'actions'"`, `"invalid type: integer, expected string"`, `"no such file or directory"`).
2. Correct the TOML in your editor. Schema reference: `CatalogFile` mirrors `WorkbenchViewDescriptor` + `WorkbenchActionDescriptor` (see [`configuration.md`](configuration.md#web) §`[web]` for the design-doc link).
3. `curl -fsS -X POST -H "Authorization: Bearer $MGMT_KEY" http://localhost:8080/api/v1/web/workbench/admin/reload-catalog` again. Success returns the usual 200 with `source: "config_file"` + `source_path`.
4. Live clients never saw the broken edit — they were reading through the unchanged snapshot the entire time. No restart required.

If the reload 500s with a `config_error` that does NOT name a file path (just says `"catalog reload requires a configured workbench with a descriptor catalog handle"`), you're hitting the TASK 8.2 defensive guard instead — see §HTTP 400 `config_error` above for that recipe.

---

### "Which bundle is live?" — identifying the loaded catalog

**What you want to know:** Someone edited `catalog.toml`, triggered a reload, and the workbench is now serving actions. Is the live catalog the edited one, the previous version, or the hardcoded `seed_p2b()` fallback?

**How to check (HTTP path — returns JSON):** POST the reload endpoint with a Management-scoped key and inspect the response `bundle` + `source_path` fields. The endpoint is idempotent on `seed_p2b()` / unchanged TOML — calling it again only forces a re-parse + re-swap of an identical snapshot.

```bash
curl -fsS -X POST \
  -H "Authorization: Bearer $MGMT_KEY" \
  http://localhost:8080/api/v1/web/workbench/admin/reload-catalog \
  | jq '{source, source_path, bundle}'
```

Possible outputs and what each means:

- `{"source": "seed_p2b", "source_path": null, "bundle": null}` — `[web] catalog_path` is NOT configured in `gadgetron.toml`. The live catalog is the hardcoded `seed_p2b()` fallback. No operator edit will ever change this until `catalog_path` is set (see [`configuration.md`](configuration.md#web)). **Note:** `jq '{...bundle}'` on an absent field renders `null`; the actual JSON omits the key via `skip_serializing_if = "Option::is_none"` — `jq '.bundle // null'` will likewise render `null`.
- `{"source": "config_file", "source_path": "/etc/gadgetron/catalog.toml", "bundle": null}` — `catalog_path` is configured, but the loaded TOML has no top-level `[bundle]` table. The file is loading correctly but without an identity stamp. Adding `[bundle]\nid = "..."\nversion = "..."` to the TOML and triggering another reload will populate the `bundle` field.
- `{"source": "config_file", "source_path": "/etc/gadgetron/catalog.toml", "bundle": {"id": "gadgetron-core", "version": "0.4.6"}}` — `catalog_path` is configured AND the TOML carries a `[bundle]` table. This is the canonical "I loaded the file I think I loaded, and it identifies itself as X@Y" state. Admin tooling should log both `source_path` and `bundle.version` alongside every reload.

**How to check (SIGHUP path — no HTTP response):** `kill -HUP $(pidof gadgetron)` emits a `workbench.admin: descriptor catalog reloaded action_count=N view_count=N source="..."` line to stderr / `gadgetron.log` but does NOT include the `bundle` field in the log line (the trace is structured around `ReloadCatalogResponse` but the signal path discards the response body). To verify bundle identity after a SIGHUP reload, follow up with the HTTP call above — it's idempotent and will show the live `bundle` from the current snapshot.

**Why the drift-test matters.** Both sources still ship on trunk: `bundles/gadgetron-core/bundle.toml` (the first-party bundle, now the harness + shipped default per TASK 9.3) + the hardcoded `seed_p2b()` (retained as a unit-test fixture + drift-guard). A drift test in the test suite asserts they produce the same action id set, so "load the bundle file" vs "fall back to seed_p2b" cannot silently diverge in what actions are exposed. If you're debugging a "missing action" issue in production with `catalog_path` or `bundles_dir` pointing at the first-party bundle, the drift test is your sanity check — if `cargo test -p gadgetron-gateway` is green, the bundle file and seed_p2b have the same action ids.

---

### HTTP 4xx — install rejected for invalid `bundle_id`

**What you observe:** `POST /api/v1/web/workbench/admin/bundles` with a body that includes a `[bundle] id = "..."` declaration returns HTTP 4xx with a `config_error` body mentioning the id regex — even though the TOML parses as valid.

**Why:** ISSUE 10 TASK 10.2 (PR #224 / v0.4.10) centralized bundle-id validation in `validate_bundle_id()` at `crates/gadgetron-gateway/src/web/workbench.rs`. The regex is `^[a-zA-Z0-9_-]{1,64}$`. Anything else — `..`, `/`, `.` prefix, empty string, length > 64 — is rejected **BEFORE** any filesystem path is built. Gate 7q.7 pins the classic `id = "../etc/passwd"` case: the rejection must happen before `std::path::PathBuf::push` runs, otherwise an attacker could make the path escape `bundles_dir` real before the error surfaces.

**Fix:** Rewrite the id to satisfy the regex. Typical operator mistakes:
- `my.bundle` → no dots allowed. Use `my-bundle` or `my_bundle`.
- `My Bundle` → no spaces, no uppercase-rejection isn't the issue but spaces are. Use `my-bundle`.
- `bundle/v2` → no slashes. Put the version in `[bundle].version` (which has no regex), not the id. Keep id stable across versions.
- Empty string / missing `id` field → the `[bundle]` table must declare both `id` AND `version` (both strings).
- 65+ chars → pick a shorter id. 64 chars is a lot; if you hit the limit, your ids probably encode state that should live elsewhere.

**`DELETE /admin/bundles/{bundle_id}` gets the same treatment.** The path parameter goes through the same `validate_bundle_id()` guard. A request to `DELETE /admin/bundles/..%2Fetc%2Fpasswd` (URL-encoded path traversal) is rejected before touching disk. Gate 7q.7 + the symmetric uninstall-side regex test (covered by Gate 7q.8's happy path AND the per-path-segment regex in the router) both guard this.

---

### HTTP 4xx — install over an existing bundle id

**What you observe:** `POST /admin/bundles` with an id that already has a `{bundles_dir}/{id}/` directory on disk returns HTTP 4xx with a `config_error` mentioning "bundle already exists" (or similar text naming the conflict). The manifest is NOT written.

**Why:** ISSUE 10 TASK 10.2 explicitly rejects silent overwrites — the semantics match `DescriptorCatalog::from_bundle_dir()` where duplicate ids across bundles are a hard `Config` error. If install silently overwrote an existing id, a fresh reload could pick up the new bundle mid-request and surprise live callers with a different catalog — the marketplace would be a foot-gun. This is a **feature**, not a bug.

**Fix:** Uninstall first, then reinstall.
```bash
# 1. Confirm what's installed
curl -fsS -H "Authorization: Bearer $MGMT_KEY" \
  http://localhost:8080/api/v1/web/workbench/admin/bundles | jq '.bundles[] | .bundle.id'

# 2. Remove the conflicting id (DELETE is idempotent — a 404 just means "already gone")
curl -fsS -X DELETE -H "Authorization: Bearer $MGMT_KEY" \
  "http://localhost:8080/api/v1/web/workbench/admin/bundles/my-bundle"

# 3. Reinstall with the new TOML
curl -fsS -X POST -H "Authorization: Bearer $MGMT_KEY" \
  -H "Content-Type: application/json" \
  -d '{"bundle_toml": "..."}' \
  http://localhost:8080/api/v1/web/workbench/admin/bundles

# 4. Activate when ready
curl -fsS -X POST -H "Authorization: Bearer $MGMT_KEY" \
  http://localhost:8080/api/v1/web/workbench/admin/reload-catalog
```

If you want install + uninstall to be a single atomic replace (no intermediate "missing" state visible to the `GET /admin/bundles` discovery endpoint), DON'T: the design deliberately separates them so the operator sees the transition explicitly. An accidental install never changes production behaviour until the operator calls reload.

---

### HTTP 503 — `config_error` on `/admin/bundles` when `bundles_dir` is not configured

**What you observe:** `GET /admin/bundles` (or `POST` / `DELETE`) returns HTTP 503 with a `config_error` body mentioning that `bundles_dir` is not configured, even with a Management-scoped key.

**Why:** ISSUE 10 TASK 10.1 + 10.2 (PR #223 + #224) both require `[web] bundles_dir` to be set in `gadgetron.toml`. Without it the handler has no directory to scan (discovery), no target path to write (install), and no directory to remove (uninstall). Deployments using `[web] catalog_path` (flat-file source, TASK 8.4) or the hardcoded `seed_p2b()` fallback don't have a bundles directory.

**Fix:** Add `bundles_dir = "/etc/gadgetron/bundles"` (or any absolute path) to the `[web]` section of your `gadgetron.toml` and restart the server. See [`configuration.md §[web]`](configuration.md#web) for the full key documentation including the precedence order (`bundles_dir` > `catalog_path` > `seed_p2b`). The shipped `bundles/gadgetron-core/bundle.toml` is a valid minimal starting point.

---

## Log interpretation

**Log file location (demo flow):** `.gadgetron/demo/gadgetron.log` inside the repo root — set by `demo.sh` via `STATE_DIR=${REPO_ROOT}/.gadgetron/demo`, `LOG_FILE="${STATE_DIR}/gadgetron.log"` (see `demo.sh:5-14`). Use `./demo.sh logs` for the default 80-line tail or `./demo.sh logs -f` to follow in real time.

For `cargo run` / `gadgetron serve` foreground runs (not via `demo.sh`), logs go to **stderr**. Redirect with `2>gadgetron.log` if you need persistence.

Enable debug logging to see the full middleware trace:

```sh
RUST_LOG=gadgetron=debug ./target/release/gadgetron
```

**Log format on trunk.** The subscriber is `tracing_subscriber::fmt()` with `EnvFilter::try_from_env("RUST_LOG")` (see `crates/gadgetron-cli/src/main.rs:2553-2560`, `init_tracing` fn). Output is the default human-readable text format, not JSON. Each line looks like:

```
2026-04-19T00:00:00.000Z  LEVEL target::path: message key1=value1 key2=value2
```

Structured tracing fields appear as bare `key=value` pairs at the end of the line (not JSON keys). Custom targets — e.g. `tracing::info!(target: "wiki_seed", ...)` — replace the module path with the target name (`wiki_seed:` in that case). Switch to JSON output by swapping in `tracing_subscriber::fmt().json()`; the default text format is what `demo.sh`-flow operators see.

Common grep recipes against the demo log path (each pattern is grounded to the exact `tracing::` call that emits it, not a guess):

```sh
# Every response rejected by the gateway error sink. The fmt subscriber
# renders `tracing::error!(error.code = ..., error.type_ = ...)` as
# `error.code=<code> error.type_=<type>` at the end of the line.
# Trace sink:      crates/gadgetron-gateway/src/error.rs:27-33
#                  (emits error.code = err.error_code() — dynamic field)
# Code strings:    crates/gadgetron-core/src/error.rs:319-328
#                  (the error_code() match arms that resolve to the literal
#                  alternation values below)
# Note: 413 `request_too_large` is NOT in this alternation — the body-size
# limit layer at crates/gadgetron-gateway/src/server.rs:68-74 emits the
# JSON error body directly, without going through the tracing sink.
grep -E 'error\.code=(invalid_api_key|forbidden|quota_exceeded|routing_failure|provider_error|stream_interrupted|config_error|billing_error)' \
  .gadgetron/demo/gadgetron.log

# Scope-denial 403s — one WARN line carries tenant_id, required_scope,
# and path together. Source: crates/gadgetron-gateway/src/middleware/scope.rs:62-67.
grep 'scope denied' .gadgetron/demo/gadgetron.log

# Startup: each provider registration emits an INFO with name= field.
# Source: crates/gadgetron-cli/src/main.rs:2542.
grep 'provider registered' .gadgetron/demo/gadgetron.log

# Penny registration — happy path and failure path have distinct strings.
# Source: crates/gadgetron-cli/src/main.rs:~1840-1870.
#   Happy:   `penny: registered (...)`
#   Failure: `penny: failed to prepare knowledge registry; skipping`
grep -E 'penny: registered|penny: failed to prepare knowledge registry' \
  .gadgetron/demo/gadgetron.log

# Wiki seed injection — happy path emits target "wiki_seed" + message
# starting "injected" (tracing::info!).
# Source: crates/gadgetron-knowledge/src/wiki/store.rs:462-466.
grep 'wiki_seed: injected' .gadgetron/demo/gadgetron.log

# Wiki seed injection — failure path (non-fatal WARN with different message
# and on a different target). If you see this, seeds were not injected but
# startup continued normally.
# Source: crates/gadgetron-knowledge/src/wiki/store.rs:82-85 (tracing::warn!
# with target="wiki_seed", message "failed to inject seed pages on first open
# (non-fatal)").
grep 'wiki_seed: failed to inject seed pages' .gadgetron/demo/gadgetron.log
```

Bump verbosity to see the full middleware trace by re-running with
`RUST_LOG=gadgetron=debug` — the same grep recipes still work; the volume
just grows.

Key log fields to look for:

| Log field | What it tells you |
|-----------|-------------------|
| `error.code` | Machine-readable error code (matches the `code` field in error responses) |
| `error.type_` | Error type category |
| `tenant_id` | Which tenant's request failed (scope failures) |
| `required_scope` | What scope was needed (scope failures) |
| `path` | The route that triggered the scope check |
| `bind` | The address the server is actually listening on |
| `name` | Provider name when a provider is registered |

### Benign WARN patterns (safe to ignore)

Not every WARN line in `gadgetron.log` indicates a regression. The P2A runtime deliberately emits three advisory WARNs that are expected on a healthy boot. The E2E harness (Gate 12 in `scripts/e2e-harness/run.sh:1443-1480`) whitelists exactly these three patterns and treats any other WARN as a regression candidate — operators can use the same triage posture.

| WARN message (grep pattern) | Target | Emitted from | Why it fires | How to silence (optional) |
|---|---|---|---|---|
| `ask mode has no effect in Phase 2A` | `agent_config` | `crates/gadgetron-core/src/agent/config.rs:408-412` | Any `agent.gadgets.write.<field> = "ask"` in `gadgetron.toml`. This setting governs the **Penny MCP-tool** approval card, not the workbench direct-action approval. The Penny-side card requires the SEC-MCP-B1 cross-process bridge, still deferred per ADR-P2A-06 item 1 — gateway treats `ask` as `auto` for Penny tools until that ISSUE lands. The workbench direct-action approval flow (`wiki-delete` → `/approvals/:id/approve`) shipped in ISSUE 3 / v0.2.6 and is unaffected by this warning. | Set the affected fields to `"auto"` or `"never"` explicitly, or accept the warning until the Penny-side bridge ISSUE lands. |
| `git config user.name / user.email not set` | `knowledge_config` | `crates/gadgetron-knowledge/src/config.rs:752-755` | The host's system gitconfig has no `user.name` / `user.email`. Each wiki page commit would have no author, so the gateway falls back to `Penny <penny@gadgetron.local>`. | `git config --global user.name "Your Name" && git config --global user.email "you@example.com"`, or set `[knowledge].wiki_git_author = "Name <email>"` in `gadgetron.toml`. |
| `scope denied` on a `path=/api/v1/...` | `gadgetron_gateway::middleware::scope` | `crates/gadgetron-gateway/src/middleware/scope.rs:62-67` | A request with `OpenAiCompat`-only scope hit a `/api/v1/` admin route that requires `Management`. The request was rejected with **HTTP 403** and audited to the 403 channel (SEC-M4). In a normal mixed deployment this line will appear **every time** a non-admin caller pokes an admin route — it is a security success, not a failure. | If you want to suppress the noise, either provision the caller a `Management`-scoped key, or narrow your client to only call `/v1/` routes. |

Any other WARN line is worth investigating. In particular, WARNs from `knowledge_*`, `penny_shared_context`, `wiki_seed`, `quota`, or the audit writer all signal runtime conditions the operator should look into — the harness fails loudly on them by design.

---

## Audit log `latency_ms` interpretation

The audit log emits `latency_ms` on every request. **Its meaning depends on whether the request was streaming or non-streaming.**

### Non-streaming requests (`stream: false`)

`latency_ms` = full middleware chain + upstream provider call + response serialization. This is the end-to-end latency you usually want. Typical values on healthy vLLM: 50–500 ms depending on model and prompt.

### Streaming requests (`stream: true`)

Streaming requests produce **two audit entries**, both sharing the same `request_id` but with distinct `event_id` values:

1. **Dispatch entry** — emitted before the first SSE byte. `latency_ms` = middleware + dispatch overhead only (typically `0 ms` on modern hardware). `output_tokens = 0`, `status = "ok"` (placeholder — the stream hasn't started yet).

2. **Amendment entry** — emitted when the stream ends, regardless of how (normal completion, client disconnect, provider error, future cancellation). Carries a chunk-count proxy for `output_tokens` (incremented once per non-empty chunk — coarse, not exact token counts), `input_tokens = 0` (the SSE chunk format carries no usage field), and the real `status`: `"ok"` for a clean stream end, `"error"` for any terminal provider error.

For end-to-end streaming latency (dispatch entry `latency_ms` is not useful for this), use:
- **TUI dashboard** (`gadgetron serve --tui`) — the Requests panel shows wall-clock latency from the `metrics_middleware` layer, which measures the full chain including the stream body
- **`/metrics` Prometheus histogram** — planned in Phase 2
- **Client-side timing** — measure `time.perf_counter()` around the OpenAI SDK call

To correlate both entries for a single stream: `WHERE request_id = '<id>' ORDER BY created_at`.
