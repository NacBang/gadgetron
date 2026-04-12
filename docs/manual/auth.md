# Authentication and Authorization

> **Note:** `gadgetron key` CLI is not yet implemented. Create keys via SQL — see [quickstart.md](quickstart.md) Step 4.

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
| `OpenAiCompat` | `"OpenAiCompat"` | All `/v1/` routes: `POST /v1/chat/completions`, `GET /v1/models` |
| `Management` | `"Management"` | All `/api/v1/` routes (nodes, model deploy, usage, costs) |
| `XaasAdmin` | `"XaasAdmin"` | All `/api/v1/xaas/` routes (reserved for internal XaaS administration) |

A key can hold multiple scopes. The `api_keys.scopes` column is a `TEXT[]` (PostgreSQL array). The default when inserting a new key without specifying scopes is `ARRAY['OpenAiCompat']`.

**Scope enforcement** is performed by `scope_guard_middleware` (layer 6, innermost of the auth stack):

| Path prefix | Required scope |
|-------------|----------------|
| `/v1/` | `OpenAiCompat` |
| `/api/v1/xaas/` | `XaasAdmin` |
| `/api/v1/` | `Management` |
| `/health`, `/ready` | none (public, never reach this layer) |

A key with `OpenAiCompat` scope that requests `GET /api/v1/nodes` (which requires `Management`) will receive HTTP 403.

---

## Creating API keys

The CLI command for creating API keys is not yet implemented (Sprint 4). Use direct SQL inserts.

To create a key manually:

1. Generate a cryptographically secure key string. The key must match the format `gad_live_<secret>` where the secret is at least 16 characters:

   ```sh
   echo "gad_live_$(openssl rand -hex 16)"
   # Example output: gad_live_a3f8e1d2c4b5a6e7f8d9c0b1a2e3d4f5
   ```

2. Compute the SHA-256 hash of the complete key string (including the `gad_live_` prefix):

   ```sh
   echo -n 'gad_live_a3f8e1d2c4b5a6e7f8d9c0b1a2e3d4f5' | sha256sum | cut -d' ' -f1
   # Example output: 7a3f... (64 hex characters)
   ```

3. Insert into PostgreSQL (substitute your actual tenant UUID and hash):

   ```sql
   INSERT INTO api_keys (tenant_id, prefix, key_hash, kind, scopes, name)
   VALUES (
     'your-tenant-uuid-here',
     'gad_live',
     'your-64-char-sha256-hash-here',
     'live',
     ARRAY['OpenAiCompat'],
     'my-api-key'
   );
   ```

4. Store the original key string (e.g. `gad_live_a3f8e1d2c4b5a6e7f8d9c0b1a2e3d4f5`) securely. Gadgetron never stores the plain key — only the hash. There is no way to recover the key from the database.

---

## Revoking API keys

To revoke a key, set its `revoked_at` timestamp in the database:

```sql
UPDATE api_keys
SET revoked_at = NOW()
WHERE prefix = 'gad_live'
  AND key_hash = 'your-64-char-sha256-hash-here';
```

The validator checks `revoked_at IS NULL` in its query. A revoked key will no longer validate. However, due to the 10-minute cache TTL, a revoked key may continue to work for up to 10 minutes after revocation until its cache entry expires.

To force immediate invalidation, the `KeyValidator.invalidate` method exists but is not yet exposed via a CLI or API endpoint. It would need to be called programmatically.

---

## Security notes

- The server never logs raw API key values. The `GADGETRON_DATABASE_URL` is wrapped in a `Secret<String>` type and is never emitted to logs.
- The `api_keys.key_hash` column stores only the SHA-256 hash. Even with direct database access, the original key cannot be recovered from the hash.
- Auth failures (401) are audited to the structured audit channel. In Sprint 1-3, audit entries are written to tracing logs. PostgreSQL persistence is Sprint 2+.
