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
[4]  auth_middleware                   — Bearer → Arc<ValidatedKey>; 401 on parse/lookup failure
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
| 4 | **HTTP 401** `invalid_api_key` | Missing/malformed `Authorization: Bearer ...`, unknown key, or revoked key. Audited via SEC-M7. |
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

## Security notes

- The server never logs raw API key values. The `GADGETRON_DATABASE_URL` is wrapped in a `Secret<String>` type and is never emitted to logs.
- The `api_keys.key_hash` column stores only the SHA-256 hash. Even with direct database access, the original key cannot be recovered from the hash.
- Auth failures (401) are audited to the structured audit channel. In the current implementation, audit entries are written to tracing logs; PostgreSQL persistence remains future work.
