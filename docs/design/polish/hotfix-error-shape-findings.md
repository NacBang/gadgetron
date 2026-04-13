# Hotfix: Error shape findings from Test 6

> **Status**: Design note
> **Scope**: 2 small error-shape fixes (no API semantics change)
> **Source**: Manual testing (Test 6 — error scenarios) on 2026-04-13

## Context
Manual QA Test 6 sent three error-triggering requests against `gadgetron serve --no-db` and inspected the responses. The 401 and 413 payloads revealed two violations of our "OpenAI API-compatible error shape" promise that real clients using the Python/TS SDKs will hit.

## Finding 1: `tenant_not_found` leaks internal terminology

### Current
```
$ curl -s -X POST http://127.0.0.1:8080/v1/chat/completions
{"error":{"code":"tenant_not_found","message":"Invalid API key. ...","type":"authentication_error"}}
```

Two complaints:
- `code` says `tenant_not_found`, but the message says "Invalid API key" — the `code` field is what machine clients switch on, not the prose message.
- `tenant_not_found` is Gadgetron-internal (we map API key → tenant row; a missing row means "no tenant"). OpenAI-compatible clients expect `invalid_api_key`. Anyone copy-pasting error-handling code from the OpenAI cookbook will never match our custom code.

### Fix
Change `GadgetronError::TenantNotFound.error_code()` in `crates/gadgetron-core/src/error.rs` from `"tenant_not_found"` to `"invalid_api_key"`. The Rust enum variant name stays (`TenantNotFound` — it still describes what the server did internally); only the user-facing string changes.

### Impact
- `message`, `type`, `http_status_code` unchanged
- Unit test `error_code_returns_stable_machine_string` → update assertion
- E2E test `tenant_not_found_401` in `crates/gadgetron-testing/tests/e2e.rs:160` → update asserted code
- No other call sites match on the literal string

## Finding 2: 413 body is plain text, not OpenAI-shaped JSON

### Current
```
$ curl -s -X POST http://127.0.0.1:8080/v1/chat/completions --data-binary @big.json
HTTP 413
length limit exceeded
```

The body comes straight from `tower_http::limit::RequestBodyLimitLayer`, which returns `PayloadTooLarge` with a plain-text body. OpenAI SDKs parse `response.json()["error"]["message"]` and will raise `JSONDecodeError` on this, surfacing as an opaque "server returned invalid JSON" to the user.

### Fix
Add a thin `map_response` layer **outside** `RequestBodyLimitLayer` (so it wraps the 413 response produced by the limit layer) in `crates/gadgetron-gateway/src/server.rs`. The layer inspects the status code; if it is `413`, it replaces the body with an OpenAI-shaped JSON payload.

**Requirement: dynamic limit string.** The message must embed the runtime `MAX_BODY_BYTES` value (formatted as human-readable MiB), not a hard-coded `"4 MB"` literal. If an operator changes `MAX_BODY_BYTES` for a large-context fleet, the error message must stay in sync. Per D-15 (implementation determinism), no magic numbers in strings.

Rather than inventing a new `GadgetronError` variant, we hard-code the 413 branch in the `map_response` closure (413 is an HTTP transport-layer concern, not a domain error):

```rust
// crates/gadgetron-gateway/src/server.rs
use axum::{body::Body, http::StatusCode, response::Response};

// Exposed via `pub(crate)` so unit tests can override via the build fn.
pub(crate) const MAX_BODY_BYTES: usize = 4_194_304;

fn format_body_limit(limit: usize) -> String {
    // 1 MiB = 1_048_576 bytes. For 4_194_304 → "4 MiB".
    let mib = limit as f64 / 1_048_576.0;
    if mib.fract() == 0.0 {
        format!("{} MiB", mib as u64)
    } else {
        format!("{:.1} MiB", mib)
    }
}

fn openai_shape_413(mut resp: Response<Body>) -> Response<Body> {
    if resp.status() != StatusCode::PAYLOAD_TOO_LARGE {
        return resp;
    }
    let body = serde_json::json!({
        "error": {
            "code": "request_too_large",
            "message": format!(
                "Request body exceeds the {} limit. Reduce your request size or split it across multiple calls.",
                format_body_limit(MAX_BODY_BYTES),
            ),
            "type": "invalid_request_error",
        }
    });
    let bytes = serde_json::to_vec(&body).expect("static JSON serializes");
    let len = bytes.len();
    *resp.body_mut() = Body::from(bytes);
    resp.headers_mut()
        .insert(axum::http::header::CONTENT_TYPE, "application/json".parse().unwrap());
    resp.headers_mut()
        .insert(axum::http::header::CONTENT_LENGTH, len.to_string().parse().unwrap());
    resp
}
```

Layer placement (outermost → innermost):
```
map_response(openai_shape_413)      ← NEW, catches 413 from below
  → RequestBodyLimitLayer(4 MB)     ← produces raw 413
  → TraceLayer
  → ...
```

### Acceptance
```
$ curl -s -X POST http://127.0.0.1:8080/v1/chat/completions --data-binary @big.json
HTTP 413
Content-Type: application/json
{"error":{"code":"request_too_large","message":"Request body exceeds the 4 MiB limit. Reduce your request size or split it across multiple calls.","type":"invalid_request_error"}}
```

## Test plan (revised post-Round 2)

### Unit tests (gadgetron-core)
1. `core::error::tests::error_code_returns_stable_machine_string` — assertion changed to `"invalid_api_key"`.

### Unit tests (gadgetron-gateway, `#[cfg(test)]`)
2. **A1** — `body_too_large_returns_413_with_json_content_type`: build router, send >MAX_BODY_BYTES via `tower::ServiceExt::oneshot`, assert `status == 413` AND `headers["content-type"]` starts with `application/json`.
3. **A2** — `body_too_large_returns_openai_shaped_json`: same setup, deserialize body as `serde_json::Value`, assert `body["error"]["code"] == "request_too_large"`, `body["error"]["type"] == "invalid_request_error"`, `body["error"]["message"]` non-empty string AND contains the word `"MiB"` (proves dynamic formatting, not a literal).
4. **A4** — extend existing `list_models_returns_200_with_empty_list`: add `assert!(resp.headers()["content-type"].to_str().unwrap().starts_with("application/json"))` as a **2xx regression guard** — proves `openai_shape_413` map_response does not corrupt successful responses.
5. **format_body_limit unit test**: pure-function test with 3 cases (`4_194_304 → "4 MiB"`, `8_388_608 → "8 MiB"`, `6_291_456 → "6 MiB"` for fractional edge).

### E2E tests (gadgetron-testing)
6. Rename/update `tenant_not_found_401` → asserts `error.code == "invalid_api_key"` (old assertion was `tenant_not_found`).
7. **A3** — extend existing `e2e_body_too_large_413`: in addition to status, assert `Content-Type: application/json` and deserialize body, check `error.code == "request_too_large"` — belt-and-suspenders check through the full HTTP stack.

### Manual smoke
8. `./target/release/gadgetron serve --no-db --provider http://10.100.1.5:8100 --bind 127.0.0.1:8080`
9. `curl -sS -o - http://127.0.0.1:8080/v1/chat/completions -X POST` (no auth header) → verify `error.code == "invalid_api_key"`
10. `curl -sS -o - http://127.0.0.1:8080/v1/chat/completions -X POST --data-binary @big.json -H "Authorization: Bearer gad_live_testkey0000000000000000000000"` → verify `error.code == "request_too_large"` and body is JSON

### Decision on A5 (test-configurable `MAX_BODY_BYTES`)
**Accepted**: `MAX_BODY_BYTES` becomes `pub(crate) const`. Gateway unit tests A1 and A2 use the 4 MiB default (send 4_194_305 bytes — one byte over the edge, cheap). The 5 MB E2E test in `gadgetron-testing` is reduced to 4_194_320 bytes for CI memory efficiency — still > 4 MiB, still triggers 413, but under 0.1% of the previous allocation.

## Non-goals
- No new error variants in `GadgetronError` — we reuse the existing enum and special-case 413 at the router boundary (body-limit rejection is an HTTP-layer concern, not a domain error)
- No change to 401/403 behavior from scope_guard or quota_exceeded paths
- No change to `http_status_code` mapping
- **`Forbidden` → `insufficient_permissions` rename is OUT OF SCOPE.** Tracked as a follow-up hotfix: the `forbidden` code on 403 responses is non-OpenAI-standard, but no manual test has exercised the 403 path yet in the current QA pass. Will be addressed once Test 6 expands to validate scope_guard behavior with real API keys. Do not touch `GadgetronError::Forbidden.error_code()` in this PR.
