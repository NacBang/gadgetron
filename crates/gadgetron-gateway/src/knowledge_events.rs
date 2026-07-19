//! CORE-T3 post-commit materializer for transactional Knowledge events.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use gadgetron_core::ingest::{BlobMetadata, BlobStore};
use gadgetron_knowledge::{
    source::{serialize_obsidian_note, FilesystemBlobStore},
    vault::{domain_note_relative_path, TenantVaultLayout},
};
use gadgetron_xaas::{
    knowledge_events::{self as events, FailureDisposition, KnowledgeEventRow},
    knowledge_sources::{self as sources, MaterializeIncidentSnapshot},
    knowledge_spaces::{self as spaces, EnsureVault, KnowledgeSpaceError, SpaceActor, SpaceRole},
    manager_oversight::{self as oversight, RecordOutcomeInput, StageEventInput},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{sync::watch, task::JoinHandle};
use uuid::Uuid;

use crate::{
    server::AppState,
    web::knowledge_jobs::{
        self as jobs_http, EnqueueJobOptions, StartBundleRole, StartJobRequest, StartRole,
    },
};

const LEASE_SECONDS: i32 = 30;
const IDLE_INTERVAL: Duration = Duration::from_millis(500);

pub struct KnowledgeEventWorkerHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl KnowledgeEventWorkerHandle {
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(10), self.join).await;
    }
}

pub fn spawn_worker(state: AppState) -> KnowledgeEventWorkerHandle {
    let (shutdown, receiver) = watch::channel(false);
    let worker_id = format!("knowledge-event-worker:{}", Uuid::new_v4());
    let join = tokio::spawn(worker_loop(state, receiver, worker_id));
    KnowledgeEventWorkerHandle { shutdown, join }
}

async fn worker_loop(state: AppState, mut shutdown: watch::Receiver<bool>, worker_id: String) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        let Some(pool) = state.pg_pool.as_ref() else {
            return;
        };
        if let Ok(Some(failed)) = events::next_unreported_failure(pool).await {
            if record_terminal_failure(
                pool,
                &failed,
                failed
                    .last_error
                    .as_deref()
                    .unwrap_or("Knowledge event materialization failed"),
            )
            .await
            {
                if let Err(error) = events::mark_oversight_recorded(pool, failed.id).await {
                    tracing::warn!(target: "knowledge_events", event_id = %failed.id, error = %error, "Knowledge event oversight receipt could not be recorded");
                }
            }
        }
        match events::lease_next(pool, &worker_id, LEASE_SECONDS).await {
            Ok(Some(event)) => match materialize(&state, &event).await {
                Ok((source_id, job_id)) => {
                    if let Err(error) =
                        events::complete(pool, event.id, &worker_id, source_id, job_id).await
                    {
                        tracing::warn!(target: "knowledge_events", event_id = %event.id, error = %error, "Knowledge event completion failed");
                    }
                }
                Err(error) => match events::fail(pool, event.id, &worker_id, &error).await {
                    Ok(FailureDisposition::Retry) => {
                        tracing::warn!(target: "knowledge_events", event_id = %event.id, error = %error, "Knowledge event materialization will retry");
                    }
                    Ok(FailureDisposition::Terminal) => {
                        if record_terminal_failure(pool, &event, &error).await {
                            if let Err(persistence) =
                                events::mark_oversight_recorded(pool, event.id).await
                            {
                                tracing::warn!(target: "knowledge_events", event_id = %event.id, error = %persistence, "Knowledge event oversight receipt could not be recorded");
                            }
                        }
                    }
                    Err(persistence) => {
                        tracing::warn!(target: "knowledge_events", event_id = %event.id, error = %persistence, "Knowledge event failure could not be persisted");
                    }
                },
            },
            Ok(None) => {
                tokio::select! {
                    _ = shutdown.changed() => {}
                    _ = tokio::time::sleep(IDLE_INTERVAL) => {}
                }
            }
            Err(error) => {
                tracing::warn!(target: "knowledge_events", error = %error, "Knowledge event lease failed");
                tokio::select! {
                    _ = shutdown.changed() => {}
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
        }
    }
}

async fn materialize(state: &AppState, event: &KnowledgeEventRow) -> Result<(Uuid, Uuid), String> {
    let pool = state
        .pg_pool
        .as_ref()
        .ok_or_else(|| "Knowledge event PostgreSQL is unavailable".to_string())?;
    let workbench = state
        .workbench
        .as_ref()
        .ok_or_else(|| "Knowledge event Workbench is unavailable".to_string())?;
    let manager = workbench
        .runtime_manager
        .as_ref()
        .ok_or_else(|| "Knowledge event Bundle runtime is unavailable".to_string())?;
    let contract = manager
        .knowledge_role_execution_contract(&event.researcher_bundle_id, &event.researcher_role_id)
        .await
        .map_err(|error| format!("{error:?}"))?;
    if contract.role.core_role.as_str() != "researcher" {
        return Err("signed Knowledge event role is not a Researcher".to_string());
    }
    let layout = workbench
        .vault_layout
        .as_ref()
        .ok_or_else(|| "Knowledge event Vault layout is unavailable".to_string())?;
    let service_actor = SpaceActor {
        tenant_id: event.tenant_id,
        user_id: event.service_actor_user_id,
    };
    let on_behalf_actor = SpaceActor {
        tenant_id: event.tenant_id,
        user_id: event.requested_by_user_id,
    };
    let vault = spaces::ensure_service_bundle_vault(
        pool,
        service_actor,
        on_behalf_actor,
        event.acting_space_id,
        EnsureVault {
            home_bundle_id: event.output_vault_bundle.clone(),
            knowledge_schema_id: event.knowledge_schema_id.clone(),
            schema_version: 1,
        },
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    if vault.home_bundle_id != event.output_vault_bundle || vault.owner_state != "enabled" {
        return Err("declared Bundle Domain Vault is unavailable".to_string());
    }

    let source = match sources::get_source(
        pool,
        on_behalf_actor,
        event.id,
        SpaceRole::Contributor,
        true,
    )
    .await
    {
        Ok(source) if source.status == "extracted" && source.source_kind == "incident_snapshot" => {
            source
        }
        Ok(_) => return Err("incident snapshot Source is only partially materialized".to_string()),
        Err(KnowledgeSpaceError::NotFound) => {
            materialize_source(pool, layout, on_behalf_actor, &vault, event).await?
        }
        Err(error) => return Err(error.to_string()),
    };

    let mut extra_input = BTreeMap::new();
    extra_input.insert(
        "originating_subject".to_string(),
        serde_json::json!({
            "owner_bundle": event.publisher_bundle_id,
            "subject_kind": canonical_subject_kind(event),
            "subject_id": event.subject_id,
            "subject_revision": event.subject_revision,
        }),
    );
    extra_input.insert(
        "knowledge_event_id".to_string(),
        Value::String(event.id.to_string()),
    );
    let job = jobs_http::enqueue_job(
        state,
        on_behalf_actor,
        event.acting_space_id,
        StartJobRequest {
            role: StartRole::Researcher,
            output_vault_id: vault.id,
            question: format!(
                "Review the closed {} {} revision {} as immutable incident evidence. Distinguish observed facts, verified outcomes, uncertainty, and a reusable Lesson candidate.",
                event.subject_kind, event.subject_id, event.subject_revision
            ),
            collection_id: None,
            collection_revision: None,
            source_ids: vec![source.id],
            outcome_ids: Vec::new(),
            lesson_revision: None,
            bundle_role: Some(StartBundleRole {
                bundle_id: event.researcher_bundle_id.clone(),
                role_id: event.researcher_role_id.clone(),
            }),
        },
        EnqueueJobOptions {
            kind: gadgetron_xaas::knowledge_jobs::KnowledgeJobKind::Event,
            idempotency_key: Some(format!(
                "knowledge-event:{}:{}:{}:{}:{}",
                event.publisher_bundle_id,
                event.subject_kind,
                event.subject_id,
                event.subject_revision,
                event.researcher_role_id
            )),
            extra_input,
        },
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    Ok((source.id, job.id))
}

fn canonical_subject_kind(event: &KnowledgeEventRow) -> String {
    format!("{}.{}", event.publisher_bundle_id, event.subject_kind)
}

async fn materialize_source(
    pool: &sqlx::PgPool,
    layout: &Arc<TenantVaultLayout>,
    actor: SpaceActor,
    vault: &spaces::KnowledgeVaultRow,
    event: &KnowledgeEventRow,
) -> Result<sources::KnowledgeSourceRow, String> {
    let bytes = serde_json::to_vec(&event.snapshot).map_err(|error| error.to_string())?;
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
    if digest != event.snapshot_hash {
        return Err("Knowledge event snapshot hash changed after enqueue".to_string());
    }
    let object_id = deterministic_object_id(event.id);
    let path = domain_note_relative_path(&event.source_path_prefix, &event.source_title, object_id)
        .map_err(|error| error.to_string())?;
    let original_name = path
        .rsplit('/')
        .next()
        .unwrap_or("incident-snapshot.md")
        .to_string();
    let store = FilesystemBlobStore::new(pool.clone(), layout.root());
    let blob = store
        .put(
            &bytes,
            &BlobMetadata {
                tenant_id: actor.tenant_id.to_string(),
                content_type: "application/json".to_string(),
                filename: original_name.clone(),
                byte_size: bytes.len() as u64,
                imported_by: actor.user_id.to_string(),
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    let properties = BTreeMap::from([
        ("id".to_string(), serde_json::json!(object_id)),
        ("title".to_string(), serde_json::json!(event.source_title)),
        ("kind".to_string(), serde_json::json!("note")),
        ("status".to_string(), serde_json::json!("draft")),
        (
            "space_id".to_string(),
            serde_json::json!(event.acting_space_id),
        ),
        (
            "home_bundle_id".to_string(),
            serde_json::json!(event.output_vault_bundle),
        ),
        ("source_ids".to_string(), serde_json::json!([event.id])),
        (
            "source_hashes".to_string(),
            serde_json::json!([event.snapshot_hash]),
        ),
        (
            "source_kind".to_string(),
            serde_json::json!("incident_snapshot"),
        ),
        (
            "subject".to_string(),
            serde_json::json!({
                "owner_bundle": event.publisher_bundle_id,
                "subject_kind": event.subject_kind,
                "subject_id": event.subject_id,
                "subject_revision": event.subject_revision,
            }),
        ),
    ]);
    let pretty =
        serde_json::to_string_pretty(&event.snapshot).map_err(|error| error.to_string())?;
    let note = serialize_obsidian_note(
        &properties,
        &format!("# {}\n\n```json\n{}\n```", event.source_title, pretty),
    )
    .map_err(|error| error.to_string())?;
    let note_hash = hex::encode(Sha256::digest(note.as_bytes()));
    let repository = layout
        .open_or_init(actor.tenant_id)
        .map_err(|error| error.to_string())?;
    repository
        .ensure_domain(vault.space_id, &vault.home_bundle_id)
        .map_err(|error| error.to_string())?;
    repository
        .write_note(
            vault.space_id,
            &vault.home_bundle_id,
            &path,
            note.as_bytes(),
            "knowledge: materialize incident snapshot Source",
        )
        .map_err(|error| error.to_string())?;
    sources::materialize_incident_snapshot(
        pool,
        actor,
        MaterializeIncidentSnapshot {
            source_id: event.id,
            object_id,
            vault_id: vault.id,
            title: event.source_title.clone(),
            original_name,
            final_uri: format!(
                "gadgetron://{}/{}/{}@{}",
                event.publisher_bundle_id,
                event.subject_kind,
                event.subject_id,
                event.subject_revision
            ),
            content_type: "application/json".to_string(),
            byte_size: bytes.len() as i64,
            content_hash: digest,
            blob_id: blob.id.0,
            path,
            note_content_hash: note_hash,
        },
    )
    .await
    .map_err(|error| error.to_string())
}

fn deterministic_object_id(event_id: Uuid) -> Uuid {
    let digest = Sha256::digest([event_id.as_bytes().as_slice(), b":source-object"].concat());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

async fn record_terminal_failure(
    pool: &sqlx::PgPool,
    event: &KnowledgeEventRow,
    detail: &str,
) -> bool {
    let summary: String = detail.chars().take(1_000).collect();
    let result = oversight::record_outcome(
        pool,
        RecordOutcomeInput {
            tenant_id: event.tenant_id,
            source_kind: "knowledge_event".to_string(),
            source_id: event.id.to_string(),
            actor_user_id: Some(event.requested_by_user_id),
            agent_label: "Knowledge event bridge".to_string(),
            agent_role: "researcher".to_string(),
            goal: "Materialize a domain event as immutable Knowledge evidence".to_string(),
            target_kind: "knowledge_revision".to_string(),
            target_id: event.subject_id.clone(),
            target_revision: Some(event.subject_revision.clone()),
            policy_decision: "auto".to_string(),
            policy_revision: None,
            evidence_refs: vec![format!("knowledge-event:{}", event.id)],
            current_stage: "execute".to_string(),
            outcome: "failed".to_string(),
            verification_state: "failed".to_string(),
            action_summary: summary.clone(),
            before_summary: None,
            after_summary: None,
            rollback_summary: Some("No partial Source registry row was committed".to_string()),
            duration_ms: 0,
            cost_minor_units: 0,
            events: vec![StageEventInput {
                stage: "execute".to_string(),
                state: "failed".to_string(),
                summary: summary.clone(),
                evidence_refs: vec![format!("knowledge-event:{}", event.id)],
            }],
            exception_severity: Some("error".to_string()),
            exception_summary: Some(summary),
        },
    )
    .await;
    if let Err(error) = result {
        tracing::warn!(target: "knowledge_events", event_id = %event.id, error = %error, "terminal Knowledge event was not exposed to Manager oversight");
        false
    } else {
        true
    }
}
