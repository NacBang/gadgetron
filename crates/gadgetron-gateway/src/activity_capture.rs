//! PSL-1d activity-capture helpers — fire-and-forget Penny shared-surface
//! capture for Ok and Err arms of the chat endpoints.
//!
//! Both helpers are non-PII (model name + token counts + latency only; no
//! message text) and never propagate errors — they log via `tracing::warn!`
//! and swallow the failure so the client request path is untouched.
//!
//! Shared by:
//! - `handlers.rs` — non-streaming Ok/Err arms and streaming dispatch-time
//!   capture (per W3-PSL-1d, D-20260418-20).
//! - `stream_end_guard.rs` — streaming stream-end amendment (PR 6).
//!
//! Authority: `docs/design/core/knowledge-candidate-curation.md` §2.1.

use std::sync::Arc;

use gadgetron_core::error::GadgetronError;
use gadgetron_core::knowledge::candidate::{
    ActivityKind, ActivityOrigin, CapturedActivityEvent, KnowledgeCandidateCoordinator,
};
use gadgetron_core::knowledge::AuthenticatedContext;
use uuid::Uuid;

/// Capture a successful chat completion into the activity stream for
/// `<gadgetron_shared_context>` projection on the next turn.
///
/// Drift-fix PR 5 (D-20260418-24): `audit_event_id` is the correlation key
/// that joins this `captured_activity_event` row to the matching
/// `audit_log` row. Callers MUST pass the SAME UUID they gave to
/// `AuditWriter::send`. Reusing `request_id` instead is a bug — see the
/// `event_id_distinct_from_request_id` unit test.
#[allow(clippy::too_many_arguments)]
pub async fn capture_chat_completion(
    coordinator: Arc<dyn KnowledgeCandidateCoordinator>,
    tenant_id: Uuid,
    request_id: Uuid,
    audit_event_id: Uuid,
    model: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    stream: bool,
) {
    let event = CapturedActivityEvent {
        id: Uuid::new_v4(),
        tenant_id,
        // TODO(doc-10): real user_id after permission-inheritance lands.
        actor_user_id: Uuid::nil(),
        request_id: Some(request_id),
        origin: ActivityOrigin::Penny,
        kind: ActivityKind::GadgetToolCall,
        title: format!("chat completion: {model}"),
        summary: format!(
            "{prompt_tokens} input / {completion_tokens} output tokens, stream={stream}"
        ),
        source_bundle: None,
        source_capability: Some("chat.completions".into()),
        audit_event_id: Some(audit_event_id),
        facts: serde_json::json!({
            "model": model,
            "stream": stream,
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
        }),
        created_at: chrono::Utc::now(),
    };

    let actor = AuthenticatedContext;
    if let Err(e) = coordinator.capture_action(&actor, event, vec![]).await {
        tracing::warn!(
            request_id = %request_id,
            error = %e,
            "penny_shared_context.capture_chat_failed"
        );
    }
}

/// Derive a short, non-PII error class string from a `GadgetronError` variant.
/// Uses the enum discriminant only — never the message or nested fields,
/// which may contain user-facing strings or path fragments.
pub fn error_class(e: &GadgetronError) -> &'static str {
    match e {
        GadgetronError::Provider(_) => "provider_error",
        GadgetronError::Routing(_) => "routing_error",
        GadgetronError::QuotaExceeded { .. } => "quota_exceeded",
        GadgetronError::TenantNotFound => "tenant_not_found",
        GadgetronError::Forbidden => "forbidden",
        GadgetronError::Database { .. } => "database_error",
        GadgetronError::StreamInterrupted { .. } => "stream_interrupted",
        GadgetronError::Knowledge { .. } => "knowledge_error",
        GadgetronError::Wiki { .. } => "wiki_error",
        GadgetronError::Penny { .. } => "penny_error",
        _ => "internal_error",
    }
}

/// Capture a failed chat completion (non-streaming Err arm OR streaming
/// stream-end amendment when `saw_error` is true) into the activity stream.
///
/// Non-PII: only model name + error class + latency; no message text, no
/// nested error fields.
#[allow(clippy::too_many_arguments)]
pub async fn capture_chat_completion_error(
    coordinator: Arc<dyn KnowledgeCandidateCoordinator>,
    tenant_id: Uuid,
    request_id: Uuid,
    audit_event_id: Uuid,
    model: String,
    error_class_str: &'static str,
    latency_ms: i32,
) {
    let event = CapturedActivityEvent {
        id: Uuid::new_v4(),
        tenant_id,
        actor_user_id: Uuid::nil(),
        request_id: Some(request_id),
        origin: ActivityOrigin::Penny,
        kind: ActivityKind::RuntimeObservation,
        title: format!("chat completion failed: {model}"),
        summary: format!("error_class={error_class_str}, latency_ms={latency_ms}"),
        source_bundle: None,
        source_capability: Some("chat.completions".into()),
        audit_event_id: Some(audit_event_id),
        facts: serde_json::json!({
            "model": model,
            "error_class": error_class_str,
            "latency_ms": latency_ms,
        }),
        created_at: chrono::Utc::now(),
    };

    let actor = AuthenticatedContext;
    if let Err(e) = coordinator.capture_action(&actor, event, vec![]).await {
        tracing::warn!(
            request_id = %request_id,
            error = %e,
            "penny_shared_context.capture_chat_error_failed"
        );
    }
}
