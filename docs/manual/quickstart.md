# Quickstart: zero to first request in 5 minutes

This guide walks through every step required to run Gadgetron and make a successful chat completion request. All commands are copy-pasteable and intended to run in order.

As of Sprint 7, Gadgetron routes to real LLM providers and includes CLI commands for tenant and API key management. Two quickstart paths are provided: one using OpenAI (requires an OpenAI API key), and one using a self-hosted vLLM instance (no external API key).

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

The server must have run at least once so that migrations create the schema. Start it briefly to apply migrations, then stop it with Ctrl-C.

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

Now create a tenant and API key using the CLI.

### Standard path (with database)

```sh
# Create a tenant. Prints the assigned UUID.
./target/release/gadgetron tenant create --name "my-team"
# Example output: tenant created id=00000000-0000-0000-0000-000000000001

# Create an API key for that tenant. Prints the raw key — save it now.
# The raw key is shown exactly once and cannot be recovered later.
./target/release/gadgetron key create --tenant-id 00000000-0000-0000-0000-000000000001
# Example output:
#   key created name=default id=00000000-0000-0000-0000-000000000002
#   key: gad_live_a3f8e1d2c4b5a6e7f8d9c0b1a2e3d4f5
```

Substitute the UUID printed by `tenant create` into the `key create` command. The raw key value (the `key:` line) is what you supply as the Bearer token in Step 6.

### No-database path (`--no-db` mode)

If you are evaluating Gadgetron without a PostgreSQL instance, create a key without a tenant:

```sh
./target/release/gadgetron key create --no-db
# Example output:
#   key: gad_live_a3f8e1d2c4b5a6e7f8d9c0b1a2e3d4f5
#   (stored in memory only — valid for this server process lifetime)
```

In `--no-db` mode the key is held in memory. It is lost when the server restarts. This mode is intended for local development and testing only.

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
