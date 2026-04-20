<!-- workspace version: 0.5.14 -->

# Multi-user operator walkthrough

This page is for operators bringing up the first multi-user deployment on a pg-backed Gadgetron server. It walks the lifecycle that landed across ISSUEs 14 through 22: first-admin bootstrap, member creation, team assignment, cookie login, cookie-vs-Bearer scope checks, self-service key mint, and tenant-pinned audit reads. Use it after the server is already running and the first admin was created through the bootstrap flow in [quickstart.md](quickstart.md). For the full contracts, use [auth.md](auth.md), [api-reference.md](api-reference.md), and [cli.md](cli.md).

---

## Concepts

- `admin` can use cookie sessions or Bearer keys. Cookie-role synthesis grants `OpenAiCompat` plus `Management`.
- `member` can use cookie sessions or Bearer keys. Cookie-role synthesis grants only `OpenAiCompat`.
- `service` is API-key only. Login rejects it, so it cannot drive cookie sessions.
- `OpenAiCompat` covers `POST /v1/chat/completions`, `GET /workbench/bootstrap`, and self-service key mint at `POST /workbench/keys`.
- `Management` covers operator endpoints such as `/admin/users`, `/admin/teams`, `/admin/audit/log`, and `/admin/billing/events`.
- `XaasAdmin` is part of the scope model, but this walkthrough does not use it. See [auth.md](auth.md) for the full scope table.
- As of ISSUE 16 / v0.5.9, Bearer and cookie auth both reach protected routes when the caller has the right scope. Cookie auth is used only when no Bearer header is present.
- Audit attribution uses three fields together: `actor_user_id`, `actor_api_key_id`, and the nil sentinel `api_key_id = 00000000-0000-0000-0000-000000000000` for cookie traffic.

---

## Walkthrough

### 1. Assumed state

Assume PostgreSQL is up, `gadgetron serve` is already running, and the first admin was created by the bootstrap block from [quickstart.md](quickstart.md). Also assume you already hold one admin Bearer key with `OpenAiCompat` and `Management`, for example from the key flow in [cli.md](cli.md).

```bash
export GAD="http://127.0.0.1:8080"
export AUTH="$GAD/api/v1/auth"
export WB="$GAD/api/v1/web/workbench"
export JAR="/tmp/gadgetron-alice.cookies"
export MGMT_KEY="gad_live_0123456789abcdef0123456789abcdef"

rm -f "$JAR"

curl -sS -o /dev/null -w '%{http_code}\n' \
  -H "Authorization: Bearer $MGMT_KEY" \
  "$WB/bootstrap"
```

Expected output:

```bash
200
```

### 2. Admin creates member Alice

Create Alice as a `member`. This is the first operator-managed user after bootstrap.

```bash
ALICE_JSON="$(
  curl -sS -X POST \
    -H "Authorization: Bearer $MGMT_KEY" \
    -H 'Content-Type: application/json' \
    -d '{
      "email": "alice@example.com",
      "display_name": "Alice Example",
      "role": "member",
      "password": "AlicePass!234"
    }' \
    "$WB/admin/users"
)"

echo "$ALICE_JSON" | jq .
export ALICE_USER_ID="$(echo "$ALICE_JSON" | jq -r '.id')"
printf '%s\n' "$ALICE_USER_ID"
```

Expected response snippet:

```json
{
  "id": "e61d1784-9cc3-46c8-aab0-5a14d7fb0f16",
  "email": "alice@example.com",
  "display_name": "Alice Example",
  "role": "member",
  "is_active": true,
  "created_at": "2026-04-20T10:21:14.118Z"
}
```

Keep `ALICE_USER_ID`. Steps 3, 8, and 9 use it.

### 3. Admin creates team `platform` and adds Alice

Create a tenant-scoped team, then attach Alice as a member.

```bash
TEAM_JSON="$(
  curl -sS -X POST \
    -H "Authorization: Bearer $MGMT_KEY" \
    -H 'Content-Type: application/json' \
    -d '{
      "id": "platform",
      "display_name": "Platform",
      "description": "Operators and platform engineers"
    }' \
    "$WB/admin/teams"
)"

echo "$TEAM_JSON" | jq .

MEMBER_JSON="$(
  curl -sS -X POST \
    -H "Authorization: Bearer $MGMT_KEY" \
    -H 'Content-Type: application/json' \
    -d "{\"user_id\":\"$ALICE_USER_ID\",\"role\":\"member\"}" \
    "$WB/admin/teams/platform/members"
)"

echo "$MEMBER_JSON" | jq .
```

Expected team-create snippet:

```json
{
  "id": "platform",
  "display_name": "Platform",
  "description": "Operators and platform engineers",
  "created_at": "2026-04-20T10:22:06.403Z"
}
```

Expected team-member snippet:

```json
{
  "team_id": "platform",
  "user_id": "e61d1784-9cc3-46c8-aab0-5a14d7fb0f16",
  "role": "member",
  "added_at": "2026-04-20T10:22:11.804Z"
}
```

### 4. Alice logs in with a cookie session

Capture the cookie jar, then confirm the session with `GET /auth/whoami`.

```bash
curl -i -sS -c "$JAR" -X POST \
  -H 'Content-Type: application/json' \
  -d '{
    "email": "alice@example.com",
    "password": "AlicePass!234"
  }' \
  "$AUTH/login"

curl -sS -b "$JAR" "$AUTH/whoami" | jq .
```

Expected login header snippet:

```bash
HTTP/1.1 200 OK
Set-Cookie: gadgetron_session=4ad0c3b601af4fc4b49f5c4dca35d9a52b6d8f685c4d4196b3d4d1e1b338e4d8; HttpOnly; SameSite=Lax; Path=/; Max-Age=86400
```

Expected `whoami` snippet:

```json
{
  "session_id": "2f3137e3-a892-44db-919d-4d7d02791507",
  "user_id": "e61d1784-9cc3-46c8-aab0-5a14d7fb0f16",
  "tenant_id": "4c7b47aa-7284-4658-86a4-831828f91f1f",
  "expires_at": "2026-04-21T10:23:09.221Z"
}
```

### 5. Alice calls `/v1/chat/completions` with the cookie jar

Do not send a Bearer header here. ISSUE 16 unified the middleware so cookie sessions reach the same protected route when Bearer is absent.

```bash
curl -sS -b "$JAR" -X POST \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "gpt-4o-mini",
    "messages": [
      {"role": "user", "content": "Reply with the single word ready."}
    ],
    "stream": false
  }' \
  "$GAD/v1/chat/completions" \
  | jq '{id, model, choices: [.choices[] | {finish_reason, message: {role: .message.role}}]}'
```

Expected response snippet:

```json
{
  "id": "chatcmpl_01JQY5MZ1J3J9M8CQ4R3K7A9K1",
  "model": "gpt-4o-mini"
}
```

### 6. Alice mints her own key at `POST /workbench/keys`

This is the self-service key path from ISSUE 14. The wire values for requested scopes are lowercase strings, so the default row here shows `openai_compat`.

```bash
ALICE_KEY_JSON="$(
  curl -sS -b "$JAR" -X POST \
    -H 'Content-Type: application/json' \
    -d '{
      "label": "alice-laptop",
      "kind": "live"
    }' \
    "$WB/keys"
)"

echo "$ALICE_KEY_JSON" | jq .
export ALICE_KEY_ID="$(echo "$ALICE_KEY_JSON" | jq -r '.id')"
export ALICE_RAW_KEY="$(echo "$ALICE_KEY_JSON" | jq -r '.raw_key')"
```

Expected response snippet:

```json
{
  "id": "2ddf6a51-3e4a-4870-9d55-8857d1f51f77",
  "raw_key": "gad_live_f0f9e60b8d0548f5b1c82f4f51d1f913",
  "scopes": [
    "openai_compat"
  ],
  "label": "alice-laptop"
}
```

`raw_key` is shown once. Store it immediately. This is the same one-time exposure rule called out in [cli.md](cli.md), and it exists for the same SEC-M7 reason.

### 7. Demonstrate the scope gates

First, show that Alice's member cookie cannot enter a `Management` route. Second, show that the self-service key mint path enforces scope narrowing.

```bash
curl -i -sS \
  -b "$JAR" \
  "$WB/admin/users"

curl -i -sS -b "$JAR" -X POST \
  -H 'Content-Type: application/json' \
  -d '{
    "label": "alice-escalation-attempt",
    "scopes": ["management"]
  }' \
  "$WB/keys"
```

Expected `GET /admin/users` failure snippet:

```bash
HTTP/1.1 403 Forbidden
```

```json
{
  "error": {
    "code": "forbidden"
  }
}
```

Expected scope-narrowing failure snippet:

```bash
HTTP/1.1 400 Bad Request
```

```json
{
  "error": {
    "message": "requested scope 'management' exceeds caller's own scopes"
  }
}
```

A member cookie reaches `OpenAiCompat`, but not `Management`, and self-service key mint cannot grant more than the caller already has.

### 8. Create one Bearer row, then query `GET /admin/audit/log`

Alice already created one cookie-attributed chat row in Step 5. Now create one Bearer-attributed chat row with the key from Step 6, then query the audit log by `actor_user_id`.

```bash
curl -sS -o /dev/null -w '%{http_code}\n' -X POST \
  -H "Authorization: Bearer $ALICE_RAW_KEY" \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "gpt-4o-mini",
    "messages": [
      {"role": "user", "content": "Reply with the single word ready."}
    ],
    "stream": false
  }' \
  "$GAD/v1/chat/completions"

curl -sS \
  -H "Authorization: Bearer $MGMT_KEY" \
  "$WB/admin/audit/log?actor_user_id=$ALICE_USER_ID&limit=10" \
  | jq .
```

Expected response snippet:

```json
{
  "rows": [
    {
      "api_key_id": "2ddf6a51-3e4a-4870-9d55-8857d1f51f77",
      "actor_user_id": "e61d1784-9cc3-46c8-aab0-5a14d7fb0f16",
      "actor_api_key_id": "2ddf6a51-3e4a-4870-9d55-8857d1f51f77",
      "status": "ok",
      "timestamp": "2026-04-20T10:27:44.111Z"
    },
    {
      "api_key_id": "00000000-0000-0000-0000-000000000000",
      "actor_user_id": "e61d1784-9cc3-46c8-aab0-5a14d7fb0f16",
      "actor_api_key_id": null,
      "status": "ok",
      "timestamp": "2026-04-20T10:24:03.441Z"
    }
  ],
  "returned": 2
}
```

Read the discriminator this way:

- `api_key_id = 00000000-0000-0000-0000-000000000000` means cookie traffic.
- Any non-nil `api_key_id` means Bearer traffic.
- `actor_api_key_id = null` means no real API key was involved.
- `actor_user_id` stays populated in both cases, so the operator can group activity by user.

`/v1/chat/completions` always emits one audit row per request on both `ok` and error paths. If a chat request failed, you would still expect a row here.

### 9. Cleanup and rollback

Key revoke is self-service in v0.5.14, so the cleanup is a two-part rollback: Alice revokes her own key, then the admin deletes Alice and the team.

```bash
curl -sS -b "$JAR" -X DELETE \
  "$WB/keys/$ALICE_KEY_ID" | jq .

curl -sS -X DELETE \
  -H "Authorization: Bearer $MGMT_KEY" \
  "$WB/admin/users/$ALICE_USER_ID" | jq .

curl -sS -X DELETE \
  -H "Authorization: Bearer $MGMT_KEY" \
  "$WB/admin/teams/platform" | jq .
```

Expected key-revoke snippet:

```json
{
  "revoked": true,
  "key_id": "2ddf6a51-3e4a-4870-9d55-8857d1f51f77"
}
```

The delete calls then return `{"deleted": true, "user_id": "<alice_uuid>"}` for Alice and `{"ok": true}` for the team.

---

## Harness invariants

| Gate ID | Pinned behavior |
|---|---|
| `7v.1` | Admin user creation works, and an `OpenAiCompat` caller gets `403` on a `Management` route. |
| `7v.2` | Self-service key mint returns the raw key once, and only once. |
| `7v.5` | Bootstrap creates the first admin when the users table is empty and `[auth.bootstrap]` is present. |
| `7v.6` | Unified middleware lets cookie sessions hit protected routes, with admin cookies reaching `Management` and `OpenAiCompat`. |
| `7v.7` | Chat audit rows persist to `audit_log`, and Bearer-attributed rows carry non-null `actor_api_key_id`. |
| `7v.8` | Admin audit-log query returns data for a `Management` caller, and rejects an `OpenAiCompat` caller with `403`. |

---

## Troubleshooting cross-refs

- [troubleshooting.md#http-401-unauthorized--cookie-session-failures-issue-16--v059-unified-auth](troubleshooting.md#http-401-unauthorized--cookie-session-failures-issue-16--v059-unified-auth) explains how to separate missing-cookie, expired-session, revoked-session, inactive-user, service-role, and no-db failures on the unified cookie path.
- [troubleshooting.md#users-table-is-empty-but-authbootstrap-is-missing-issue-14-task-142--v057](troubleshooting.md#users-table-is-empty-but-authbootstrap-is-missing-issue-14-task-142--v057) shows the hard startup failure when the `users` table is empty but `[auth.bootstrap]` is absent.
