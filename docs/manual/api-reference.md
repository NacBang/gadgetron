# API Reference

All authenticated endpoints require a `Bearer` token in the `Authorization` header. See [auth.md](auth.md) for the API key format and scope requirements.

Every response from an authenticated endpoint includes an `x-request-id` header with a UUID value. Include this value when filing bug reports.

---

## Error response format

All errors return a JSON body in the following shape, matching the OpenAI error format:

```json
{
  "error": {
    "message": "Human-readable description of what went wrong and what to do.",
    "type": "authentication_error",
    "code": "tenant_not_found"
  }
}
```

- `message` — a human-readable sentence describing what happened and what the caller should do. Never contains internal implementation details.
- `type` — error category (see table below).
- `code` — machine-readable error code for programmatic handling.

---

## Error codes

All 12 error codes, their HTTP status, and the type they map to:

| `code` | HTTP status | `type` | When it occurs |
|--------|-------------|--------|----------------|
| `tenant_not_found` | 401 | `authentication_error` | Missing, malformed, or revoked API key |
| `forbidden` | 403 | `permission_error` | Valid key but wrong scope for the requested route |
| `quota_exceeded` | 429 | `quota_error` | Tenant's daily spending limit reached |
| `config_error` | 400 | `invalid_request_error` | Server configuration is invalid |
| `routing_failure` | 503 | `invalid_request_error` | No provider available to serve the request |
| `provider_error` | 502 | `api_error` | Upstream LLM provider returned an error |
| `stream_interrupted` | 502 | `api_error` | SSE stream was interrupted mid-response |
| `billing_error` | 500 | `api_error` | Internal billing calculation error |
| `download_failed` | 500 | `api_error` | Model download failed (node subsystem, Sprint 4+) |
| `hotswap_failed` | 500 | `api_error` | Model hot-swap failed (node subsystem, Sprint 4+) |
| `db_pool_timeout` | 503 | `server_error` | PostgreSQL connection pool exhausted |
| `db_row_not_found` | 404 | `server_error` | Requested database record does not exist |

Additional database sub-codes (`db_connection_failed`, `db_migration_failed`, `db_constraint`, `db_query_failed`, `db_error`) all return HTTP 500 with type `server_error`.

Node sub-code `node_invalid_mig_profile` returns HTTP 400; all other node sub-codes return HTTP 500.

---

## OpenAI-compatible endpoints

### POST /v1/chat/completions

Requires scope: `OpenAiCompat`

Submit a chat completion request. Compatible with OpenAI's `/v1/chat/completions` API. Existing OpenAI clients can point at Gadgetron without code changes by changing the base URL.

**Request body:**

```json
{
  "model": "gpt-4o-mini",
  "messages": [
    {"role": "user", "content": "Hello!"}
  ],
  "stream": false
}
```

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `model` | string | yes | Must match a model ID from one of the configured providers |
| `messages` | array | yes | Array of `{"role": "user"|"assistant"|"system", "content": "..."}` objects |
| `stream` | boolean | no | `false` (default) for a single JSON response; `true` for SSE streaming |

Additional OpenAI request fields (`temperature`, `max_tokens`, `top_p`, etc.) are forwarded to the upstream provider as-is. Gadgetron does not validate them beyond JSON parsing.

Maximum request body size: **4 MB** (enforced by the gateway before authentication).

**Non-streaming response (stream: false):**

HTTP 200

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
        "content": "Hello! How can I help you?"
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 10,
    "completion_tokens": 8,
    "total_tokens": 18
  }
}
```

**`reasoning_content` field (reasoning models — SGLang GLM-5.1 and similar):**

Some models (e.g. GLM-5.1 served via SGLang) include a `reasoning_content` field inside `message`. When the upstream provider returns this field, Gadgetron forwards it unchanged. It contains the model's chain-of-thought text that preceded the final answer.

```json
{
  "id": "chatcmpl-abc123",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "glm-4-9b-chat",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "reasoning_content": "Let me think about this step by step...",
        "content": "The answer is 42."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 12,
    "completion_tokens": 30,
    "total_tokens": 42
  }
}
```

`reasoning_content` is absent when the upstream provider does not return it. Clients should treat it as an optional field.

**Streaming response (stream: true):**

HTTP 200 with `Content-Type: text/event-stream`

Each token arrives as a `data:` SSE frame containing a JSON chunk:

```
data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"role":"assistant","content":"Hello"},"finish_reason":null}]}

data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":"!"},"finish_reason":"stop"}]}

data: [DONE]
```

The final frame is always `data: [DONE]` on success.

**Streaming error frame:**

If the stream is interrupted, an `event: error` SSE frame is emitted and the stream terminates. `data: [DONE]` is NOT sent after an error.

```
event: error
data: {"error":{"message":"The response stream was interrupted. This may indicate a provider timeout or network issue.","type":"api_error","code":"stream_interrupted"}}
```

**SSE keep-alive:** a blank comment frame (`: keep-alive`) is sent every 15 seconds to prevent proxy idle-connection timeouts.

**Example (non-streaming):**

```sh
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer gad_live_your32chartoken00000000000000" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Hi"}],"stream":false}' \
  | jq .
```

**Example (streaming):**

```sh
curl -N http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer gad_live_your32chartoken00000000000000" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Hi"}],"stream":true}'
```

---

### GET /v1/models

Requires scope: `OpenAiCompat`

Returns all models available from configured providers.

**Response:**

HTTP 200

```json
{
  "object": "list",
  "data": [
    {
      "id": "gpt-4o-mini",
      "object": "model",
      "owned_by": "openai"
    },
    {
      "id": "claude-sonnet-4-5",
      "object": "model",
      "owned_by": "anthropic"
    }
  ]
}
```

If no providers are configured, `data` is an empty array. This is not an error.

**Example:**

```sh
curl -s http://localhost:8080/v1/models \
  -H "Authorization: Bearer gad_live_your32chartoken00000000000000" \
  | jq .
```

---

## Health endpoints

These endpoints require no authentication and no scope. They are intended for load balancers, Kubernetes liveness/readiness probes, and monitoring systems.

### GET /health

Always returns HTTP 200 with `{"status":"ok"}`. Does not check PostgreSQL connectivity.

```sh
curl -s http://localhost:8080/health
# {"status":"ok"}
```

### GET /ready

Performs a PostgreSQL connection pool health check and returns the result.

- **HTTP 200** — PostgreSQL is reachable. Body: `{"status":"ready"}`
- **HTTP 503** — PostgreSQL connection pool is unhealthy (pool exhausted or host unreachable). Body: `{"status":"unavailable"}`

Use this endpoint as a Kubernetes readiness probe or load-balancer health check. Routing traffic to an instance returning 503 will result in database-backed operations (auth, quota, audit) failing.

```sh
curl -s -o /dev/null -w "%{http_code}" http://localhost:8080/ready
# 200  (PostgreSQL up)
# 503  (PostgreSQL down or pool exhausted)
```

```sh
# Full response body
curl -s http://localhost:8080/ready
# {"status":"ready"}
```

---

## Admin endpoints (not yet implemented)

The following routes are defined in the router but return HTTP 501 (not yet implemented). They require scope `Management`.

| Method | Path | What it will do (future) |
|--------|------|--------------------------|
| `GET` | `/api/v1/nodes` | List registered GPU nodes |
| `POST` | `/api/v1/models/deploy` | Deploy a model to a node |
| `DELETE` | `/api/v1/models/{id}` | Undeploy a model |
| `GET` | `/api/v1/models/status` | Get model deployment status |
| `GET` | `/api/v1/usage` | Tenant usage report |
| `GET` | `/api/v1/costs` | Tenant cost report |

Sending a request to any of these endpoints with a valid `Management`-scoped key returns HTTP 501:

```sh
curl -s http://localhost:8080/api/v1/nodes \
  -H "Authorization: Bearer gad_live_your_management_key_here"
# HTTP 501 (no body)
```

Sending with an `OpenAiCompat`-scoped key returns HTTP 403 (scope guard fires before the stub handler).
