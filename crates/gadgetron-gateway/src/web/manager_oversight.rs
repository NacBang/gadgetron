//! Manager oversight HTTP surface, execution projections, and the single
//! outbound terminal-exception webhook worker.

use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Extension, Json, Router,
};
use chrono::Utc;
use gadgetron_core::{
    context::TenantContext, knowledge::AuthenticatedContext,
    workbench::InvokeWorkbenchActionResponse,
};
use gadgetron_xaas::manager_oversight::{
    self as store, CreateDirectiveInput, ManagerOversightError, RecordOutcomeInput,
    StageEventInput, TransitionDirectiveInput, WebhookSettingsInput,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio::{sync::watch, task::JoinHandle};
use uuid::Uuid;

use crate::server::AppState;

use super::workbench::WorkbenchHttpError;

const WEBHOOK_INTERVAL: Duration = Duration::from_secs(2);
const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_WEBHOOK_ATTEMPTS: i32 = 4;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/oversight", get(list_oversight_handler))
        .route(
            "/admin/oversight/{oversight_id}",
            get(get_oversight_handler),
        )
        .route(
            "/admin/directives",
            get(list_directives_handler).post(create_directive_handler),
        )
        .route(
            "/admin/directives/{directive_id}",
            get(get_directive_handler),
        )
        .route(
            "/admin/directives/{directive_id}/transition",
            post(transition_directive_handler),
        )
        .route("/admin/exceptions", get(list_exceptions_handler))
        .route(
            "/admin/exceptions/{exception_id}/transition",
            post(transition_exception_handler),
        )
        .route(
            "/admin/exception-webhook",
            get(get_webhook_settings_handler).patch(update_webhook_settings_handler),
        )
        .route(
            "/admin/exception-webhook/deliveries",
            get(list_webhook_deliveries_handler),
        )
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
}

const fn default_limit() -> i64 {
    100
}

#[derive(Debug, Serialize)]
struct OversightListResponse {
    records: Vec<store::OversightRecord>,
    returned: usize,
}

#[derive(Debug, Serialize)]
struct DirectiveListResponse {
    directives: Vec<store::CorrectiveDirective>,
    returned: usize,
}

#[derive(Debug, Serialize)]
struct ExceptionListResponse {
    exceptions: Vec<store::ManagerException>,
    returned: usize,
}

#[derive(Debug, Serialize)]
struct DeliveryListResponse {
    deliveries: Vec<store::WebhookDelivery>,
    returned: usize,
}

#[derive(Debug, Deserialize)]
struct ExceptionTransitionRequest {
    expected_revision: i64,
    state: String,
    summary: String,
}

#[derive(Debug, Deserialize)]
struct UpdateWebhookRequest {
    enabled: bool,
    #[serde(default)]
    destination_url: Option<String>,
    #[serde(default)]
    review_base_url: Option<String>,
    expected_revision: i64,
}

async fn list_oversight_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Query(query): Query<ListQuery>,
) -> Result<Json<OversightListResponse>, WorkbenchHttpError> {
    let records = store::list_oversight(pool(&state)?, ctx.tenant_id, query.limit)
        .await
        .map_err(manager_error)?;
    let returned = records.len();
    Ok(Json(OversightListResponse { records, returned }))
}

async fn get_oversight_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(oversight_id): Path<Uuid>,
) -> Result<Json<store::OversightDetail>, WorkbenchHttpError> {
    Ok(Json(
        store::oversight_detail(pool(&state)?, ctx.tenant_id, oversight_id)
            .await
            .map_err(manager_error)?,
    ))
}

async fn list_directives_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Query(query): Query<ListQuery>,
) -> Result<Json<DirectiveListResponse>, WorkbenchHttpError> {
    let directives = store::list_directives(pool(&state)?, ctx.tenant_id, query.limit)
        .await
        .map_err(manager_error)?;
    let returned = directives.len();
    Ok(Json(DirectiveListResponse {
        directives,
        returned,
    }))
}

async fn create_directive_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(request): Json<CreateDirectiveInput>,
) -> Result<Json<store::DirectiveDetail>, WorkbenchHttpError> {
    Ok(Json(
        store::create_directive(pool(&state)?, ctx.tenant_id, manager_user(&ctx)?, request)
            .await
            .map_err(manager_error)?,
    ))
}

async fn get_directive_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(directive_id): Path<Uuid>,
) -> Result<Json<store::DirectiveDetail>, WorkbenchHttpError> {
    Ok(Json(
        store::directive_detail(pool(&state)?, ctx.tenant_id, directive_id)
            .await
            .map_err(manager_error)?,
    ))
}

async fn transition_directive_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(directive_id): Path<Uuid>,
    Json(request): Json<TransitionDirectiveInput>,
) -> Result<Json<store::DirectiveDetail>, WorkbenchHttpError> {
    Ok(Json(
        store::transition_directive(
            pool(&state)?,
            ctx.tenant_id,
            directive_id,
            manager_user(&ctx)?,
            request,
        )
        .await
        .map_err(manager_error)?,
    ))
}

async fn list_exceptions_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ExceptionListResponse>, WorkbenchHttpError> {
    let exceptions = store::list_exceptions(pool(&state)?, ctx.tenant_id, query.limit)
        .await
        .map_err(manager_error)?;
    let returned = exceptions.len();
    Ok(Json(ExceptionListResponse {
        exceptions,
        returned,
    }))
}

async fn transition_exception_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(exception_id): Path<Uuid>,
    Json(request): Json<ExceptionTransitionRequest>,
) -> Result<Json<store::ManagerException>, WorkbenchHttpError> {
    Ok(Json(
        store::transition_exception(
            pool(&state)?,
            ctx.tenant_id,
            exception_id,
            manager_user(&ctx)?,
            request.expected_revision,
            &request.state,
            &request.summary,
        )
        .await
        .map_err(manager_error)?,
    ))
}

async fn get_webhook_settings_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
) -> Result<Json<store::WebhookSettingsView>, WorkbenchHttpError> {
    Ok(Json(
        store::webhook_settings(pool(&state)?, ctx.tenant_id)
            .await
            .map_err(manager_error)?,
    ))
}

async fn update_webhook_settings_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(request): Json<UpdateWebhookRequest>,
) -> Result<Json<store::WebhookSettingsView>, WorkbenchHttpError> {
    let parsed = request
        .destination_url
        .as_deref()
        .map(validate_webhook_url)
        .transpose()?;
    let review_base_url = request
        .review_base_url
        .as_deref()
        .map(validate_review_base_url)
        .transpose()?;
    Ok(Json(
        store::update_webhook_settings(
            pool(&state)?,
            ctx.tenant_id,
            manager_user(&ctx)?,
            WebhookSettingsInput {
                enabled: request.enabled,
                endpoint_url: parsed.as_ref().map(|url| url.as_str().to_string()),
                destination_host: parsed
                    .as_ref()
                    .and_then(reqwest::Url::host_str)
                    .map(str::to_string),
                review_base_url,
                expected_revision: request.expected_revision,
            },
        )
        .await
        .map_err(manager_error)?,
    ))
}

async fn list_webhook_deliveries_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Query(query): Query<ListQuery>,
) -> Result<Json<DeliveryListResponse>, WorkbenchHttpError> {
    let deliveries = store::list_deliveries(pool(&state)?, ctx.tenant_id, query.limit)
        .await
        .map_err(manager_error)?;
    let returned = deliveries.len();
    Ok(Json(DeliveryListResponse {
        deliveries,
        returned,
    }))
}

fn pool(state: &AppState) -> Result<&PgPool, WorkbenchHttpError> {
    state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(gadgetron_core::error::GadgetronError::Config(
            "Manager oversight requires PostgreSQL".into(),
        ))
    })
}

fn manager_user(ctx: &TenantContext) -> Result<Uuid, WorkbenchHttpError> {
    ctx.actor_user_id
        .ok_or(WorkbenchHttpError::ManagerIdentityRequired)
}

fn manager_error(error: ManagerOversightError) -> WorkbenchHttpError {
    match error {
        ManagerOversightError::NotFound => WorkbenchHttpError::ManagerNotFound,
        ManagerOversightError::Conflict => WorkbenchHttpError::ManagerConflict,
        ManagerOversightError::InvalidInput(detail) => {
            WorkbenchHttpError::ManagerInvalidInput { detail }
        }
        ManagerOversightError::Database(error) => {
            WorkbenchHttpError::Core(gadgetron_core::error::GadgetronError::Config(format!(
                "Manager oversight database operation failed: {error}"
            )))
        }
    }
}

fn validate_webhook_url(value: &str) -> Result<reqwest::Url, WorkbenchHttpError> {
    let parsed =
        reqwest::Url::parse(value).map_err(|_| WorkbenchHttpError::ManagerInvalidInput {
            detail: "Webhook destination must be an absolute HTTP or HTTPS URL".into(),
        })?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(WorkbenchHttpError::ManagerInvalidInput {
            detail: "Webhook destination must use HTTP(S), include a host, and omit user-info and fragments"
                .into(),
        });
    }
    Ok(parsed)
}

fn validate_review_base_url(value: &str) -> Result<String, WorkbenchHttpError> {
    let mut parsed =
        reqwest::Url::parse(value).map_err(|_| WorkbenchHttpError::ManagerInvalidInput {
            detail: "Review base URL must be an absolute HTTP or HTTPS origin".into(),
        })?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(WorkbenchHttpError::ManagerInvalidInput {
            detail: "Review base URL must be a plain HTTP(S) origin".into(),
        });
    }
    parsed.set_path("");
    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

pub struct WorkbenchActionOutcome<'a> {
    pub actor: &'a AuthenticatedContext,
    pub source_event_id: Uuid,
    pub action_id: &'a str,
    pub args: &'a Value,
    pub response: Option<&'a InvokeWorkbenchActionResponse>,
    pub error_code: Option<&'a str>,
    pub elapsed_ms: u64,
    pub policy_decision: &'a str,
    pub policy_revision: Option<String>,
}

pub async fn record_workbench_action(pool: &PgPool, outcome: WorkbenchActionOutcome<'_>) {
    let WorkbenchActionOutcome {
        actor,
        source_event_id,
        action_id,
        args,
        response,
        error_code,
        elapsed_ms,
        policy_decision,
        policy_revision,
    } = outcome;
    let payload = response.and_then(|response| response.result.payload.as_ref());
    let analysis = analyze_payload(payload, error_code);
    let target_id = target_id(args, payload).unwrap_or_else(|| action_id.to_string());
    let evidence_refs = evidence_refs(payload);
    let mut events = vec![
        stage(
            "target",
            "recorded",
            format!("Target recorded for {}", human_label(action_id)),
        ),
        stage(
            "plan",
            "completed",
            format!(
                "Signed action {} passed its execution boundary",
                human_label(action_id)
            ),
        ),
    ];
    events.push(stage(
        "execute",
        if analysis.outcome == "succeeded" {
            "completed"
        } else {
            "failed"
        },
        analysis.summary.clone(),
    ));
    events.push(StageEventInput {
        stage: "verify".into(),
        state: match analysis.verification.as_str() {
            "verified" => "completed",
            "failed" => "failed",
            _ => "skipped",
        }
        .into(),
        summary: analysis.verification_summary.clone(),
        evidence_refs: evidence_refs.clone(),
    });
    let exception = exception_for(&analysis, "Action", &target_id);
    let input = RecordOutcomeInput {
        tenant_id: actor.tenant_id,
        source_kind: "workbench_action".into(),
        source_id: source_event_id.to_string(),
        actor_user_id: actor.real_user_id,
        agent_label: "Penny".into(),
        agent_role: "operator".into(),
        goal: format!("Complete {} for {}", human_label(action_id), target_id),
        target_kind: "action".into(),
        target_id,
        target_revision: None,
        policy_decision: normalized_policy(policy_decision),
        policy_revision,
        evidence_refs,
        current_stage: "verify".into(),
        outcome: analysis.outcome,
        verification_state: analysis.verification,
        action_summary: analysis.summary,
        before_summary: analysis.before,
        after_summary: analysis.after,
        rollback_summary: analysis.rollback,
        duration_ms: i64::try_from(elapsed_ms).unwrap_or(i64::MAX),
        cost_minor_units: 0,
        events,
        exception_severity: exception.as_ref().map(|(severity, _)| severity.clone()),
        exception_summary: exception.map(|(_, summary)| summary),
    };
    if let Err(error) = store::record_outcome(pool, input).await {
        tracing::error!(
            target: "manager_oversight",
            action_id,
            audit_event_id = %source_event_id,
            error = %error,
            "action completed but Manager oversight persistence failed"
        );
    }
}

#[derive(Debug, Clone)]
pub struct BackgroundOutcome {
    pub tenant_id: Uuid,
    pub source_kind: &'static str,
    pub source_id: String,
    pub actor_user_id: Option<Uuid>,
    pub agent_label: String,
    pub agent_role: String,
    pub goal: String,
    pub target_id: String,
    pub target_revision: Option<String>,
    pub policy_decision: String,
    pub policy_revision: Option<String>,
    pub outcome: String,
    pub verification_state: String,
    pub summary: String,
    pub verification_summary: String,
    pub evidence_refs: Vec<String>,
    pub duration_ms: i64,
    pub exception_severity: Option<String>,
    pub exception_summary: Option<String>,
}

pub async fn record_background_outcome(pool: &PgPool, observation: BackgroundOutcome) {
    let execute_state = if observation.outcome == "succeeded" {
        "completed"
    } else {
        "failed"
    };
    let verify_state = match observation.verification_state.as_str() {
        "verified" => "completed",
        "failed" => "failed",
        _ => "skipped",
    };
    let input = RecordOutcomeInput {
        tenant_id: observation.tenant_id,
        source_kind: observation.source_kind.into(),
        source_id: observation.source_id.clone(),
        actor_user_id: observation.actor_user_id,
        agent_label: observation.agent_label,
        agent_role: observation.agent_role,
        goal: observation.goal,
        target_kind: "job".into(),
        target_id: observation.target_id,
        target_revision: observation.target_revision,
        policy_decision: normalized_policy(&observation.policy_decision),
        policy_revision: observation.policy_revision,
        evidence_refs: observation.evidence_refs.clone(),
        current_stage: "verify".into(),
        outcome: observation.outcome,
        verification_state: observation.verification_state,
        action_summary: observation.summary.clone(),
        before_summary: None,
        after_summary: None,
        rollback_summary: None,
        duration_ms: observation.duration_ms.max(0),
        cost_minor_units: 0,
        events: vec![
            stage(
                "target",
                "recorded",
                "Background target and immutable job identity recorded",
            ),
            stage(
                "plan",
                "completed",
                "Signed background recipe and policy boundary selected",
            ),
            stage("execute", execute_state, observation.summary),
            StageEventInput {
                stage: "verify".into(),
                state: verify_state.into(),
                summary: observation.verification_summary,
                evidence_refs: observation.evidence_refs,
            },
        ],
        exception_severity: observation.exception_severity,
        exception_summary: observation.exception_summary,
    };
    if let Err(error) = store::record_outcome(pool, input).await {
        tracing::error!(
            target: "manager_oversight",
            source_kind = observation.source_kind,
            source_id = %observation.source_id,
            error = %error,
            "background result reached terminal state but Manager oversight persistence failed"
        );
    }
}

#[derive(Debug)]
pub(crate) struct PayloadAnalysis {
    pub(crate) outcome: String,
    pub(crate) verification: String,
    pub(crate) summary: String,
    pub(crate) verification_summary: String,
    pub(crate) before: Option<String>,
    pub(crate) after: Option<String>,
    pub(crate) rollback: Option<String>,
}

pub(crate) fn analyze_payload(
    payload: Option<&Value>,
    error_code: Option<&str>,
) -> PayloadAnalysis {
    if let Some(error_code) = error_code {
        return PayloadAnalysis {
            outcome: "failed".into(),
            verification: "failed".into(),
            summary: format!("Action execution failed ({})", bounded_text(error_code, 80)),
            verification_summary: "Execution failed before the desired state could be verified"
                .into(),
            before: None,
            after: None,
            rollback: None,
        };
    }
    let output = payload.and_then(|value| value.get("output")).or(payload);
    let status = output
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str);
    let outcomes = payload
        .and_then(|value| value.get("outcomes"))
        .and_then(Value::as_array);
    let outcome_statuses = outcomes
        .into_iter()
        .flatten()
        .filter_map(|outcome| outcome.get("status").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let failed = outcome_statuses.contains(&"failed");
    let verified = !outcome_statuses.is_empty()
        && outcome_statuses.iter().all(|status| *status == "succeeded")
        && output.and_then(|value| value.get("before")).is_some()
        && output.and_then(|value| value.get("after")).is_some();
    let safe_stopped = status == Some("safe_stopped");
    let outcome = if safe_stopped {
        "safe_stopped"
    } else if failed {
        "failed"
    } else {
        "succeeded"
    };
    let verification = if safe_stopped || failed {
        "failed"
    } else if verified {
        "verified"
    } else {
        "not_provided"
    };
    let outcome_summary = outcomes
        .into_iter()
        .flatten()
        .find_map(|outcome| outcome.get("summary").and_then(Value::as_str));
    let summary = outcome_summary
        .or_else(|| {
            output
                .and_then(|value| value.get("action"))
                .and_then(Value::as_str)
        })
        .or(status)
        .map(|value| bounded_text(value, 500))
        .unwrap_or_else(|| "Action completed".into());
    let before = output
        .and_then(|value| value.get("before"))
        .and_then(safe_value_summary);
    let after = output
        .and_then(|value| value.get("after"))
        .and_then(safe_value_summary);
    let rollback = output
        .and_then(|value| value.get("rollback_available"))
        .and_then(Value::as_bool)
        .map(|available| {
            if available {
                "A signed compensating action is available"
            } else {
                "No compensating action was declared"
            }
            .to_string()
        });
    let verification_summary = match verification {
        "verified" => "Signed outcome reports succeeded with bounded before/after state".into(),
        "failed" if safe_stopped => {
            "Execution stopped safely before the desired state was verified".into()
        }
        "failed" => "The signed outcome reported failure".into(),
        _ => "The action returned no authoritative before/after verification".into(),
    };
    PayloadAnalysis {
        outcome: outcome.into(),
        verification: verification.into(),
        summary,
        verification_summary,
        before,
        after,
        rollback,
    }
}

pub(crate) fn evidence_refs(payload: Option<&Value>) -> Vec<String> {
    payload
        .and_then(|value| value.get("evidence"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|evidence| {
            evidence
                .get("content_sha256")
                .and_then(Value::as_str)
                .map(|digest| format!("sha256:{digest}"))
                .or_else(|| {
                    evidence
                        .get("source")
                        .and_then(Value::as_str)
                        .map(|source| format!("source:{}", bounded_text(source, 200)))
                })
        })
        .take(64)
        .collect()
}

fn target_id(args: &Value, payload: Option<&Value>) -> Option<String> {
    const KEYS: [&str; 8] = [
        "target_id",
        "target",
        "host_id",
        "server_id",
        "trip_id",
        "proposal_id",
        "finding_id",
        "id",
    ];
    let output = payload.and_then(|value| value.get("output")).or(payload);
    [Some(args), output]
        .into_iter()
        .flatten()
        .find_map(|value| {
            KEYS.into_iter().find_map(|key| {
                value
                    .get(key)
                    .and_then(scalar_string)
                    .map(|value| bounded_text(&value, 512))
            })
        })
}

fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn safe_value_summary(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::String(value) => Some(bounded_text(value, 1_000)),
        Value::Array(values) => Some(format!("{} bounded item(s)", values.len().min(100))),
        Value::Object(values) => {
            let summary = values
                .iter()
                .filter(|(key, value)| !sensitive_key(key) && scalar_string(value).is_some())
                .take(12)
                .filter_map(|(key, value)| {
                    scalar_string(value)
                        .map(|value| format!("{}={}", human_label(key), bounded_text(&value, 100)))
                })
                .collect::<Vec<_>>()
                .join(", ");
            (!summary.is_empty()).then(|| bounded_text(&summary, 1_000))
        }
    }
}

fn sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
    [
        "password",
        "secret",
        "token",
        "authorization",
        "apikey",
        "privatekey",
        "credential",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn exception_for(
    analysis: &PayloadAnalysis,
    subject: &str,
    target_id: &str,
) -> Option<(String, String)> {
    match analysis.outcome.as_str() {
        "safe_stopped" => Some((
            "warning".into(),
            bounded_text(&format!("{subject} stopped safely for {target_id}"), 300),
        )),
        "failed" => Some((
            "error".into(),
            bounded_text(&format!("{subject} failed for {target_id}"), 300),
        )),
        _ => None,
    }
}

fn stage(stage: &str, state: &str, summary: impl Into<String>) -> StageEventInput {
    StageEventInput {
        stage: stage.into(),
        state: state.into(),
        summary: bounded_text(&summary.into(), 2_000),
        evidence_refs: Vec::new(),
    }
}

fn normalized_policy(value: &str) -> String {
    match value {
        "auto" | "review" | "deny" => value,
        _ => "unknown",
    }
    .into()
}

fn human_label(value: &str) -> String {
    value
        .replace(['.', '_', '-'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn bounded_text(value: &str, max: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max {
        value.to_string()
    } else {
        let mut bounded = value
            .chars()
            .take(max.saturating_sub(1))
            .collect::<String>();
        bounded.push('…');
        bounded
    }
}

pub struct ManagerWebhookWorkerHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl ManagerWebhookWorkerHandle {
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(10), self.join).await;
    }
}

pub fn spawn_webhook_worker(pool: PgPool) -> ManagerWebhookWorkerHandle {
    let (shutdown, receiver) = watch::channel(false);
    let join = tokio::spawn(webhook_worker_loop(pool, receiver));
    ManagerWebhookWorkerHandle { shutdown, join }
}

async fn webhook_worker_loop(pool: PgPool, mut shutdown: watch::Receiver<bool>) {
    let client = match reqwest::Client::builder()
        .timeout(WEBHOOK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(target: "manager_webhook", error = %error, "webhook client initialization failed");
            return;
        }
    };
    let mut ticker = tokio::time::interval(WEBHOOK_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() { return; }
            }
            _ = ticker.tick() => {
                if let Err(error) = deliver_due_webhooks(&pool, &client).await {
                    tracing::warn!(target: "manager_webhook", error = %error, "terminal exception delivery batch failed");
                }
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct WebhookPayload {
    event_id: Uuid,
    severity: String,
    summary: String,
    occurred_at: chrono::DateTime<Utc>,
    review_url: String,
}

async fn deliver_due_webhooks(
    pool: &PgPool,
    client: &reqwest::Client,
) -> Result<(), ManagerOversightError> {
    let deliveries = store::claim_due_deliveries(pool, 20).await?;
    for delivery in deliveries {
        let payload = WebhookPayload {
            event_id: delivery.exception_id,
            severity: delivery.severity.clone(),
            summary: delivery.summary.clone(),
            occurred_at: delivery.occurred_at,
            review_url: format!(
                "{}/web/review?tab=exceptions&id={}",
                delivery.review_base_url.trim_end_matches('/'),
                delivery.exception_id
            ),
        };
        let result = client
            .post(&delivery.endpoint_url)
            .header("Idempotency-Key", &delivery.idempotency_key)
            .json(&payload)
            .send()
            .await;
        let (state, status, error_code) = classify_delivery(result, delivery.attempt_count);
        store::finish_delivery(
            pool,
            delivery.delivery_id,
            delivery.attempt_count,
            state,
            status,
            error_code,
            retry_delay_seconds(delivery.attempt_count),
        )
        .await?;
    }
    Ok(())
}

fn classify_delivery(
    result: Result<reqwest::Response, reqwest::Error>,
    attempt_count: i32,
) -> (&'static str, Option<u16>, Option<&'static str>) {
    match result {
        Ok(response) if response.status().is_success() => {
            ("sent", Some(response.status().as_u16()), None)
        }
        Ok(response) => {
            let status = response.status();
            let retryable =
                status.is_server_error() || status.as_u16() == 408 || status.as_u16() == 429;
            if retryable && attempt_count < MAX_WEBHOOK_ATTEMPTS {
                (
                    "failed_retryable",
                    Some(status.as_u16()),
                    Some("http_retryable"),
                )
            } else {
                (
                    "failed_terminal",
                    Some(status.as_u16()),
                    Some("http_rejected"),
                )
            }
        }
        Err(error) => {
            let code = if error.is_timeout() {
                "timeout"
            } else if error.is_connect() {
                "connect"
            } else {
                "request"
            };
            if attempt_count < MAX_WEBHOOK_ATTEMPTS {
                ("failed_retryable", None, Some(code))
            } else {
                ("failed_terminal", None, Some(code))
            }
        }
    }
}

fn retry_delay_seconds(attempt_count: i32) -> i64 {
    match attempt_count {
        1 => 5,
        2 => 30,
        3 => 300,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, sync::Arc};

    use axum::{body::Bytes, extract::State, http::HeaderMap, routing::post, Router};
    use tokio::sync::{oneshot, Mutex};

    use super::*;

    type CapturedRequest = (HeaderMap, Bytes);
    type CaptureSender = oneshot::Sender<CapturedRequest>;

    #[test]
    fn payload_analysis_does_not_promote_unverified_success() {
        let unverified = analyze_payload(Some(&serde_json::json!({"status":"ok"})), None);
        assert_eq!(unverified.outcome, "succeeded");
        assert_eq!(unverified.verification, "not_provided");

        let verified = analyze_payload(
            Some(&serde_json::json!({
                "output": {"status":"applied","before":{"state":"old"},"after":{"state":"new"}},
                "outcomes": [{"status":"succeeded","summary":"State verified"}]
            })),
            None,
        );
        assert_eq!(verified.verification, "verified");
    }

    #[test]
    fn sensitive_before_after_fields_are_not_rendered() {
        let summary = safe_value_summary(&serde_json::json!({
            "state": "current",
            "api_token": "never-store-this",
            "private_key": "also-secret"
        }))
        .unwrap();
        assert!(summary.contains("State=current"));
        assert!(!summary.contains("never-store-this"));
        assert!(!summary.contains("also-secret"));
    }

    #[tokio::test]
    async fn webhook_posts_only_the_five_safe_fields_with_idempotency() {
        #[derive(Clone)]
        struct Capture {
            tx: Arc<Mutex<Option<CaptureSender>>>,
        }
        async fn capture(
            State(state): State<Capture>,
            headers: HeaderMap,
            body: Bytes,
        ) -> &'static str {
            if let Some(tx) = state.tx.lock().await.take() {
                let _ = tx.send((headers, body));
            }
            "ok"
        }
        let (tx, rx) = oneshot::channel();
        let state = Capture {
            tx: Arc::new(Mutex::new(Some(tx))),
        };
        let app = Router::new()
            .route("/hook", post(capture))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let event_id = Uuid::new_v4();
        let payload = WebhookPayload {
            event_id,
            severity: "error".into(),
            summary: "A bounded operation failed".into(),
            occurred_at: Utc::now(),
            review_url: format!("http://127.0.0.1:18085/web/review?tab=exceptions&id={event_id}"),
        };
        let response = reqwest::Client::new()
            .post(format!("http://{address}/hook"))
            .header("Idempotency-Key", format!("manager-exception:{event_id}"))
            .json(&payload)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let (headers, body) = rx.await.unwrap();
        assert_eq!(
            headers.get("Idempotency-Key").unwrap(),
            &format!("manager-exception:{event_id}")
        );
        let body: Value = serde_json::from_slice(&body).unwrap();
        let keys = body
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "event_id".to_string(),
                "occurred_at".to_string(),
                "review_url".to_string(),
                "severity".to_string(),
                "summary".to_string(),
            ])
        );
        server.abort();
    }
}
