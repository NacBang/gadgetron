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
| `quota_exceeded` | 429 | `quota_error` | Tenant's daily spending limit reached. **Every HTTP 429 response carries both the `Retry-After: 60` HTTP header AND a `retry_after_seconds: 60` field inside the error JSON body** (ISSUE 11 TASK 11.1 / v0.5.1 / PR #230) — SDK clients that honor either surface back off deterministically instead of retrying in a tight loop. `retry_after_seconds` is `Null`-absent from non-429 responses (403, 401, 404 keep the base body shape since `Retry-After` has specific HTTP semantics). `QUOTA_RETRY_AFTER_SECONDS = 60` is a conservative constant today; TASK 11.2 (PR #231, shipped) added the `TokenBucketRateLimiter` but did NOT yet thread its exact refill time through `QuotaToken` — the `Retry-After` value still reports the 60s upper bound for both the rate-limit and daily-cost rejection paths. Real refill-countdown threading is a future follow-up not tracked in the currently-numbered ISSUE 11 TASKs (which are 11.3 Postgres-backed spend tracking + 11.4 `/web` 429 UI surface). |
| `config_error` | 400 | `invalid_request_error` | Server configuration is invalid |
| `routing_failure` | 503 | `server_error` | No provider available to serve the request |
| `provider_error` | 502 | `api_error` | Upstream LLM provider returned an error |
| `stream_interrupted` | 502 | `api_error` | SSE stream was interrupted mid-response |
| `billing_error` | 500 | `api_error` | Internal billing calculation error |
| `download_failed` | 500 | `api_error` | Model download failed — error variant exists in `gadgetron-core::error` but the node-subsystem path that emits it is scheduled for EPIC 5 (cluster platform, post-1.0 per ROADMAP v2). The "Sprint 4+" label from Phase 1 planning did not carry into ROADMAP v2. |
| `hotswap_failed` | 500 | `api_error` | Model hot-swap failed — same contract as `download_failed`: variant exists, emission path is EPIC 5 scope (post-1.0). |
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

### GET /v1/tools

Requires scope: `OpenAiCompat`

MCP-style tool discovery. Returns the operator-allowed Gadget schema
set — the same set Penny is allowed to dispatch at runtime — so external
agents (`claude-code`, custom MCP clients, or any HTTP caller that
wants to enumerate capabilities before issuing a tool call) can
discover what is available on this deployment.

Shipped in ISSUE 7 (v0.2.10). The backing trait is
`gadgetron_core::agent::tools::GadgetCatalog`; concrete catalogs are
assembled at startup when `[knowledge]` is configured. A deployment
without Gadgets returns `{"tools": [], "count": 0}` — this is the
shape-stable empty case, not an error.

**Response:**

HTTP 200

```json
{
  "tools": [
    {
      "name": "wiki.search",
      "description": "Search the wiki for pages matching a query.",
      "tier": "read",
      "input_schema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] },
      "idempotent": true
    }
  ],
  "count": 1
}
```

Fields:

- `tools[].name` — namespaced gadget name (`{category}.{gadget}`), matches the `--allowed-tools` format used by Claude Code.
- `tools[].tier` — one of `"read" | "write" | "destructive"`. Write/Destructive tools may still be filtered out of Penny's allowed set by operator config (`[agent.gadgets.write]` / `[agent.gadgets.destructive]`); the `/v1/tools` listing already reflects that filter.
- `tools[].input_schema` — JSON Schema (draft-07) for the `args` object passed to a tool call.
- `tools[].idempotent` — hint only. `null` = no claim; `true` = safe to retry; `false` = MUST NOT be retried.
- `count` — deduped tool count (duplicates in the raw registry collapse to the last registration per `GadgetRegistryBuilder::freeze`).

**Example:**

```sh
curl -s http://localhost:8080/v1/tools \
  -H "Authorization: Bearer gad_live_your32chartoken00000000000000" \
  | jq .
```

E2E Gate 7i.2 pins both the shape and the 401-on-missing-auth contract.

---

### POST /v1/tools/{name}/invoke

Requires scope: `OpenAiCompat`

MCP-style tool invocation. Executes a gadget the caller discovered via
`GET /v1/tools`. Dispatch flows through the same `GadgetDispatcher`
Penny uses, so the operator-config L3 allowed-names gate runs here too
— a tool the operator disabled for Penny is ALSO unreachable on this
path.

Shipped in ISSUE 7 TASK 7.2 (v0.2.11). Deployments that did not wire
`[knowledge]` at startup return **503** `{"error": {"code":
"mcp_not_available", ...}}` so clients don't retry a dispatcher that
can never run.

**Path param:** `name` — the full namespaced gadget name (e.g.
`wiki.list`, `knowledge.search`). Dots are not reserved in axum path
segments, so no percent-encoding is required.

**Request body:** the gadget's `args` object. The JSON Schema for each
gadget is exposed at `GET /v1/tools` under `tools[].input_schema`.
Sending `{}` for a zero-arg gadget is valid.

**Success response:**

HTTP 200

```json
{
  "content": { "pages": ["README", "setup", "rfc-0001"], "total": 3 },
  "is_error": false
}
```

Fields:

- `content` — opaque JSON value defined per-gadget. Rendered back to the external agent as the MCP `tool_result.content` block.
- `is_error` — if `true`, the gadget ran to completion but the tool author considers the outcome an error (e.g. `wiki.read` for a missing page). MCP clients typically display this in a different color but do NOT retry.

**Protocol errors:**

| HTTP | `error.code`             | When                                                                     |
|------|--------------------------|--------------------------------------------------------------------------|
| 400  | `mcp_invalid_args`       | `args` failed the gadget's own input validation.                         |
| 403  | `mcp_denied_by_policy`   | L3 allowed-names gate rejected: operator disabled the tool.              |
| 404  | `mcp_unknown_tool`       | No gadget registered with that name.                                     |
| 408  | `mcp_approval_timeout`   | Destructive tool's approval prompt timed out. **Not emitted on trunk today** — the Penny-side cross-process approval bridge (ADR-P2A-06 SEC-MCP-B1) hasn't been wired yet, so the 408 surface stays dormant. The error-code string is reserved in the enum so clients can key on it in advance. The direct-action workbench approval flow (`wiki-delete` → `/approvals/{id}/approve|deny`) shipped at v0.2.6 and surfaces different error codes — see §Approvals. |
| 429  | `mcp_rate_limited`       | Per-tool hourly cap exceeded.                                            |
| 500  | `mcp_execution_failed`   | The gadget itself errored — usually infrastructure (wiki disk full, etc.). |
| 503  | `mcp_not_available`      | Dispatcher unwired on this deployment (no `[knowledge]` section).        |

Error body shape is `{"error": {"code": "...", "message": "..."}}` —
same shape as other gateway errors so SDK error handlers work
uniformly.

**Example:**

```sh
curl -s -X POST http://localhost:8080/v1/tools/wiki.list/invoke \
  -H "Authorization: Bearer gad_live_your32chartoken00000000000000" \
  -H "Content-Type: application/json" \
  -d '{}' \
  | jq .
```

E2E Gate 7i.3 pins the happy path (`wiki.list` → 200 with populated
`content`), the unknown-gadget 404 + `mcp_unknown_tool` code, and the
401-on-no-auth contract.

### Cross-session audit

Every successful or failed call to `POST /v1/tools/{name}/invoke` lands
a row in the `tool_audit_events` table (same table Penny uses for its
own tool-call trail). The row sets:

- `owner_id = <api_key_id>` — the authenticated principal
- `tenant_id = <tenant_id>` — the authenticated tenant
- `conversation_id = NULL` — external MCP calls have no Penny session
- `claude_session_uuid = NULL`

Penny-internal calls in P2A populate both `owner_id` and `tenant_id`
as NULL. Operators can therefore filter `WHERE owner_id IS NOT NULL`
(or the equivalent `GET
/api/v1/web/workbench/audit/tool-events?tool_name=...` response
filtered client-side) to pick out cross-session (external-agent)
callers.

Shipped in ISSUE 7 TASK 7.3 (v0.2.12). E2E Gate 7i.4 pins the
invariant: after an `/v1/tools/wiki.list/invoke` call, at least one
`tool_audit_events` row for `wiki.list` has a non-null `owner_id`.

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

The workbench projection API surfaces activity, knowledge plug health, and registered view/action descriptors to the Web UI shell.

**Route inventory (fifteen routes, always mounted on trunk):**

| Set | Count | Scope | Shipped in |
|-----|-------|-------|-----------|
| Read + action + evidence + knowledge-status + views + data + `/actions` list + invoke | 8 | `OpenAiCompat` | ISSUE 1–2 / v0.2.0–v0.2.5 |
| Approval approve + deny + `GET /audit/events` | 3 | `OpenAiCompat` | ISSUE 3 / v0.2.6 (PR #188) |
| `/usage/summary` + `/events/ws` | 2 | `OpenAiCompat` | ISSUE 4 / v0.2.7 (PR #194) |
| `/audit/tool-events` | 1 | `OpenAiCompat` | ISSUE 5 / v0.2.8 (PR #199) |
| `/admin/reload-catalog` | 1 | **`Management`** | ISSUE 8 TASK 8.2 / v0.4.2 (PR #213) |

The `/admin/*` sub-tree is the one scope exception — `scope_guard_middleware` matches the `/admin/` prefix with `Management` **before** the broader workbench rule, so an OpenAiCompat workbench key cannot self-reload the catalog (returns 403 `scope_required`).

**Degraded mode.** `build_workbench(knowledge_service, candidate_coordinator, penny_registry, pg_pool)` (`crates/gadgetron-cli/src/main.rs:1256-1331`) returns `Some(...)` even when all four arguments are `None`. Per-subsystem fallbacks:

- `bootstrap` + descriptor catalog — still reachable.
- Gadget dispatch — returns empty payload.
- Activity capture — no-op.
- Approval store — falls back to in-memory.
- `ActionAuditSink` — falls back to `NoopActionAuditSink`.
- `/usage/summary`, `/audit/events`, `/audit/tool-events` — return 400 `config_error` without a pool.
- `/events/ws` — opens against a zero-publisher `ActivityBus`.
- Penny-attributed activity capture — skipped when `candidate_coordinator` is `None` (ISSUE 6's `GadgetAuditEventWriter::with_coordinator()` plumbing).

**Parameter-addition history** — how `build_workbench`'s signature grew over the EPIC 1/2 ISSUEs:

- PR #188 / v0.2.6 — added `pg_pool` (fourth parameter) so the action-audit writer + approval store can take a Postgres pool when one is configured.
- PR #194 / v0.2.7 — reused `pg_pool` for the usage rollup + audit query.
- PR #199 / v0.2.8 — extended `pg_pool` use to Penny tool-call audit persistence.
- PR #201 / v0.2.9 — threaded `candidate_coordinator` through the Penny registration path so tool-call audit fans out to `CapturedActivityEvent` rows alongside DB persistence.

**What is real on trunk today:**
- `GET /bootstrap` — returns live `gateway_version` + `knowledge-status` booleans + registered descriptor catalog.
- `GET /views`, `GET /actions` — return the five descriptors in `seed_p2b` (see `GET /actions` below; ISSUE 3 / v0.2.6 added `wiki-delete` as the fifth, the canonical approval-gated action).
- `POST /actions/{action_id}` — dispatches to the registered Gadget via `Arc<dyn GadgetDispatcher>` (`crates/gadgetron-core/src/agent/tools.rs:50-63`). When the gateway has a Penny `GadgetRegistry` wired, this reaches the same `wiki.search` / `wiki.list` / `wiki.get` / `wiki.write` gadgets Penny uses. The response's `result.payload` carries the raw `GadgetResult.content` — real wiki data, not a stub.
- `POST /admin/reload-catalog` — atomic `Arc<ArcSwap<CatalogSnapshot>>` store (ISSUE 8 TASK 8.2 / v0.4.2 endpoint shape; TASK 8.3 / v0.4.3 upgraded the handle from `DescriptorCatalog` to `CatalogSnapshot { catalog, validators }` via PR #214 so a reload swaps catalog + pre-compiled JSON-schema validators in lockstep). Management-scoped; in-flight requests finish against their snapshot. Today's only source is `seed_p2b`; TASK 8.4 widens to file-based loading. See §POST /admin/reload-catalog below.

**What is stubbed on trunk today:**
- `activity.entries` — the endpoint read path still returns `[]` (see §/activity). The underlying capture flow IS live: `CapturedActivityEvent` rows land in the coordinator for both direct-action (ISSUE 3) and Penny tool calls (ISSUE 6 / PR #201). Read-side projection (PSL-1) is the remaining gap.
- `request_evidence` — always 404 (see §/evidence).
- `refresh_view_ids` — always `[]` on every action response (see §POST /actions).

**Shipped since this overview was first written** (historically listed as stubbed):
- `audit_event_id` — **populated on every terminal path** (ok / pending_approval / dispatch-error) by `ActionAuditSink` wired at server startup. Landed in ISSUE 3 / v0.2.6 (PR #188). See §POST /actions §Audit and §GET /audit/events below.

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
  "gateway_version": "0.5.4",
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

**Field contract.** Sourced from the Rust structs `WorkbenchBootstrapResponse`, `PlugHealth`, and `WorkbenchKnowledgeSummary` at `crates/gadgetron-core/src/workbench/mod.rs:27-54`. E2E Gate 7 asserts the inner shape on every healthy boot (`scripts/e2e-harness/run.sh`).

| Field | Type | Notes |
|-------|------|-------|
| `gateway_version` | non-empty string | Cargo workspace version, e.g. `"0.5.4"`. |
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

Recent workbench activity feed: Penny turns, direct actions, system events. **On trunk today the HTTP endpoint is still a stub** — `InProcessWorkbenchProjection::activity` at `crates/gadgetron-gateway/src/web/projection.rs:101-106` always returns `{"entries": [], "is_truncated": false}` regardless of `limit` or real traffic. The response shape documented below is the contract future activity-source wiring (PSL-1 read path) will populate; build client code against the shape, but don't expect non-empty data from the endpoint until that ships. E2E Gate 7c asserts the empty-state shape.

**Progress since ISSUE 6 / v0.2.9** (PR #201). The underlying write path is live: Penny tool calls now fan out through `GadgetAuditEventWriter.with_coordinator()` into `KnowledgeCandidateCoordinator::capture_action`, producing `CapturedActivityEvent { origin: Penny, kind: GadgetToolCall }` rows. Direct-action dispatch has been producing `CapturedActivityEvent { origin: UserDirect, kind: DirectAction }` rows since ISSUE 3. What remains for a non-empty endpoint response is the read-projection wiring — reading from the coordinator's backing store into `WorkbenchActivityResponse.entries`. Until that ships, inspect the captured rows via `tracing` logs (`target: "action_audit"` / `"penny_audit"`) or by querying the coordinator directly in tests.

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

**`result.payload` is the real gadget output.** When the descriptor has a `gadget_name` and the gateway was built with a `GadgetDispatcher` wired (i.e. a Penny `GadgetRegistry` was registered at startup — the default for any build with `[knowledge]` configured), the action service calls `dispatcher.dispatch_gadget(gadget_name, args)` and places the returned `GadgetResult.content` into `payload` (`crates/gadgetron-gateway/src/web/action_service.rs:355-397`; PR #188 / v0.2.6 widened this block to also emit the `DirectActionCompleted` audit event on the Err arm). Callers interpret the payload per gadget:

- `knowledge-search` / `wiki-list`: an array of page objects.
- `wiki-read`: the page object (`{ "name": "...", "content": "...", "updated_at": "..." }`).
- `wiki-write`: an empty object `{}` or a minimal confirmation object — the operator-visible effect is the on-disk wiki update, not the payload.

When no `GadgetDispatcher` is wired (degraded mode — see §Workbench overview above) `payload` stays `null` and no side effect occurs. A dispatch error (unknown gadget, gadget-internal failure) surfaces as HTTP 500 with the `GadgetError` converted into the OpenAI envelope; the response body never sets `payload` in that case.

`result.status`: `"ok"` | `"pending_approval"` (when the descriptor has `destructive = true` — either flag alone routes the invoke through step 6 of the action service). In the `seed_p2b` catalog, `wiki-delete` is the canonical approval-gated action; the other four return `"ok"` directly. When `pending_approval`, `approval_id` is set and dispatch is deferred until `POST /api/v1/web/workbench/approvals/{approval_id}/approve` resolves the record — at which point `resume_approval` re-enters the dispatch path with the persisted args and returns the final `ok` / error response. `POST /approvals/{approval_id}/deny` terminates without dispatch. See §Approvals (below) for the full lifecycle. E2E Gate 7h.7 exercises invoke → approve → re-invoke → `ok`; second approve of the same id returns 409.

`refresh_view_ids` is typed as `Vec<String>` on the wire (non-null, possibly empty). On trunk today every construction site returns an empty array (`action_service.rs:324`, `:477`, `:570` — pending_approval path, ok path, and post-approve resume path respectively) — reserved for future per-action policy. Web UI shells should loop over the array (handling zero gracefully); do not pin to specific view ids.

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

### POST /api/v1/web/workbench/admin/reload-catalog

Atomically swap the in-memory `CatalogSnapshot` (catalog + pre-compiled JSON-schema validators) for a fresh one. Landed in ISSUE 8 TASK 8.2 / v0.4.2 (PR #213) as the first endpoint under the new `/api/v1/web/workbench/admin/` subtree. TASK 8.3 / v0.4.3 (PR #214) upgraded the underlying handle from `Arc<ArcSwap<DescriptorCatalog>>` to `Arc<ArcSwap<CatalogSnapshot>>` so a reload now swaps catalog and validators in lockstep — closing the TASK 8.2 known-limitation where validators were pre-compiled at service construction and not rebuilt on reload.

**Scope:** `Management` — **not** `OpenAiCompat`. Added as a new rule in `scope_guard_middleware` (`crates/gadgetron-gateway/src/middleware/scope.rs:38-43`) that matches `/api/v1/web/workbench/admin/` **before** the broader `/api/v1/web/workbench/` rule, so workbench users cannot self-reload the catalog. Any call from an OpenAiCompat key returns 403 (`scope_required`).

**Request:** empty body (`Content-Type: application/json` is accepted but ignored — the handler reads no fields).

**Example (curl, local demo):**
```bash
# Use a Management-scoped key (the OpenAiCompat chat key will 403).
export MGMT_KEY=$(gadgetron key create --tenant-id "$(gadgetron tenant list | jq -r '.[0].id')" --scope management | jq -r '.raw_key')

curl -fsS -X POST \
  -H "Authorization: Bearer $MGMT_KEY" \
  http://localhost:8080/api/v1/web/workbench/admin/reload-catalog
```

**Response (HTTP 200):**

When `[web] catalog_path` is NOT configured (fallback to hand-coded `seed_p2b()`):
```json
{
  "reloaded": true,
  "action_count": 5,
  "view_count": 3,
  "source": "seed_p2b",
  "source_path": null
}
```

When `[web] catalog_path = "/path/to/catalog.toml"` is configured and the TOML file carries a `[bundle]` table (ISSUE 8 TASK 8.4 / v0.4.4 + ISSUE 9 TASK 9.1 / v0.4.6):
```json
{
  "reloaded": true,
  "action_count": 5,
  "view_count": 3,
  "source": "config_file",
  "source_path": "/path/to/catalog.toml",
  "bundle": {
    "id": "gadgetron-core",
    "version": "0.5.4"
  }
}
```

When `[web] catalog_path` points at a legacy anonymous TOML (no `[bundle]` table) OR the fallback `seed_p2b()` path, the `bundle` field is omitted from the JSON (serde `skip_serializing_if = "Option::is_none"`).

When `[web] bundles_dir = "/path/to/bundles"` is configured and at least one `<subdir>/bundle.toml` merges successfully (ISSUE 9 TASK 9.2 / v0.4.7 + TASK 9.3 / v0.4.8):
```json
{
  "reloaded": true,
  "action_count": 7,
  "view_count": 4,
  "source": "bundles_dir",
  "source_path": "/path/to/bundles",
  "bundles": [
    {"id": "gadgetron-core", "version": "0.5.4"},
    {"id": "acme-ops", "version": "1.2.0"}
  ]
}
```

The `bundles` field lists **every contributing bundle** in the merged catalog. When the source is a single flat TOML (TASK 8.4) or `seed_p2b()`, the field is omitted entirely via serde `skip_serializing_if = "Vec::is_empty"`.

- `reloaded` — always `true` on 200. Clients key observability on this wire field rather than on the HTTP status so a structured audit log can quote the exact flag.
- `action_count` / `view_count` — counts in the catalog **after** the swap, computed with all three scopes (`OpenAiCompat`, `Management`, `XaasAdmin`) in the visibility filter so the totals reflect every descriptor the fresh catalog carries.
- `source` — wire-stable enum. Three values on trunk: `"seed_p2b"` when neither `bundles_dir` nor `catalog_path` is configured (hand-coded `DescriptorCatalog::seed_p2b()` fallback); `"config_file"` when `catalog_path` is configured and the single TOML file parsed successfully (flat-file loader, TASK 8.4); `"bundles_dir"` when `bundles_dir` is configured and its subdirectories merged successfully (multi-bundle loader, TASK 9.2). The E2E harness boots against `source == "bundles_dir"` as of PR #222 (TASK 9.3 / v0.4.8) — the shipped `bundles/gadgetron-core/` subtree is the canonical default. Gate 7q.1 pins this end-to-end.
- `source_path` — `null` when `source == "seed_p2b"`; the absolute path of the TOML file when `source == "config_file"` (TASK 8.4); the absolute path of the bundles directory when `source == "bundles_dir"` (TASK 9.2 / TASK 9.3). Admin tooling uses this to confirm which on-disk artifact produced the live catalog.
- `bundle` — optional `{id, version, required_scope?}` struct (ISSUE 9 TASK 9.1 / v0.4.6 / PR #219 + ISSUE 10 TASK 10.3 / v0.4.11 / PR #226 added the optional `required_scope`). Populated when the loaded TOML carries a top-level `[bundle]` table with `id` and `version` fields (single-bundle / flat-file path only). Operators use this to identify **which catalog they loaded** without out-of-band tracking. `seed_p2b()`, anonymous flat TOMLs, and multi-bundle aggregation paths all produce `None` here — the multi-bundle case populates `bundles` (below) instead. The first-party bundle file at `bundles/gadgetron-core/bundle.toml` carries `{id: "gadgetron-core", version: <workspace version>}` — a drift test asserts this bundle file and `seed_p2b()` produce the same action id set so the two sources stay in lockstep.
- `bundles` — `Vec<BundleMetadata>` (ISSUE 9 TASK 9.2 / v0.4.7 / PR #220). Populated with **every contributing bundle** when the loaded catalog came from a bundle directory (`[web] bundles_dir`). The handler scans every immediate subdirectory of `bundles_dir` for a `bundle.toml`, merges matching manifests in alphabetical path order so reloads are idempotent, and records each contributor's metadata here (including `required_scope` when the bundle declares one, post-PR #226). Subdirectories without `bundle.toml` are silently skipped (operator workspace dirs, hidden dirs). Empty in every other case (seed_p2b / flat-file path / anonymous TOML). Single-bundle callers should read `bundle`; multi-bundle admin tooling should read `bundles` and distinguish "1 bundle" from "N bundles" without a special flag.

**Bundle-level scope inheritance (ISSUE 10 TASK 10.3 / v0.4.11 / PR #226).** A bundle manifest can declare `required_scope = "Management"` at the `[bundle]` level. On parse (both `from_toml_file` and `from_bundle_dir`), a post-parse pass walks every view + action and **inherits** the bundle's `required_scope` onto descriptors that don't have their own. Descriptors with explicit `required_scope` keep theirs (narrower wins). Zero-overhead for bundles that don't declare a scope (default `None`, no inheritance pass). Design motivation: a bundle with N operator-only actions used to need `required_scope = "Management"` on every descriptor; one declaration at the bundle level now covers the whole set AND travels with the manifest file so `POST /admin/bundles` installs inherit the scope floor automatically. Actors without the floored scope see NONE of the bundle's descriptors (the effective scope also lands on the descriptor itself so downstream audit/log can introspect without re-reading the bundle).

**Plumbing.** The handler reads the shared `Arc<ArcSwap<CatalogSnapshot>>` handle wired by `build_workbench` (ISSUE 8 TASK 8.1 / PR #211 substrate, rev-bumped to `CatalogSnapshot` in TASK 8.3 / PR #214). To build the fresh catalog the handler picks a source in precedence order: (1) **`WebConfig.bundles_dir`** is `Some(dir)` (TASK 9.2 / PR #220) → calls `DescriptorCatalog::from_bundle_dir(dir)` which scans every `<dir>/<name>/bundle.toml` and merges into one catalog; deterministic alphabetical order; duplicate action OR view ids across bundles surface as a hard `Config` error naming the id + both contributing bundle ids so operators must rename or remove one before the next reload succeeds; `allow_direct_actions` is OR-folded across bundles (if ANY manifest opts in, the merged catalog opts in); (2) **`WebConfig.catalog_path`** is `Some(path)` (TASK 8.4 / PR #216) → calls `DescriptorCatalog::from_toml_file(path)` via the `CatalogFile` shape; (3) fallback → `DescriptorCatalog::seed_p2b()`. The resulting `DescriptorCatalog` is then turned into a snapshot via `into_snapshot()` (compiles JSON-schema validators for every action), and `ArcSwap::store()` atomically publishes the `(catalog, validators)` pair. In-flight requests holding an `Arc<CatalogSnapshot>` snapshot finish reading BOTH catalog and validators from the old pointer; any request that reads the handle after the store sees the new pointer for both sides. See `crates/gadgetron-gateway/src/web/workbench.rs::reload_catalog_handler`, `crates/gadgetron-gateway/src/web/catalog.rs::CatalogSnapshot`, `DescriptorCatalog::from_toml_file`, and `DescriptorCatalog::from_bundle_dir` in the same catalog module.

**Errors:**

| Code | HTTP | When |
|------|------|------|
| `scope_required` | 403 | Caller's API key scope is not `Management` (e.g. OpenAiCompat workbench key). Enforced by `scope_guard_middleware`, not the handler. |
| `config_error` | 503-class (400 on trunk wire) | `state.workbench.descriptor_catalog` is `None`. This only happens in headless test builds that skip `build_workbench` — production paths always set the handle, so the guard is defensive. |
| `config_error` | 500 | `catalog_path` is configured but the TOML file failed to read or parse (file missing, invalid syntax, schema mismatch). Error message embeds the file path and the specific serde / IO error (TASK 8.4 / PR #216). **The running snapshot is NOT replaced on failure** — a malformed edit cannot take the workbench down. Fix the TOML and retry the reload. |
| `config_error` | 500 | `bundles_dir` is configured but directory scan / merge failed: (a) directory missing; (b) one of the `<subdir>/bundle.toml` files failed to read/parse; (c) **duplicate action or view id across bundles** — message names the conflicting id and both bundle ids that declared it (TASK 9.2 / PR #220). **The running snapshot is NOT replaced on failure** — same guarantee as TASK 8.4. Rename or remove the duplicate entry in one of the bundles and retry the reload. |

**Validator rebuild (TASK 8.3 shipped in PR #214).** The TASK 8.2 known-limitation — validators on `InProcessWorkbenchActionService` pre-compiled at construction and NOT rebuilt by reload — is closed. `CatalogSnapshot` bundles the catalog with its pre-compiled JSON-schema validators so one `ArcSwap::store()` replaces both sides. No reader can observe a new catalog paired with old validators (or vice versa). This was a hard prerequisite for TASK 8.4's file-based source (where a legitimate TOML edit can change schemas between reloads); TASK 8.4 / PR #216 ships that file source on top of this foundation.

**Signal-based equivalent (ISSUE 8 TASK 8.5 / v0.4.5 / PR #217).** The same reload code path is also reachable via POSIX `SIGHUP` — `kill -HUP <pid>`. This is the **non-HTTP alternative** for operators who prefer Unix signals or need to reload without going through auth / scope / network. On receipt, `spawn_sighup_reloader()` (Unix-only tokio task installed at server startup) calls the same shared `perform_catalog_reload()` helper as `reload_catalog_handler` above — the in-memory effect, the `ReloadCatalogResponse` struct, and the `workbench.admin` tracing telemetry are all identical to the HTTP path. The only wire-level difference: the signal path does not return a JSON body to the operator (use the HTTP endpoint if you need to inspect `action_count` / `source_path` programmatically). On Windows / non-Unix platforms the signal handler doesn't install — operators must use the HTTP endpoint instead. See [`docs/manual/configuration.md`](configuration.md#web) §`[web]` for the operator workflow recipe.

E2E Gate 7q.1 verifies the swap lands (shape assertion + cross-check that `action_count` in the response equals the count `GET /workbench/actions` reports immediately after — catches the "swap happened but read path still points at the old pointer" regression). Gate 7q.2 verifies that an OpenAiCompat-scoped key gets 403 on this endpoint (RBAC enforced).

---

### GET /api/v1/web/workbench/admin/bundles

Read-only enumeration of every bundle discoverable under the configured `[web] bundles_dir`. Landed in ISSUE 10 TASK 10.1 / v0.4.9 (PR #223) as the first step toward the bundle marketplace (install / uninstall / scope isolation / signed manifests are TASK 10.2 – 10.4). The endpoint does NOT touch the live `Arc<ArcSwap<CatalogSnapshot>>` handle — operators can safely poll it while requests are in flight.

**Scope:** `Management` — same admin sub-tree rule as `POST /admin/reload-catalog`. An OpenAiCompat key returns 403 `scope_required` (Gate 7q.5 pins this).

**Request:** no parameters, no body.

**Example (curl):**
```bash
curl -fsS -H "Authorization: Bearer $MGMT_KEY" \
  http://localhost:8080/api/v1/web/workbench/admin/bundles \
  | jq .
```

**Response (HTTP 200):**
```json
{
  "bundles_dir": "/etc/gadgetron/bundles",
  "count": 2,
  "bundles": [
    {
      "bundle": {"id": "gadgetron-core", "version": "0.5.4"},
      "source_path": "/etc/gadgetron/bundles/gadgetron-core/bundle.toml",
      "action_count": 5,
      "view_count": 3
    },
    {
      "bundle": {"id": "acme-ops", "version": "1.2.0"},
      "source_path": "/etc/gadgetron/bundles/acme-ops/bundle.toml",
      "action_count": 2,
      "view_count": 1
    }
  ]
}
```

- `bundles_dir` — echoed absolute path of the configured directory, so admin tooling can confirm it's reading the same disk location it thinks it is.
- `count` — length of the `bundles` array (convenience so clients don't re-count).
- `bundles[].bundle` — `Option<BundleMetadata>` (same shape as the reload response `bundle` field; id + version). `null` for manifests that don't declare a top-level `[bundle]` table; admin UIs should nudge operators to add metadata to those because `POST /admin/bundles` install + `DELETE /admin/bundles/{bundle_id}` uninstall (TASK 10.2 / PR #224, shipped) require the id as the path parameter — a legacy anonymous bundle cannot be uninstalled via the HTTP surface, only by direct filesystem removal + reload.
- `bundles[].source_path` — absolute path of the specific `bundle.toml` file (not just the subdirectory).
- `bundles[].action_count` / `bundles[].view_count` — descriptor counts from that bundle alone (pre-merge). The merged catalog's counts come from the `POST /admin/reload-catalog` response instead.
- **Ordering** — `bundles` is sorted by subdirectory path so the sequence matches exactly what `DescriptorCatalog::from_bundle_dir()` produces for a reload. Tooling can rely on deterministic output across process restarts.

**Read-only semantics.** The handler scans `bundles_dir` subdirectories, reads each `bundle.toml`, and serializes metadata + counts. It does NOT: (a) compile schema validators, (b) merge actions into a `DescriptorCatalog`, (c) call `ArcSwap::store()`, (d) emit any tracing telemetry that would conflict with `workbench.admin` reload events. Subdirectories without `bundle.toml` are silently skipped (same rule as the reload path — operator workspaces, hidden dirs).

**Errors:**

| Code | HTTP | When |
|------|------|------|
| `scope_required` | 403 | Caller's API key scope is not `Management`. Enforced by `scope_guard_middleware`. Gate 7q.5 pins this. |
| `config_error` | 503-class (400 on trunk wire) | `state.workbench.descriptor_catalog` is `None`. Defensive guard — only happens in headless test builds. |
| `config_error` | 503 | `bundles_dir` is NOT configured (deployment is using `catalog_path` or the `seed_p2b` fallback — there's no directory to list). Message embeds the failure mode so the admin UI can surface "configure `[web] bundles_dir` to use this endpoint". |
| `config_error` | 500 | One of the `<subdir>/bundle.toml` files failed to read or parse. Same error envelope as the reload endpoint's TOML parse failure — error message embeds the file path and the serde / IO error. The endpoint is read-only, so nothing has been swapped; fix the bad TOML and retry. |

E2E Gate 7q.4 pins the response shape and asserts the first-party `gadgetron-core` bundle is enumerated with `action_count == 5`. Gate 7q.5 pins the RBAC contract.

---

### POST /api/v1/web/workbench/admin/bundles

Install a new bundle manifest into the configured `[web] bundles_dir`. Landed in ISSUE 10 TASK 10.2 / v0.4.10 (PR #224). **Does NOT swap the live catalog** — the handler writes the TOML to disk but leaves `Arc<ArcSwap<CatalogSnapshot>>` untouched. Operators activate the new bundle by triggering `POST /admin/reload-catalog` or `SIGHUP` when ready; the `reload_hint` field in the response points at the reload endpoint explicitly.

**Scope:** `Management` (admin sub-tree). An OpenAiCompat key returns 403 `scope_required`.

**Request body:**
```json
{
  "bundle_toml": "[bundle]\nid = \"acme-ops\"\nversion = \"1.2.0\"\n\n[[actions]]\n...",
  "signature_hex": "9a3f…ed25519 signature over the bundle_toml string, hex-encoded"
}
```

- `bundle_toml` — a complete manifest as a single string (not a filesystem reference). The handler parses it to verify schema + extract the id, then writes the string verbatim to disk.
- `signature_hex` (optional, ISSUE 10 TASK 10.4 / v0.4.12 / PR #227) — hex-encoded Ed25519 signature over the raw `bundle_toml` string. Verified against `[web.bundle_signing].public_keys_hex` trust anchors BEFORE the TOML is parsed, so a signed-malformed manifest takes the same error path as an unsigned-malformed one (no "which signer did you claim?" leak via error text). Required when `[web.bundle_signing].require_signature = true`; otherwise unsigned installs still work for backwards compatibility with TASK 10.2 deployments that haven't rotated to signed bundles yet.

**Signature policy matrix (TASK 10.4).** `verify_bundle_signature` runs before TOML parse and enforces all six branches:

| Config | Request | Outcome |
|--------|---------|---------|
| `require_signature = false`, no trust anchors | no `signature_hex` | accept (backwards-compat with TASK 10.2) |
| `require_signature = true`, any config | no `signature_hex` | reject 4xx `Config` — "signature required" |
| trust anchors present | valid sig matching one of the anchors | accept |
| trust anchors present | sig value tampered (verification fails against every anchor) | reject 4xx `Config` — "signature invalid" |
| trust anchors present | sig formatted correctly but from a pubkey not in anchors | reject 4xx `Config` — "signature from unknown signer" |
| no trust anchors, `signature_hex` present | (trust set is empty so nothing to verify against) | reject 4xx `Config` — "signature provided but no trust anchors configured" |

The last branch is loud-fail by design — silently accepting a signed request when trust anchors are missing would let a misconfigured deployment trust any signer. Operators who want unsigned installs should leave `signature_hex` empty AND keep `require_signature = false`; operators who want signed installs MUST configure trust anchors.

**Response (HTTP 200):**
```json
{
  "installed": true,
  "bundle_id": "acme-ops",
  "manifest_path": "/etc/gadgetron/bundles/acme-ops/bundle.toml",
  "reload_hint": "POST /api/v1/web/workbench/admin/reload-catalog to activate"
}
```

- `installed` — always `true` on 200. Clients key observability on this flag (same pattern as `reloaded` on reload-catalog).
- `bundle_id` — echoed id extracted from the `[bundle]` table. Admin UIs use this to confirm the manifest they sent was the one the server parsed.
- `manifest_path` — absolute path of the file the handler wrote. Typically `{bundles_dir}/{id}/bundle.toml`. Operators can `cat` this to verify contents match what they sent.
- `reload_hint` — human-readable reminder that installing does NOT activate; caller must call the reload endpoint (or send SIGHUP) next.

**Security invariant — path-traversal safe.** The `[bundle]` id must match `^[a-zA-Z0-9_-]{1,64}$`. `validate_bundle_id()` centralizes this check and every filesystem-touching path (install + uninstall) goes through it. Ids containing `..`, `/`, leading dot, or any other character outside the regex are rejected with 4xx `Config` BEFORE any disk write. Gate 7q.7 pins this: a request with `id = "../etc/passwd"` must return 4xx.

**Collision policy — no silent overwrites.** Installing over an existing id is a hard `Config` error (4xx). The handler checks `{bundles_dir}/{id}/` first and bails if it exists. Operators must `DELETE /admin/bundles/{bundle_id}` before reinstalling. This matches the `from_bundle_dir` duplicate-id hard-error semantics — the marketplace never silently prefers one version over another.

**Errors:**

| Code | HTTP | When |
|------|------|------|
| `scope_required` | 403 | Caller's API key scope is not `Management`. |
| `config_error` | 503 | `bundles_dir` is not configured in `[web]`. |
| `config_error` | 4xx | Request body missing `bundle_toml` / not JSON / TOML parse failure / missing `[bundle]` table / missing `id` / missing `version` / id fails the `[a-zA-Z0-9_-]{1,64}` regex / bundle id already exists under `bundles_dir`. Error message distinguishes the cause so admin UIs can present actionable feedback. |
| `config_error` | 500 | Filesystem write failure (disk full, permission denied, parent-dir IO error). The handler does not leave partial state — if the `bundle.toml` write fails, `mkdir` is rolled back where possible. |

E2E Gate 7q.6 installs a dummy bundle and verifies it appears in the next `GET /admin/bundles` call. Gate 7q.7 pins the path-traversal rejection.

---

### DELETE /api/v1/web/workbench/admin/bundles/{bundle_id}

Remove an installed bundle. Landed in ISSUE 10 TASK 10.2 / v0.4.10 (PR #224). **Does NOT swap the live catalog** — same contract as install. Operator triggers reload to drop the removed bundle from the merged catalog.

**Scope:** `Management`.

**Path parameter:** `bundle_id` — must match `^[a-zA-Z0-9_-]{1,64}$` (same regex as install; rejected BEFORE any filesystem access).

**Request body:** none.

**Example:**
```bash
curl -fsS -X DELETE \
  -H "Authorization: Bearer $MGMT_KEY" \
  http://localhost:8080/api/v1/web/workbench/admin/bundles/acme-ops
```

**Response (HTTP 200):**
```json
{
  "uninstalled": true,
  "bundle_id": "acme-ops",
  "manifest_path": "/etc/gadgetron/bundles/acme-ops/bundle.toml",
  "reload_hint": "POST /api/v1/web/workbench/admin/reload-catalog to drop from live catalog"
}
```

`manifest_path` is the path that no longer exists (the directory was removed via `std::fs::remove_dir_all`).

**Errors:**

| Code | HTTP | When |
|------|------|------|
| `scope_required` | 403 | Caller's API key scope is not `Management`. |
| `config_error` | 503 | `bundles_dir` is not configured. |
| `config_error` | 4xx | `bundle_id` path parameter fails the id regex (path-traversal attempt / empty / too long / contains `/` or `..`). Rejected BEFORE touching disk. |
| `config_error` | 404 | No `{bundles_dir}/{bundle_id}/` directory exists. Idempotent-friendly: a second DELETE of the same id surfaces this 404 so scripts can treat it as "already gone". |
| `config_error` | 500 | `remove_dir_all` failure (permission denied, disk I/O error). Partial-removal state is possible on filesystem failure; a retry after fixing the underlying issue is safe because install prevents re-creating over an existing id. |

E2E Gate 7q.8 uninstalls a previously-installed bundle and verifies it no longer appears in `GET /admin/bundles`.

**Compose with reload.** Install + uninstall both leave the live `CatalogSnapshot` untouched. The typical operator workflow is: (1) `POST /admin/bundles` to stage, (2) `DELETE /admin/bundles/{old}` if replacing, (3) `POST /admin/reload-catalog` (or `kill -HUP <pid>`) to activate. This keeps the "snapshot swap" moment explicit — an accidental install never changes behaviour until the operator decides to reload.

#### Bundle operator recipes (install → activate → verify → rollback)

**Deploy a new bundle to production** (composite 5-step flow):

```sh
MGMT_KEY="gad_live_your_management_key"
GAD="http://localhost:8080"

# Stage 1: Install the manifest. Writes to disk, does NOT swap live catalog.
BUNDLE_TOML=$(cat my-bundle.toml)
curl -fsS -X POST "$GAD/api/v1/web/workbench/admin/bundles" \
  -H "Authorization: Bearer $MGMT_KEY" \
  -H 'Content-Type: application/json' \
  -d "$(jq -n --arg t "$BUNDLE_TOML" '{bundle_toml: $t}')" | jq .
# Response: {"installed":true,"bundle_id":"acme-ops","manifest_path":"...","reload_hint":"..."}

# Stage 2: Verify the new bundle is on disk AND the discovery endpoint sees it.
# (Optional — the install response already confirms manifest_path, but this
#  cross-checks that `GET /admin/bundles` discovery agrees, catching a
#  `bundles_dir` misconfiguration that would leave the file lying unfound.)
curl -fsS "$GAD/api/v1/web/workbench/admin/bundles" \
  -H "Authorization: Bearer $MGMT_KEY" \
  | jq '.bundles[] | select(.bundle.id == "acme-ops")'

# Stage 3: Activate via reload. The live CatalogSnapshot swaps atomically.
curl -fsS -X POST "$GAD/api/v1/web/workbench/admin/reload-catalog" \
  -H "Authorization: Bearer $MGMT_KEY" | jq .
# Response: {"reloaded":true,"action_count":N,"view_count":M,"source":"bundles_dir",
#            "source_path":null,"bundles":[{id:"gadgetron-core",...},{id:"acme-ops",...}]}

# Stage 4: Verify live catalog includes the new bundle's actions.
# Cross-check reload response `action_count` against the live /actions listing
# (same pattern as harness Gate 7q.1 — proves catalog + validators published
#  together, no "swap happened but read path sees old pointer" regression).
ACTIONS_FROM_RELOAD=$(curl -fsS -X POST "$GAD/api/v1/web/workbench/admin/reload-catalog" \
  -H "Authorization: Bearer $MGMT_KEY" | jq .action_count)
ACTIONS_LIVE=$(curl -fsS "$GAD/api/v1/web/workbench/actions" \
  -H "Authorization: Bearer $MGMT_KEY" | jq 'length')
[ "$ACTIONS_FROM_RELOAD" = "$ACTIONS_LIVE" ] && echo "OK: catalog + read path in sync"

# Stage 5 (if broken): Rollback by uninstall + reload.
curl -fsS -X DELETE "$GAD/api/v1/web/workbench/admin/bundles/acme-ops" \
  -H "Authorization: Bearer $MGMT_KEY"
curl -fsS -X POST "$GAD/api/v1/web/workbench/admin/reload-catalog" \
  -H "Authorization: Bearer $MGMT_KEY" | jq .action_count
# action_count returns to the pre-install baseline.
```

**Install a signed bundle** (when `[web.bundle_signing].require_signature = true`, see [`configuration.md §[web.bundle_signing]`](configuration.md#web-bundle_signing)):

```sh
BUNDLE_FILE=my-bundle.toml
PRIV_KEY=signer.ed25519.key   # 32-byte raw Ed25519 private key

# Sign the manifest (openssl or any Ed25519 library; the signature is over
# the raw TOML bytes, NOT the JSON-wrapped request body)
SIG_HEX=$(openssl pkeyutl -sign -inkey <(cat "$PRIV_KEY") \
  -rawin -in "$BUNDLE_FILE" | xxd -p -c 9999)

# Install with the signature attached
curl -fsS -X POST "$GAD/api/v1/web/workbench/admin/bundles" \
  -H "Authorization: Bearer $MGMT_KEY" \
  -H 'Content-Type: application/json' \
  -d "$(jq -n --arg t "$(cat $BUNDLE_FILE)" --arg s "$SIG_HEX" \
    '{bundle_toml: $t, signature_hex: $s}')" | jq .
```

See the TASK 10.4 signature-policy matrix (6 branches) in this section above for when a signed install is accepted vs rejected.

**Key invariants pinned by harness 7q gates**:

- `reloaded == true` AND `action_count == GET /actions.length` (Gate 7q.1) — catalog + read path published together.
- `.bundles[0].id == "gadgetron-core"` on `bundles_dir`-sourced reloads (Gate 7q.3) — first-party bundle rename trips the gate.
- `GET /admin/bundles` `.bundles[0].action_count == 5` against the harness fixture (Gate 7q.4) — seed action-set drift detected.
- `OpenAiCompat` → `403 scope_required` on every `/admin/*` endpoint (Gates 7q.2 / 7q.5) — scope isolation from workbench subtree.
- Install with `id = "../etc/passwd"` → `4xx config_error` BEFORE any disk write (Gate 7q.7) — `validate_bundle_id()` path-traversal guard.

---

### GET /api/v1/web/workbench/admin/billing/events

Operator-scoped read over the append-only `billing_events` ledger written by `PgQuotaEnforcer::record_post` alongside the `quota_configs` counter UPDATE. Landed in **ISSUE 12 TASK 12.1 / v0.5.5 (PR #236)** as the first half of the integer-cent billing pipeline (remaining TASKs materialize invoices, reconcile counter-vs-ledger drift, and wire Stripe ingest). Handler: `crates/gadgetron-gateway/src/web/workbench.rs::list_billing_events`; query: `crates/gadgetron-xaas/src/billing/events.rs::query_billing_events`; migration: `crates/gadgetron-xaas/migrations/20260420000002_billing_events.sql`.

**Scope:** `Management`. `OpenAiCompat` keys get `403 scope_required` via `scope_guard_middleware` — tenants do **not** read their own billing rows through this endpoint (use `GET /quota/status` for tenant-facing usage introspection, which is OpenAiCompat-scoped).

**Tenant boundary.** The handler WHERE-pins `tenant_id` to `ctx.tenant_id` before SQL dispatch. There is no `tenant_id` query parameter; cross-tenant reads are impossible regardless of what the caller sends. A Management key for tenant A reading this endpoint sees only tenant A's ledger rows.

**Query parameters** (all optional):

| Name | Type | Default | Meaning |
|------|------|---------|---------|
| `since` | ISO-8601 timestamp | unbounded | Lower bound on `created_at`. Strict `>` comparison (supply the last row's `created_at` to page forward without re-reading it). |
| `limit` | integer | `100` | Clamped to `1..=500` at the handler. Values outside this range silently clip to the nearest bound. |

Rows are returned newest-first (`ORDER BY created_at DESC, id DESC`), so the natural "tail of the ledger" cursor is the last-returned `created_at`.

**Example:**
```bash
curl -fsS \
  -H "Authorization: Bearer $MGMT_KEY" \
  "http://localhost:8080/api/v1/web/workbench/admin/billing/events?limit=5"
```

**Response (HTTP 200):**
```json
{
  "events": [
    {
      "id": 42,
      "tenant_id": "018fa1a2-…-uuid",
      "event_kind": "chat",
      "source_event_id": null,
      "cost_cents": 17,
      "model": null,
      "provider": null,
      "actor_user_id": null,
      "created_at": "2026-04-20T18:12:03.441Z"
    }
  ],
  "returned": 1
}
```

- `id` is `BIGSERIAL` — strictly increasing per-row, safe as a tie-breaker cursor when `created_at` collides.
- `event_kind` is one of `"chat" | "tool" | "action"` (CHECK constraint at the DB layer; new kinds require a migration). All three emitters now ship — chat from `PgQuotaEnforcer::record_post` (TASK 12.1), tool from `/v1/tools/{name}/invoke` success path (TASK 12.2), action from workbench direct-action + approved-action success terminals (TASK 12.2).
- `source_event_id`: currently populated **only on `action` rows** (carries the `audit_event_id` from `action_audit_events`) so invoice materialization can join ledger → action audit for line-item explanations. `chat` + `tool` rows leave it `null` today — `chat` because the enforcer emits before an audit UUID is generated, `tool` because the tool-dispatch path hasn't been threaded yet. FK-less per the Stripe-style writer-independence pattern — the ledger writer never blocks on whether the source event has been persisted.
- `cost_cents` is the `actual_cost_cents` the enforcer already stored into `quota_configs.daily_used_cents` for `chat` rows (integer cents end-to-end per ADR-D-8). `tool` + `action` rows carry `cost_cents = 0` — emission exists for audit-trail completeness, monetary cost is only assigned at the chat terminal today. Per-action / per-tool pricing attribution lands with TASKs 12.3 (invoice materialization) / ISSUE 13 (HuggingFace catalog) — both DEFERRED as commercialization layer.
- `model` / `provider`: **repurposed by `event_kind`**. For `chat` rows both fields are currently `null` (threading chat model + provider onto the enforcer surface is a future follow-up). For `tool` rows `model` carries the gadget name invoked (e.g., `"wiki.search"`) and `provider` stays `null`. For `action` rows `model` carries the workbench action id (e.g., `"knowledge-search"`) and `provider` stays `null`. Operator queries that want "which gadget generated this tool-event" read `model` on kind=tool rows.
- `actor_user_id` (ISSUE 23 / PR #271 / v0.5.15): nullable UUID, populated on `tool` + `action` rows from the invoking `TenantContext.actor_user_id` (which flows from `ValidatedKey.user_id` per ISSUE 17). `chat` rows currently leave it `null` — the enforcer emit-path hasn't been widened to carry user_id yet; ISSUE 24 will thread user_id through `QuotaToken` + `QuotaEnforcer` trait. Legacy tool/action traffic predating PR #271 also surfaces `null`. Index `(actor_user_id, created_at DESC)` supports per-user spend-report queries without a cross-table join through audit. FK intentionally skipped (heterogeneous callers; best-effort telemetry); `LEFT JOIN users u ON u.id = be.actor_user_id` at read time.

**Errors:**

| Code | HTTP | When |
|------|------|------|
| `scope_required` | 403 | Caller's API key scope is not `Management`. Emitted by `scope_guard_middleware` before the handler runs. |
| `config_error` | 503 | `pg_pool` is not wired (`--no-db` or missing `database_url`). The ledger has no storage layer to read from. |
| `internal` | 500 | Underlying SQL error (connection exhaustion, migration not applied, etc.). Error body shape follows the OpenAI-compat `{error: {message, type, code}}` contract. |

Malformed `since` (non-ISO-8601) surfaces as an axum 400 from the query deserializer before hitting the handler.

**Reconciliation model.** The ledger is the authoritative spend record; `quota_configs.daily_used_cents` is a fast counter. If an INSERT fails (write amplification, pool saturation) the counter is already incremented and the warn log is the only trace:

```
WARN billing tenant_id=... error=...
  failed to persist billing_events row — counter ahead of ledger until reconciled
```

TASK 12.4 will land the reconciliation pass (scan `quota_configs` vs `SUM(cost_cents) GROUP BY tenant_id, usage_day` and emit either corrective INSERTs or operator-visible drift alerts). Until then, operator-visible divergence between `/quota/status` remaining-cents and `/admin/billing/events` sum-of-cost-cents is expected when the ledger falls behind.

**Harness coverage.** Gate 7k.6 runs a non-streaming chat completion, waits briefly, then polls `/admin/billing/events?limit=5` and asserts `.events[] | select(.event_kind == "chat") | length >= 1` — proving the `record_post` → `insert_billing_event` write path actually persists under the harness's default `PgQuotaEnforcer` configuration.

**Design reference.** Full STRIDE threat model, SQL schema, trait signatures, and reconciliation plan for TASK 12.4: [`docs/design/xaas/phase2-billing.md`](../design/xaas/phase2-billing.md).

#### Billing query operator recipes

**Paginate through a tenant's full ledger** (reconciliation / export use case):

```sh
MGMT_KEY="gad_live_your_management_key"
GAD="http://localhost:8080"

# Cold start — newest 500 rows
BATCH=$(curl -fsS \
  -H "Authorization: Bearer $MGMT_KEY" \
  "$GAD/api/v1/web/workbench/admin/billing/events?limit=500")
echo "$BATCH" | jq '.events'

# Subsequent pages — use the OLDEST created_at in the batch as the exclusive
# upper bound. Since the endpoint filters on `created_at > since` (strictly
# greater), we iterate backwards by querying with `since=<second-oldest>`
# WARNING: the current endpoint only supports `since` (lower bound). Full
# backwards pagination needs a `before` (upper bound) parameter — TASK 12.4
# follow-up. For now, operators who need historical snapshot dumps should
# SQL directly against `billing_events` ORDER BY created_at ASC.
```

**Forward-tail pagination** (watch new events land — e.g., a dashboard updating every 30s):

```sh
# Persist the last-seen created_at between polls
STATE_FILE=/tmp/billing-cursor
[ -f "$STATE_FILE" ] && LAST_SEEN=$(cat "$STATE_FILE") || LAST_SEEN="2026-01-01T00:00:00Z"

RESP=$(curl -fsS \
  -H "Authorization: Bearer $MGMT_KEY" \
  "$GAD/api/v1/web/workbench/admin/billing/events?since=$LAST_SEEN&limit=100")

# Update cursor to the NEWEST created_at in the response
NEW_LAST=$(echo "$RESP" | jq -r '.events[0].created_at // empty')
[ -n "$NEW_LAST" ] && echo "$NEW_LAST" > "$STATE_FILE"

echo "$RESP" | jq '.events | map(select(.event_kind == "chat")) | length'
```

**Audit ↔ ledger join for `action` rows** (line-item explanation for workbench actions):

```sh
TENANT_ID="018fa1a2-..."

# 1. Pull recent action rows from billing
ACTION_ROWS=$(curl -fsS \
  -H "Authorization: Bearer $MGMT_KEY" \
  "$GAD/api/v1/web/workbench/admin/billing/events?limit=50" \
  | jq '[.events[] | select(.event_kind == "action")]')

# 2. For each action row, source_event_id is the audit_log UUID.
#    Cross-ref via /audit/events (same tenant, OpenAiCompat scope works):
for AUDIT_ID in $(echo "$ACTION_ROWS" | jq -r '.[].source_event_id'); do
  curl -fsS \
    -H "Authorization: Bearer $MGMT_KEY" \
    "$GAD/api/v1/web/workbench/audit/events?action_id=knowledge-search" \
    | jq --arg id "$AUDIT_ID" '.events[] | select(.request_id == $id)'
done
```

**Per-user spend report** (ISSUE 23 / PR #271 / v0.5.15 — `actor_user_id` column). Covers tool + action usage per-user today; chat rows will populate once ISSUE 24 widens `QuotaToken`:

```sql
-- Monthly tool/action events by user for a tenant
SELECT
  date_trunc('month', be.created_at) AS month,
  be.actor_user_id,
  u.email,
  be.event_kind,
  COUNT(*) AS events,
  SUM(be.cost_cents) AS cents
FROM billing_events be
LEFT JOIN users u ON u.id = be.actor_user_id
WHERE be.tenant_id = 'your-tenant-uuid'
  AND be.created_at >= date_trunc('month', now()) - interval '3 months'
  AND be.actor_user_id IS NOT NULL
GROUP BY 1, 2, 3, 4
ORDER BY 1 DESC, cents DESC;

-- Tool events attributed to a specific user (last 7 days)
SELECT event_kind, model AS gadget_or_action, COUNT(*) AS events, created_at::date AS day
FROM billing_events
WHERE tenant_id = 'your-tenant-uuid'
  AND actor_user_id = '<alice-uuid>'
  AND created_at >= now() - interval '7 days'
  AND event_kind IN ('tool', 'action')
GROUP BY event_kind, model, day
ORDER BY day DESC, events DESC;
```

The `LEFT JOIN users` keeps rows where `actor_user_id` has been soft-deleted (email surfaces as `NULL` but the event count + total still count against historical reporting). Chat rows carry `actor_user_id = NULL` today; add `OR event_kind = 'chat'` and a separate join through `audit_log.actor_user_id` if you need to include chat in the report before ISSUE 24 lands.

**Per-kind aggregation** (daily rollup — SQL directly, faster than HTTP for bulk):

```sql
-- Monthly spend by event kind for a tenant
SELECT
  date_trunc('month', created_at) AS month,
  event_kind,
  COUNT(*) AS events,
  SUM(cost_cents) AS cents,
  SUM(cost_cents) / 100.0 AS dollars
FROM billing_events
WHERE tenant_id = 'your-tenant-uuid'
  AND created_at >= date_trunc('month', now()) - interval '3 months'
GROUP BY 1, 2
ORDER BY 1 DESC, 2;

-- Counter-vs-ledger divergence check (sanity test for TASK 12.4 reconciliation):
SELECT
  qc.tenant_id,
  qc.daily_used_cents AS counter_cents,
  COALESCE(SUM(be.cost_cents), 0) AS ledger_cents,
  qc.daily_used_cents - COALESCE(SUM(be.cost_cents), 0) AS drift_cents
FROM quota_configs qc
LEFT JOIN billing_events be
  ON be.tenant_id = qc.tenant_id
 AND be.created_at >= qc.usage_day::timestamp
 AND be.created_at < qc.usage_day::timestamp + interval '1 day'
 AND be.event_kind = 'chat'
WHERE qc.usage_day = CURRENT_DATE
GROUP BY qc.tenant_id, qc.daily_used_cents;
```

Non-zero `drift_cents` is expected when the ledger INSERT failed but the counter UPDATE succeeded (see "Reconciliation model" above). TASK 12.4 will land an operator-visible drift alert for positive drift above a threshold.

---

### GET /api/v1/web/workbench/admin/audit/log

Management-scoped read over tenant-pinned `audit_log` rows, shipped in **ISSUE 22 TASK 22.1 / PR #269 / v0.5.14**; handler: `list_audit_log_handler` in `crates/gadgetron-gateway/src/web/workbench.rs:1036`.

**Scope:** `Management`. `OpenAiCompat` keys get `403 forbidden` (type `permission_error`) via `scope_guard_middleware`, so `/api/v1/web/workbench/admin/audit/log` is an operator endpoint, not a tenant self-service endpoint.

**Tenant boundary.** The handler always WHERE-pins `tenant_id` to `ctx.tenant_id` before calling the query helper. There is no `tenant_id` query parameter, so the caller cannot widen or override the tenant scope from the URL. Cross-tenant reads are impossible by construction. A Management key for tenant A sees only tenant A rows, even if the operator knows another tenant UUID.

**Query parameters** (all optional):

| Name | Type | Default | Meaning |
|------|------|---------|---------|
| `actor_user_id` | UUID | unbounded | Narrow to rows attributed to a single user. Useful when a tenant has multiple admins or mixed cookie-session and Bearer traffic. |
| `since` | ISO-8601 timestamp | unbounded | Lower bound on `timestamp` (`>=` semantics, inclusive). Paging forward with the last-seen timestamp will re-read the boundary row; use `id` or `request_id` client-side for dedup when paginating through bursty traffic. |
| `limit` | integer | `100` | Clamped to `1..=500` at the handler. Values below `1` clip to `1`, values above `500` clip to `500`. |

Rows are returned newest-first. The response shape is `{rows: Vec<AuditLogRow>, returned: N}` where `returned` is the number of rows actually emitted after filters and limit clamp.

Implementation note: `query_audit_log(pool, tenant_id, actor_user_id, since, limit)` lives in `crates/gadgetron-xaas/src/audit/writer.rs`. It uses `sqlx::QueryBuilder` to assemble the WHERE clause from compile-time SQL literals plus `push_bind` for every user-controllable value (`tenant_id`, `actor_user_id`, `since`, `limit`). Placeholders are allocated positionally by the builder — no manual `$N` indexing — so counter drift on future filters is structurally impossible. `QueryBuilder` also forbids `format!` on the fragment stream, so splicing a value directly into the SQL string is compile-time prevented. The tenant pin is unconditional (first bind). Earlier releases used four explicit prepared-statement shapes, one per `(actor_user_id, since)` permutation; the builder form was greenlit by security-compliance-lead review for refactor-cycle #3 because no SQLi property was lost.

**Example:**
```bash
curl -fsS \
  -H "Authorization: Bearer $MGMT_KEY" \
  "http://localhost:8080/api/v1/web/workbench/admin/audit/log?limit=5"
```

**Response (HTTP 200):**
```json
{
  "rows": [
    {
      "id": "f4a88a62-7d89-4e28-b5b9-0b59b31b2d2c",
      "tenant_id": "4c7b47aa-7284-4658-86a4-831828f91f1f",
      "api_key_id": "00000000-0000-0000-0000-000000000000",
      "actor_user_id": "e61d1784-9cc3-46c8-aab0-5a14d7fb0f16",
      "actor_api_key_id": null,
      "request_id": "7dba0b98-39aa-45f3-9c07-f9fb47f43795",
      "model": "gpt-4o-mini",
      "provider": "openai",
      "status": "ok",
      "input_tokens": 182,
      "output_tokens": 49,
      "cost_cents": 7,
      "latency_ms": 421,
      "timestamp": "2026-04-19T09:12:03.441Z"
    }
  ],
  "returned": 1
}
```

- `rows` is an array of `AuditLogRow` projections. The handler emits the newest rows first, so `rows[0]` is the latest matching row at read time.
- `returned` is the actual row count in this response. It can be smaller than `limit` when the tenant has fewer matching rows or when the filter narrows the result set.
- `id` is the audit row UUID primary key. It identifies the persisted audit record itself, not the HTTP request lifecycle as a whole.
- `tenant_id` is the tenant foreign key pinned by the handler. It is present in every row, but the caller cannot change it through the query string.
- `api_key_id` is never `null`. This field records the request-path credential marker that entered the audit pipeline.
- `api_key_id = 00000000-0000-0000-0000-000000000000` is the cookie-session sentinel from ISSUE 16. It means the request came through the session-cookie path, not a real Bearer API key row.
- `api_key_id` holding any non-nil UUID means the row came from a Bearer credential path. For those rows, the UUID is the request credential recorded at emit time.
- `actor_user_id` is the owning user UUID when the request identity was plumbed through `ValidatedKey.user_id`. This is the main field to use when you want "activity by user" rather than "activity by key".
- `actor_user_id = null` is expected for legacy API keys that predate the ISSUE 14 TASK 14.1 ownership backfill. It does not mean the audit row is malformed.
- `actor_api_key_id` is the real `api_keys.id` for Bearer callers when the key survived validation and the tenant context preserved the concrete key identity.
- `actor_api_key_id = null` is expected for cookie-session callers. `tenant_context_middleware` converts the nil-sentinel cookie path into `None` here, so operators do not have to string-match on the sentinel to understand the caller type.
- `api_key_id` and `actor_api_key_id` intentionally differ. `api_key_id` is the path marker, always present, while `actor_api_key_id` is the real persisted API key owner handle and is absent on cookie-session traffic.
- `api_key_id = 00000000-0000-0000-0000-000000000000` plus `actor_api_key_id = null` is the normal cookie-session combination.
- `api_key_id != 00000000-0000-0000-0000-000000000000` plus non-null `actor_api_key_id` is the normal Bearer combination.
- `request_id` is the HTTP request correlation UUID. Multiple audit rows can share one `request_id`, so use `id` when you need the audit row identity and `request_id` when you need to correlate related request work.
- `model` is the resolved model id when the write path knew it, for example `gpt-4o-mini`. It can be `null` when the upstream emit site did not set a model.
- `provider` is the resolved provider string when available, for example `openai`. It can be `null` for rows emitted by code paths that did not populate provider metadata.
- `status` is the write-side status string persisted by the audit writer. As of v0.5.14 the common values are `ok`, `error`, and `stream_interrupted`.
- `input_tokens` is the integer input token count persisted for the audited request.
- `output_tokens` is the integer output token count persisted for the audited request.
- `cost_cents` is the integer-cent cost attributed to the audited request at write time. This is the same unit used elsewhere in the XaaS billing and quota surfaces.
- `latency_ms` is the request latency in milliseconds as recorded by the write path.
- `timestamp` is the UTC write timestamp used for newest-first ordering and `since` filtering.

**Errors:**

| Code | HTTP | When |
|------|------|------|
| `forbidden` | 403 | Caller's API key scope is not `Management`. `OpenAiCompat` callers fail in `scope_guard_middleware` before the handler runs; response `type` is `permission_error`. |
| `config_error` | 400 | Either (a) `pg_pool` is not wired (for example `--no-db` mode or missing `database_url`) — the handler calls `require_pg_pool(&state, "admin audit log query")` and fails before any SQL executes; or (b) SQL execution failed and the handler mapped the `sqlx` error to `GadgetronError::Config` with message prefix `audit log query: ...`. Both collapse to HTTP 400 because `WorkbenchHttpError::Core(GadgetronError::Config(_))` → `http_status_code() = 400` (`crates/gadgetron-core/src/error.rs:535`). No-db vs SQL-failure distinguishable by reading the `error.message` body field. |
| `n/a` | 4xx | Axum query deserialization rejected the URL before the handler ran, for example malformed `actor_user_id` (non-UUID) or `since` (non-ISO-8601) or a non-integer `limit`. |

Malformed `actor_user_id` also fails in the query deserializer path because the field is typed as UUID, but the common operator mistakes seen during manual testing are malformed `since` and non-integer `limit`.

#### Operator recipes

**Who accessed this tenant in the last hour?**

Use `since` to narrow the window, then extract unique `actor_user_id` values from the returned rows. This shows known user identities; legacy-key rows with `actor_user_id = null` are omitted by the `jq` filter.

```bash
GAD="http://localhost:8080"
MGMT_KEY="gad_live_your_management_key"
SINCE=$(
  python3 - <<'PY'
from datetime import datetime, timedelta, timezone
print((datetime.now(timezone.utc) - timedelta(hours=1)).replace(microsecond=0).isoformat().replace("+00:00", "Z"))
PY
)

curl -fsS \
  -H "Authorization: Bearer $MGMT_KEY" \
  "$GAD/api/v1/web/workbench/admin/audit/log?since=$SINCE&limit=500" \
  | jq -r '.rows[] | .actor_user_id // empty' \
  | sort -u
```

If you also want request counts per user, replace the final `jq` pipeline with:

```bash
curl -fsS \
  -H "Authorization: Bearer $MGMT_KEY" \
  "$GAD/api/v1/web/workbench/admin/audit/log?since=$SINCE&limit=500" \
  | jq -r '.rows[] | .actor_user_id // empty' \
  | sort \
  | uniq -c \
  | sort -nr
```

**All chat completions by alice**

When you already know Alice's user UUID, combine `actor_user_id` and `since` in the same request. `audit_log` is the chat audit persistence surface after ISSUE 21, so this endpoint is the direct operator read path for recent chat completions attributed to that user.

```bash
GAD="http://localhost:8080"
MGMT_KEY="gad_live_your_management_key"
ALICE_USER_ID="e61d1784-9cc3-46c8-aab0-5a14d7fb0f16"
SINCE="2026-04-20T00:00:00Z"

curl -fsS \
  -H "Authorization: Bearer $MGMT_KEY" \
  "$GAD/api/v1/web/workbench/admin/audit/log?actor_user_id=$ALICE_USER_ID&since=$SINCE&limit=500" \
  | jq '.rows[] | {
      timestamp,
      request_id,
      model,
      provider,
      status,
      input_tokens,
      output_tokens,
      cost_cents,
      latency_ms
    }'
```

If you need only successful rows, add `select(.status == "ok")` in the `jq` filter:

```bash
curl -fsS \
  -H "Authorization: Bearer $MGMT_KEY" \
  "$GAD/api/v1/web/workbench/admin/audit/log?actor_user_id=$ALICE_USER_ID&since=$SINCE&limit=500" \
  | jq '.rows[]
        | select(.status == "ok")
        | {timestamp, request_id, model, input_tokens, output_tokens, cost_cents}'
```

**Cross-reference audit_log with billing_events**

For bulk reconciliation or incident response, SQL is faster than paginating both HTTP endpoints. The join key below is `audit_log.request_id = billing_events.source_event_id`, which lets you line up request-correlated audit rows with any billing rows that carried the same source event UUID.

```sql
SELECT
  al.request_id,
  al.timestamp AS audit_timestamp,
  al.actor_user_id,
  al.model AS audit_model,
  al.provider AS audit_provider,
  al.status,
  al.input_tokens,
  al.output_tokens,
  al.cost_cents AS audit_cost_cents,
  be.id AS billing_event_id,
  be.event_kind,
  be.cost_cents AS billing_cost_cents,
  be.created_at AS billing_created_at
FROM audit_log al
JOIN billing_events be
  ON be.source_event_id = al.request_id
WHERE al.tenant_id = '4c7b47aa-7284-4658-86a4-831828f91f1f'
ORDER BY al.timestamp DESC
LIMIT 100;
```

Use `LEFT JOIN` instead of `JOIN` when you want to find audit rows that do not yet have a correlated billing row:

```sql
SELECT
  al.request_id,
  al.timestamp,
  al.status,
  al.cost_cents,
  be.id AS billing_event_id
FROM audit_log al
LEFT JOIN billing_events be
  ON be.source_event_id = al.request_id
WHERE al.tenant_id = '4c7b47aa-7284-4658-86a4-831828f91f1f'
  AND al.timestamp >= now() - interval '1 day'
ORDER BY al.timestamp DESC;
```

**Cookie-vs-Bearer breakdown**

`api_key_id` is the fast discriminator for caller path. Cookie-session rows use the nil sentinel, Bearer rows use any non-nil UUID. `IS DISTINCT FROM` is used below so the Bearer branch stays correct even if future schema changes ever allow nullable intermediate projections.

```sql
SELECT
  'cookie' AS caller_type,
  COUNT(*) AS rows,
  COUNT(actor_user_id) AS rows_with_user_id,
  COUNT(actor_api_key_id) AS rows_with_real_key_id
FROM audit_log
WHERE tenant_id = '4c7b47aa-7284-4658-86a4-831828f91f1f'
  AND api_key_id = '00000000-0000-0000-0000-000000000000'

UNION ALL

SELECT
  'bearer' AS caller_type,
  COUNT(*) AS rows,
  COUNT(actor_user_id) AS rows_with_user_id,
  COUNT(actor_api_key_id) AS rows_with_real_key_id
FROM audit_log
WHERE tenant_id = '4c7b47aa-7284-4658-86a4-831828f91f1f'
  AND api_key_id IS DISTINCT FROM '00000000-0000-0000-0000-000000000000';
```

For a recent-window breakdown instead of all-time tenant history, add a timestamp predicate to both branches:

```sql
AND timestamp >= now() - interval '7 days'
```

**Harness coverage.** Gate 7v.8 covers both scope enforcement and happy-path reads. The harness first lands at least one `audit_log` row through the ISSUE 21 `run_audit_log_writer` path by making an earlier chat completion, then asserts that a Management key gets HTTP 200 with `.rows.length >= 1`. The same gate also calls this endpoint with an `OpenAiCompat` key and expects HTTP 403.

**Design reference.** Identity and audit-field background lives in [`docs/design/phase2/08-identity-and-users.md`](../design/phase2/08-identity-and-users.md) and the shipped version chain is tracked in [`docs/ROADMAP.md`](../ROADMAP.md). Follow-ups that are tracked but not blocking `v1.0.0` include cursor-based pagination for result sets larger than 500 rows, additional filters such as `status`, `model`, and `request_id`, and `billing_events` user-id plumbing for easier cross-surface joins.

After ISSUE 21, `audit_log` is the canonical persistence target for chat audit rows. The write path is `run_audit_log_writer`, which is separate from this read-only endpoint; `GET /api/v1/web/workbench/admin/audit/log` only projects rows that have already been persisted.

---

### Tenant self-service endpoints (ISSUE 14 — v0.5.7)

Landed by PR #246. Spec: [`docs/design/phase2/08-identity-and-users.md`](../design/phase2/08-identity-and-users.md) §2.6.

#### GET /api/v1/web/workbench/admin/users

List users in the caller's tenant. **Management** scope. Tenant boundary pinned by handler.

Response: `{ users: [UserRow...], returned: N }` where `UserRow = { id, tenant_id, email, display_name, role, is_active, created_at, updated_at, last_login_at }`.

#### POST /api/v1/web/workbench/admin/users

Create a user. **Management** scope.

Body: `{ email, display_name, role: "member"|"admin"|"service", password?: string }`.

`service`-role users MUST omit `password` (400 otherwise). `member`/`admin` require a password.

#### DELETE /api/v1/web/workbench/admin/users/{user_id}

Delete a user. **Management** scope. **Single-admin guard**: refuses when the target is the last active admin in the tenant.

#### GET /api/v1/web/workbench/admin/teams

List teams in the caller's tenant. **Management** scope.

#### POST /api/v1/web/workbench/admin/teams

Create a team. Body: `{ id, display_name, description? }`. `id` is kebab-case, max 32 chars, `'admins'` reserved (400 on violation).

#### DELETE /api/v1/web/workbench/admin/teams/{team_id}

Delete a team. Cascade removes `team_members` rows.

#### GET/POST /api/v1/web/workbench/admin/teams/{team_id}/members

List or add members. POST body: `{ user_id, role?: "member"|"lead" }`.

#### DELETE /api/v1/web/workbench/admin/teams/{team_id}/members/{user_id}

Remove a member. Does not delete the user.

#### GET/POST /api/v1/web/workbench/keys

User self-service API keys. **OpenAiCompat** scope (any authenticated caller). Tenant + user bounded by handler via `caller_user_id` lookup.

`GET`: list keys owned by the calling user.
`POST`: create a new key. Body: `{ label?, scopes?: ["openai_compat"|"management"|...], kind?: "live"|"test" }`. **Scope narrowing**: requested scopes MUST be a subset of caller's own scopes (400 otherwise). Response includes `raw_key` EXACTLY ONCE.

#### DELETE /api/v1/web/workbench/keys/{key_id}

Revoke a key owned by the caller. Idempotent (re-revoke returns 200).

#### Operator recipes

**Provision a new member user** (Management-scope caller):

```sh
MANAGEMENT_KEY="gad_live_your_admin_key"

# 1. Create the user
curl -sS -X POST http://localhost:8080/api/v1/web/workbench/admin/users \
  -H "Authorization: Bearer $MANAGEMENT_KEY" \
  -H 'Content-Type: application/json' \
  -d '{
    "email": "alice@example.com",
    "display_name": "Alice",
    "role": "member",
    "password": "alice-initial-password"
  }' | jq .
# Response: {"user":{"id":"<uuid>","tenant_id":"...","email":"alice@example.com",...}}

# 2. Alice can now login via the cookie-session API or mint her own key.
```

**Alice mints her own API key** (OpenAiCompat-scope self-service). She logs in via cookie session first, then POSTs to `/keys` using the cookie:

```sh
# Alice logs in (captures the cookie in a jar)
curl -sS -c /tmp/alice.jar -X POST \
  -H 'Content-Type: application/json' \
  -d '{"email":"alice@example.com","password":"alice-initial-password"}' \
  http://localhost:8080/api/v1/auth/login

# Alice creates a key using her cookie (v0.5.9+ unified middleware accepts cookie on /keys)
curl -sS -b /tmp/alice.jar -X POST \
  -H 'Content-Type: application/json' \
  -d '{"label":"alice-laptop","kind":"live"}' \
  http://localhost:8080/api/v1/web/workbench/keys | jq .
# Response: {"key":{"id":"<uuid>","label":"alice-laptop","kind":"live",...},
#           "raw_key":"gad_live_..."}   # shown ONCE — save it now
```

**Rotate a key** (revoke-old-after-new pattern — avoids a window with no live key):

```sh
OLD_KEY_ID="<uuid>"

# 1. Create the replacement
curl -sS -b /tmp/alice.jar -X POST \
  -H 'Content-Type: application/json' \
  -d '{"label":"alice-laptop-v2"}' \
  http://localhost:8080/api/v1/web/workbench/keys | jq .raw_key
# Save the new raw key. Update the client.

# 2. Verify the client works with the new key. Then revoke the old one:
curl -sS -b /tmp/alice.jar -X DELETE \
  http://localhost:8080/api/v1/web/workbench/keys/$OLD_KEY_ID
# 200 on first call, 200 on re-run (idempotent).
```

**Team membership** (Management-scope caller):

```sh
# Create a team (id must be kebab-case, 'admins' reserved)
curl -sS -X POST http://localhost:8080/api/v1/web/workbench/admin/teams \
  -H "Authorization: Bearer $MANAGEMENT_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"id":"platform","display_name":"Platform Team"}'

# Add Alice to the team
curl -sS -X POST \
  http://localhost:8080/api/v1/web/workbench/admin/teams/platform/members \
  -H "Authorization: Bearer $MANAGEMENT_KEY" \
  -H 'Content-Type: application/json' \
  -d "{\"user_id\":\"$ALICE_UUID\",\"role\":\"member\"}"

# Remove Alice from the team (does NOT delete the user)
curl -sS -X DELETE \
  http://localhost:8080/api/v1/web/workbench/admin/teams/platform/members/$ALICE_UUID \
  -H "Authorization: Bearer $MANAGEMENT_KEY"
```

**Common error shapes**:

| Status | `code` | Trigger |
|--------|--------|---------|
| 400 | `validation_error` | kebab-case regex fail on team id, `'admins'` reserved-name violation, `service` role with password, email format invalid |
| 400 | `scope_narrowing_violation` | POST /keys requested scope exceeds caller's own scopes |
| 403 | `scope_required` | Management-only endpoint called with OpenAiCompat-only key |
| 409 | `single_admin_guard` | DELETE /admin/users on the last remaining active admin in the tenant |
| 4xx | `cross_tenant_rejected` | POST /admin/teams/{id}/members with a user_id from a different tenant |

---

### Cookie-session endpoints (ISSUE 15 — v0.5.8)

Landed by PR #248. Mounted on **public** routes (no Bearer auth); each handler self-authenticates. Spec: [`docs/design/phase2/08-identity-and-users.md`](../design/phase2/08-identity-and-users.md) §2.2.4.

**Related (ISSUE 16 — v0.5.9 / PR #259)**: the three endpoints below are the only **public** cookie routes. As of v0.5.9 the `auth_middleware` on every OTHER protected route (`/v1/*`, `/api/v1/web/workbench/*`, `/api/v1/xaas/*`) also accepts the `gadgetron_session` cookie as a fallback when no `Authorization: Bearer` header is present — scope is synthesized from the user's `role` (admin → `[OpenAiCompat, Management]`; member → `[OpenAiCompat]`). See [auth.md §Cookie-session auth](auth.md#cookie-session-auth-issue-15-task-151--v058--issue-16-task-161--v059) for the full fallback chain.

#### POST /api/v1/auth/login

Body: `{ email, password }`. On success returns 200 + `Set-Cookie: gadgetron_session=<token>; HttpOnly; SameSite=Lax; Path=/; Max-Age=86400` + body `{ session_id, user_id, tenant_id, expires_at }`. On invalid credentials returns 401 `{ error: { code: "invalid_credentials" } }`. Service-role users are rejected.

Password verification uses argon2id via the `argon2` crate. The session cookie token is 32 random bytes hex-encoded; only its SHA-256 hash is stored server-side.

**Secure flag**: operator terminates TLS at the proxy so the cookie travels inside the secure tunnel; gateway does NOT emit `Secure` so loopback development works.

#### GET /api/v1/auth/whoami

Reads the `gadgetron_session` cookie and returns `{ session_id, user_id, tenant_id, expires_at }`. Touches `last_active_at` for idle-rotation tracking. 401 on missing/expired/revoked.

#### POST /api/v1/auth/logout

Revokes the session (idempotent). Returns 200 + `Set-Cookie: gadgetron_session=; Max-Age=0` to clear the cookie client-side.

**Harness coverage.** Gate 7v.5 drives login → whoami → bad-password (401) → logout → post-logout-whoami (401).

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

E2E Gate 7h.8 verifies an unfiltered GET returns the rows from prior gates and that `?action_id=wiki-write` narrows server-side (not client-side). The sibling endpoint `GET /audit/tool-events` (below) exposes the other audit plane (Penny tool-calls, `tool_audit_events`) with the same query shape.

---

### GET /api/v1/web/workbench/audit/tool-events

Tenant-scoped read over `tool_audit_events` — the Penny tool-call trail, parallel to `/audit/events` which covers workbench direct-action audit. Landed in ISSUE 5 / v0.2.8 (`crates/gadgetron-gateway/src/web/workbench.rs::list_tool_audit_events`, backed by `crates/gadgetron-xaas/src/audit/tool_event.rs::query_tool_audit_events`). Until PR #199 the sink that emitted these rows was a `NoopGadgetAuditEventSink`, so the table filled only in dev fixtures — this endpoint makes the plane readable now that the Postgres-backed `run_gadget_audit_writer` persists each row.

**Auth:** `OpenAiCompat`.

**Query parameters** (all optional):

| Name | Type | Default | Notes |
|---|---|---|---|
| `tool_name` | string | — | Exact-match filter (e.g. `wiki.write`). No wildcard / prefix matching. |
| `since` | RFC3339 timestamp | — | Inclusive lower bound on `created_at`. |
| `limit` | integer | 100 | Clamped to `[1, 500]`. Out-of-range values are silently clamped. |

**Tenant boundary:** the handler always pins queries to the authenticated actor's `tenant_id`. No cross-tenant read path.

**Response:**
```json
{
  "events": [
    {
      "id": 12345,
      "tool_name": "wiki.write",
      "tier": "write",
      "category": "knowledge",
      "outcome": "success",
      "error_code": null,
      "elapsed_ms": 84,
      "conversation_id": "conv-abc",
      "claude_session_uuid": null,
      "owner_id": null,
      "tenant_id": "manycoresoft",
      "created_at": "2026-04-19T08:04:30.412Z"
    }
  ],
  "returned": 1
}
```

- Rows are ordered `created_at DESC` (newest first).
- `outcome` ∈ `"success"` | `"error"`. Unlike `/audit/events`, there is no `pending_approval` outcome on this plane — approval-flow variants are planned future work on `GadgetAuditEvent` (see ADR-P2A-06 remaining items).
- `error_code` is non-null only when `outcome == "error"`. Value is the short-form `GadgetError::error_code()` string.
- `conversation_id` / `claude_session_uuid` / `owner_id` are nullable — populated by the emit site when a native Claude session is active; P2A/P2B single-user paths leave them NULL.
- `returned` mirrors `events.len()`.

**Errors:**

| Code | HTTP | When |
|---|---|---|
| `config_error` | 400 | `pg_pool` not configured (in-memory / demo mode). The sink falls back to Noop and the query endpoint has nothing to return. |
| `config_error` | 400 | Underlying SQL query failed — message includes the sqlx error. |

E2E Gate 7k.4 covers the shape + clamp contract.

---

### GET /api/v1/web/workbench/usage/summary

Tenant-scoped operations rollup over a sliding time window, aggregating the three audit tables (`audit_log` chat, `action_audit_events` workbench direct actions, `tool_audit_events` Penny tool calls) into a fixed-shape response the `/web/dashboard` UI consumes. Landed in ISSUE 4 / v0.2.7 (`crates/gadgetron-gateway/src/web/workbench.rs::get_usage_summary`).

**Auth:** `OpenAiCompat`.

**Query parameters:**

| Name | Type | Default | Notes |
|---|---|---|---|
| `window_hours` | integer | 24 | Clamped to `[1, 168]` (one week). Out-of-range values are silently clamped. |

**Tenant boundary:** the handler PINS queries to the authenticated actor's `tenant_id`. No cross-tenant read path.

**Response** (fields are fixed, zero when the window has no data so the dashboard renders a stable layout):
```json
{
  "window_hours": 24,
  "chat": {
    "requests": 1200,
    "errors": 3,
    "total_input_tokens": 842000,
    "total_output_tokens": 311000,
    "total_cost_cents": 1250,
    "avg_latency_ms": 412.7
  },
  "actions": {
    "total": 87,
    "success": 83,
    "error": 1,
    "pending_approval": 3,
    "avg_elapsed_ms": 18.4
  },
  "tools": {
    "total": 142,
    "errors": 2
  }
}
```

- `chat.total_cost_cents` is populated by the `gadgetron_core::pricing` table introduced in ISSUE 4 — model pricing drives real integer cents.
- `actions.pending_approval` surfaces the same `pending_approval` rows that appear in `GET /audit/events`; they are not errors — approval resolution happens via `POST /approvals/:id/approve|deny`.

**Errors:**

| Code | HTTP | When |
|---|---|---|
| `config_error` | 400 | `pg_pool` not configured (in-memory / demo mode). |

E2E Gate 7k.3 verifies the response shape (all three sub-objects present, fields populated, `window_hours` echoed).

---

### GET /api/v1/web/workbench/events/ws

WebSocket endpoint — tenant-filtered live activity feed. Subscribers receive `ActivityEvent` JSON frames in real time as the `ActivityBus` publishes them. Shipped publishers today: `ChatCompleted` (ISSUE 4 / PR #194), `ToolCallCompleted` (ISSUE 5 / PR #199 — Penny tool-call trail fans out from the audit writer). ISSUE 6 (PR #201, v0.2.9) added a SIBLING fan-out path — Penny tool calls also produce `CapturedActivityEvent { origin: Penny, kind: GadgetToolCall }` for the durable `/workbench/activity` read path — but those do NOT appear as `/events/ws` frames; they flow through the coordinator capture layer, not the broadcast bus. Landed in ISSUE 4 / v0.2.7 (`crates/gadgetron-gateway/src/web/workbench.rs::events_ws_handler`).

**Auth:** `OpenAiCompat`. The standard `Authorization: Bearer …` header works for WebSocket upgrade requests issued from non-browser clients. **Browser clients** cannot set `Authorization` on WS upgrades — use the **query-token fallback** `?token=gad_live_…` scoped to this route. Middleware strips `?token=` before logging (`crates/gadgetron-gateway/src/middleware/auth.rs`).

**Protocol:** one JSON text frame per event. No framing envelope — each frame is a complete `ActivityEvent`. Example:
```json
{"type":"chat_completed","tenant_id":"…","request_id":"…","model":"gpt-4o-mini","input_tokens":150,"output_tokens":42,"latency_ms":380,"cost_cents":1,"at":"2026-04-19T08:04:30.412Z"}
```

**Lag behavior:** when the server-side broadcast channel lags (subscriber slower than publishers), the server sends a structured lag notice and closes. Clients MUST reconnect + re-sync via `GET /usage/summary`:
```json
{"type":"lag","missed":42,"message":"subscriber lagged; reconnect to resume"}
```
The close happens immediately after the lag frame so silent drops don't mask the problem.

**Tenant filter:** events from other tenants are dropped on the handler side before send — subscribers only see their tenant's events.

**Errors:**

| Code | HTTP | When |
|---|---|---|
| `config_error` | 400 | `activity_bus` / pool not configured. |
| 401 / 403 | — | Auth handled by the standard middleware stack; WS upgrade is rejected with an HTTP response before the protocol switch. |

E2E Gate 11f covers the `/web/dashboard` page that consumes this stream via `?token=` auth.

**Common queries (operator recipes).**

Find every action this tenant ran in the last hour:
```sh
SINCE=$(date -u -v-1H '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null || date -u -d '1 hour ago' '+%Y-%m-%dT%H:%M:%SZ')
curl -s -H "Authorization: Bearer gad_live_…" \
  "http://localhost:8080/api/v1/web/workbench/audit/events?since=${SINCE}&limit=500" | jq '.events'
```

Only failures (useful for triage):
```sh
curl -s -H "Authorization: Bearer gad_live_…" \
  "http://localhost:8080/api/v1/web/workbench/audit/events?limit=200" \
  | jq '.events | map(select(.outcome == "error"))'
```
Server-side filtering on `outcome` is not supported today (no wildcard / `outcome=` param) — the `jq` filter runs client-side over the 200-row window. For large windows fetch with a tighter `since` before piping to `jq`.

Only destructive / approval-gated actions:
```sh
curl -s -H "Authorization: Bearer gad_live_…" \
  "http://localhost:8080/api/v1/web/workbench/audit/events?action_id=wiki-delete&limit=100" | jq '.events'
```

Approve-then-dispatch trace for a specific approval (two rows — one `pending_approval` at step 6, one `success` at post-approve dispatch):
```sh
APPROVAL=...; curl -s -H "Authorization: Bearer gad_live_…" \
  "http://localhost:8080/api/v1/web/workbench/audit/events?action_id=wiki-delete&limit=500" \
  | jq --arg a "$APPROVAL" '.events | map(select(.gadget_name == "wiki.delete"))'
```
Note: the `approval_id` is not a column on `action_audit_events` in v0.2.6; use `gadget_name` + timestamp bracketing to reconstruct the pair for now. A dedicated `approval_id` column is tracked as a follow-up under ADR-P2A-06 item 9 (`args_digest` + `rationale_digest` + approval correlation in audit schema).

Tail on a rolling window (ops dashboard):
```sh
# Every 30 seconds, print anything newer than the last flush.
LAST="2026-04-19T00:00:00Z"
while sleep 30; do
  EVENTS=$(curl -s -H "Authorization: Bearer gad_live_…" \
    "http://localhost:8080/api/v1/web/workbench/audit/events?since=${LAST}&limit=500")
  echo "$EVENTS" | jq -c '.events[]'
  NEWEST=$(echo "$EVENTS" | jq -r '.events[0].created_at // empty')
  [ -n "$NEWEST" ] && LAST="$NEWEST"
done
```
This is a pull-model fallback; the live-feed ISSUE (`ROADMAP.md §ISSUE 4 operator observability`) wires a WebSocket push for the same surface.

---

### GET /api/v1/web/workbench/quota/status

Tenant-scoped quota introspection. Landed in ISSUE 11 TASK 11.4 / v0.5.4 (PR #234) — the ISSUE 11 close. Users check their own daily + monthly spend without needing Management rights; the UI uses this to render the /web quota banner + retry countdown on 429s. Handler: `crates/gadgetron-gateway/src/web/workbench.rs::get_quota_status`.

**Auth:** `OpenAiCompat` — the same scope that owns `/v1/chat/completions`. Tenants reading their own numbers don't need Management scope (that would force an XaaS operator to surface a per-tenant view for end users).

**Query parameters:** none. The handler reads the caller's `tenant_id` out of `TenantContext` — cross-tenant reads aren't reachable from this endpoint by design.

**Response (HTTP 200):**
```json
{
  "usage_day": "2026-04-20",
  "daily": {
    "used_cents": 342,
    "limit_cents": 1000000,
    "remaining_cents": 999658
  },
  "monthly": {
    "used_cents": 15240,
    "limit_cents": 10000000,
    "remaining_cents": 9984760
  }
}
```

- `usage_day` — ISO 8601 `YYYY-MM-DD` representing the UTC day the numbers apply to. Because the SQL does the CASE-expression rollover inline (daily zeros at UTC midnight, monthly at first-of-month), `usage_day` is always CURRENT_DATE in UTC. If the tenant hasn't made a chargeable request since the last boundary crossing, the server's `quota_configs.usage_day` column may still carry the previous day — the SQL projects the rolled-over values into the response anyway so the reader sees up-to-date numbers.
- `daily.used_cents` — integer cents consumed today. Writes happen in `PgQuotaEnforcer::record_post` (ISSUE 11 TASK 11.3 / PR #232) — see [§2.C.2 flow diagram](../architecture/platform-architecture.md) in the architecture doc.
- `daily.limit_cents` — configured daily ceiling from `quota_configs.daily_limit_cents`.
- `daily.remaining_cents` — computed `max(limit_cents - used_cents, 0)`. Clients watching for throttling should drive retry timing off `remaining_cents == 0` rather than a 429 response; the response is cheap and safe to poll.
- `monthly.*` — same shape as daily, indexed on the `monthly_used_cents` + `monthly_limit_cents` columns.

**Bootstrap-gap fallback (v0.5.4 behavior).** When the tenant has no row in `quota_configs` (tenant was just created and nothing has populated the row yet), the handler does NOT 404. Instead it returns the schema defaults: `limit_cents = 1_000_000` (daily, i.e. $10k), `limit_cents = 10_000_000` (monthly, i.e. $100k), `used_cents = 0`, `remaining_cents = limit_cents`, `usage_day = <today UTC>`. Rationale: the UI renders "fresh tenant, full quota" while tenant provisioning catches up, instead of a 400/404 that confuses new users. Gate 7k.5 specifically exercises this fallback path — the harness test config doesn't populate `quota_configs` so the gate asserts the fallback produces the expected shape + numbers.

**Errors:**

| Code | HTTP | When |
|------|------|------|
| `invalid_api_key` | 401 | Missing, malformed, or revoked Bearer token. Standard auth-layer response. |
| `forbidden` | 403 | Caller is authenticated but not `OpenAiCompat` scope. Should be rare in practice since any key issued to a chat-using tenant has `OpenAiCompat`. |
| `config_error` | 503 | `pg_pool` is not wired — `PgQuotaEnforcer` is unavailable, so there's no snapshot to read. Returned by the `require_pg_quota_enforcer` defensive guard. |

E2E Gate 7k.5 covers the happy path (shape assertion + `daily.limit_cents > 0` + `daily.remaining_cents + daily.used_cents == daily.limit_cents`) against a tenant that has no `quota_configs` row, so it pins the fallback behavior. See [§`[quota_rate_limit]`](configuration.md#quota_rate_limit) in the configuration manual and [§5.6 EPIC 4 ISSUE 11 enforcement stack](../modules/xaas-platform.md#56-epic-4-issue-11-enforcement-stack--landed-on-trunk-today) in the xaas module doc for the rest of the ISSUE 11 pipeline.

---

## Admin endpoints (not yet implemented)

The following routes are defined in the router but return HTTP 501 (not yet implemented). They require scope `Management`.

| Method | Path | What it will do (future) |
|--------|------|--------------------------|
| `GET` | `/api/v1/nodes` | List registered GPU nodes |
| `POST` | `/api/v1/models/deploy` | Deploy a model to a node |
| `DELETE` | `/api/v1/models/{id}` | Undeploy a model |
| `GET` | `/api/v1/models/status` | Get model deployment status |
| `GET` | `/api/v1/usage` | Admin (Management-scope, cross-tenant) usage report. The tenant-scoped equivalent for a single caller's tenant is the shipped `GET /api/v1/web/workbench/usage/summary` (ISSUE 4, §above) — that one is under `OpenAiCompat`, not Management, and does not cross tenants. |
| `GET` | `/api/v1/costs` | Admin cross-tenant cost report. Partial equivalent: the `chat.total_cost_cents` field inside `/api/v1/web/workbench/usage/summary` (ISSUE 4) populates real integer cents from `gadgetron_core::pricing` for the caller's own tenant. The broader admin cross-tenant view remains unimplemented. |

Sending a request to any of these endpoints with a valid `Management`-scoped key returns HTTP 501 today:

```sh
curl -s http://localhost:8080/api/v1/nodes \
  -H "Authorization: Bearer gad_live_your_management_key_here"
# HTTP 501 (no body)
```

E2E Gates 7k and 7k.2 assert the **RBAC positive path** — any status except 401/403 is acceptable for a Management key on these routes (currently 501; will be 200 once each aggregator lands, or 503 during PostgreSQL pool outages). Your monitoring should treat 501 as "feature not shipped" and 401/403 as real auth regressions.

Sending with an `OpenAiCompat`-scoped key returns HTTP 403 (scope guard fires before the stub handler).
