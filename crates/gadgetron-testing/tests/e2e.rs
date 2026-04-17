// E2E integration tests for gadgetron-gateway.
//
// Scenarios 1-7 per docs/archive/phase1/sprint5-e2e-bench-tui.md §2.6.
//
// Each test:
//   1. Spins up a local PostgreSQL test database (PgHarness::new()).
//   2. Starts the real gateway on a random port (GatewayHarness::start()).
//   3. Makes HTTP requests via reqwest.
//   4. Asserts response status and body.
//   5. Tears down gateway + database.
//
// Run: cargo test -p gadgetron-testing --test e2e

mod common;

use common::E2EFixture;
use gadgetron_testing::{
    harness::{gateway::GatewayHarness, pg::PgHarness},
    mocks::{
        provider::{FailMode, FailingProvider, FakeLlmProvider},
        xaas::ExhaustedQuotaEnforcer,
    },
};
use serde_json::{json, Value};
use std::sync::Arc;

// ───────────────────────────────────────────────────────────────────
// Scenario 1 — Non-streaming chat completion → 200 + content
// ───────────────────────────────────────────────────────────────────

/// POST /v1/chat/completions with stream:false → HTTP 200, correct content.
///
/// Verifies the full middleware stack (auth → tenant-ctx → quota → handler)
/// with a real PostgreSQL API-key lookup.
#[tokio::test]
async fn e2e_chat_completion_non_streaming() {
    let fx = E2EFixture::new("hello from fake", 0).await;

    let resp = fx
        .gw
        .authed_post("/v1/chat/completions", &fx.api_key)
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": false
        }))
        .send()
        .await
        .expect("request failed");

    let status = resp.status().as_u16();
    assert_eq!(status, 200, "expected 200 OK");

    let body: Value = resp.json().await.expect("body parse failed");
    assert_eq!(
        body["object"], "chat.completion",
        "envelope object must be 'chat.completion'"
    );
    assert!(body["id"].is_string(), "id must be present");

    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .expect("content missing");
    assert_eq!(content, "hello from fake");

    fx.teardown().await;
}

// ───────────────────────────────────────────────────────────────────
// Scenario 2 — SSE streaming → Content-Type: text/event-stream + [DONE]
// ───────────────────────────────────────────────────────────────────

/// POST /v1/chat/completions with stream:true → SSE with ≥2 chunks + [DONE].
#[tokio::test]
async fn e2e_chat_completion_streaming() {
    let fx = E2EFixture::new("x", 2).await;

    let resp = fx
        .gw
        .authed_post("/v1/chat/completions", &fx.api_key)
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true
        }))
        .send()
        .await
        .expect("request failed");

    let status = resp.status().as_u16();
    assert_eq!(status, 200, "streaming must return 200");

    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        ct.contains("text/event-stream"),
        "content-type must be text/event-stream, got: {ct}"
    );

    let text: String = resp.text().await.expect("body text failed");
    let data_lines: Vec<&str> = text
        .lines()
        .filter(|l: &&str| l.starts_with("data: "))
        .collect();

    // 2 chunks + [DONE] = at least 3 data: lines.
    assert!(
        data_lines.len() >= 3,
        "expected ≥3 data: lines (2 chunks + [DONE]), got: {:?}",
        data_lines
    );

    let last = *data_lines.last().expect("no data lines");
    assert_eq!(last, "data: [DONE]", "last SSE event must be [DONE]");

    // First chunk must be valid JSON with the correct object type.
    let first_payload = data_lines[0]
        .strip_prefix("data: ")
        .expect("strip prefix failed");
    let chunk: Value = serde_json::from_str(first_payload).expect("first chunk must be valid JSON");
    assert_eq!(
        chunk["object"], "chat.completion.chunk",
        "chunk object must be 'chat.completion.chunk'"
    );

    fx.teardown().await;
}

// ───────────────────────────────────────────────────────────────────
// Scenario 3 — Missing Authorization → 401
// ───────────────────────────────────────────────────────────────────

/// POST /v1/chat/completions with no Authorization header → HTTP 401.
#[tokio::test]
async fn e2e_auth_missing_401() {
    let fx = E2EFixture::new("x", 0).await;

    let resp = fx
        .gw
        .client
        .post(format!("{}/v1/chat/completions", fx.gw.url))
        .header("Content-Type", "application/json")
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .expect("request failed");

    let status = resp.status().as_u16();
    assert_eq!(status, 401, "missing auth must return 401");

    let body: Value = resp.json().await.expect("body parse failed");
    assert_eq!(
        body["error"]["code"], "invalid_api_key",
        "error code must be 'invalid_api_key' (OpenAI-standard 401 code)"
    );
    assert_eq!(body["error"]["type"], "authentication_error");

    fx.teardown().await;
}

// ───────────────────────────────────────────────────────────────────
// Scenario 4 — Wrong scope → 403
// ───────────────────────────────────────────────────────────────────

/// GET /api/v1/nodes with an OpenAiCompat-only key → HTTP 403.
#[tokio::test]
async fn e2e_wrong_scope_403() {
    let fx = E2EFixture::new("x", 0).await;

    let resp = fx
        .gw
        .authed_get("/api/v1/nodes", &fx.api_key)
        .send()
        .await
        .expect("request failed");

    let status = resp.status().as_u16();
    assert_eq!(status, 403, "wrong scope must return 403");

    let body: Value = resp.json().await.expect("body parse failed");
    assert_eq!(
        body["error"]["code"], "forbidden",
        "error code must be 'forbidden'"
    );

    fx.teardown().await;
}

// ───────────────────────────────────────────────────────────────────
// Scenario 5 — Quota exceeded → 429
// ───────────────────────────────────────────────────────────────────

/// POST /v1/chat/completions with ExhaustedQuotaEnforcer → HTTP 429.
#[tokio::test]
async fn e2e_quota_exceeded_429() {
    let pg = PgHarness::new().await;
    let (_, api_key) = pg.insert_test_tenant().await;
    let provider = Arc::new(FakeLlmProvider::new(
        "x",
        0,
        vec!["gpt-4o-mini".to_string()],
    ));
    let gw =
        GatewayHarness::start_with_quota(provider, &pg, Arc::new(ExhaustedQuotaEnforcer)).await;

    let resp = gw
        .authed_post("/v1/chat/completions", &api_key)
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .expect("request failed");

    let status = resp.status().as_u16();
    assert_eq!(status, 429, "exhausted quota must return 429");

    let body: Value = resp.json().await.expect("body parse failed");
    assert_eq!(
        body["error"]["code"], "quota_exceeded",
        "error code must be 'quota_exceeded'"
    );

    gw.shutdown().await;
    pg.cleanup().await;
}

// ───────────────────────────────────────────────────────────────────
// Scenario 6 — Body too large → 413
// ───────────────────────────────────────────────────────────────────

/// POST /v1/chat/completions with an oversized body → HTTP 413 +
/// OpenAI-shaped JSON body (`error.code == "request_too_large"`).
///
/// Uses the smallest body that trips the 4 MiB limit (MAX_BODY_BYTES + 16) so
/// CI memory usage stays bounded. Still exercises the full HTTP stack:
/// reqwest → tower → RequestBodyLimitLayer → openai_shape_413 map_response.
#[tokio::test]
async fn e2e_body_too_large_413() {
    let fx = E2EFixture::new("x", 0).await;

    // MAX_BODY_BYTES = 4_194_304 (4 MiB). One byte over is enough to trigger 413.
    // We use +16 for safety margin against any trivial header encoding differences.
    let large_body = vec![b'x'; 4_194_304 + 16];

    let resp = fx
        .gw
        .client
        .post(format!("{}/v1/chat/completions", fx.gw.url))
        .header("Content-Type", "application/json")
        .body(large_body)
        .send()
        .await
        .expect("request failed");

    let status = resp.status().as_u16();
    assert_eq!(
        status, 413,
        "body over MAX_BODY_BYTES must return 413 Payload Too Large"
    );

    // A3 — body-shape assertions: hotfix-error-shape-findings.md
    let content_type = resp
        .headers()
        .get("content-type")
        .expect("413 response must carry Content-Type")
        .to_str()
        .expect("Content-Type must be ASCII");
    assert!(
        content_type.starts_with("application/json"),
        "413 Content-Type must be JSON (OpenAI SDK calls response.json()), got {content_type:?}"
    );

    let body: Value = resp
        .json()
        .await
        .expect("413 body must deserialize as JSON (not plain text)");
    assert_eq!(
        body["error"]["code"], "request_too_large",
        "413 error.code must be 'request_too_large'"
    );
    assert_eq!(
        body["error"]["type"], "invalid_request_error",
        "413 error.type must be 'invalid_request_error'"
    );
    let msg = body["error"]["message"]
        .as_str()
        .expect("error.message must be a string");
    assert!(
        msg.contains("MiB"),
        "error.message must embed the runtime body limit in MiB, got {msg:?}"
    );

    fx.teardown().await;
}

// ───────────────────────────────────────────────────────────────────
// Scenario 7 — Provider failure → 5xx
// ───────────────────────────────────────────────────────────────────

/// POST /v1/chat/completions with FailingProvider → HTTP 5xx (502 or 503).
///
/// Sprint 5 does not implement a circuit breaker. All 4 requests fail because
/// the provider always returns `GadgetronError::Provider("immediate fail")`.
/// The error code must be "routing_failure" (503) or "provider_error" (502).
#[tokio::test]
async fn e2e_provider_failure_5xx() {
    let pg = PgHarness::new().await;
    let (_, api_key) = pg.insert_test_tenant().await;
    let provider = Arc::new(FailingProvider::new(FailMode::ImmediateFail));
    let gw = GatewayHarness::start(provider, &pg).await;

    let body = json!({
        "model": "gpt-4o-mini",
        "messages": [{"role": "user", "content": "hi"}]
    });

    // Requests 1-3: all fail (accumulate circuit breaker counter if implemented).
    for i in 1..=3 {
        let resp = gw
            .authed_post("/v1/chat/completions", &api_key)
            .json(&body)
            .send()
            .await
            .expect("request failed");
        let status = resp.status().as_u16();
        assert!(
            (500..600).contains(&status),
            "request {i}: expected 5xx, got {status}"
        );
    }

    // Request 4: circuit open (503) or continued failure (502).
    let resp4 = gw
        .authed_post("/v1/chat/completions", &api_key)
        .json(&body)
        .send()
        .await
        .expect("request 4 failed");
    let status4 = resp4.status().as_u16();
    assert!(
        (500..600).contains(&status4),
        "request 4: expected 5xx, got {status4}"
    );

    let body4: Value = resp4.json().await.expect("body4 parse failed");
    let code = body4["error"]["code"].as_str().unwrap_or("");
    assert!(
        code == "routing_failure" || code == "provider_error",
        "error code must be 'routing_failure' or 'provider_error', got: {code}"
    );

    gw.shutdown().await;
    pg.cleanup().await;
}
