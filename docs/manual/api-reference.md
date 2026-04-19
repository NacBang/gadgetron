# API Reference

All authenticated endpoints require a `Bearer` token in the `Authorization` header. See [auth.md](auth.md) for the API key format and scope requirements.

Every response from an authenticated endpoint includes an `x-request-id` header with a UUID value. Include this value when filing bug reports.

**Python (openai SDK):**

```python
from openai import OpenAI

client = OpenAI(base_url="http://localhost:8080/v1", api_key="gad_live_...")
resp = client.chat.completions.create(
    model="gpt-4o-mini",
    messages=[{"role": "user", "content": "Hello!"}],
)
print(resp.choices[0].message.content)
```

Replace `api_key` with the key you created in [quickstart.md](quickstart.md) Step 4. Replace `model` with a model ID returned by `GET /v1/models`.

---

## Error response format

Errors that route through `GadgetronError::IntoResponse` return a JSON body in the following shape, matching the OpenAI error format. Two exceptions: (1) the admin-stub endpoints (`/api/v1/nodes`, `/api/v1/models/deploy`, etc.) return bare HTTP 501 with **no body** (see §Admin endpoints below); (2) HTTP 204 from `/favicon.ico` also has no body. The `map_response(openai_shape_413)` outermost Tower layer additionally rewrites the raw plain-text 413 from `RequestBodyLimitLayer` into the JSON shape below.

```json
{
  "error": {
    "message": "Human-readable description of what went wrong and what to do.",
    "type": "authentication_error",
    "code": "invalid_api_key"
  }
}
```

- `message` — a human-readable sentence describing what happened and what the caller should do. Never contains internal implementation details.
- `type` — error category (see table below).
- `code` — machine-readable error code for programmatic handling.

---

## Error codes

Foundational error codes emitted by the request pipeline — auth, scoping, routing, database, and knowledge failures. Penny / Wiki / workbench error codes are listed inline under their respective endpoint sections (`penny_*` / `wiki_*` in §Penny / Wiki error bodies below; `workbench_*` in each §Workbench endpoint's Errors row).

| `code` | HTTP status | `type` | When it occurs |
|--------|-------------|--------|----------------|
| `invalid_api_key` | 401 | `authentication_error` | Missing, malformed, or revoked API key |
| `forbidden` | 403 | `permission_error` | Valid key but wrong scope for the requested route |
| `request_too_large` | 413 | `invalid_request_error` | Request body exceeds the 4 MiB limit — see [Troubleshooting — HTTP 413](troubleshooting.md#http-413--request-body-too-large) |
| `quota_exceeded` | 429 | `quota_error` | Tenant's daily spending limit reached |
| `config_error` | 400 | `invalid_request_error` | Server configuration is invalid |
| `routing_failure` | 503 | `server_error` | No provider available to serve the request |
| `provider_error` | 502 | `api_error` | Upstream LLM provider returned an error |
| `stream_interrupted` | 502 | `api_error` | SSE stream was interrupted mid-response |
| `billing_error` | 500 | `api_error` | Internal billing calculation error |
| `download_failed` | 500 | `api_error` | Model download failed (node subsystem, Sprint 4+) |
| `hotswap_failed` | 500 | `api_error` | Model hot-swap failed (node subsystem, Sprint 4+) |
| `db_pool_timeout` | 503 | `server_error` | PostgreSQL connection pool exhausted |
| `db_row_not_found` | 404 | `server_error` | Requested database record does not exist |
| `knowledge_backend_not_registered` | 500 | `server_error` | No knowledge plug is registered for the requested backend (operator misconfiguration) |
| `knowledge_backend_unavailable` | 503 | `server_error` | Registered knowledge plug is unreachable (pgvector RPC failure, connection dropped) |
| `knowledge_document_not_found` | 404 | `invalid_request_error` | Wiki path does not exist — returned by get, delete, and rename gadgets |
| `knowledge_invalid_query` | 400 | `invalid_request_error` | Query input to a wiki search gadget is malformed or empty |
| `knowledge_derived_apply_failed` | 500 | `server_error` | Derived index update failed under `await_derived` write consistency mode |
| `bundle_manifest_error` | 400 | `invalid_request_error` | Bundle manifest (`bundle.toml`) failed to parse or validate |
| `bundle_install_failed` | 500 | `server_error` | Bundle install step failed (unpack, copy, register) |
| `bundle_plug_id_invalid` | 400 | `invalid_request_error` | Plug id referenced by a bundle is malformed or collides with a reserved id |
| `bundle_home_unresolvable` | 500 | `server_error` | `GADGETRON_BUNDLES_HOME` / `GADGETRON_DATA_DIR` could not be resolved to a usable directory |

Additional database sub-codes (`db_connection_failed`, `db_migration_failed`, `db_constraint`, `db_query_failed`, `db_error`) all return HTTP 500 with type `server_error`.

Node sub-code `node_invalid_mig_profile` returns HTTP 400; all other node sub-codes return HTTP 500. Node variants not in the specific sub-code list surface as `node_error` (500, `server_error`) — the catch-all from `error_code()` in `crates/gadgetron-core/src/error.rs`.

### Knowledge error bodies (examples)

All `knowledge_*` errors use the same OpenAI-shaped envelope shown above (`{"error": {"message", "type", "code"}}`). The `message` strings below are emitted verbatim by `GadgetronError::error_message()` in `crates/gadgetron-core/src/error.rs` — the `{plug}`, `{path}`, `{reason}` slots are interpolated at runtime from the failing `KnowledgeService` call. Clients can match either on `code` (stable) or on `type` (OpenAI taxonomy — `invalid_request_error` is user-recoverable; `server_error` is operator-recoverable).

`knowledge_backend_not_registered` (HTTP 500, `server_error`) — operator misconfiguration, returned during startup or first use when `[knowledge]` references a plug id that no enabled bundle provides:

```json
{
  "error": {
    "message": "Knowledge backend \"pgvector\" is referenced in configuration but was not registered at startup. Check `[knowledge]` canonical_store / search_plugs / relation_plugs against the enabled bundles.",
    "type": "server_error",
    "code": "knowledge_backend_not_registered"
  }
}
```

`knowledge_backend_unavailable` (HTTP 503, `server_error`) — plug is registered but its backend is currently unreachable (pgvector pool exhausted, external daemon down). 503 carries the usual RFC 9110 §15.6.4 semantics: transient, callers may retry after a backoff; operators should inspect the backend's own health channel. Upstream stack traces are deliberately **not** echoed (Phase 2 STRIDE row 4):

```json
{
  "error": {
    "message": "Knowledge backend \"pgvector\" is currently unavailable. Check the backend's health / connectivity (pgvector pool, external runtime) and retry.",
    "type": "server_error",
    "code": "knowledge_backend_unavailable"
  }
}
```

`knowledge_document_not_found` (HTTP 404, `invalid_request_error`) — wiki path does not exist. Returned by `wiki.get`, `wiki.delete`, `wiki.rename`, and the workbench view loader when a referenced page is absent:

```json
{
  "error": {
    "message": "Knowledge document not found: notes/missing-page. Use `wiki.list` or `wiki.search` to discover existing paths.",
    "type": "invalid_request_error",
    "code": "knowledge_document_not_found"
  }
}
```

`knowledge_invalid_query` (HTTP 400, `invalid_request_error`) — query input to a wiki search gadget is malformed or empty. The `reason` slot carries the validator's explanation (e.g. `"query must not be empty"`, `"limit must be between 1 and 100"`):

```json
{
  "error": {
    "message": "Knowledge query is invalid: query must not be empty.",
    "type": "invalid_request_error",
    "code": "knowledge_invalid_query"
  }
}
```

`knowledge_derived_apply_failed` (HTTP 500, `server_error`) — emitted under `write_consistency = "await_derived"` when the canonical store write succeeded but a derived index/relation plug failed to re-project. The canonical wiki page is intact; rerun `gadgetron reindex` to reconcile:

```json
{
  "error": {
    "message": "Derived index / relation plug \"pgvector-embeddings\" failed to apply a write. The canonical store succeeded; rerun `gadgetron reindex` to recover.",
    "type": "server_error",
    "code": "knowledge_derived_apply_failed"
  }
}
```

All five bodies are emitted with the standard `x-request-id` header; include that UUID in any bug report. Streaming callers (`/v1/chat/completions` with `stream: true`) that hit a terminal error receive the envelope inside an `event: error` SSE frame and the stream terminates **without** a trailing `data: [DONE]` — see the [Streaming error frame](#post-v1chatcompletions) example under `POST /v1/chat/completions` below. E2E Gate 9b formally certifies this contract. Every streaming request also produces two correlated AuditEntry rows (dispatch + amendment); on the error path the amendment's `status` is `"error"` rather than `"ok"` (see [troubleshooting.md §Streaming requests](troubleshooting.md#streaming-requests-stream-true)).

### Penny / Wiki error bodies (examples)

These bodies surface from the Penny subprocess boundary (`gadgetron-penny`) and from the wiki MCP gadget family (`wiki.get`, `wiki.put`, `wiki.delete`, `wiki.rename`). They use the same OpenAI-shaped envelope as every other error — the `message` strings below are emitted verbatim by `GadgetronError::error_message()` in `crates/gadgetron-core/src/error.rs`, with `{path}`, `{bytes}`, `{limit}`, `{pattern}`, `{seconds}`, `{name}`, `{reason}`, `{tool}`, `{remaining}`, `{conversation_id}` slots interpolated at runtime. Penny subprocess variants never leak raw subprocess stderr (enforced by the `penny_agent_error_message_does_not_contain_stderr` test in core). Streaming callers receive the same envelope inside an `event: error` SSE frame; the stream terminates without `data: [DONE]`.

`penny_not_installed` (HTTP 503, `server_error`) — the `claude` CLI is not on PATH. Operator fix: install Claude Code and run `claude login` on the server (see [penny.md](penny.md)):

```json
{
  "error": {
    "message": "The Penny assistant is not available. The Claude Code CLI (`claude`) was not found on the server. Contact your administrator to install Claude Code and run `claude login`.",
    "type": "server_error",
    "code": "penny_not_installed"
  }
}
```

`penny_spawn_failed` (HTTP 503, `server_error`) — subprocess could not be spawned (permissions, ulimit, SELinux). Investigate via `RUST_LOG=gadgetron_penny=debug`:

```json
{
  "error": {
    "message": "The Penny assistant is not available. The server could not start the Claude Code process. Run `gadgetron serve` with `RUST_LOG=gadgetron_penny=debug` for spawn diagnostics, or check `journalctl -u gadgetron` for spawn errors.",
    "type": "server_error",
    "code": "penny_spawn_failed"
  }
}
```

`penny_agent_error` (HTTP 500, `server_error`) — subprocess exited non-zero mid-stream. The stderr tail is redacted server-side before logging and is **never** echoed in this body:

```json
{
  "error": {
    "message": "The Penny assistant encountered an error and stopped. The assistant process exited unexpectedly. Try again; if the problem persists, contact your administrator.",
    "type": "server_error",
    "code": "penny_agent_error"
  }
}
```

`penny_timeout` (HTTP 504, `server_error`) — wallclock exceeded `penny.request_timeout_secs`. 504 carries RFC 9110 §15.5.5 semantics: the upstream (Penny subprocess) did not respond in time; the caller may retry with a simpler request or raise the limit:

```json
{
  "error": {
    "message": "The Penny assistant did not respond in time (limit: 120s). Your request may have been too complex. Try a shorter or simpler request.",
    "type": "server_error",
    "code": "penny_timeout"
  }
}
```

`penny_tool_unknown` (HTTP 500, `server_error`) — agent called a Gadget name the live registry does not know. Usually a cached-manifest/registry mismatch; restart `gadgetron serve` to refresh:

```json
{
  "error": {
    "message": "The agent requested tool \"wiki.archive\", which is not registered on this server. This usually means a version mismatch between the agent's cached tool manifest and the live MCP registry. Restart `gadgetron serve` to refresh the manifest.",
    "type": "server_error",
    "code": "penny_tool_unknown"
  }
}
```

`penny_tool_denied` (HTTP 403, `permission_error`) — Gadget call blocked by policy (never-mode subcategory, feature gate, reserved namespace). Operator-facing `reason` string from the MCP server; safe to surface:

```json
{
  "error": {
    "message": "A tool call was denied by policy: destructive tier disabled in [agent.tools.destructive]. Check your `[agent.tools.*]` configuration in `gadgetron.toml`.",
    "type": "permission_error",
    "code": "penny_tool_denied"
  }
}
```

`penny_tool_rate_limited` (HTTP 429, `quota_error`) — Destructive-tier tool exceeded `max_per_hour`. Wait or raise the limit:

```json
{
  "error": {
    "message": "Tool \"wiki.delete\" is rate-limited (0/10 calls remaining this hour). Wait and retry, or increase `[agent.tools.destructive].max_per_hour` in `gadgetron.toml`.",
    "type": "quota_error",
    "code": "penny_tool_rate_limited"
  }
}
```

`penny_tool_approval_timeout` (HTTP 504, `server_error`) — Penny-side approval path timed out waiting for a decision. This error code targets the Penny-stream approval surface (MCP grandchild approval bridge, SEC-MCP-B1) which is still deferred beyond ISSUE 3. The direct-action workbench approval flow (`/api/v1/web/workbench/actions/{id}` → `pending_approval` → `/approvals/:id/approve`) shipped with ISSUE 3 / v0.2.6 and surfaces resolved outcomes through the dedicated `/approvals/:id` endpoints below, not through this error:

```json
{
  "error": {
    "message": "A tool call required user approval but none arrived within 300 seconds. (The Penny-side approval bridge is reserved for a later ROADMAP ISSUE; this code surfaces only on forward-compat paths.)",
    "type": "server_error",
    "code": "penny_tool_approval_timeout"
  }
}
```

`penny_tool_invalid_args` (HTTP 400, `invalid_request_error`) — agent passed args that failed the tool's input schema. Agent-side bug; rephrase the user request:

```json
{
  "error": {
    "message": "The agent passed invalid arguments to a tool: missing required field `path`. This is an agent-side bug; try rephrasing your request.",
    "type": "invalid_request_error",
    "code": "penny_tool_invalid_args"
  }
}
```

`penny_tool_execution` (HTTP 500, `server_error`) — tool dispatch succeeded but the provider-side execution failed (SearXNG HTTP error, wiki write I/O fault, etc.). Check server logs for the underlying cause:

```json
{
  "error": {
    "message": "A tool failed to execute: SearXNG backend returned HTTP 502. Check server logs for details.",
    "type": "server_error",
    "code": "penny_tool_execution"
  }
}
```

`penny_session_not_found` (HTTP 404, `server_error`) — caller sent `conversation_id` but `SessionStore` has no entry (expired / evicted / cold start). Retry without `conversation_id` or with a fresh id:

```json
{
  "error": {
    "message": "Conversation \"conv_01HP...\" is not known to this server. The conversation may have expired or been evicted from the session store. Start a new conversation without a conversation_id, or with a fresh id.",
    "type": "server_error",
    "code": "penny_session_not_found"
  }
}
```

`penny_session_concurrent` (HTTP 429, `server_error`) — two concurrent requests for the same `conversation_id`; the second lost the per-session mutex race. Retry after the first turn settles:

```json
{
  "error": {
    "message": "Conversation \"conv_01HP...\" is already serving another request. Wait for the current turn to finish, then retry.",
    "type": "server_error",
    "code": "penny_session_concurrent"
  }
}
```

`penny_session_corrupted` (HTTP 500, `server_error`) — Claude Code reported the session UUID as unknown or the jsonl file is unreadable. The store entry is discarded server-side; the next retry falls through the first-turn branch and creates a fresh session:

```json
{
  "error": {
    "message": "Conversation \"conv_01HP...\" session state is unreadable. The session has been discarded; retry with the same conversation_id to start a fresh session.",
    "type": "server_error",
    "code": "penny_session_corrupted"
  }
}
```

`wiki_invalid_path` (HTTP 400, `invalid_request_error`) — path traversal rejected by `wiki::fs::resolve_path`. `..`, absolute paths, or control characters all fail here:

```json
{
  "error": {
    "message": "The requested wiki page path is invalid. Page paths must not contain `..`, absolute paths, or special characters.",
    "type": "invalid_request_error",
    "code": "wiki_invalid_path"
  }
}
```

`wiki_page_too_large` (HTTP 413, `invalid_request_error`) — body exceeds `wiki_max_page_bytes`. 413 carries RFC 9110 §15.5.14 semantics (content too large — a request-level constraint, not a server bug). Split the content into multiple smaller pages:

```json
{
  "error": {
    "message": "Page too large: 2097152 bytes exceeds the 1048576-byte limit. Split the content into multiple smaller pages.",
    "type": "invalid_request_error",
    "code": "wiki_page_too_large"
  }
}
```

`wiki_credential_blocked` (HTTP 422, `invalid_request_error`) — content matched a BLOCK credential pattern (PEM / AKIA / GCP / etc. per `01-knowledge-layer.md §4.8`). 422 carries RFC 9110 §15.5.21 semantics (syntactically valid but semantically rejected). Remove the secret and retry:

```json
{
  "error": {
    "message": "Credential detected in content (pattern: AKIA_ACCESS_KEY). Wiki writes must not contain unambiguous secrets. Remove the credential and retry.",
    "type": "invalid_request_error",
    "code": "wiki_credential_blocked"
  }
}
```

`wiki_git_corrupted` (HTTP 503, `server_error`) — git repo is in an inconsistent state (locked index, detached HEAD, missing objects). Operator-only fix; inspect the wiki directory manually:

```json
{
  "error": {
    "message": "The wiki git repository is in an inconsistent state. Run `git status` in the wiki directory and resolve manually.",
    "type": "server_error",
    "code": "wiki_git_corrupted"
  }
}
```

`wiki_conflict` (HTTP 409, `server_error`) — merge conflict during auto-commit; another writer mutated the same path concurrently. 409 carries RFC 9110 §15.5.10 semantics (resource state conflict). Resolve manually then retry:

```json
{
  "error": {
    "message": "A wiki page could not be saved because it was modified by another process (path: notes/release-playbook.md). Resolve the git conflict in the wiki directory, then retry.",
    "type": "server_error",
    "code": "wiki_conflict"
  }
}
```

`wiki_page_not_found` (HTTP 404, `invalid_request_error`) — requested page does not exist on `wiki.get` / `wiki.delete` / `wiki.rename`. Use `wiki.list` or `wiki.search` to discover existing paths. (The `KnowledgeService` boundary normalizes this to `knowledge_document_not_found` with the same 404; `wiki_page_not_found` surfaces when a tool path bypasses the service wrapper.):

```json
{
  "error": {
    "message": "Wiki page not found: notes/missing-page.md. Check the page name; use `wiki.list` or `wiki.search` to find existing pages.",
    "type": "invalid_request_error",
    "code": "wiki_page_not_found"
  }
}
```

All bodies are emitted with the standard `x-request-id` header; include that UUID in any bug report. Streaming requests that terminate on any of these errors follow the same SSE `event: error` convention documented for knowledge errors above (no trailing `data: [DONE]`), and every streaming request produces the same dispatch + amendment AuditEntry pair with `status = "error"`.

---

## OpenAI-compatible endpoints

### POST /v1/chat/completions

Requires scope: `OpenAiCompat`

Submit a chat completion request to Gadgetron's `/v1/chat/completions` endpoint. The Python `openai` SDK is the canonical tested client — point it at Gadgetron by setting `base_url="http://<host>:<port>/v1"`. E2E Gate 9c exercises this path end-to-end; the SDK examples below are directly derived from that gate. No other SDK is formally verified here; use others at your own risk against the raw wire-shape contract below.

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

Gadgetron deserializes request bodies into a closed Rust `ChatRequest` struct (`crates/gadgetron-core/src/provider.rs`). Only fields explicitly modeled there — `model`, `messages`, `stream`, and the per-provider typed parameters — are carried through to the upstream provider. Unknown JSON fields are **silently dropped** at deserialization time. If you need a field that Gadgetron doesn't yet model (e.g. `seed`, `response_format`, tool-calling params), the doc for that field's support is gated on the gateway adding it to `ChatRequest` — sending it today is a no-op.

Maximum request body size: **4 MiB** (enforced by the gateway before authentication). Oversized requests return HTTP 413 with `error.code = "request_too_large"` and an OpenAI-shaped JSON body.

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

**Gadgetron wire-shape contract.** The table below is the non-streaming wire-shape contract formally asserted by E2E Gate 8 (`scripts/e2e-harness/run.sh:985-997`). The Python `openai` SDK's `ChatCompletion` model defines matching typed fields (`id`, `object`, `created`, `model`, `choices`, `usage`), but strict response validation is **disabled by default** in the SDK (`_strict_response_validation=False`), so a type mismatch will typically surface as a silent coercion rather than a raised exception. Treat Gate 8 as the load-bearing contract; SDK consumers get downstream-of-the-gate safety, not a guaranteed pre-flight `ValidationError`.

| Field | Type | Gate 8 assertion | Purpose |
|-------|------|-----------------|---------|
| `id` | non-empty string; shape depends on provider | `(.id | type == "string" and startswith("chatcmpl-"))` — Gate 8 pins the `chatcmpl-` prefix because the mock + OpenAI-compatible upstreams (OpenAI, vLLM, SGLang, Ollama) emit it. Non-OpenAI-shape adapters emit different prefixes: Gemini synthesizes `gemini-<uuid>` (`gadgetron-provider/src/gemini.rs`), Anthropic forwards its own upstream id verbatim (`gadgetron-provider/src/anthropic.rs`). Client code that pins on `chatcmpl-` will break on those providers. | Deduplication + log correlation on the client side. |
| `object` | `"chat.completion"` literal | `.object == "chat.completion"` | Distinguishes the non-streaming response shape from `chat.completion.chunk` streaming frames. |
| `created` | integer (unix seconds) | not in Gate 8 but emitted | Response time, useful for client-side latency breakdowns. |
| `model` | non-empty string | `(.model | type == "string" and length > 0)` | Identifies which provider-routed model served the request. For Gadgetron this may be the upstream model ID (e.g. `gpt-4o-mini`), not the `model` field the client sent. |
| `choices` | array, length ≥ 1 | `(.choices | length >= 1)` | Each choice carries `index`, `message`, `finish_reason`. |
| `choices[].finish_reason` | string (`stop` / `length` / `tool_calls` / etc.) | `(.choices[0].finish_reason | type == "string")` | Must be a string on terminal response. Streaming chunks emit `null` until the final chunk. |
| `usage.total_tokens` | number | `(.usage.total_tokens | type == "number")` | `prompt_tokens + completion_tokens`. All three are emitted for provider-returned usage; Gadgetron does not re-compute or estimate. |

Gadgetron's `ChatResponse` struct is closed and does not carry a generic passthrough bag — fields the OpenAI spec defines that Gadgetron does not model (e.g. `system_fingerprint`, `service_tier`, logprobs scaffolding) are **dropped** by the adapter layer, even if the upstream provider sends them. If you depend on such a field, filing an issue against `gadgetron-core` is the path; there is no config toggle that re-enables the passthrough today.

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

**Reasoning models (e.g. GLM-5.1 via SGLang):** `delta` may also carry a `reasoning_content` field alongside `content`. Example frame:

```
data: {"id":"chatcmpl-xyz","object":"chat.completion.chunk","created":1700000000,"model":"glm-4-9b-chat","choices":[{"index":0,"delta":{"reasoning_content":"step 1: parse the question"},"finish_reason":null}]}
```

`reasoning_content` is absent for providers that do not emit chain-of-thought tokens; treat it as an optional field exactly like the non-streaming response body.

**Streaming chunk-shape contract.** Every non-`[DONE]` `data:` frame is a JSON object with at minimum:

| Field | Value | Gate 9 assertion |
|-------|-------|-----------------|
| `id` | non-empty string. For OpenAI-shape upstreams (OpenAI, vLLM, SGLang, Ollama, the harness mock) the same `id` is reused across every chunk in one stream — the Python `openai` SDK's `ChatCompletionChunk.id` is documented as "A unique identifier for the chat completion. Each chunk has the same ID." The `GeminiProvider::chat_stream` adapter violates this: it synthesizes a fresh `gemini-chunk-<uuid>` per chunk (`gadgetron-provider/src/gemini.rs`), so client code that groups chunks by id will see one distinct id per frame for Gemini streams. Gate 9's cross-chunk consistency assertion runs against the mock (OpenAI-shape) and is therefore silent on Gemini. | Shape: `(.id | type == "string" and length > 0)`. Cross-chunk consistency: assertion `jq -r '.id'` over all frames returns exactly one unique value (OpenAI-shape path only). |
| `object` | `"chat.completion.chunk"` literal (distinguishes streaming frames from the non-streaming shape) | `.object == "chat.completion.chunk"` |
| `choices` | non-empty array; each entry has `index`, `delta`, `finish_reason` | `(.choices | length >= 1)` |
| `choices[].delta` | object — the **incremental** update for this chunk. First chunk typically carries `role: "assistant"`; subsequent chunks carry only `content` (or `reasoning_content`). | — |
| `choices[].finish_reason` | `null` on all chunks except the final one; final chunk emits `"stop"` / `"length"` / etc. | — |

E2E Gate 9 asserts the per-chunk shape on the first non-`[DONE]` frame (`scripts/e2e-harness/run.sh:1063-1065`); a companion gate asserts that every chunk in the stream shares the same `.id`. This matches the Python `openai` SDK source, which types `object` as the literal `chat.completion.chunk` and documents `ChatCompletionChunk.id` as stable across chunks. A regression that flipped `object` to `chat.completion` (non-streaming shape) on a streaming response, or rotated ids mid-stream, would break the SDK's streaming iterator and Gate 9 in lockstep.

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

**Python OpenAI SDK — non-streaming.** The intro example at the top of this page covers the minimum; the expanded form below shows the fields the SDK's `ChatCompletion` model types. Note that the SDK defaults to `_strict_response_validation=False`, so a wrong-type field would typically coerce rather than raise — the guardrail here is E2E Gate 8, not the SDK. Gate 9c's harness scenario (`scripts/e2e-harness/sdk-client.py:45-68`) exercises the same code path and also asserts harness-specific fields (exact mock-content substring, exact token counts) that don't generalize beyond the mock.

```python
from openai import OpenAI

client = OpenAI(base_url="http://localhost:8080/v1", api_key="gad_live_...")

resp = client.chat.completions.create(
    model="gpt-4o-mini",
    messages=[{"role": "user", "content": "Hello"}],
    stream=False,
)

# Wire-shape fields Gate 8 asserts (the SDK exposes these as typed
# attributes but does not strict-validate by default — Gate 8 is the
# load-bearing contract, the asserts here are defensive client-side):
assert resp.id.startswith("chatcmpl-")
assert resp.object == "chat.completion"
assert resp.model  # non-empty string
assert resp.choices[0].finish_reason  # non-None on terminal response
assert isinstance(resp.usage.total_tokens, int)

print(resp.choices[0].message.content)
```

**Python OpenAI SDK — streaming.** The same SDK iterates over SSE frames transparently; `stream=True` returns an iterator of `ChatCompletionChunk` objects. Gate 9c's streaming scenario (`sdk-client.py:72-95`) formally asserts that every chunk in a single stream shares the same `id`, at least one chunk carries `finish_reason`, and the accumulated `delta.content` is non-empty:

```python
from openai import OpenAI

client = OpenAI(base_url="http://localhost:8080/v1", api_key="gad_live_...")

accum = ""
chunk_ids: set[str] = set()
finished = False

stream = client.chat.completions.create(
    model="gpt-4o-mini",
    messages=[{"role": "user", "content": "Hi"}],
    stream=True,
)
for chunk in stream:
    chunk_ids.add(chunk.id)
    if chunk.choices and chunk.choices[0].delta.content:
        accum += chunk.choices[0].delta.content
    if chunk.choices and chunk.choices[0].finish_reason:
        finished = True

assert finished, "no finish_reason observed — stream ended abnormally"
assert len(chunk_ids) == 1, f"chunks must share one id, got {chunk_ids}"
assert accum, "no content accumulated"
print(accum)
```

The `len(chunk_ids) == 1` assertion matches the OpenAI Python SDK's own chunk contract — the installed SDK documents `ChatCompletionChunk.id` as "A unique identifier for the chat completion. Each chunk has the same ID." A regression that rotated chunk ids mid-stream would break this example and E2E Gate 9c in lockstep.

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

### GET /favicon.ico

Returns HTTP 204 No Content with an empty body. Only mounted when the binary is built with the default `web-ui` feature (the headless build does not register this route). Required because browsers unconditionally request `/favicon.ico` when rendering `/web`; without it, every page load emits a 404 into the gateway log.

No authentication, no scope.

### GET /web/

Permanent redirect (HTTP 308) to the `/web` base path (`gadgetron_web::BASE_PATH`). Present because some browser flows follow a trailing-slash-insensitive URL convention. Only mounted when built with the `web-ui` feature.

```sh
# Full response body
curl -s http://localhost:8080/ready
# {"status":"ready"}
```

---

## Workbench endpoints (Phase 2A)

The workbench projection API surfaces activity, knowledge plug health, and registered view/action descriptors to the Web UI shell. All endpoints require `OpenAiCompat` scope — the same scope as `/v1/` routes.

All eight routes are always mounted on trunk; the CLI's `build_workbench(knowledge_service, candidate_coordinator, penny_registry)` helper at `crates/gadgetron-cli/src/main.rs:1256-1298` returns `Some(...)` even when all three arguments are `None` (degraded mode: bootstrap + catalog still reachable; gadget dispatch returns empty payload; activity capture no-ops).

**What is real on trunk today:**
- `GET /bootstrap` — returns live `gateway_version` + `knowledge-status` booleans + registered descriptor catalog.
- `GET /views`, `GET /actions` — return the four descriptors in `seed_p2b` (see `GET /actions` below).
- `POST /actions/{action_id}` — dispatches to the registered Gadget via `Arc<dyn GadgetDispatcher>` (`crates/gadgetron-core/src/agent/tools.rs:45-59`). When the gateway has a Penny `GadgetRegistry` wired, this reaches the same `wiki.search` / `wiki.list` / `wiki.get` / `wiki.write` gadgets Penny uses. The response's `result.payload` carries the raw `GadgetResult.content` — real wiki data, not a stub.

**What is stubbed on trunk today:**
- `activity.entries` — always `[]` (see §/activity).
- `request_evidence` — always 404 (see §/evidence).
- `refresh_view_ids` — always `[]` on every action response (see §POST /actions).
- `audit_event_id` — always `null`. Direct-action dispatch bypasses Penny's `GadgetAuditEventSink` by design (tracked as `TODO(audit-direct-action)` in `GadgetDispatcher`'s doc comment).

The `config_error` 400 path exists in `require_workbench(&state)` for the case where `state.workbench` is `None`, but no production build path on trunk produces that state — it is a defensive guard for test harness configurations.

---

### GET /api/v1/web/workbench/bootstrap

Gateway version, default model, active plug health, and knowledge plane readiness. Called by the Web UI shell on mount.

**Server-side injection into chat completions.** Injection runs on `POST /v1/chat/completions` only when BOTH gates are satisfied (`crates/gadgetron-gateway/src/handlers.rs:70-108`): (a) `[agent.shared_context].enabled` is `true` (the default), AND (b) `state.penny_assembler` is `Some(...)` — which requires a Penny-capable build with the knowledge layer wired. When injection runs, the gateway wraps the same bootstrap payload (truncated to `[agent.shared_context].digest_summary_chars` per `gadgetron.toml`) inside `<gadgetron_shared_context>...</gadgetron_shared_context>` tags and prepends it to the request's system message surface via `inject_shared_context_block` (`handlers.rs:411-428`). When either gate fails, the chat request proceeds with the caller's original `messages` unchanged. Build failures (timeout, knowledge unavailable) also degrade gracefully without failing the chat request — a WARN is emitted on `penny_shared_context` and the original messages pass through.

Two injection modes, selected by the shape of the caller's first message:

| First message shape | Mode | Emitted tracing line |
|---------------------|------|----------------------|
| `role: "system"` with `content: <string>` | `prepend_to_system` — block is inserted at the start of the existing text followed by `\n\n` then the original text. One combined system message; index 0 preserved. | `penny_shared_context.inject: shared context block injected injection_mode=prepend_to_system` |
| Anything else (empty messages array / first message not `system` / system message with multi-part `content`) | `insert_new_system` — a brand-new `Message::system(block)` is inserted at index 0, pushing every existing message down. | `penny_shared_context.inject: shared context block injected injection_mode=insert_new_system` |

In both modes the block lives in the **first message** of what the provider sees, always with `role: "system"`, always opening with the literal `<gadgetron_shared_context>` tag. E2E Gate 10 asserts this contract on the mock provider log.

Observability: grep the gateway log for `penny_shared_context.inject:` to see which mode fired for a given `request_id`; the `rendered_bytes` field on the same line tells you the post-truncation size of the block. Disable injection entirely by setting `[agent.shared_context].enabled = false` — use only for emergency rollback (see [configuration.md §\[agent.shared_context\]](configuration.md#agentshared_context)).

**Auth:** `OpenAiCompat`

**Response:**

```json
{
  "gateway_version": "0.2.5",
  "default_model": "penny",
  "active_plugs": [
    { "id": "wiki-canonical", "role": "canonical", "healthy": true, "note": null }
  ],
  "degraded_reasons": [],
  "knowledge": {
    "canonical_ready": true,
    "search_ready": false,
    "relation_ready": false,
    "last_ingest_at": "2026-04-19T12:00:00Z"
  }
}
```

**Field contract.** Sourced from the Rust structs `WorkbenchBootstrapResponse`, `PlugHealth`, and `WorkbenchKnowledgeSummary` at `crates/gadgetron-core/src/workbench/mod.rs:23-50`. E2E Gate 7 asserts the inner shape on every healthy boot (`scripts/e2e-harness/run.sh`).

| Field | Type | Notes |
|-------|------|-------|
| `gateway_version` | non-empty string | Cargo workspace version, e.g. `"0.2.5"`. |
| `default_model` | string or `null` | The model ID the Web UI shell should pre-select. `null` when no default is configured; consumers receive either a string or `null`. |
| `active_plugs` | array of `PlugHealth`, length ≥ 1 on a healthy boot | Each entry has `id`, `role`, `healthy`, `note`. |
| `active_plugs[].id` | non-empty string | Plug identifier — stable across restarts. |
| `active_plugs[].role` | `"canonical"` \| `"search"` \| `"relation"` \| `"extractor"` | Which port the plug fills. The /web UI groups plugs by role. |
| `active_plugs[].healthy` | boolean | Gate 7's shape check asserts this is strictly boolean, not coerced from string or int. |
| `active_plugs[].note` | string or `null` | Free-text human-readable note when the plug is degraded, e.g. `"stale index >30s"`. |
| `degraded_reasons` | array of strings | Non-empty when the bootstrap ran but one or more subsystems are unhealthy. Each string is operator-facing. |
| `knowledge.canonical_ready` | boolean | Canonical wiki store is accepting reads + writes. |
| `knowledge.search_ready` | boolean | Keyword / embedding index is queryable. False when an index rebuild is in flight. |
| `knowledge.relation_ready` | boolean | Relation plug is queryable (P2C+). |
| `knowledge.last_ingest_at` | RFC 3339 timestamp or `null` | Null before the first ingest after startup. |

The three `*_ready` booleans are the observable contract gate 7 pins down (`(.knowledge.canonical_ready | type == "boolean")` etc); a regression that replaces `false` with `"false"` (stringified) would break the /web UI's knowledge-status indicator silently. The shell renders the knowledge panel based on `canonical_ready` alone; `search_ready` / `relation_ready` degrade individual UI features without hiding the panel.

---

### GET /api/v1/web/workbench/activity

Recent workbench activity feed: Penny turns, direct actions, system events. **On trunk today this is a stub** — `InProcessWorkbenchProjection::activity` at `crates/gadgetron-gateway/src/web/projection.rs:101-106` always returns `{"entries": [], "is_truncated": false}` regardless of `limit` or real traffic. The response shape documented below is the contract future activity-source wiring (PSL-1) will populate; build client code against the shape, but don't expect non-empty data until that ships. E2E Gate 7c asserts the empty-state shape.

**Auth:** `OpenAiCompat`

**Query parameters:**

| Name | Type | Default | Range | Description |
|------|------|---------|-------|-------------|
| `limit` | integer | `50` | `[1, 100]` | Maximum entries to return |

**Response:**

```json
{
  "entries": [
    {
      "event_id": "uuid",
      "at": "2026-04-19T12:00:00Z",
      "origin": "penny",
      "kind": "chat_turn",
      "title": "Summarise last incident",
      "request_id": "uuid",
      "summary": null
    }
  ],
  "is_truncated": true
}
```

`origin`: `"penny"` | `"user_direct"` | `"system"`. `kind`: `"chat_turn"` | `"direct_action"` | `"system_event"`.

---

### GET /api/v1/web/workbench/requests/{request_id}/evidence

Per-request evidence: tool traces, knowledge citations, and knowledge candidates created during that request. **On trunk today this is a stub** — `InProcessWorkbenchProjection::request_evidence` at `crates/gadgetron-gateway/src/web/projection.rs:109-115` unconditionally returns `RequestNotFound` (HTTP 404 `workbench_request_not_found`) for every `request_id`. The 200 body example below is the contract future evidence-source wiring (PSL-1) will populate. E2E Gate 7m relies on the uniform 404 behavior.

**Auth:** `OpenAiCompat`

**Path parameters:** `request_id` — UUID of the gateway request.

**Response (future / post-PSL-1 — today always 404):**

```json
{
  "request_id": "uuid",
  "tool_traces": [
    { "gadget_name": "wiki.search", "args_digest": "a3f2...", "outcome": "success", "latency_ms": 42 }
  ],
  "citations": [
    { "label": "^1", "page_name": "ops/incidents/2026-04-19", "anchor": null }
  ],
  "candidates": []
}
```

`tool_traces[].outcome`: `"success"` | `"denied"` | `"error"`. `args_digest` is a 16-character SHA-256 prefix of the raw args — not the raw args.

**Errors:** `404 workbench_request_not_found` when `request_id` is not registered. (The endpoint currently has no actor-scoped visibility filter beyond the `OpenAiCompat` route gate; ACL-hiding of specific requests is a planned extension that would return 404 rather than 403 to match the `/views/{id}/data` convention.)

---

### GET /api/v1/web/workbench/knowledge-status

Knowledge plane readiness: canonical, search, and relation plug status plus last ingest timestamp.

**Auth:** `OpenAiCompat`

**Response:**

```json
{
  "canonical_ready": true,
  "search_ready": false,
  "relation_ready": false,
  "stale_reasons": ["search index last rebuilt >30s ago"],
  "last_ingest_at": "2026-04-19T12:00:00Z"
}
```

---

### GET /api/v1/web/workbench/views

Actor-visible registered view descriptors. The shell uses these to build the left rail and center panels.

**Auth:** `OpenAiCompat`

**Scope-based filtering:** the route gate requires `OpenAiCompat` (per [auth.md](auth.md)), but each descriptor also carries an optional `required_scope` that drives a second, per-descriptor filter. Descriptors whose `required_scope` is not held by the caller's API key are **omitted** from the response entirely — not returned with `disabled_reason` set. A key with only `OpenAiCompat` sees a smaller list than a key that also holds `Management`.

**Response:**

On trunk today the `seed_p2b` catalog (`crates/gadgetron-gateway/src/web/catalog.rs`) registers exactly one view, emitted as:

```json
{
  "views": [
    {
      "id": "knowledge-activity-recent",
      "title": "최근 활동",
      "owner_bundle": "core",
      "source_kind": "activity",
      "source_id": "recent",
      "placement": "left_rail",
      "renderer": "timeline",
      "data_endpoint": "/api/v1/web/workbench/views/knowledge-activity-recent/data",
      "refresh_seconds": 5,
      "action_ids": ["knowledge-search"],
      "required_scope": null,
      "disabled_reason": null
    }
  ]
}
```

`placement`: `"left_rail"` | `"center_main"` | `"evidence_pane"`. `renderer`: `"table"` | `"timeline"` | `"cards"` | `"markdown_doc"`.

---

### GET /api/v1/web/workbench/views/{view_id}/data

Payload for a single registered view. The shell calls `data_endpoint` from the view descriptor.

**Auth:** `OpenAiCompat`

**Path parameters:** `view_id` — string ID from `GET /views`.

**Response:**

```json
{
  "view_id": "knowledge-activity-recent",
  "payload": { ... }
}
```

`payload` shape is renderer-specific and typed at the bundle layer. On trunk today only one view ships (`knowledge-activity-recent`, a timeline stub from the seed_p2b bundle) whose payload is always `{"entries": []}` — the real activity-feed wiring is tracked as W3-WEB-3 follow-up work. Concretely:

```json
{
  "view_id": "knowledge-activity-recent",
  "payload": { "entries": [] }
}
```

**Errors:** `404 workbench_view_not_found` — returned both when `view_id` is not registered AND when the caller's scopes do not admit an existing-but-scope-gated view. The two cases are deliberately indistinguishable to avoid leaking existence of scope-restricted views (callers without admin scope get 404, not 403, on views they shouldn't know exist).

---

### GET /api/v1/web/workbench/actions

Actor-visible registered action descriptors. The shell renders these as affordances.

**Auth:** `OpenAiCompat`

**Scope-based filtering:** same per-descriptor filter as `GET /views` — each action carries an optional `required_scope`; descriptors the caller's scopes don't admit are stripped from the response.

**Response:**

The `seed_p2b` catalog registers **four** actions on trunk, all backed by the wiki gadget family (`crates/gadgetron-gateway/src/web/catalog.rs::seed_p2b()`). The four ids form the browser-driven CRUD surface exercised by the `/web/wiki` page:

| `id` | `gadget_name` | `kind` | `input_schema.required` | Purpose |
|---|---|---|---|---|
| `knowledge-search` | `wiki.search` | `query` | `["query"]` | Full-text search across wiki pages. Extra input: optional `max_results` (1–20). |
| `wiki-list` | `wiki.list` | `query` | `[]` | List all pages. No args. |
| `wiki-read` | `wiki.get` | `query` | `["name"]` | Fetch a page by name. |
| `wiki-write` | `wiki.write` | `mutation` | `["name", "content"]` | Create or overwrite a page. `destructive: false`, `requires_approval: false` — dispatches synchronously. |
| `wiki-delete` | `wiki.delete` | `dangerous` | `["name"]` | Soft-delete a page. `destructive: true` — `POST /actions/wiki-delete` returns `status=pending_approval` + `approval_id`; dispatch happens on `POST /approvals/{approval_id}/approve` (see §Approvals below). Landed in ISSUE 3 / v0.2.6. |

Example response (`knowledge-search` shown; the other three follow the same descriptor shape):

```json
{
  "actions": [
    {
      "id": "knowledge-search",
      "title": "지식 검색",
      "owner_bundle": "core",
      "source_kind": "gadget",
      "source_id": "wiki.search",
      "gadget_name": "wiki.search",
      "placement": "center_main",
      "kind": "query",
      "input_schema": {
        "type": "object",
        "properties": {
          "query": { "type": "string", "minLength": 1, "maxLength": 500 },
          "max_results": { "type": "integer", "minimum": 1, "maximum": 20 }
        },
        "required": ["query"],
        "additionalProperties": false
      },
      "destructive": false,
      "requires_approval": false,
      "knowledge_hint": "wiki.search 가젯을 직접 호출합니다.",
      "required_scope": null,
      "disabled_reason": null
    }
    // + wiki-list, wiki-read, wiki-write descriptors
  ]
}
```

`placement`: `"left_rail"` | `"center_main"` | `"evidence_pane"` | `"context_menu"`. `kind`: `"query"` | `"mutation"` | `"dangerous"`. `input_schema` is a JSON Schema fragment; arg validation on `POST /actions/{action_id}` runs against it (additionalProperties=false rejects unknown keys). E2E Gate 7f asserts the catalog surfaces all five ids (the 2026-04-19 `wiki-delete` addition widened the assertion from `>= 4` to `>= 5`).

---

### POST /api/v1/web/workbench/actions/{action_id}

Invoke a registered direct action.

**Auth:** `OpenAiCompat`

**Path parameters:** `action_id` — string ID from `GET /actions`.

**Request body:** shape depends on the target action's `input_schema`. For the seed `knowledge-search` action:

```json
{
  "args": { "query": "wiki seed", "max_results": 5 },
  "client_invocation_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

`client_invocation_id` is optional. When provided, the server holds a 5-minute TTL replay cache keyed on `(tenant_id, action_id, client_invocation_id)` to deduplicate double-clicks and retries — a repeat invocation with the same `client_invocation_id` within the TTL returns the cached response (including the original `activity_event_id`) rather than re-executing the action. The cache stores a typed `InvokeWorkbenchActionResponse` and re-serializes on each hit; because the struct is deterministic and serde's JSON output is stable, the re-serialized bytes are identical in practice. E2E Gate 7h.2 verifies this end-to-end by asserting the two response bodies are byte-for-byte equal.

**Response:**

```json
{
  "result": {
    "status": "ok",
    "approval_id": null,
    "activity_event_id": "uuid",
    "audit_event_id": "uuid",
    "refresh_view_ids": [],
    "knowledge_candidates": [],
    "payload": null
  }
}
```

**`result.payload` is the real gadget output.** When the descriptor has a `gadget_name` and the gateway was built with a `GadgetDispatcher` wired (i.e. a Penny `GadgetRegistry` was registered at startup — the default for any build with `[knowledge]` configured), the action service calls `dispatcher.dispatch_gadget(gadget_name, args)` and places the returned `GadgetResult.content` into `payload` (`crates/gadgetron-gateway/src/web/action_service.rs:262-292`). Callers interpret the payload per gadget:

- `knowledge-search` / `wiki-list`: an array of page objects.
- `wiki-read`: the page object (`{ "name": "...", "content": "...", "updated_at": "..." }`).
- `wiki-write`: an empty object `{}` or a minimal confirmation object — the operator-visible effect is the on-disk wiki update, not the payload.

When no `GadgetDispatcher` is wired (degraded mode — see §Workbench overview above) `payload` stays `null` and no side effect occurs. A dispatch error (unknown gadget, gadget-internal failure) surfaces as HTTP 500 with the `GadgetError` converted into the OpenAI envelope; the response body never sets `payload` in that case.

`result.status`: `"ok"` | `"pending_approval"` (when the descriptor has `destructive = true` — either flag alone routes the invoke through step 6 of the action service). In the `seed_p2b` catalog, `wiki-delete` is the canonical approval-gated action; the other four return `"ok"` directly. When `pending_approval`, `approval_id` is set and dispatch is deferred until `POST /api/v1/web/workbench/approvals/{approval_id}/approve` resolves the record — at which point `resume_approval` re-enters the dispatch path with the persisted args and returns the final `ok` / error response. `POST /approvals/{approval_id}/deny` terminates without dispatch. See §Approvals (below) for the full lifecycle. E2E Gate 7h.7 exercises invoke → approve → re-invoke → `ok`; second approve of the same id returns 409.

`refresh_view_ids` is typed as `Vec<String>` on the wire (non-null, possibly empty). On trunk today both paths return an empty array (`action_service.rs:207-215,284-292`) — reserved for future per-action policy. Web UI shells should loop over the array (handling zero gracefully); do not pin to specific view ids.

**Identity capture:** the server propagates `api_key_id` from `TenantContext` into `AuthenticatedContext.user_id` and `tenant_id` into `AuthenticatedContext.tenant_id` before invoking the action service. Activity captures (when the candidate coordinator is wired) record the real caller via `activity_event_id`.

**Audit:** `audit_event_id` is populated on every terminal path (ok / pending_approval / dispatch-error) by the `ActionAuditSink` wired at server startup (ISSUE 3 / v0.2.6). The same UUID the sink receives is returned in the response, and when Postgres is configured an `action_audit_events` row is persisted under that id within a few milliseconds. Use `GET /api/v1/web/workbench/audit/events` (below) to read rows back; each row carries `event_id`, `action_id`, `gadget_name`, `actor_user_id`, `tenant_id`, `outcome` (`success` | `error` | `pending_approval`), optional `error_code`, `elapsed_ms`, and `created_at`.

**Errors:**

| Code | HTTP | When |
|------|------|------|
| `workbench_action_not_found` | 404 | `action_id` not registered, OR caller's scopes do not admit an action whose `required_scope` they lack (returned as 404 to avoid leaking existence of scope-gated actions, matching `GET /views/{id}/data` behavior) |
| `workbench_action_invalid_args` | 400 | `args` fails the descriptor's `input_schema` validation |
| `forbidden` | 403 | This instance has disabled direct actions (`DirectActionsDisabled` policy) |
| `config_error` | 400 | Workbench service not wired (no `[knowledge]` configured), or action service not wired in this build |

---

### POST /api/v1/web/workbench/approvals/{approval_id}/approve

Resolve a `pending_approval` record into an `ok` dispatch. Introduced in ISSUE 3 / v0.2.6 (`crates/gadgetron-gateway/src/web/workbench.rs::approve_action`).

**Auth:** `OpenAiCompat` (same as `/actions/{id}`).

**Path parameters:** `approval_id` — the UUID returned from the originating invoke response (`result.approval_id`).

**Request body:** empty `{}`.

**Response:** `InvokeWorkbenchActionResponse` — identical shape to `POST /actions/{id}`. On success the server marks the approval `Approved` with the calling actor as resolver and then runs `resume_approval`, which re-enters the dispatch path with the originally-persisted args; the returned `result.status` is `"ok"` with `result.payload` carrying the gadget output. `result.approval_id` is set to the resolved id and `result.audit_event_id` is populated for the dispatch event.

**Errors:**

| Code | HTTP | When |
|------|------|------|
| `workbench_approval_not_found` | 404 | `approval_id` is unknown to the store |
| `workbench_approval_already_resolved` | 409 | The record is already `approved` / `denied` — message carries the current state |
| `forbidden` | 403 | Cross-tenant: the approval belongs to a different tenant than the authenticated actor |
| `config_error` | 400 | `approval_store` or action service is not wired in this build (serving without Postgres or without a `GadgetDispatcher`) |

E2E Gate 7h.7 covers the happy path + the double-approve conflict (second approve on the same id → 409).

---

### POST /api/v1/web/workbench/approvals/{approval_id}/deny

Refuse a `pending_approval` record. No dispatch occurs.

**Auth:** `OpenAiCompat`.

**Path parameters:** `approval_id` — UUID.

**Request body** (all fields optional):
```json
{ "reason": "policy violation" }
```

`reason` is a free-form string (≤ ~256 chars practical; no hard server limit enforced today) and is persisted on the approval row. Omit the body (or send `{}`) to deny without a reason.

**Response:**
```json
{
  "id": "uuid",
  "state": "denied",
  "resolved_at": "2026-04-19T05:15:30Z",
  "resolved_by_user_id": "uuid",
  "reason": "policy violation"
}
```

**Errors:** same shape as `approve` (404 / 409 / 403 / 400). Cross-tenant deny returns `forbidden`.

---

### GET /api/v1/web/workbench/audit/events

Tenant-scoped read over `action_audit_events` — the rows the `ActionAuditSink` persisted at each action-service terminal. Landed in ISSUE 3 / v0.2.6 (`crates/gadgetron-gateway/src/web/workbench.rs::list_audit_events`, backed by `crates/gadgetron-xaas/src/audit/action_event.rs::query_action_audit_events`).

**Auth:** `OpenAiCompat`.

**Query parameters** (all optional):

| Name | Type | Default | Notes |
|---|---|---|---|
| `action_id` | string | — | Exact-match filter (e.g. `wiki-write`). No wildcard / prefix matching. |
| `since` | RFC3339 timestamp | — | Inclusive lower bound on `created_at` (e.g. `2026-04-19T00:00:00Z`). |
| `limit` | integer | 100 | Clamped to `[1, 500]`. Out-of-range values are silently clamped, not rejected. |

**Tenant boundary:** the handler ALWAYS pins the query to the authenticated actor's `tenant_id` — there is no query parameter to read another tenant's rows. Cross-tenant audit access is not reachable from this HTTP surface regardless of what the caller sends.

**Response:**
```json
{
  "events": [
    {
      "event_id": "uuid",
      "action_id": "wiki-write",
      "gadget_name": "wiki.write",
      "actor_user_id": "uuid-or-tenant-key-id",
      "tenant_id": "uuid",
      "outcome": "success",
      "error_code": null,
      "elapsed_ms": 42,
      "created_at": "2026-04-19T05:15:30.123Z"
    }
  ],
  "returned": 1
}
```

- Rows are ordered `created_at DESC` (newest first).
- `outcome` ∈ `"success"` | `"error"` | `"pending_approval"`. The approve-then-dispatch-ok path emits two rows: one `pending_approval` at step 6, one `success` at the post-approve dispatch.
- `error_code` is non-null only when `outcome == "error"`.
- `returned` mirrors `events.len()` — convenience so clients don't re-count.

**Errors:**

| Code | HTTP | When |
|------|------|------|
| `config_error` | 400 | `pg_pool` is not configured on this server (in-memory / demo mode). The endpoint requires Postgres. |
| `config_error` | 400 | Underlying SQL query failed — message includes the sqlx error. |

E2E Gate 7h.8 verifies an unfiltered GET returns the rows from prior gates and that `?action_id=wiki-write` narrows server-side (not client-side).

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

Sending a request to any of these endpoints with a valid `Management`-scoped key returns HTTP 501 today:

```sh
curl -s http://localhost:8080/api/v1/nodes \
  -H "Authorization: Bearer gad_live_your_management_key_here"
# HTTP 501 (no body)
```

E2E Gates 7k and 7k.2 assert the **RBAC positive path** — any status except 401/403 is acceptable for a Management key on these routes (currently 501; will be 200 once each aggregator lands, or 503 during PostgreSQL pool outages). Your monitoring should treat 501 as "feature not shipped" and 401/403 as real auth regressions.

Sending with an `OpenAiCompat`-scoped key returns HTTP 403 (scope guard fires before the stub handler).
