use std::time::Duration;

use chrono::{DateTime, Days, TimeZone, Utc};
use gadgetron_core::policy::{
    EnforcementPath, GadgetPolicyMetadata, PolicyAuthorization, PolicyEffect,
    PolicyEvaluationRequest, PolicyIdentity, PolicyReviewState, PolicyRisk,
};
use gadgetron_xaas::{
    knowledge_collections::{
        self as collections, CollectionRunControl, CompleteCollectionItem,
        KnowledgeCollectionRunItemRow, KnowledgeCollectionRunRow,
    },
    knowledge_sources as sources,
    knowledge_spaces::{SpaceActor, SpaceRole},
};
use tokio::{sync::watch, task::JoinHandle};
use uuid::Uuid;

use crate::{
    policy_enforcement::background_input,
    server::AppState,
    web::{
        bundle_runtime::BundleRuntimeState,
        knowledge_sources::{capture_url_source, CaptureUrlSource, SourceRetention},
        workbench::WorkbenchHttpError,
    },
};

const LEASE_SECONDS: i32 = 30;
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5);
const IDLE_INTERVAL: Duration = Duration::from_millis(500);
const MAX_SOURCE_BYTES: i64 = 16_777_216;

pub struct KnowledgeCollectionWorkerHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl KnowledgeCollectionWorkerHandle {
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(25), self.join).await;
    }
}

pub fn spawn_worker(state: AppState) -> KnowledgeCollectionWorkerHandle {
    let (shutdown, receiver) = watch::channel(false);
    let join = tokio::spawn(worker_loop(
        state,
        receiver,
        format!("collection-worker:{}", Uuid::new_v4()),
    ));
    KnowledgeCollectionWorkerHandle { shutdown, join }
}

async fn worker_loop(state: AppState, mut shutdown: watch::Receiver<bool>, worker_id: String) {
    let Some(pool) = state.pg_pool.clone() else {
        return;
    };
    let mut discovery = tokio::time::interval(DISCOVERY_INTERVAL);
    discovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *shutdown.borrow() {
            return;
        }
        tokio::select! {
            _ = discovery.tick() => enqueue_due(&state).await,
            result = collections::lease_next(&pool, &worker_id, LEASE_SECONDS) => {
                match result {
                    Ok(Some(run)) => run_leased(&state, &worker_id, run, &mut shutdown).await,
                    Ok(None) => {
                        tokio::select! {
                            _ = shutdown.changed() => {},
                            _ = tokio::time::sleep(IDLE_INTERVAL) => {},
                        }
                    }
                    Err(error) => {
                        tracing::warn!(target: "knowledge_collections", detail = %error, "collection lease failed");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
    }
}

async fn enqueue_due(state: &AppState) {
    let Some(pool) = state.pg_pool.as_ref() else {
        return;
    };
    let due = match collections::due_collections(pool, 100).await {
        Ok(due) => due,
        Err(error) => {
            tracing::warn!(target: "knowledge_collections", detail = %error, "collection schedule discovery failed");
            return;
        }
    };
    for collection in due {
        let Some(schedule) = collection.schedule.as_deref() else {
            continue;
        };
        let next = match next_schedule_after(schedule, Utc::now()) {
            Ok(next) => next,
            Err(error) => {
                tracing::warn!(target: "knowledge_collections", collection_id = %collection.id, detail = %error, "collection schedule is unsupported");
                continue;
            }
        };
        let policy = match active_policy_revision(state, collection.tenant_id).await {
            Ok(policy) => policy,
            Err(error) => {
                tracing::warn!(target: "knowledge_collections", collection_id = %collection.id, detail = %error, "collection schedule has no active policy");
                continue;
            }
        };
        if let Err(error) = collections::enqueue_scheduled_run(pool, collection, policy, next).await
        {
            tracing::warn!(target: "knowledge_collections", detail = %error, "scheduled collection enqueue failed");
        }
    }
}

async fn run_leased(
    state: &AppState,
    worker_id: &str,
    run: KnowledgeCollectionRunRow,
    shutdown: &mut watch::Receiver<bool>,
) {
    let result = execute_run(state, worker_id, &run, shutdown).await;
    match result {
        Ok(reason) => {
            if let Err(error) = collections::finish_run(
                state.pg_pool.as_ref().expect("worker requires PostgreSQL"),
                run.id,
                worker_id,
                reason.as_deref(),
            )
            .await
            {
                tracing::warn!(target: "knowledge_collections", run_id = %run.id, detail = %error, "collection completion failed");
            }
        }
        Err(ExecuteRunError::Shutdown) => {
            if let Err(error) = collections::release_lease(
                state.pg_pool.as_ref().expect("worker requires PostgreSQL"),
                run.id,
                worker_id,
            )
            .await
            {
                tracing::warn!(target: "knowledge_collections", run_id = %run.id, detail = %error, "collection shutdown lease release failed");
            }
        }
        Err(ExecuteRunError::Terminal(detail)) => {
            if let Err(error) = collections::finish_run(
                state.pg_pool.as_ref().expect("worker requires PostgreSQL"),
                run.id,
                worker_id,
                Some(&detail),
            )
            .await
            {
                tracing::warn!(target: "knowledge_collections", run_id = %run.id, detail = %error, "collection failure transition failed");
            }
        }
    }
}

#[derive(Debug)]
enum ExecuteRunError {
    Shutdown,
    Terminal(String),
}

async fn execute_run(
    state: &AppState,
    worker_id: &str,
    initial: &KnowledgeCollectionRunRow,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<Option<String>, ExecuteRunError> {
    validate_profile_snapshot(state, initial).await?;
    let pool = state
        .pg_pool
        .as_ref()
        .ok_or_else(|| ExecuteRunError::Terminal("collection database is unavailable".into()))?;
    let actor = SpaceActor {
        tenant_id: initial.tenant_id,
        user_id: initial.on_behalf_of_user_id,
    };
    collections::validate_execution_actor(pool, actor, initial)
        .await
        .map_err(|error| {
            ExecuteRunError::Terminal(format!(
                "collection actor or Space authority is unavailable: {error}"
            ))
        })?;
    authorize_collection(state, initial).await?;

    loop {
        if *shutdown.borrow() {
            return Err(ExecuteRunError::Shutdown);
        }
        let (run, control) = collections::heartbeat(pool, initial.id, worker_id, LEASE_SECONDS)
            .await
            .map_err(|error| ExecuteRunError::Terminal(error.to_string()))?;
        match control {
            CollectionRunControl::Continue => {}
            CollectionRunControl::CancelRequested => {
                return Ok(Some("collection cancelled by user".into()))
            }
            CollectionRunControl::WallTimeExceeded => {
                return Ok(Some("collection wall-time budget exhausted".into()))
            }
            CollectionRunControl::ByteBudgetExceeded => {
                return Ok(Some("collection byte budget exhausted".into()))
            }
        }
        let Some(item) = collections::claim_next_item(pool, initial.id, worker_id)
            .await
            .map_err(|error| ExecuteRunError::Terminal(error.to_string()))?
        else {
            return Ok(None);
        };
        let remaining_bytes = run.max_bytes.saturating_sub(run.used_bytes);
        if remaining_bytes <= 0 {
            return Ok(Some("collection byte budget exhausted".into()));
        }
        let Some(remaining_wall) = remaining_wall(&run) else {
            return Ok(Some("collection wall-time budget exhausted".into()));
        };
        let outcome =
            capture_item(state, actor, &run, &item, remaining_bytes, remaining_wall).await;
        let stop = outcome.stop;
        collections::complete_item(pool, run.id, worker_id, item.id, outcome.result)
            .await
            .map_err(|error| ExecuteRunError::Terminal(error.to_string()))?;
        if let Some(detail) = stop {
            return Err(ExecuteRunError::Terminal(detail));
        }
    }
}

struct CaptureOutcome {
    result: CompleteCollectionItem,
    stop: Option<String>,
}

async fn capture_item(
    state: &AppState,
    actor: SpaceActor,
    run: &KnowledgeCollectionRunRow,
    item: &KnowledgeCollectionRunItemRow,
    remaining_bytes: i64,
    remaining_wall: Duration,
) -> CaptureOutcome {
    let url = match provider_request(&run.connector, &item.locator) {
        Ok(url) => url,
        Err((code, detail)) => {
            return CaptureOutcome {
                result: failed_item(code, &detail),
                stop: None,
            }
        }
    };
    let request = CaptureUrlSource {
        vault_id: run.output_vault_id,
        url,
        title: item.title.clone(),
        max_bytes: usize::try_from(remaining_bytes.min(MAX_SOURCE_BYTES)).unwrap_or(1),
        allowed_domains: Some(run.allowed_domains.clone()),
        timeout: remaining_wall.min(Duration::from_secs(20)),
        retention: if run.connector == "core-social-api" {
            SourceRetention::Purgeable
        } else {
            SourceRetention::Versioned
        },
        conversation_id: None,
    };
    match capture_url_source(state, actor, request).await {
        Ok(captured) => {
            let fetched_at = captured.source.fetched_at.unwrap_or_else(Utc::now);
            let fresh_until = fetched_at
                .checked_add_signed(chrono::Duration::seconds(run.freshness_seconds))
                .unwrap_or(fetched_at);
            let unchanged = match item.previous_source_id {
                Some(previous_id) => sources::get_source(
                    state.pg_pool.as_ref().expect("worker requires PostgreSQL"),
                    actor,
                    previous_id,
                    SpaceRole::Viewer,
                    false,
                )
                .await
                .ok()
                .is_some_and(|previous| previous.content_hash == captured.source.content_hash),
                None => false,
            };
            CaptureOutcome {
                result: CompleteCollectionItem {
                    status: if unchanged { "unchanged" } else { "captured" }.into(),
                    source_id: Some(captured.source.id),
                    canonical_locator: captured.source.final_uri,
                    content_hash: captured.source.content_hash,
                    byte_size: captured.source.byte_size,
                    http_status: captured.http_status,
                    fetched_at: Some(fetched_at),
                    fresh_until: Some(fresh_until),
                    deletion_observed_at: None,
                    failure_code: None,
                    failure_detail: None,
                },
                stop: None,
            }
        }
        Err(WorkbenchHttpError::KnowledgeSourceFailed {
            source_id,
            code,
            detail,
        }) => capture_failure(state, actor, source_id, code, detail).await,
        Err(WorkbenchHttpError::KnowledgeForbidden | WorkbenchHttpError::KnowledgeNotFound) => {
            let detail = "collection actor lost access to its Space or Vault".to_string();
            CaptureOutcome {
                result: failed_item("collection_actor_forbidden", &detail),
                stop: Some(detail),
            }
        }
        Err(error) => {
            let detail = format!("collection source capture failed: {error:?}");
            CaptureOutcome {
                result: failed_item("collection_capture_failed", &detail),
                stop: Some(detail),
            }
        }
    }
}

fn provider_request(connector: &str, locator: &str) -> Result<String, (&'static str, String)> {
    if !matches!(connector, "core-community-api" | "core-social-api") {
        return Ok(locator.to_string());
    }
    let mut url = reqwest::Url::parse(locator).map_err(|_| {
        (
            "provider_locator_invalid",
            "Provider request URL is invalid".into(),
        )
    })?;
    let mut window_days = None;
    let provider_host = url.host_str().map(str::to_string);
    let pairs = url
        .query_pairs()
        .filter_map(|(key, value)| {
            if key == "gadgetron_window_days" {
                window_days = value.parse::<u32>().ok();
                None
            } else {
                Some((key.into_owned(), value.into_owned()))
            }
        })
        .collect::<Vec<_>>();
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(&key, &value);
        }
        let window_days = window_days.ok_or((
            "provider_query_invalid",
            "Provider query has no signed lookback window".into(),
        ))?;
        match provider_host.as_deref() {
            Some("api.stackexchange.com") => {
                let from = Utc::now()
                    .checked_sub_signed(chrono::Duration::days(i64::from(window_days)))
                    .ok_or((
                        "provider_query_invalid",
                        "Provider lookback window is invalid".into(),
                    ))?;
                query.append_pair("fromdate", &from.timestamp().to_string());
            }
            Some("oauth.reddit.com") => {
                query.append_pair("t", reddit_window(window_days));
            }
            Some("api.bsky.app") if connector == "core-social-api" => {
                let since = Utc::now()
                    .checked_sub_signed(chrono::Duration::days(i64::from(window_days)))
                    .ok_or((
                        "provider_query_invalid",
                        "Provider lookback window is invalid".into(),
                    ))?;
                query.append_pair("since", &since.to_rfc3339());
            }
            _ => {
                return Err((
                    "provider_locator_invalid",
                    "Provider host is outside the signed Core connector".into(),
                ))
            }
        }
    }
    if provider_host.as_deref() == Some("oauth.reddit.com") {
        return Err((
            "provider_retention_unsupported",
            "Reddit collection is unavailable until Source storage can purge deleted content history"
                .into(),
        ));
    }
    Ok(url.to_string())
}

fn reddit_window(days: u32) -> &'static str {
    match days {
        0..=1 => "day",
        2..=7 => "week",
        8..=31 => "month",
        32..=365 => "year",
        _ => "all",
    }
}

fn remaining_wall(run: &KnowledgeCollectionRunRow) -> Option<Duration> {
    let started_at = run.started_at?;
    let deadline = started_at
        .checked_add_signed(chrono::Duration::seconds(i64::from(run.max_wall_seconds)))?;
    deadline.signed_duration_since(Utc::now()).to_std().ok()
}

async fn capture_failure(
    state: &AppState,
    actor: SpaceActor,
    source_id: Uuid,
    code: String,
    detail: String,
) -> CaptureOutcome {
    let pool = state.pg_pool.as_ref().expect("worker requires PostgreSQL");
    let source = sources::get_source(pool, actor, source_id, SpaceRole::Viewer, false)
        .await
        .ok();
    let attempts = sources::source_attempts(pool, actor, source_id)
        .await
        .unwrap_or_default();
    let http_status = attempts
        .iter()
        .rev()
        .find_map(|attempt| attempt.http_status);
    let deleted = matches!(http_status, Some(404 | 410));
    CaptureOutcome {
        result: CompleteCollectionItem {
            status: if deleted { "deleted" } else { "failed" }.into(),
            source_id: Some(source_id),
            canonical_locator: source.as_ref().and_then(|source| source.final_uri.clone()),
            content_hash: source
                .as_ref()
                .and_then(|source| source.content_hash.clone()),
            byte_size: source.as_ref().and_then(|source| source.byte_size),
            http_status,
            fetched_at: source.as_ref().and_then(|source| source.fetched_at),
            fresh_until: None,
            deletion_observed_at: deleted.then(Utc::now),
            failure_code: (!deleted).then_some(code),
            failure_detail: Some(detail),
        },
        stop: None,
    }
}

fn failed_item(code: &str, detail: &str) -> CompleteCollectionItem {
    CompleteCollectionItem {
        status: "failed".into(),
        source_id: None,
        canonical_locator: None,
        content_hash: None,
        byte_size: None,
        http_status: None,
        fetched_at: None,
        fresh_until: None,
        deletion_observed_at: None,
        failure_code: Some(code.into()),
        failure_detail: Some(detail.into()),
    }
}

async fn validate_profile_snapshot(
    state: &AppState,
    run: &KnowledgeCollectionRunRow,
) -> Result<(), ExecuteRunError> {
    let manager = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.runtime_manager.as_ref())
        .ok_or_else(|| ExecuteRunError::Terminal("Bundle runtime is unavailable".into()))?;
    let status = manager
        .status(&run.bundle_id)
        .await
        .map_err(|error| ExecuteRunError::Terminal(format!("Bundle is unavailable: {error:?}")))?;
    if status.state != BundleRuntimeState::Enabled {
        return Err(ExecuteRunError::Terminal(
            "Bundle collection owner is not enabled".into(),
        ));
    }
    let projection = manager
        .knowledge_profiles(&run.bundle_id)
        .map_err(|error| {
            ExecuteRunError::Terminal(format!("Bundle profile is unavailable: {error:?}"))
        })?;
    let profile = projection
        .collections
        .iter()
        .find(|candidate| candidate.profile.id.as_str() == run.profile_id)
        .ok_or_else(|| ExecuteRunError::Terminal("signed collection profile was removed".into()))?;
    let budget_matches = i32::try_from(profile.profile.budget.max_sources).ok()
        == Some(run.max_sources)
        && i64::try_from(profile.profile.budget.max_bytes).ok() == Some(run.max_bytes)
        && i32::try_from(profile.profile.budget.max_wall_seconds).ok()
            == Some(run.max_wall_seconds);
    if projection.package_manifest_sha256 != run.package_manifest_sha256
        || profile.recipe_sha256 != run.recipe_sha256
        || profile.profile.recipe_asset.as_str() != run.recipe_asset_id
        || profile.profile.connector.as_str() != run.connector
        || profile
            .profile
            .source_classes
            .iter()
            .map(|value| value.as_str())
            .ne(run.source_classes.iter().map(String::as_str))
        || profile.profile.allowlisted_domains != run.allowed_domains
        || i64::try_from(profile.profile.freshness_seconds).ok() != Some(run.freshness_seconds)
        || !budget_matches
    {
        return Err(ExecuteRunError::Terminal(
            "signed collection profile changed after enqueue".into(),
        ));
    }
    Ok(())
}

async fn authorize_collection(
    state: &AppState,
    run: &KnowledgeCollectionRunRow,
) -> Result<(), ExecuteRunError> {
    let evaluator = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.policy_evaluator.as_ref())
        .ok_or_else(|| {
            ExecuteRunError::Terminal("collection policy evaluator is unavailable".into())
        })?;
    let pinned = PolicyIdentity::from_revision_ref(&run.tool_policy_revision)
        .map_err(|error| ExecuteRunError::Terminal(format!("invalid pinned policy: {error}")))?;
    let input = background_input(
        "knowledge.collection.fetch",
        &run.bundle_id,
        GadgetPolicyMetadata {
            effect: PolicyEffect::Read,
            risk: PolicyRisk::Low,
            requested_scopes: Default::default(),
            requires_evidence: false,
            outcome_verifiable: true,
            outcome_ref: None,
            rollback_available: false,
            rollback_ref: None,
        },
        std::iter::empty(),
    )
    .and_then(|input| {
        input
            .with_parameters(&serde_json::json!({
                "collection_id": run.collection_id,
                "run_id": run.id,
                "space_id": run.space_id,
                "source_count": run.max_sources,
            }))
            .map_err(|error| gadgetron_core::policy::PolicyEvaluationError {
                code: "policy_input_invalid",
                detail: error.to_string(),
            })
    })
    .map_err(|error| ExecuteRunError::Terminal(error.detail))?;
    let evaluation = evaluator
        .evaluate(PolicyEvaluationRequest {
            tenant_id: run.tenant_id,
            path: EnforcementPath::KnowledgeBackground,
            input,
            pinned_policy: Some(pinned),
            approval_id: None,
            review_state: PolicyReviewState::Pending,
        })
        .await
        .map_err(|error| ExecuteRunError::Terminal(error.detail))?;
    match evaluation.authorization {
        PolicyAuthorization::Auto | PolicyAuthorization::ApprovedReview => Ok(()),
        PolicyAuthorization::Denied => Err(ExecuteRunError::Terminal(format!(
            "collection denied by policy: {}",
            evaluation.trace.reason
        ))),
        PolicyAuthorization::PendingReview => Err(ExecuteRunError::Terminal(format!(
            "collection stopped for Review: {}",
            evaluation.trace.reason
        ))),
    }
}

async fn active_policy_revision(state: &AppState, tenant_id: Uuid) -> Result<String, String> {
    state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.policy_evaluator.as_ref())
        .ok_or_else(|| "policy evaluator is unavailable".to_string())?
        .active_identity(tenant_id)
        .await
        .map(|identity| identity.to_revision_ref())
        .map_err(|error| error.detail)
}

pub(crate) fn next_schedule_after(
    schedule: &str,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, String> {
    let fields: Vec<_> = schedule.split_whitespace().collect();
    if fields.len() != 5 || fields[2..] != ["*", "*", "*"] {
        return Err("only minute intervals and daily UTC schedules are supported".into());
    }
    if fields[1] == "*" {
        let minutes = fields[0]
            .strip_prefix("*/")
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|value| (1..=60).contains(value))
            .ok_or_else(|| "minute interval must be */1 through */60".to_string())?;
        let step = minutes * 60;
        let next = (now.timestamp().div_euclid(step) + 1) * step;
        return DateTime::from_timestamp(next, 0)
            .ok_or_else(|| "schedule timestamp is outside the supported range".to_string());
    }
    let minute = fields[0]
        .parse::<u32>()
        .ok()
        .filter(|value| *value < 60)
        .ok_or_else(|| "daily schedule minute must be 0 through 59".to_string())?;
    let hour = fields[1]
        .parse::<u32>()
        .ok()
        .filter(|value| *value < 24)
        .ok_or_else(|| "daily schedule hour must be 0 through 23".to_string())?;
    let date = now.date_naive();
    let candidate = Utc.from_utc_datetime(
        &date
            .and_hms_opt(hour, minute, 0)
            .ok_or_else(|| "daily schedule time is invalid".to_string())?,
    );
    if candidate > now {
        return Ok(candidate);
    }
    let next_date = date
        .checked_add_days(Days::new(1))
        .ok_or_else(|| "schedule date is outside the supported range".to_string())?;
    Ok(Utc.from_utc_datetime(
        &next_date
            .and_hms_opt(hour, minute, 0)
            .ok_or_else(|| "daily schedule time is invalid".to_string())?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_schedule_is_bounded_and_deterministic() {
        let now = DateTime::parse_from_rfc3339("2026-07-13T05:59:30Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            next_schedule_after("0 6 * * *", now).unwrap(),
            DateTime::parse_from_rfc3339("2026-07-13T06:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
        assert_eq!(
            next_schedule_after("*/15 * * * *", now).unwrap(),
            DateTime::parse_from_rfc3339("2026-07-13T06:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
        assert!(next_schedule_after("0 6 * * MON", now).is_err());
    }

    #[test]
    fn provider_request_resolves_window_only_at_fetch_time() {
        let locator = "https://api.stackexchange.com/2.3/search/advanced?site=stackoverflow&q=rust&gadgetron_window_days=30";
        let resolved = provider_request("core-community-api", locator).unwrap();
        let url = reqwest::Url::parse(&resolved).unwrap();
        let pairs = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            pairs.get("site").map(|value| value.as_ref()),
            Some("stackoverflow")
        );
        assert!(pairs.contains_key("fromdate"));
        assert!(!pairs.contains_key("gadgetron_window_days"));

        let reddit = "https://oauth.reddit.com/r/rust/search.json?q=async&gadgetron_window_days=7";
        assert_eq!(
            provider_request("core-community-api", reddit)
                .unwrap_err()
                .0,
            "provider_retention_unsupported"
        );

        let bluesky = "https://api.bsky.app/xrpc/app.bsky.feed.searchPosts?q=linux&sort=latest&limit=5&gadgetron_window_days=7";
        let resolved = provider_request("core-social-api", bluesky).unwrap();
        let url = reqwest::Url::parse(&resolved).unwrap();
        let pairs = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert!(pairs.contains_key("since"));
        assert!(!pairs.contains_key("gadgetron_window_days"));
    }
}
