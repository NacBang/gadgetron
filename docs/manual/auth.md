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

## Scope system

Each API key holds a list of scopes. A scope is a coarse-grained permission. The three defined scopes are:

| Scope | String value in DB | What it permits |
|-------|--------------------|-----------------|
| `OpenAiCompat` | `"OpenAiCompat"` | All `/v1/` routes (`POST /v1/chat/completions`, `GET /v1/models`) **and** all `/api/v1/web/workbench/` routes |
| `Management` | `"Management"` | All other `/api/v1/` routes (nodes, model deploy, usage, costs) |
| `XaasAdmin` | `"XaasAdmin"` | Reserved for `/api/v1/xaas/` routes (internal XaaS administration). **No routes are mounted under this prefix on trunk yet**; keys with this scope will hit 404 until XaaS admin endpoints land in a later phase. |

A key can hold multiple scopes. The `api_keys.scopes` column is a `TEXT[]` (PostgreSQL array). The default when inserting a new key without specifying scopes is `ARRAY['OpenAiCompat']`.

**Scope enforcement** is performed by `scope_guard_middleware` (layer 6, innermost of the auth stack):

| Path prefix | Required scope | Note |
|-------------|----------------|------|
| `/v1/` | `OpenAiCompat` | |
| `/api/v1/web/workbench/` | `OpenAiCompat` | W3-WEB-2 exception — workbench uses the same scope as `/v1/` |
| `/api/v1/xaas/` | `XaasAdmin` | |
| `/api/v1/` | `Management` | Catch-all for admin routes |
| `/health`, `/ready` | none | Public; never reach this layer |

A key with `OpenAiCompat` scope can access `/v1/` routes and `/api/v1/web/workbench/` routes. It cannot access other `/api/v1/` routes (which require `Management`) and will receive HTTP 403 if it tries.

---

## Creating API keys

### Using the CLI (Sprint 7+)

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
