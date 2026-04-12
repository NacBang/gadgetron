# Quickstart: zero to first request in 5 minutes

This guide walks through every step required to run Gadgetron and make a successful chat completion request. All commands are copy-pasteable and intended to run in order.

As of Sprint 4, Gadgetron routes to real LLM providers. Two quickstart paths are provided: one using OpenAI (requires an OpenAI API key), and one using a self-hosted vLLM instance (no external API key).

**Prerequisites (OpenAI path):** Rust 1.80+, Docker (for the PostgreSQL one-liner), an OpenAI API key.

**Prerequisites (vLLM path):** Rust 1.80+, Docker (for the PostgreSQL one-liner), a reachable vLLM HTTP endpoint.

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
git clone https://github.com/NacBang/gadgetron.git
cd gadgetron
cargo build --release -p gadgetron-cli
```

The binary is at `./target/release/gadgetron`.

---

## Step 3 — Write a minimal config file

Create `gadgetron.toml` in the directory where you will run the server.

**Option A — OpenAI provider:**

```toml
[server]
bind = "0.0.0.0:8080"

[providers.openai]
type = "openai"
api_key = "${OPENAI_API_KEY}"
models = ["gpt-4o-mini"]
```

**Option B — vLLM provider (self-hosted, no external API key):**

```toml
[server]
bind = "0.0.0.0:8080"

[providers.gemma4]
type = "vllm"
endpoint = "http://10.100.1.5:8100"
models = ["gemma-4-27b-it"]
```

Replace `10.100.1.5:8100` with the host and port of your vLLM instance. The model name must match the `--served-model-name` value (or the default model name) that vLLM reports.

---

## Step 4 — Create a tenant and API key

Gadgetron's tenant and API key management CLI commands are not yet implemented. For now, insert directly into PostgreSQL.

The server must have run at least once first so that migrations create the schema. Start it briefly to apply migrations, then stop it with Ctrl-C.

For the OpenAI path:

```sh
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@localhost:5432/gadgetron"
export OPENAI_API_KEY="sk-your-openai-key-here"
./target/release/gadgetron
# Wait for "listening" log line, then press Ctrl-C
```

For the vLLM path:

```sh
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@localhost:5432/gadgetron"
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

The `provider registered` line shows the key you chose under `[providers.*]` (e.g. `name=gemma4` for the vLLM example).

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

For the OpenAI path:

```sh
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@localhost:5432/gadgetron"
export OPENAI_API_KEY="sk-your-openai-key-here"
./target/release/gadgetron
```

For the vLLM path (no upstream API key needed):

```sh
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@localhost:5432/gadgetron"
./target/release/gadgetron
```

Expected log output:

```
INFO gadgetron starting bind=0.0.0.0:8080
INFO database migrations applied
INFO provider registered name=openai
INFO listening addr=0.0.0.0:8080
```

The server is ready when the `listening` line appears.

---

## Step 6 — Send your first request

Open a second terminal. Replace `gad_live_quickstart0000000000000000` with your actual key secret.

**OpenAI provider example:**

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

**vLLM provider example** (substitute the model name you configured):

```sh
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer gad_live_quickstart0000000000000000" \
  -d '{
    "model": "gemma-4-27b-it",
    "messages": [{"role": "user", "content": "Say hello in one sentence."}],
    "stream": false
  }' | jq .
```

### Expected response (non-streaming)

```json
{
  "id": "chatcmpl-abc123",
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

### Expected streaming output

```
data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"role":"assistant","content":"1"},"finish_reason":null}]}

data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":", 2"},"finish_reason":null}]}

data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":", 3."},"finish_reason":"stop"}]}

data: [DONE]
```

The final line is always `data: [DONE]`. Each intermediate chunk contains a `delta` with incremental content. The `finish_reason` field is `null` for all chunks except the last, where it is `"stop"` (or another reason such as `"length"`).

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

---

## Optional: open the TUI dashboard

The terminal dashboard runs independently of the server. In a separate terminal:

```sh
./target/release/gadgetron --tui
```

The dashboard shows a 3-column layout (Nodes / Models / Requests) with color-coded GPU metrics. In Sprint 5 it displays demo data; live cluster data requires Sprint 6. Press `q` or `Esc` to exit. See [tui.md](tui.md) for the full reference.
