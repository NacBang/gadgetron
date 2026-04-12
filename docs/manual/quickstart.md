# Quickstart: zero to first request in 5 minutes

This guide walks through every step required to run Gadgetron and make a successful chat completion request. It uses OpenAI as the upstream provider. All commands are copy-pasteable and intended to run in order.

**Prerequisites:** Rust 1.80+, Docker (for the PostgreSQL one-liner), an OpenAI API key.

---

## Step 1 — Start PostgreSQL

```sh
docker run -d \
  --name gadgetron-pg \
  -e POSTGRES_USER=gadgetron \
  -e POSTGRES_PASSWORD=secret \
  -e POSTGRES_DB=gadgetron \
  -p 5432:5432 \
  postgres:16
```

Wait approximately 5 seconds for PostgreSQL to become ready:

```sh
docker exec gadgetron-pg pg_isready -U gadgetron
```

Expected output: `localhost:5432 - accepting connections`

---

## Step 2 — Clone and build

```sh
git clone https://github.com/your-org/gadgetron.git
cd gadgetron
cargo build --release -p gadgetron-cli
```

The binary is at `./target/release/gadgetron`.

---

## Step 3 — Write a minimal config file

Create `gadgetron.toml` in the directory where you will run the server:

```toml
[server]
bind = "0.0.0.0:8080"

[providers.openai]
type = "openai"
api_key = "${OPENAI_API_KEY}"
models = ["gpt-4o-mini"]
```

---

## Step 4 — Create a tenant and API key

Gadgetron's tenant and API key management CLI commands are not yet implemented (planned for Sprint 4). For now, insert directly into PostgreSQL.

The server must have run at least once first so that migrations create the schema. Start it briefly to apply migrations, then stop it with Ctrl-C:

```sh
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@localhost:5432/gadgetron"
export OPENAI_API_KEY="sk-your-openai-key-here"
./target/release/gadgetron
# Wait for "listening" log line, then press Ctrl-C
```

Expected log output (last line before you press Ctrl-C):

```
INFO gadgetron starting bind=0.0.0.0:8080
INFO database migrations applied
INFO provider registered name=openai
INFO listening addr=0.0.0.0:8080
```

Now insert a tenant and API key. The API key secret below is `gad_live_quickstart0000000000000000`. You must hash it with SHA-256 before storing; the hash shown here is the SHA-256 of that exact string.

```sh
docker exec -i gadgetron-pg psql -U gadgetron -d gadgetron <<'SQL'
-- Insert a tenant
INSERT INTO tenants (id, name, status)
VALUES ('00000000-0000-0000-0000-000000000001', 'quickstart-tenant', 'Active');

-- Insert an API key.
-- The key secret is: gad_live_quickstart0000000000000000
-- SHA-256 of that secret (hex):
--   Run: echo -n 'gad_live_quickstart0000000000000000' | sha256sum
-- Replace the key_hash value below with the output of that command.
INSERT INTO api_keys (id, tenant_id, prefix, key_hash, kind, scopes, name)
VALUES (
  '00000000-0000-0000-0000-000000000002',
  '00000000-0000-0000-0000-000000000001',
  'gad_live',
  '$(echo -n "gad_live_quickstart0000000000000000" | sha256sum | cut -d" " -f1)',
  'live',
  ARRAY['OpenAiCompat'],
  'quickstart-key'
);

-- Insert a quota config so the tenant has spending headroom.
INSERT INTO quota_configs (tenant_id, daily_limit_cents, monthly_limit_cents)
VALUES ('00000000-0000-0000-0000-000000000001', 100000, 1000000);
SQL
```

**Note on the key hash:** the SQL above uses shell command substitution which will not expand inside `psql`. Run the hash command separately and substitute the literal value:

```sh
# Step A: get the hash
echo -n 'gad_live_quickstart0000000000000000' | sha256sum | cut -d' ' -f1
# Example output: 3e7a2f1c... (64 hex characters)

# Step B: insert with the literal hash value
docker exec -i gadgetron-pg psql -U gadgetron -d gadgetron <<SQL
INSERT INTO tenants (id, name, status)
VALUES ('00000000-0000-0000-0000-000000000001', 'quickstart-tenant', 'Active')
ON CONFLICT DO NOTHING;

INSERT INTO api_keys (id, tenant_id, prefix, key_hash, kind, scopes, name)
VALUES (
  '00000000-0000-0000-0000-000000000002',
  '00000000-0000-0000-0000-000000000001',
  'gad_live',
  'PASTE_YOUR_64_CHAR_HASH_HERE',
  'live',
  ARRAY['OpenAiCompat'],
  'quickstart-key'
)
ON CONFLICT DO NOTHING;

INSERT INTO quota_configs (tenant_id, daily_limit_cents, monthly_limit_cents)
VALUES ('00000000-0000-0000-0000-000000000001', 100000, 1000000)
ON CONFLICT (tenant_id) DO NOTHING;
SQL
```

---

## Step 5 — Start the server

```sh
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@localhost:5432/gadgetron"
export OPENAI_API_KEY="sk-your-openai-key-here"
./target/release/gadgetron
```

Expected log output:

```
INFO gadgetron starting bind=0.0.0.0:8080
INFO database migrations applied
INFO provider registered name=openai
INFO listening addr=0.0.0.0:8080
```

---

## Step 6 — Send your first request

Open a second terminal. Replace `gad_live_quickstart0000000000000000` with your actual key secret.

```sh
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer gad_live_quickstart0000000000000000" \
  -d '{
    "model": "gpt-4o-mini",
    "messages": [{"role": "user", "content": "Say hello in one sentence."}],
    "stream": false
  }' | jq .
```

### Expected response

```json
{
  "id": "chatcmpl-...",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "gpt-4o-mini",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "Hello! I'm here and ready to assist you today."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 15,
    "completion_tokens": 14,
    "total_tokens": 29
  }
}
```

---

## Streaming request

```sh
curl -N http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer gad_live_quickstart0000000000000000" \
  -d '{
    "model": "gpt-4o-mini",
    "messages": [{"role": "user", "content": "Count to three."}],
    "stream": true
  }'
```

Each chunk arrives as a `data: {...}` SSE line. The final line is `data: [DONE]`.

---

## Verify health probes (no auth required)

```sh
curl -s http://localhost:8080/health | jq .
# {"status":"ok"}

curl -s http://localhost:8080/ready | jq .
# {"status":"ready"}
```

---

## What's next

- Add more providers: see [configuration.md](configuration.md)
- Understand the auth and scope system: see [auth.md](auth.md)
- Full API reference: see [api-reference.md](api-reference.md)
- Troubleshoot errors: see [troubleshooting.md](troubleshooting.md)
