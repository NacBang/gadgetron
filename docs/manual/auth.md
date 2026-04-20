# Authentication and Authorization

---

## API key format

Gadgetron API keys have the format:

```
gad_{kind}_{secret}
```

Where:
- `gad_` is a fixed prefix identifying the key as a Gadgetron key
- `{kind}` is either `live` or `test`
- `{secret}` is at least 16 characters long (alphanumeric)

Example keys (these are illustrative; generate real keys with a cryptographically secure random source):

```
gad_live_a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6
gad_test_a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6
```

**Validation rules** (enforced by `ApiKey::parse` before any database lookup):
- Must start with `gad_`
- Must have at least three underscore-delimited segments (`gad`, `live`/`test`, secret)
- The secret segment must be at least 16 characters long
- Keys not matching these rules return HTTP 401 immediately, before any database query

---

## How authentication works

Every request to an authenticated endpoint must include:

```
Authorization: Bearer gad_live_your_key_here
```

The gateway processes this in the `auth_middleware` layer (layer 4 in the Tower middleware stack):

1. **Extract** the `Authorization` header value. If the header is absent or does not begin with `Bearer `, return HTTP 401.

2. **Parse** the token via `ApiKey::parse`. This validates the `gad_` prefix, segment count, and minimum secret length. On parse failure, return HTTP 401. The token is never stored; only its hash is used further.

3. **Hash** the raw token with SHA-256 (hex-encoded, 64 characters). This hash is what is stored in the `api_keys` table under the `key_hash` column.

4. **Validate** the hash against the database via `PgKeyValidator`:
   - First checks a moka in-memory cache (max 10,000 entries, 10-minute TTL). Cache hit costs approximately 50 microseconds.
   - On cache miss, runs a PostgreSQL query: `SELECT id, tenant_id, scopes FROM api_keys WHERE key_hash = $1 AND revoked_at IS NULL`. Cache miss costs approximately 5 milliseconds.
   - If no matching active key exists, return HTTP 401.

5. **Inject** the `ValidatedKey` (containing `api_key_id`, `tenant_id`, `scopes`) into the request extension chain. Downstream middleware reads it.

6. **Audit** every 401 failure (SOC2 CC6.7). Failed authentication attempts are logged to the audit channel regardless of the failure reason. The tenant and key IDs are `00000000-0000-0000-0000-000000000000` for unauthenticated failures (no real IDs are available).

**Query-token fallback for `/events/ws`** (ISSUE 4 / v0.2.7). Browser JavaScript cannot attach an `Authorization` header to a `new WebSocket(url)` upgrade request, so the auth middleware accepts `?token=gad_live_…` as a secondary source **scoped only to the `/events/ws` route**. The fallback reuses the same `ApiKey::parse` + hash-lookup path (no separate validator) and strips `?token=` from the request URI before tracing spans and request-log lines land, so raw keys never appear in `gadgetron.log`. Every other authenticated route still rejects requests that lack the `Authorization` header — 401. Non-browser WS clients (curl `--header`, websocat `-H`, `tokio-tungstenite`) should continue using the header. See [troubleshooting.md §`/events/ws` upgrade returns HTTP 401 from the browser](troubleshooting.md#events-ws-upgrade-returns-http-401-from-the-browser) for the operator recipe.

After successful authentication, `tenant_context_middleware` constructs a `TenantContext` from the `ValidatedKey`, and `scope_guard_middleware` checks that the key's scopes satisfy the route's requirement.

---

## Middleware stack order

`auth_middleware` and `scope_guard_middleware` are two layers inside a longer chain wrapping every authenticated route. Understanding the full order lets an operator look at a failed request's status code and immediately tell which layer produced it.

Stack order from outermost (first to run on inbound requests) to innermost (closest to the handler), reconstructed from the actual `.layer()` call sequence at `crates/gadgetron-gateway/src/server.rs:268-294` (inline comments at `:260-265` describe the same order). The top-of-function rustdoc at `:220-238` is a simplified view — it omits `map_response(openai_shape_413)` and `metrics_middleware`; the code site is authoritative.

```
inbound request
    ↓
[1a] map_response(openai_shape_413)   — rewrites raw 413 body to OpenAI JSON envelope
[1b] RequestBodyLimitLayer (4 MiB)    — SEC-M2; emits 413 with plain-text body (rewritten by 1a)
[2]  TraceLayer                        — tower-http distributed tracing spans
[3]  request_id_middleware             — UUID → extensions + `x-request-id` response header
[4]  auth_middleware                   — Bearer OR cookie → Arc<ValidatedKey>; 401 on parse/lookup failure
[5]  tenant_context_middleware         — ValidatedKey → TenantContext (tenant_id, scopes, request_id)
[6]  scope_guard_middleware            — per-route scope check; 403 on mismatch (audited, SEC-M4)
[7]  metrics_middleware                — RequestLog broadcast (innermost, runs after handler)
    ↓
route handler
```

(The `.layer()` calls in `server.rs:268-294` are written innermost-to-outermost because each `.layer()` invocation wraps all previously applied layers. The numbering above flips them back to outermost-to-innermost for reading.)

### Failure mode lookup

| Layer | Produces | Observable signals |
|---|---|---|
| 1a / 1b | **HTTP 413** `request_too_large` (OpenAI-shape body after 1a rewrites 1b's raw text) | Request body exceeded 4 MiB (`MAX_BODY_BYTES`). E2E Gate 12 pattern `error.code=request_too_large` is NOT emitted by the tracing sink — 413 bypasses it. |
| 2 | no failures directly; wraps everything else in a tracing span | Look for the span name in `RUST_LOG=gadgetron=debug` output. |
| 3 | no failures directly | Every response has an `x-request-id` header; if absent, something before this layer failed. |
| 4 | **HTTP 401** `invalid_api_key` | Bearer path: missing/malformed `Authorization: Bearer ...`, unknown key, or revoked key. Cookie path (v0.5.9+): missing `gadgetron_session` cookie, expired session, session referencing deleted/inactive user, or service-role user. Audited via SEC-M7 for both paths. |
| 5 | **HTTP 401** on a defensive `TenantNotFound` path (`middleware/tenant_context.rs:27-35`) if `ValidatedKey` is absent from request extensions. The code comment notes this branch "should never happen when layer order is correct" — it's defensive for test-isolation. In production layer 4 will have already returned 401 in that scenario. | If this 401 fires without layer 4 firing first, a layer-ordering regression is the cause. |
| 6 | **HTTP 403** `forbidden` | Valid key but scope doesn't match route. Emits `scope denied` WARN with `tenant_id` + `required_scope` + `path` fields (whitelisted in E2E Gate 12 since it's expected on Management-route hits from OpenAiCompat keys). |
| 7 | no failures directly; emits `RequestLog` broadcast after handler | Failure in the handler itself (500 / 502 / 503) still triggers this layer — the broadcast captures status regardless. |

The route guard tables in [Scope system](#scope-system) below are the per-route policy that layer 6 enforces.

**What is NOT in the stack:** `CorsLayer::permissive()` is deliberately absent (D-6 — no permissive CORS). Host validation is not a separate layer; TCP bind + reverse-proxy are expected to enforce host allowlisting externally.

---

## Scope system

Each API key holds a list of scopes. A scope is a coarse-grained permission. The three defined scopes are:

| Scope | String value in DB | What it permits |
|-------|--------------------|-----------------|
| `OpenAiCompat` | `"OpenAiCompat"` | All `/v1/` routes (`POST /v1/chat/completions`, `GET /v1/models`) **and** all `/api/v1/web/workbench/` routes |
| `Management` | `"Management"` | All other `/api/v1/` routes (nodes, model deploy, usage, costs) |
| `XaasAdmin` | `"XaasAdmin"` | Reserved for `/api/v1/xaas/` routes (internal XaaS administration). **No routes are mounted under this prefix on trunk yet**; keys with this scope will hit 404 until XaaS admin endpoints land in a later phase. |

A key can hold multiple scopes. The `api_keys.scopes` column is a `TEXT[]` (PostgreSQL array). The default when inserting a new key without specifying scopes is `ARRAY['OpenAiCompat']`.

**Scope enforcement** is performed by `scope_guard_middleware` (layer 6 of the stack — see [Middleware stack order](#middleware-stack-order) above):

| Path prefix | Required scope | Note |
|-------------|----------------|------|
| `/v1/` | `OpenAiCompat` | |
| `/api/v1/web/workbench/` | `OpenAiCompat` | W3-WEB-2 exception — workbench uses the same scope as `/v1/` |
| `/api/v1/xaas/` | `XaasAdmin` | |
| `/api/v1/` | `Management` | Catch-all for admin routes |
| `/health`, `/ready` | none | Public; never reach this layer |

A key with `OpenAiCompat` scope can access `/v1/` routes and `/api/v1/web/workbench/` routes. It cannot access other `/api/v1/` routes (which require `Management`) and will receive HTTP 403 if it tries.

### Scope-based workbench filtering

The route gate above is only the first of two scope checks on the workbench surface. Inside the workbench, each registered **view** and **action** descriptor carries an optional `required_scope` field. `TenantContext.scopes` is threaded from the handler through the projection and action services, which apply a second per-descriptor filter:

- `GET /api/v1/web/workbench/views` and `GET /api/v1/web/workbench/actions` **strip** descriptors whose `required_scope` is not held by the caller. A key with only `OpenAiCompat` sees a shorter list than a key that also holds `Management`.
- `GET /api/v1/web/workbench/views/{view_id}/data` returns HTTP **404** `workbench_view_not_found` when the caller's scopes do not admit the view. The response is deliberately indistinguishable from a nonexistent view, so scope-restricted views do not leak via a 403 vs 404 signal.
- `POST /api/v1/web/workbench/actions/{action_id}` returns HTTP **404** `workbench_action_not_found` under the same scope-mismatch condition, for the same reason.

In short: scopes are not a strict binary route gate on the workbench surface. Any automated tooling that discovers views or actions must treat the response as a **dynamic, per-key subset** of the catalog.

---

## Creating API keys

### Using the CLI

The recommended way to create tenants and keys is the CLI. The CLI handles key generation, hashing, and database insertion for you.

**Step 1 — create a tenant:**

```sh
./target/release/gadgetron tenant create --name "my-team"
```

Output (stdout):

```
Tenant Created

  ID:    9f1c5a2e-8d4b-4f0d-b3a2-7c0e1f5b6d4e
  Name:  my-team

  Next: gadgetron key create --tenant-id 9f1c5a2e-8d4b-4f0d-b3a2-7c0e1f5b6d4e
```

**Step 2 — create a key for that tenant:**

```sh
./target/release/gadgetron key create --tenant-id 9f1c5a2e-8d4b-4f0d-b3a2-7c0e1f5b6d4e
```

Output (**stderr**, not stdout — SEC-M7 prevents accidental capture in pipelines):

```
  API Key Created

  Key:     gad_live_a3f8e1d2c4b5a6e7f8d9c0b1a2e3d4f5
  Tenant:  9f1c5a2e-8d4b-4f0d-b3a2-7c0e1f5b6d4e
  Scopes:  OpenAiCompat

  Save this key — it will not be shown again.
```

The raw key (the `Key:` line) is printed exactly once to **stderr**. Copy it now — it cannot be recovered from the database, because Gadgetron stores only the SHA-256 hash. Scripts that pipe or capture `key create` output must redirect stderr (`2>&1` or `2>out`); redirecting only stdout will lose the key entirely.

Current `key create` flags:

| Flag | Description |
|------|-------------|
| `--tenant-id <uuid>` | Required for PostgreSQL-backed key creation; omit it in no-db mode |
| `--scope <scope>` | Scope string stored with the key (default: `OpenAiCompat`) |

Example — create a management key:

```sh
./target/release/gadgetron key create --tenant-id <uuid> --scope Management
```

### No-database mode

For local development without PostgreSQL, skip tenant creation and omit `--tenant-id`:

```sh
./target/release/gadgetron key create
# Output includes a generated gad_live_* key
```

The generated key is not stored anywhere. In no-db mode, the built-in validator accepts any syntactically valid `gad_live_*` or `gad_test_*` key and returns a synthetic identity with all scopes. This mode is intended for local development only; do not use it in production.

---

## Listing and revoking API keys

List active keys for a tenant:

```sh
./target/release/gadgetron key list --tenant-id <uuid>
```

Revoke a key by its UUID:

```sh
./target/release/gadgetron key revoke --key-id <key-uuid>
```

Revocation sets `revoked_at = NOW()` for the key. The validator checks `revoked_at IS NULL`, so new lookups fail immediately. Due to the 10-minute validator cache TTL, a revoked key may continue to work for up to 10 minutes after revocation until its cache entry expires.

---

## Cookie-session auth (ISSUE 15 TASK 15.1 / v0.5.8 + ISSUE 16 TASK 16.1 / v0.5.9)

In addition to the Bearer-token API key flow above, Gadgetron v0.5.8+ ships a parallel cookie-session surface for browser clients. Three public endpoints:

```
POST /api/v1/auth/login    {email, password}  → Set-Cookie: gadgetron_session=...
POST /api/v1/auth/logout   (cookie)            → Set-Cookie: gadgetron_session=; Max-Age=0
GET  /api/v1/auth/whoami   (cookie)            → {session_id, user_id, tenant_id, expires_at}
```

**Relationship to Bearer auth**:

- **`/auth/*` endpoints bypass Bearer**: the three `/auth/*` routes mount on `public_routes` — they use cookie self-authentication only. Bearer keys do not share a middleware gate with them.
- **As of v0.5.9, all other protected routes accept EITHER Bearer OR cookie** (ISSUE 16 TASK 16.1 / PR #259). The `auth_middleware` in `crates/gadgetron-gateway/src/middleware/auth.rs` checks the `Authorization` header first — if a Bearer token is present, the existing API-key validation path runs unchanged. If the header is absent AND a `gadgetron_session` cookie is present, the middleware falls back to `validate_session_and_build_key(pool, token)` which looks up the session, resolves `role` from `users`, and synthesizes a `ValidatedKey` with role-derived scopes (admin → `[OpenAiCompat, Management]`; member → `[OpenAiCompat]`; service rejected — shouldn't reach here because login blocks it). This means `/v1/*` + `/api/v1/web/workbench/*` + `/api/v1/xaas/*` now all work for browser cookie clients; no separate middleware variant per surface.
- **No-db mode silently skips the cookie path** — without a pg pool there's no `user_sessions` table to validate against. `--no-db` deployments are effectively Bearer-only.
- **Audit attribution for cookie-auth requests**: `api_key_id = Uuid::nil()` sentinel distinguishes cookie sessions from Bearer API keys in audit rows. Downstream audit writers branch on `key_id == Uuid::nil()` to emit `actor_user_id` (from the session row) instead of `actor_api_key_id` when the `audit_log.actor_user_id` column plumbing (TASK 14.1 migration) is fully threaded through the audit writer.
- **Service-role users cannot log in by password** — `SessionError::ServiceRole` pre-empts the password verify. Service accounts must use Bearer API keys (created via `gadgetron key create` or the user self-service `POST /api/v1/web/workbench/keys`).

**Password hashing**: argon2id via the `argon2` crate (v0.5). PHC-string format stored in `users.password_hash` (set by the `[auth.bootstrap]` first-admin flow per ISSUE 14 TASK 14.2, or by `POST /api/v1/web/workbench/admin/users` per ISSUE 14 TASK 14.3). No bcrypt / PBKDF2. Pre-login inactive-user check avoids timing leaks.

**Cookie storage**: the raw 32-byte hex token sent in `Set-Cookie` is hashed SHA-256 server-side before DB lookup. The DB row in `user_sessions` holds only the SHA-256; even with direct DB access the raw cookie cannot be recovered. No argon2 on the cookie — 128+ bit entropy on the random 32-byte token already resists brute force, and a per-request argon2 would add measurable latency without security benefit.

**Cookie attributes**:

| Attribute  | Value                     | Rationale |
|-----------|--------------------------|-----------|
| Name       | `gadgetron_session`      | Single canonical name across all deployments |
| `HttpOnly` | set                      | Blocks JavaScript `document.cookie` read — XSS can't exfiltrate |
| `SameSite` | `Lax`                    | Top-level GET from other origins OK; cross-site POST/XHR blocked |
| `Path`     | `/`                      | Single session cookie across all routes |
| `Max-Age`  | `86400` (24h)            | Server-side `expires_at` is authoritative; browser cookie Max-Age matches |
| `Secure`   | **not set by gateway**   | Operators terminate TLS externally (reverse proxy / load balancer). The `Secure` flag is deployment-layer concern — handler emits without it so loopback harness + `curl --cookie-jar` work over plaintext HTTP. Production deployments MUST use HTTPS and should add `Secure` at the proxy layer. |

**Session TTL + cleanup**: `user_sessions.expires_at` is set to 24h from login. Expired rows are rejected at lookup time (returned as 401). Cleanup is opportunistic — subsequent logins sweep expired rows for the same user. No background cron job.

**Example flow** (curl):

```sh
# 1. Login — save cookie to a jar
curl -sS -c /tmp/gad-cookies.txt \
  -X POST http://localhost:8080/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@example.com","password":"your-password"}'

# 2. whoami — reuse the cookie
curl -sS -b /tmp/gad-cookies.txt \
  http://localhost:8080/api/v1/auth/whoami

# 3. Logout — server revokes + browser cookie cleared
curl -sS -b /tmp/gad-cookies.txt -c /tmp/gad-cookies.txt \
  -X POST http://localhost:8080/api/v1/auth/logout

# 4. Re-try whoami — 401, cookie is gone
curl -sS -b /tmp/gad-cookies.txt \
  http://localhost:8080/api/v1/auth/whoami
```

#### Session management operator recipes

The recipes below require direct Postgres access (`psql -U gadgetron`). All
SQL is runnable against the `user_sessions` table described in the schema
section above.

---

**1. List all active sessions for a user**

"Active" means `revoked_at IS NULL`. "Valid" means active AND `expires_at >
NOW()`. This query returns both so operators can see near-expiry sessions.

```sql
-- Replace <user-id> with the target UUID.
SELECT
    id,
    created_at,
    expires_at,
    last_active_at,
    user_agent,
    ip_address,
    CASE
        WHEN expires_at > NOW() THEN 'valid'
        ELSE 'expired-but-not-revoked'
    END AS status
FROM user_sessions
WHERE user_id = '<user-id>'        -- idx_sessions_user_active is used here
  AND revoked_at IS NULL
ORDER BY created_at DESC;
```

> Each row corresponds to one browser or device. A user who logged in from
> three browsers will have three rows.

---

**2. Force-logout a user across all devices**

Login always inserts a NEW `user_sessions` row. It never rotates or deletes
the prior row. Both the old and the new cookie stay valid until explicitly
revoked. To kick a user off all devices, revoke every active row at once.

```sql
-- Revoke all active sessions for a user. Safe to re-run (idempotent).
UPDATE user_sessions
SET    revoked_at = NOW()
WHERE  user_id    = '<user-id>'
  AND  revoked_at IS NULL;

-- Verify: should return 0 rows after the UPDATE above.
SELECT id, created_at, last_active_at
FROM   user_sessions
WHERE  user_id    = '<user-id>'
  AND  revoked_at IS NULL;
```

After revocation, any in-flight request that carries one of the old cookies
will receive a `401` on the next middleware validation hit. The user must
re-authenticate to obtain a new cookie.

---

**3. Investigate a suspected compromised session**

Work through the following steps in order.

a. **Locate the session row.** If you have the raw cookie value, hash it and
   look up directly:

```sql
-- cookie_hash is stored as a hex-encoded SHA-256. Compute the hash first,
-- then query. Replace <hex-hash> with the actual hash.
SELECT id, user_id, created_at, expires_at, last_active_at,
       user_agent, ip_address, revoked_at
FROM   user_sessions
WHERE  cookie_hash = '<hex-hash>';
```

If you do not have the cookie value, triangulate by user identity and
fingerprint:

```sql
SELECT id, cookie_hash, created_at, expires_at, last_active_at,
       user_agent, ip_address
FROM   user_sessions
WHERE  user_id    = '<user-id>'
  AND  user_agent LIKE '%<partial-ua>%'   -- optional
  AND  created_at > NOW() - interval '7 days'
ORDER  BY created_at DESC;
```

b. **Pull audit log activity for that session.** There is no foreign key from
   `audit_log` to `user_sessions`. Cross-reference by time window and user.
   Cookie-session requests are identified by the sentinel
   `api_key_id = '00000000-0000-0000-0000-000000000000'`.

```sql
-- Replace timestamps from the session row you found in step (a).
SELECT al.id, al.timestamp, al.action, al.resource_type, al.resource_id,
       al.outcome, al.ip_address
FROM   audit_log al
WHERE  al.actor_user_id = '<user-id>'
  AND  al.api_key_id    = '00000000-0000-0000-0000-000000000000'  -- web UI only
  AND  al.timestamp     BETWEEN '<session.created_at>' AND COALESCE('<session.revoked_at>', '<session.expires_at>')
ORDER  BY al.timestamp ASC;
```

c. **Revoke the session and rotate API keys.** Revoke the suspicious session
   row first, then revoke any API keys owned by that user if the audit log
   shows key issuance or unusual API activity.

```sql
-- Revoke the single suspicious session.
UPDATE user_sessions
SET    revoked_at = NOW()
WHERE  id         = '<session-id>'
  AND  revoked_at IS NULL;
```

---

**4. Session expiry cleanup**

The validation query already excludes expired rows (`expires_at > NOW()`), so
stale rows have no effect on runtime behavior. Cleanup is pure disk-space
hygiene. Keep revoked-but-within-TTL rows for at least the audit retention
window before deleting them.

```sql
-- Delete sessions that expired more than 30 days ago.
-- Adjust the interval to match your audit retention policy.
DELETE FROM user_sessions
WHERE  expires_at < NOW() - interval '30 days';
```

> Do not delete rows where `expires_at > NOW() - interval '30 days'` but
> `revoked_at IS NOT NULL`. Those rows may still be referenced in audit
> cross-joins for recent incidents.

---

**5. Verify session-fixation protection**

The ISSUE 15 design contract requires that every login creates a new
`user_sessions` row with a fresh `cookie_hash`. To confirm this has not
regressed on a live deployment:

```bash
# Log in twice as the same user and capture both Set-Cookie headers.
curl -sS -c /tmp/cookies-a.txt -X POST http://localhost:8080/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"alice","password":"test-password"}' | jq .session_id

curl -sS -c /tmp/cookies-b.txt -X POST http://localhost:8080/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"alice","password":"test-password"}' | jq .session_id
```

Then confirm the database holds two distinct rows with different hashes:

```sql
-- Both rows should appear, with different id and cookie_hash values.
SELECT id, created_at, cookie_hash
FROM   user_sessions
WHERE  user_id    = (SELECT id FROM users WHERE username = 'alice')
  AND  revoked_at IS NULL
ORDER  BY created_at DESC
LIMIT  5;
```

If both rows share the same `cookie_hash`, the INSERT-only contract has
regressed and must be investigated before the next release.

**Shipped in ISSUE 16 (v0.5.9 / PR #259)**:

- Unified middleware that accepts Bearer OR cookie on the same path — `auth_middleware` now covers `/v1/*` + `/api/v1/web/workbench/*` + `/api/v1/xaas/*` for both authentication surfaces. See the "Relationship to Bearer auth" bullet above for the full fallback chain.

**Shipped in ISSUE 17 (v0.5.10 / PR #260)**:

- `ValidatedKey.user_id: Option<Uuid>` populated by both auth paths. `PgKeyValidator::validate` SELECTs `api_keys.user_id` alongside the existing `(id, tenant_id, scopes)` tuple. Cookie-session path populates from `session.user_id`. Legacy API keys predating the ISSUE 14 TASK 14.1 `api_keys.user_id` backfill surface as `user_id = None`. Downstream audit/billing/telemetry can now attribute activity to users without an extra DB round-trip — the wiring into audit writers themselves is **ISSUE 19**.

**Deferred to ISSUE 18**:

- React + Tailwind login form in `gadgetron-web` that consumes the three `/auth/*` endpoints.

**Shipped in ISSUE 19 (v0.5.11 / PR #262) + ISSUE 20 (v0.5.12 / PR #263)**:

- **ISSUE 19 (struct shape)**: `AuditEntry` gains `actor_user_id: Option<Uuid>` + `actor_api_key_id: Option<Uuid>`. 7 call sites across the workspace (tests, bench fixtures, chat handler, stream_end_guard, auth-fail audit, scope-denial audit) updated; defaults to `None` until ISSUE 20 wires the values.
- **ISSUE 20 (plumbing)**: `TenantContext` gains `actor_user_id` + `actor_api_key_id`, populated by `tenant_context_middleware` from `ValidatedKey.user_id` and the non-nil-sentinel `ValidatedKey.api_key_id`. Chat handler's 3 `AuditEntry` literals (non-stream Ok, stream Ok+dispatch, stream Ok+spawn) now read ctx fields. Cookie-session callers (ISSUE 16, `api_key_id = Uuid::nil()` sentinel) resolve to `actor_api_key_id = None`; Bearer callers with backfilled `api_keys.user_id` get `Some(key_id)`. No new harness gate at this stage — chat audit stays tracing-only until the pg consumer lands in ISSUE 21.

**Deferred to ISSUE 21**:

- Background task spawned from `init_serve_runtime` drains the `AuditWriter` mpsc channel and writes rows to `audit_log` using the new `actor_user_id` + `actor_api_key_id` columns (migration already added in ISSUE 14 TASK 14.1 — columns exist but are unused until this TASK). New operator query endpoint `GET /api/v1/web/workbench/admin/audit/log` (Management-scoped, newest-first, filter params). Harness gate extension pins `actor_user_id` non-NULL on cookie-auth rows + `actor_api_key_id` non-NULL on Bearer-with-backfilled-user-id rows. Same treatment planned for `billing_events` rows.

**Post-ISSUE-18 roadmap** (tracked separately on `project_multiuser_login_google`):

- Google OAuth social-login flow — will stack on top of the same `user_sessions` table + cookie shape, so the middleware from ISSUE 16 + user-id plumbing from ISSUE 17 continue to apply unchanged.

---

## Security notes

- The server never logs raw API key values. The `GADGETRON_DATABASE_URL` is wrapped in a `Secret<String>` type and is never emitted to logs.
- The `api_keys.key_hash` column stores only the SHA-256 hash. Even with direct database access, the original key cannot be recovered from the hash.
- Auth failures (401) are audited to the structured audit channel. In the current implementation, audit entries are written to tracing logs; PostgreSQL persistence remains future work.
- Cookie-session: `user_sessions.cookie_hash` follows the same SHA-256-only discipline as `api_keys.key_hash`. `users.password_hash` stores argon2id PHC string (not recoverable; incremental cost tuning possible via argon2 parameters in `crates/gadgetron-xaas/src/auth.rs`).

---

## Audit log retention and tenant lifecycle (operator responsibility)

Gadgetron persists audit + billing rows **indefinitely**. No automatic purge job exists on trunk (verified: zero references to `purge_audit_log`, `DELETE FROM audit_log`, or a retention-sweep task in `crates/gadgetron-xaas/`; deferred to P2B per [`04-mcp-tool-registry.md:512`](../design/phase2/04-mcp-tool-registry.md)). At 100 chat requests/minute that is ~52M `audit_log` rows/year, plus the sibling `tool_audit_events`, `action_audit_events`, `billing_events`, `activity_events`, and `knowledge_candidates` tables. This is fine for small single-tenant deployments but becomes a disk-pressure + query-latency problem at scale, and it intersects with GDPR Art. 17 erasure obligations. Operators own retention.

### Measuring growth

Inspect the top-5 offenders by disk size per tenant:

```sql
SELECT
    relname AS table,
    pg_size_pretty(pg_total_relation_size(c.oid)) AS size,
    (SELECT COUNT(*) FROM pg_stat_user_tables WHERE relname = c.relname) AS populated,
    n_live_tup AS rows
FROM pg_class c
JOIN pg_stat_user_tables s ON s.relid = c.oid
WHERE relname IN (
    'audit_log', 'tool_audit_events', 'action_audit_events',
    'billing_events', 'activity_events', 'knowledge_candidates',
    'user_sessions'
)
ORDER BY pg_total_relation_size(c.oid) DESC;
```

Per-tenant `audit_log` breakdown:

```sql
SELECT tenant_id, COUNT(*) AS rows,
       MIN(timestamp) AS oldest, MAX(timestamp) AS newest
FROM audit_log
GROUP BY tenant_id
ORDER BY rows DESC
LIMIT 20;
```

### Time-based `audit_log` pruning

The `audit_log_tenant_ts_idx` composite index (`(tenant_id, timestamp DESC)`) supports cheap time-range scans. Archive-then-delete with `VACUUM` is the safe default — a naive `DELETE` without `VACUUM` leaves the rows as dead tuples and the table keeps its old disk footprint until autovacuum catches up.

```sh
# 1. Export the slice you're about to delete (one file per month; compresses well).
CUTOFF="2026-01-01T00:00:00Z"
psql "$GADGETRON_DATABASE_URL" -c "\COPY (
    SELECT * FROM audit_log WHERE timestamp < '$CUTOFF'
) TO '/backup/audit_log-pre-$CUTOFF.csv' WITH (FORMAT csv, HEADER true)"
gzip "/backup/audit_log-pre-$CUTOFF.csv"

# 2. Delete the rows.
psql "$GADGETRON_DATABASE_URL" -c \
    "DELETE FROM audit_log WHERE timestamp < '$CUTOFF'"

# 3. Reclaim disk. VACUUM FULL rewrites the table (takes a lock) — use it once
#    after a large purge, not for incremental trims. Everyday maintenance: just
#    let autovacuum run.
psql "$GADGETRON_DATABASE_URL" -c "VACUUM (VERBOSE, ANALYZE) audit_log"
# For major reclaim after the first-ever purge:
# psql "$GADGETRON_DATABASE_URL" -c "VACUUM FULL audit_log"   -- locks the table
```

Apply the same pattern to `tool_audit_events` (`created_at` column), `action_audit_events` (`created_at`), `billing_events` (`created_at`). Compliance floor is operator-set — common choice is 90 days for chat/tool audit and 7 years for `billing_events` if invoices depend on them.

### Full tenant deletion (order matters)

A plain `DELETE FROM tenants WHERE id = $1` will **fail** on a populated tenant — the FK cascade coverage is mixed:

| Table | `tenant_id` FK behavior | Blocks tenant DELETE? |
|---|---|---|
| `api_keys` | `ON DELETE CASCADE` | no |
| `quota_configs` | `ON DELETE CASCADE` | no |
| `billing_events` | `ON DELETE CASCADE` | no |
| `audit_log` | no clause (→ `NO ACTION`) | **yes** — must pre-delete |
| `users` | no clause | **yes** |
| `teams` | no clause | **yes** |
| `tool_audit_events` | `tenant_id TEXT`, no FK | no (no constraint) |
| `action_audit_events` | `tenant_id TEXT`, no FK | no |

Also note `api_keys.user_id REFERENCES users(id)` carries no CASCADE — dropping `users` before nullifying/deleting those keys fails with FK violation.

**Correct deletion sequence:**

```sql
BEGIN;

-- 1. Archive (optional, but strongly recommended — the tenant row itself
--    and its cascaded descendants go first; audit_log + users + teams go
--    explicitly). Export before this transaction if you need the data.

-- 2. Free api_keys.user_id FK so users can be deleted.
UPDATE api_keys
   SET user_id = NULL
 WHERE tenant_id = :tenant_id;

-- 3. audit_log has no cascade — delete explicitly.
DELETE FROM audit_log WHERE tenant_id = :tenant_id;

-- 4. teams has no cascade (tenant_id), but team_members cascades off team_id.
DELETE FROM teams WHERE tenant_id = :tenant_id;

-- 5. users has no cascade (tenant_id), but user_sessions + team_members
--    cascade off user_id so dropping users clears them.
DELETE FROM users WHERE tenant_id = :tenant_id;

-- 6. Drop the tenant. api_keys + quota_configs + billing_events cascade
--    off tenant_id automatically.
DELETE FROM tenants WHERE id = :tenant_id;

COMMIT;
```

The `tool_audit_events` / `action_audit_events` rows remain after this — they reference `tenant_id` as plain `TEXT` without a constraint. If the tenant is being deleted for GDPR erasure (not just offboarding), also:

```sql
DELETE FROM tool_audit_events   WHERE tenant_id = :tenant_id::text;
DELETE FROM action_audit_events WHERE tenant_id = :tenant_id::text;
```

### Per-user erasure (GDPR Art. 17)

No built-in `gadgetron user erase <user_id>` subcommand on trunk. The operator procedure preserves the tenant and sibling users while zeroing the deleted user's audit trail. Mind the mixed nullability across the six audit tables:

| Table | user-attribution column | type | nullable | FK to `users(id)` |
|---|---|---|---|---|
| `audit_log` | `actor_user_id` | UUID | yes | yes (blocks naive user DELETE) |
| `billing_events` | `actor_user_id` | UUID | yes | **no** (ISSUE 23 intentional) |
| `tool_audit_events` | `owner_id` | TEXT | yes | no |
| `action_audit_events` | `actor_user_id` | TEXT | **NO** | no |
| `activity_events` | `actor_user_id` | UUID | **NO** | no |
| `knowledge_candidates` | `actor_user_id` | UUID | **NO** | no |

Three of the six tables carry `NOT NULL` actor columns and one carries a real FK — a simple `DELETE FROM users WHERE id = :user_id` FAILS unless all references are handled first.

```sql
BEGIN;

-- 1. Free api_keys owned by the user (no cascade on api_keys.user_id).
DELETE FROM api_keys WHERE user_id = :user_id;

-- 2. Zero the user's audit attribution on nullable columns (preserves
--    tenant-level stats; severs the PII link — typical Art. 17 posture
--    of "forget the person, keep the statistics"). The audit_log FK
--    accepts NULL so this also unblocks step 4.
UPDATE audit_log         SET actor_user_id = NULL WHERE actor_user_id = :user_id;
UPDATE billing_events    SET actor_user_id = NULL WHERE actor_user_id = :user_id;
UPDATE tool_audit_events SET owner_id      = NULL WHERE owner_id      = :user_id::text;

-- 3. NOT NULL columns — must DELETE or rewrite. DELETE is simpler but
--    loses the row from tenant-level aggregates. Pick one strategy per
--    table and document it in your compliance log. Representative delete:
DELETE FROM action_audit_events  WHERE actor_user_id  = :user_id::text;
DELETE FROM activity_events      WHERE actor_user_id  = :user_id;
DELETE FROM knowledge_candidates WHERE actor_user_id  = :user_id;
-- (Alternative — rewrite to a reserved "deleted-user" sentinel UUID so
--  tenant-level counts stay intact. Requires pre-creating the sentinel.)

-- 4. Delete the user. user_sessions + team_members cascade off user_id.
DELETE FROM users WHERE id = :user_id;

COMMIT;
```

The nullable/`NOT NULL` split is a known schema asymmetry — it pre-dates the formal erasure contract and hasn't been reconciled on trunk. File an ISSUE against EPIC 4 if your deployment needs uniform nullability; the current pragmatic path is DELETE for the three `NOT NULL` tables. If the operator needs a cryptographic shred instead of a delete, add a hashing step before the UPDATE — but Gadgetron does not ship a helper for it on trunk.

---

## Production security hardening

`troubleshooting.md §"failed to run database migrations"` suggests `GRANT ALL PRIVILEGES` as a quick unblock — that's a **development** shortcut. Production deployments should run with least-privilege PostgreSQL roles + rotated secrets. This section collects the hardening recipes scattered across `architecture/platform-architecture.md` §Role grants, `installation.md §1 systemd unit`, and `auth.md §Cookie-session auth` into one actionable checklist.

### Pre-launch checklist

| # | Item | Rationale / reference |
|---|---|---|
| 1 | TLS terminates at the reverse proxy, not the gateway | `installation.md §2 Nginx` / §3 Caddy. Gateway does NOT emit `Secure` on session cookies; TLS is an operator responsibility. |
| 2 | `CorsLayer::permissive()` is NOT mounted | D-6 — deliberately absent. Cross-origin browser access is blocked unless you add CORS at the reverse proxy for specific origins. |
| 3 | `GADGETRON_DATABASE_URL` is populated via a secret manager (env var, Vault, KMS), never hardcoded in `gadgetron.toml` | The `Secret<String>` wrapper masks it in tracing logs, but the config file on disk is not encrypted by Gadgetron. |
| 4 | `[auth.bootstrap]` password env var is set for first boot, then **removed** once the admin user is created | The bootstrap path fires exactly once (when `users` is empty) — re-running it is a no-op, but leaving the env var wired leaks the password into process env for no benefit. |
| 5 | Minimum PostgreSQL privileges applied (see §Production PG role below) | Production ≠ development. `GRANT ALL` on a live tenant database breaks audit-trail integrity guarantees. |
| 6 | API key rotation cadence defined (recommend 90 days for `Management`-scope, 30 for service keys) | `api_keys.revoked_at` supports the revoke-new-after-old pattern per `api-reference.md §Rotate a key`. |
| 7 | Admin `[auth.bootstrap]` password rotated before first production use | The initial value was likely shared via config or env during provisioning — rotate to a secret the bootstrapping operator never saw. |
| 8 | `audit_log` retention policy defined (see §Audit log retention and tenant lifecycle above) | Unbounded growth is a liability. Pick a cutoff (90d / 365d / 7y depending on compliance) and schedule pruning. |
| 9 | Observability alerts wired for the 7 signals in `installation.md §7.3` | Most notably: migration failure + 401 surge + `audit_log` / `billing_events` disk-growth. |
| 10 | `gadgetron doctor` + `curl /health` + `curl /ready` all green from an external probe, not just localhost | Per `installation.md §4 Health probe pattern` — what reaches the probe is what's actually serving. |

Run through this list before first production cutover. Store the signed-off version (with dates + operator initials) in your ops repo as the go-live gate.

### Production PostgreSQL role

Gadgetron does not ship the role SQL — your DBA / IaC pipeline must apply it. The minimum-privilege shape, derived from the 16 tables on trunk (`crates/gadgetron-xaas/migrations/*.sql`):

```sql
-- 1. Runtime role (what `gadgetron serve` connects as)
CREATE ROLE gadgetron_app LOGIN PASSWORD :'GADGETRON_DB_PASSWORD';

-- 2. Full CRUD on mutable business tables
GRANT SELECT, INSERT, UPDATE, DELETE ON
    tenants, api_keys, quota_configs,
    users, teams, team_members, user_sessions,
    wiki_pages, wiki_chunks,
    knowledge_candidates, candidate_decisions
TO gadgetron_app;

-- 3. INSERT-only on append-only audit + ledger tables.
--    audit_log enforces compliance append-only at the DB layer — an attacker
--    who compromises the application process cannot rewrite history through
--    this connection even if application code is bypassed.
GRANT SELECT, INSERT ON
    audit_log, billing_events,
    tool_audit_events, action_audit_events,
    activity_events
TO gadgetron_app;

REVOKE UPDATE, DELETE, TRUNCATE ON
    audit_log, billing_events,
    tool_audit_events, action_audit_events,
    activity_events
FROM gadgetron_app;

-- 4. Connect + schema usage on the target database
GRANT CONNECT ON DATABASE gadgetron TO gadgetron_app;
GRANT USAGE ON SCHEMA public TO gadgetron_app;

-- 5. Future-table inheritance — new tables from future migrations get the
--    same CRUD default; tighten on append-only tables in that migration's
--    companion GRANT block.
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO gadgetron_app;
```

**Separate migration role** (runs only at deploy time):

```sql
CREATE ROLE gadgetron_migrator LOGIN PASSWORD :'GADGETRON_MIGRATOR_PASSWORD';
GRANT CONNECT ON DATABASE gadgetron TO gadgetron_migrator;
GRANT CREATE ON SCHEMA public TO gadgetron_migrator;
-- CREATE / DROP / ALTER privs on existing tables are implicit via table ownership;
-- easier: make gadgetron_migrator the owner of all tables after initial creation.
ALTER SCHEMA public OWNER TO gadgetron_migrator;
```

On trunk, `gadgetron serve` itself runs `sqlx::migrate!` on startup (`crates/gadgetron-cli/src/main.rs:761/799/1428`) — so `gadgetron_app` also needs CREATE on the schema if you only maintain one role. The two-role split is cleaner: override the connection string to `gadgetron_migrator` for one-shot migration runs (e.g. a systemd `ExecStartPre` that runs a migration-only binary, or a separate deploy step), then `gadgetron serve` uses `gadgetron_app`. This is deployment-specific — pick the shape your operations allows.

### Secret + key rotation cadence

| Secret | Rotation interval | How |
|---|---|---|
| `GADGETRON_DATABASE_URL` password | 180 days | `ALTER ROLE gadgetron_app WITH PASSWORD ...` + restart gadgetron (brief outage) |
| Admin API keys (Management scope) | 90 days | `api-reference.md §Rotate a key` (create-new-then-revoke pattern — no outage) |
| Member API keys (OpenAiCompat) | 180 days | Same rotate pattern; for many keys, run in batches |
| Service role keys (programmatic callers) | 30 days | Aggressive rotation recommended since these often land in CI secrets; same rotate pattern |
| Admin user passwords | 90 days | `gadgetron user` subcommand does not ship a password-reset flow yet — operators currently update `users.password_hash` directly with a freshly argon2id'd digest (see `crates/gadgetron-xaas/src/auth.rs` for the argon2 parameters; any argon2id library with the same params round-trips). A proper CLI reset is tracked as a post-v1.0.0 DX item. |
| Cookie session secret | Automatic — `user_sessions.cookie_hash` rotates per login, no global secret to manage | — |

Record each rotation in your ops runbook. The `audit_log` captures the resulting key-usage pattern change — unusual gaps in a rotated key's traffic are a signal that a downstream client didn't get the memo.

### Common hardening mistakes (flagged by real incident reports)

- **Leaving `[auth.bootstrap]` enabled after first boot.** The bootstrap is a one-shot. Once the first admin exists, the config block is inert — but keeping the env var wired means the password is readable by anyone with `/proc/<pid>/environ` access. Remove the env var after go-live.
- **Granting `ALL PRIVILEGES` to `gadgetron_app`.** Loses the append-only guarantee on `audit_log` + `billing_events`. A compromised process can rewrite history.
- **Exposing `/health` or `/ready` to the internet instead of the LB only.** These probes return stack details on failure — keep them on the loopback / internal network.
- **Using the same API key across services.** Audit attribution lumps all traffic under one key. Mint per-service keys (`label` field in POST /keys helps) so you can revoke granularly without collateral damage.
- **Forgetting the reverse-proxy WebSocket upgrade directive** (Nginx only — see `installation.md §2`). `/api/v1/web/workbench/events/ws` fails silently without it, which masks the dashboard going dark.
