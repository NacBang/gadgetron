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

## Log interpretation

Enable debug logging to see the full middleware trace:

```sh
RUST_LOG=gadgetron=debug ./target/release/gadgetron
```

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
