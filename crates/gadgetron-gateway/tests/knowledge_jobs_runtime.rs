use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use gadgetron_gateway::knowledge_jobs::{
    self as runtime, AgentExecution, AgentExecutionError, AgentInvocation, KnowledgeAgentExecutor,
};
use gadgetron_knowledge::vault::{TenantVaultLayout, VaultNoteWrite};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    knowledge_jobs::{
        self as jobs, BundleRoleSnapshot, EnqueueKnowledgeJob, JobBudget, KnowledgeJobKind,
        KnowledgeJobRole, MaterializedObjectInput, RuntimeSnapshot,
    },
    knowledge_sources::{self as sources, AttachSourceBlob, CreateSource},
    knowledge_spaces::{self as spaces, CreateProject, EnsureVault, SpaceActor},
};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

#[derive(Debug)]
struct ScriptedExecutor {
    source_id: Uuid,
    invocations: Mutex<Vec<(String, Vec<String>)>>,
}

#[derive(Debug)]
struct BlockingExecutor;

#[derive(Debug)]
struct PartialBlockingExecutor {
    started: Notify,
}

#[derive(Debug)]
struct SourceScoutExecutor {
    invocations: Mutex<Vec<(String, String, Vec<String>)>>,
}

#[derive(Debug)]
struct InsightExecutor {
    source_ids: [Uuid; 2],
    outcome_id: Uuid,
    invocations: Mutex<Vec<(String, Vec<String>)>>,
}

#[derive(Debug)]
struct LessonRevisionExecutor {
    source_id: Uuid,
    outcome_id: Uuid,
    lesson_id: Uuid,
    lesson_revision: i64,
    prompts: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl KnowledgeAgentExecutor for BlockingExecutor {
    async fn execute(
        &self,
        _request: AgentInvocation,
    ) -> Result<AgentExecution, AgentExecutionError> {
        std::future::pending().await
    }
}

#[async_trait]
impl KnowledgeAgentExecutor for PartialBlockingExecutor {
    async fn execute(
        &self,
        request: AgentInvocation,
    ) -> Result<AgentExecution, AgentExecutionError> {
        request.output_capture.record(
            "{\"title\":\"Partial recovery dossier\",\"summary\":\"Agent was still researching\"",
        );
        self.started.notify_one();
        std::future::pending().await
    }
}

#[async_trait]
impl KnowledgeAgentExecutor for SourceScoutExecutor {
    async fn execute(
        &self,
        request: AgentInvocation,
    ) -> Result<AgentExecution, AgentExecutionError> {
        self.invocations.lock().await.push((
            request.job.role.clone(),
            request.prompt,
            request.allowed_tools,
        ));
        Ok(AgentExecution {
            text: serde_json::json!({
                "title": "Recovery source plan",
                "summary": "Add official guidance and field reports before research.",
                "coverage_summary": "The Space has no source covering recovery verification.",
                "gaps": [{
                    "label": "Official recovery criteria",
                    "reason": "No authoritative recovery checklist is present.",
                    "priority": "high"
                }],
                "candidates": [{
                    "label": "Vendor recovery guide",
                    "source_class": "documentation",
                    "query": "official service recovery verification guide",
                    "expected_value": "Authoritative recovery checks and limitations.",
                    "rationale": "Closes the highest-priority coverage gap.",
                    "confidence": 0.88
                }]
            })
            .to_string(),
            prompt_tokens: 80,
            completion_tokens: 60,
        })
    }
}

#[async_trait]
impl KnowledgeAgentExecutor for InsightExecutor {
    async fn execute(
        &self,
        request: AgentInvocation,
    ) -> Result<AgentExecution, AgentExecutionError> {
        self.invocations
            .lock()
            .await
            .push((request.job.role.clone(), request.allowed_tools));
        let citations = self
            .source_ids
            .iter()
            .enumerate()
            .map(|(index, source_id)| {
                serde_json::json!({
                    "source_id": source_id,
                    "locator": format!("paragraph {}", index + 1),
                    "claim": format!("Evidence {} supports verified recovery.", index + 1),
                    "stance": "supports"
                })
            })
            .collect::<Vec<_>>();
        let payload = if request.job.role == "insight_synthesizer" {
            let importance = [
                "operational_impact",
                "evidence_quality",
                "novelty",
                "recurrence",
                "cross_bundle_reuse",
                "contradiction_value",
                "outcome_support",
            ]
            .map(|factor| {
                serde_json::json!({
                    "factor": factor,
                    "score": 0.8,
                    "reason": "Multiple sources agree with a verified result"
                })
            });
            serde_json::json!({
                "title": "Verified recovery synthesis",
                "summary": "Two sources explain a recovery result that succeeded in operation.",
                "dossier_markdown": "Independent evidence and the verified operation support the recovery check.",
                "candidate_title": "Verified recovery insight",
                "candidate_summary": "Use independent health evidence before declaring recovery.",
                "citations": citations,
                "candidate": {
                    "schema_version": 1,
                    "target_kind": "insight",
                    "claim": "Independent health checks improve recovery decisions",
                    "claims": [{
                        "id": "runbook",
                        "statement": "The runbook requires a health check.",
                        "source_ids": [self.source_ids[0]]
                    }, {
                        "id": "field-report",
                        "statement": "The field report confirms recovery after the health check.",
                        "source_ids": [self.source_ids[1]]
                    }],
                    "supporting_claim_ids": ["runbook", "field-report"],
                    "contradicting_claim_ids": [],
                    "applicability": ["Service recovery with observable health signals"],
                    "limitations": ["Does not establish the original fault cause"],
                    "freshness": {"status": "current", "reason": "Pinned sources and recent Outcome"},
                    "confidence": 0.88,
                    "importance": importance,
                    "verified_outcome_ids": [self.outcome_id]
                }
            })
        } else {
            serde_json::json!({
                "title": "Add verified recovery insight",
                "summary": "A reviewable Insight connected to evidence and outcome.",
                "operations": [{
                    "op": "create_note",
                    "title": "Verified recovery insight",
                    "body": "Independent health checks improve recovery decisions."
                }],
                "citations": citations
            })
        };
        Ok(AgentExecution {
            text: payload.to_string(),
            prompt_tokens: 140,
            completion_tokens: 100,
        })
    }
}

#[async_trait]
impl KnowledgeAgentExecutor for ScriptedExecutor {
    async fn execute(
        &self,
        request: AgentInvocation,
    ) -> Result<AgentExecution, AgentExecutionError> {
        self.invocations
            .lock()
            .await
            .push((request.job.role.clone(), request.allowed_tools));
        let citation = serde_json::json!({
            "source_id": self.source_id,
            "locator": "paragraph 1",
            "claim": "The runbook requires a health check."
        });
        let payload = if request.job.role == "researcher" {
            let importance = [
                "operational_impact",
                "evidence_quality",
                "novelty",
                "recurrence",
                "cross_bundle_reuse",
                "contradiction_value",
                "outcome_support",
            ]
            .map(|factor| {
                serde_json::json!({
                    "factor": factor,
                    "score": 0.6,
                    "reason": "Source-backed review priority"
                })
            });
            serde_json::json!({
                "title": "Runbook research",
                "summary": "One source-backed finding",
                "dossier_markdown": "The runbook requires a health check.",
                "candidate_title": "Health-check guidance",
                "candidate_summary": "Add the verified health-check step.",
                "citations": [citation],
                "candidate": {
                    "schema_version": 1,
                    "target_kind": "lesson",
                    "claim": "Run a health check before declaring recovery",
                    "claims": [{
                        "id": "health-check",
                        "statement": "The runbook requires a health check.",
                        "source_ids": [self.source_id]
                    }],
                    "supporting_claim_ids": ["health-check"],
                    "contradicting_claim_ids": [],
                    "applicability": ["Service recovery"],
                    "limitations": ["Does not identify the original fault"],
                    "freshness": {
                        "status": "current",
                        "reason": "Current pinned runbook"
                    },
                    "confidence": 0.82,
                    "importance": importance,
                    "verified_outcome_ids": []
                }
            })
        } else {
            serde_json::json!({
                "title": "Add health-check guidance",
                "summary": "A reviewable note derived from the runbook.",
                "operations": [{
                    "op": "create_note",
                    "path": "notes/health-check-guidance.md",
                    "title": "Health-check guidance",
                    "body": "Run a health check before declaring recovery."
                }],
                "citations": [citation]
            })
        };
        Ok(AgentExecution {
            text: payload.to_string(),
            prompt_tokens: 120,
            completion_tokens: 80,
        })
    }
}

#[async_trait]
impl KnowledgeAgentExecutor for LessonRevisionExecutor {
    async fn execute(
        &self,
        request: AgentInvocation,
    ) -> Result<AgentExecution, AgentExecutionError> {
        self.prompts
            .lock()
            .await
            .push((request.job.role.clone(), request.prompt.clone()));
        let citation = serde_json::json!({
            "source_id": self.source_id,
            "locator": "paragraph 1",
            "claim": "The recovery outcome confirms the Lesson needs an explicit verification step.",
            "stance": "supports"
        });
        let payload = if request.job.role == "researcher" {
            let importance = [
                "operational_impact",
                "evidence_quality",
                "novelty",
                "recurrence",
                "cross_bundle_reuse",
                "contradiction_value",
                "outcome_support",
            ]
            .map(|factor| {
                serde_json::json!({
                    "factor": factor,
                    "score": 0.8,
                    "reason": "Pinned evidence and a verified Outcome support the revision"
                })
            });
            serde_json::json!({
                "title": "Recovery Lesson revision research",
                "summary": "The verified outcome confirms the existing recovery Lesson.",
                "dossier_markdown": "The existing Lesson should explicitly retain the final verification step.",
                "candidate_title": "Revise recovery Lesson",
                "candidate_summary": "Keep the health verification step explicit.",
                "citations": [citation],
                "candidate": {
                    "schema_version": 1,
                    "target_kind": "lesson",
                    "claim": "Run and record the health check before declaring recovery",
                    "claims": [{
                        "id": "verified-health-check",
                        "statement": "The recovery outcome was satisfied only after a health check.",
                        "source_ids": [self.source_id]
                    }],
                    "supporting_claim_ids": ["verified-health-check"],
                    "contradicting_claim_ids": [],
                    "applicability": ["Closed server incidents with a verified recovery outcome"],
                    "limitations": ["The outcome does not establish root cause"],
                    "freshness": {"status": "current", "reason": "Pinned Lesson source and verified outcome"},
                    "confidence": 0.88,
                    "importance": importance,
                    "verified_outcome_ids": [self.outcome_id]
                }
            })
        } else {
            serde_json::json!({
                "title": "Revise recovery Lesson",
                "summary": "A reviewable update to the existing Lesson.",
                "operations": [{
                    "op": "update_note",
                    "object_id": self.lesson_id,
                    "expected_revision": self.lesson_revision,
                    "title": "Closed incident recovery Lesson",
                    "body": "Run and record the health check before declaring recovery. The verified outcome confirmed this step."
                }],
                "citations": [citation]
            })
        };
        Ok(AgentExecution {
            text: payload.to_string(),
            prompt_tokens: 150,
            completion_tokens: 110,
        })
    }
}

async fn pg_available() -> bool {
    let admin_url = std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
        .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
    let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
    else {
        return false;
    };
    let available: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
        "SELECT default_version FROM pg_available_extensions WHERE name = 'vector'",
    )
    .fetch_optional(&pool)
    .await;
    pool.close().await;
    matches!(available, Ok(Some(_)))
}

async fn tenant_and_admin(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'R2.5 worker test')")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO users (id, tenant_id, email, display_name, role, password_hash)
           VALUES ($1,$2,$3,'R2.5 Admin','admin','test')"#,
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(format!("r2-5-{tenant_id}@example.test"))
    .execute(pool)
    .await
    .unwrap();
    (tenant_id, user_id)
}

async fn extracted_source(
    pool: &sqlx::PgPool,
    layout: &TenantVaultLayout,
    actor: SpaceActor,
    space_id: Uuid,
    vault_id: Uuid,
    bundle_id: &str,
) -> Uuid {
    extracted_source_with(
        pool,
        layout,
        actor,
        space_id,
        vault_id,
        bundle_id,
        "Recovery runbook",
        b"# Recovery runbook\n\nRun a health check before declaring recovery.\n",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn extracted_source_with(
    pool: &sqlx::PgPool,
    layout: &TenantVaultLayout,
    actor: SpaceActor,
    space_id: Uuid,
    vault_id: Uuid,
    bundle_id: &str,
    title: &str,
    bytes: &[u8],
) -> Uuid {
    let note_hash = hex::encode(Sha256::digest(bytes));
    let blob_hash = format!("sha256:{note_hash}");
    let blob_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO knowledge_blobs
           (id, tenant_id, content_hash, storage_key, byte_size, content_type, original_name, created_by)
           VALUES ($1,$2,$3,$4,$5,'text/markdown',$6,$7)"#,
    )
    .bind(blob_id)
    .bind(actor.tenant_id)
    .bind(&blob_hash)
    .bind(format!("sha256/{}/{}", &note_hash[..2], note_hash))
    .bind(bytes.len() as i64)
    .bind(format!("{}.md", title.to_lowercase().replace(' ', "-")))
    .bind(actor.user_id)
    .execute(pool)
    .await
    .unwrap();
    let source = sources::create_pending_source(
        pool,
        actor,
        CreateSource {
            vault_id,
            conversation_id: None,
            source_kind: "upload".to_string(),
            title: title.to_string(),
            original_name: format!("{}.md", title.to_lowercase().replace(' ', "-")),
            requested_uri: None,
        },
    )
    .await
    .unwrap();
    let source = sources::attach_source_blob(
        pool,
        actor,
        source.id,
        source.revision,
        AttachSourceBlob {
            blob_id,
            content_type: "text/markdown".to_string(),
            byte_size: bytes.len() as i64,
            content_hash: blob_hash,
            final_uri: None,
        },
    )
    .await
    .unwrap();
    let path = format!("notes/{}.md", source.id);
    let repository = layout.open_or_init(actor.tenant_id).unwrap();
    repository.ensure_domain(space_id, bundle_id).unwrap();
    repository
        .write_note(
            space_id,
            bundle_id,
            &path,
            bytes,
            "test: add extracted source",
        )
        .unwrap();
    let object_id = Uuid::new_v4();
    sources::register_source_object(pool, actor, source.id, object_id, &path, &note_hash)
        .await
        .unwrap();
    sources::complete_source(pool, actor, source.id, source.revision, object_id)
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn r3_4a_source_scout_produces_audited_proposal_without_sources_or_tools() {
    if !pg_available().await {
        eprintln!("skipping Source Scout worker test: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let temp = tempfile::tempdir().unwrap();
    let layout = Arc::new(TenantVaultLayout::new(temp.path()));
    let (tenant_id, admin_id) = tenant_and_admin(pool).await;
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        pool,
        actor,
        CreateProject {
            slug: "source-scout".to_string(),
            title: "Source Scout".to_string(),
            goal: "Find missing recovery knowledge".to_string(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let vault = spaces::ensure_vault(
        pool,
        actor,
        project.space.id,
        EnsureVault {
            home_bundle_id: "server-operations".to_string(),
            knowledge_schema_id: "server.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let job = jobs::enqueue(
        pool,
        actor,
        EnqueueKnowledgeJob {
            space_id: project.space.id,
            output_vault_id: vault.id,
            role: KnowledgeJobRole::SourceScout,
            kind: KnowledgeJobKind::OnDemand,
            priority: 5,
            input: serde_json::json!({
                "question": "service recovery verification",
                "coverage": {
                    "source_count": 0,
                    "shown_count": 0,
                    "truncated": false,
                    "sources": []
                }
            }),
            idempotency_key: "source-scout:recovery".to_string(),
            source_ids: Vec::new(),
            runtime: RuntimeSnapshot {
                backend: "codex_exec".to_string(),
                model: "gpt-5.6-sol".to_string(),
                effort: "medium".to_string(),
                endpoint_id: None,
                model_source: "default".to_string(),
                local_base_url: String::new(),
                local_api_key_env: String::new(),
                prompt_contract_revision: "source-scout-v1".to_string(),
                tool_policy_revision: "knowledge-read-v1".to_string(),
                role_profile_source: None,
                role_profile_ref: None,
            },
            bundle_role: None,
            budget: JobBudget {
                max_tokens: 4_096,
                max_sources: 12,
                max_wall_seconds: 30,
                max_attempts: 2,
            },
            scheduled_at: None,
        },
    )
    .await
    .unwrap();
    let executor = Arc::new(SourceScoutExecutor {
        invocations: Mutex::new(Vec::new()),
    });
    let worker = runtime::spawn_worker(pool.clone(), layout, executor.clone(), None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let current = jobs::get(pool, actor, job.id).await.unwrap();
        if current.status == "succeeded" {
            assert_eq!(current.used_sources, 0);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Source Scout job did not complete"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    worker.shutdown().await;

    let artifacts = jobs::artifacts(pool, actor, job.id).await.unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, "source_proposal");
    assert_eq!(artifacts[0].payload["approval_state"], "suggested");
    assert_eq!(artifacts[0].payload["schema_version"], 1);
    assert_eq!(artifacts[0].citations, serde_json::json!([]));
    let invocations = executor.invocations.lock().await;
    assert_eq!(invocations.len(), 1);
    assert_eq!(invocations[0].0, "source_scout");
    assert!(invocations[0].1.contains("Coverage snapshot"));
    assert!(invocations[0].2.is_empty());
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT event_kind FROM knowledge_job_events WHERE tenant_id = $1 AND job_id = $2 ORDER BY id",
    )
    .bind(tenant_id)
    .bind(job.id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(events, vec!["queued", "leased", "succeeded"]);
    drop(invocations);
    drop(executor);
    harness.cleanup().await;
}

#[tokio::test]
async fn r3_4a_insight_synthesis_pins_two_sources_and_verified_outcome() {
    if !pg_available().await {
        eprintln!("skipping Insight Synthesizer worker test: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let temp = tempfile::tempdir().unwrap();
    let layout = Arc::new(TenantVaultLayout::new(temp.path()));
    let (tenant_id, admin_id) = tenant_and_admin(pool).await;
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        pool,
        actor,
        CreateProject {
            slug: "insight-synthesis".to_string(),
            title: "Insight Synthesis".to_string(),
            goal: "Connect evidence with verified operational results".to_string(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let bundle_id = "server-operations";
    let vault = spaces::ensure_vault(
        pool,
        actor,
        project.space.id,
        EnsureVault {
            home_bundle_id: bundle_id.to_string(),
            knowledge_schema_id: "server.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let source_ids = [
        extracted_source_with(
            pool,
            layout.as_ref(),
            actor,
            project.space.id,
            vault.id,
            bundle_id,
            "Recovery runbook",
            b"# Recovery runbook\n\nRun a health check before declaring recovery.\n",
        )
        .await,
        extracted_source_with(
            pool,
            layout.as_ref(),
            actor,
            project.space.id,
            vault.id,
            bundle_id,
            "Recovery field report",
            b"# Recovery field report\n\nHealth checks passed after the repair and the service stayed available.\n",
        )
        .await,
    ];
    let subject_id = Uuid::new_v4().to_string();
    let (outcome_id, created_at): (Uuid, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        r#"INSERT INTO knowledge_outcome_feedback
           (tenant_id, actor_user_id, consumer_bundle_id, feedback_id,
            experience_revision, subject_owner_bundle, subject_kind,
            subject_stable_id, subject_revision, operation_id,
            predicate_result, verification_summary, before_state, after_state,
            used_citations, feedback_json)
           VALUES ($1,$2,'server-administrator','recovery-check',
                   $3,'server-administrator','server',$4,'7','repair-7',
                   'satisfied','Health checks passed after repair','{}','{}','[]',$5)
           RETURNING id, created_at"#,
    )
    .bind(tenant_id)
    .bind(admin_id)
    .bind(format!("sha256:{}", "b".repeat(64)))
    .bind(&subject_id)
    .bind(serde_json::json!({
        "authority": {"allowed_space_ids": [project.space.id.to_string()]}
    }))
    .fetch_one(pool)
    .await
    .unwrap();
    let job = jobs::enqueue(
        pool,
        actor,
        EnqueueKnowledgeJob {
            space_id: project.space.id,
            output_vault_id: vault.id,
            role: KnowledgeJobRole::InsightSynthesizer,
            kind: KnowledgeJobKind::OnDemand,
            priority: 5,
            input: serde_json::json!({
                "question": "What recovery practice is supported by evidence and outcome?",
                "outcomes": [{
                    "id": outcome_id,
                    "experience_revision": format!("sha256:{}", "b".repeat(64)),
                    "consumer_bundle_id": "server-administrator",
                    "subject_owner_bundle": "server-administrator",
                    "subject_kind": "server",
                    "subject_stable_id": subject_id,
                    "subject_revision": "7",
                    "operation_id": "repair-7",
                    "context_query_id": null,
                    "context_revision": null,
                    "predicate_result": "satisfied",
                    "verification_summary": "Health checks passed after repair",
                    "used_citations": [],
                    "created_at": created_at
                }]
            }),
            idempotency_key: format!("insight:{outcome_id}"),
            source_ids: source_ids.to_vec(),
            runtime: RuntimeSnapshot {
                backend: "codex_exec".to_string(),
                model: "gpt-5.6-sol".to_string(),
                effort: "high".to_string(),
                endpoint_id: None,
                model_source: "default".to_string(),
                local_base_url: String::new(),
                local_api_key_env: String::new(),
                prompt_contract_revision: "insight-synthesizer-v1".to_string(),
                tool_policy_revision: "knowledge-read-v1".to_string(),
                role_profile_source: None,
                role_profile_ref: None,
            },
            bundle_role: None,
            budget: JobBudget {
                max_tokens: 4_096,
                max_sources: 4,
                max_wall_seconds: 30,
                max_attempts: 2,
            },
            scheduled_at: None,
        },
    )
    .await
    .unwrap();
    let executor = Arc::new(InsightExecutor {
        source_ids,
        outcome_id,
        invocations: Mutex::new(Vec::new()),
    });
    let worker = runtime::spawn_worker(pool.clone(), layout, executor.clone(), None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let change_set = loop {
        if let Some(change_set) = jobs::list_change_sets(pool, actor, project.space.id, 10)
            .await
            .unwrap()
            .into_iter()
            .next()
        {
            break change_set;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Insight synthesis chain timed out"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    worker.shutdown().await;

    let completed = jobs::get(pool, actor, job.id).await.unwrap();
    assert_eq!(completed.status, "succeeded");
    let artifacts = jobs::artifacts(pool, actor, job.id).await.unwrap();
    assert_eq!(artifacts.len(), 2);
    assert_eq!(artifacts[1].payload["target_kind"], "insight");
    assert_eq!(
        artifacts[1].payload["verified_outcome_ids"][0],
        outcome_id.to_string()
    );
    assert_eq!(change_set.status, "pending_user_review");
    assert_eq!(change_set.candidate_artifact_id, Some(artifacts[1].id));
    let invocations = executor.invocations.lock().await;
    assert_eq!(
        invocations.as_slice(),
        [
            (
                "insight_synthesizer".to_string(),
                vec!["wiki.search".to_string(), "wiki.read".to_string()]
            ),
            (
                "gardener".to_string(),
                vec!["wiki.search".to_string(), "wiki.read".to_string()]
            )
        ]
    );
    drop(invocations);
    drop(executor);
    harness.cleanup().await;
}

#[tokio::test]
async fn r3_4c_worker_runs_signed_bundle_researcher_then_distiller() {
    if !pg_available().await {
        eprintln!("skipping R2.5 worker test: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let temp = tempfile::tempdir().unwrap();
    let layout = Arc::new(TenantVaultLayout::new(temp.path()));
    let (tenant_id, admin_id) = tenant_and_admin(pool).await;
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        pool,
        actor,
        CreateProject {
            slug: "worker-loop".to_string(),
            title: "Worker Loop".to_string(),
            goal: "Prove the Researcher and Gardener chain".to_string(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let bundle_id = "computer-science-research";
    let vault = spaces::ensure_vault(
        pool,
        actor,
        project.space.id,
        EnsureVault {
            home_bundle_id: bundle_id.to_string(),
            knowledge_schema_id: "cs.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let source_id = extracted_source(
        pool,
        layout.as_ref(),
        actor,
        project.space.id,
        vault.id,
        bundle_id,
    )
    .await;
    let research_runtime = RuntimeSnapshot {
        backend: "codex_exec".to_string(),
        model: "gpt-5.6-sol".to_string(),
        effort: "high".to_string(),
        endpoint_id: None,
        model_source: "default".to_string(),
        local_base_url: String::new(),
        local_api_key_env: String::new(),
        prompt_contract_revision: "news-research-v1".to_string(),
        tool_policy_revision: "knowledge-read-v1".to_string(),
        role_profile_source: Some("bundle".to_string()),
        role_profile_ref: Some("d".repeat(64)),
    };
    let research_role = BundleRoleSnapshot {
        bundle_id: "news-intelligence".to_string(),
        bundle_role_id: "news-researcher".to_string(),
        package_manifest_sha256: "a".repeat(64),
        recipe_asset_id: "news-research".to_string(),
        recipe_sha256: "b".repeat(64),
    };
    let distiller_runtime = RuntimeSnapshot {
        prompt_contract_revision: "news-distillation-v2".to_string(),
        role_profile_ref: Some("e".repeat(64)),
        ..research_runtime.clone()
    };
    let distiller_role = BundleRoleSnapshot {
        bundle_role_id: "news-distiller".to_string(),
        recipe_asset_id: "news-distillation".to_string(),
        recipe_sha256: "c".repeat(64),
        ..research_role.clone()
    };
    let bundle_execution = runtime::BundleExecutionSnapshot {
        bundle_role: research_role.clone(),
        runtime: research_runtime.clone(),
        prompt_contract_revision: "news-research-v1".to_string(),
        max_wall_seconds: 30,
        recipe: serde_json::json!({"objective": "Build cited event chronology"}),
        gadget_allowlist: vec![
            "news.article-upsert".to_string(),
            "news.claim-upsert".to_string(),
        ],
        followup: Some(Box::new(runtime::BundleExecutionSnapshot {
            bundle_role: distiller_role,
            runtime: distiller_runtime,
            prompt_contract_revision: "news-distillation-v2".to_string(),
            max_wall_seconds: 20,
            recipe: serde_json::json!({"objective": "Distill a cited briefing"}),
            gadget_allowlist: vec![
                "news.briefing-upsert".to_string(),
                "news.event-graph".to_string(),
            ],
            followup: None,
        })),
    };
    let job = jobs::enqueue(
        pool,
        actor,
        EnqueueKnowledgeJob {
            space_id: project.space.id,
            output_vault_id: vault.id,
            role: KnowledgeJobRole::Researcher,
            kind: KnowledgeJobKind::OnDemand,
            priority: 5,
            input: serde_json::json!({
                "question": "What recovery check is required?",
                "collection_binding": {
                    "collection_id": project.space.id,
                    "collection_revision": 3,
                },
                "bundle_execution": bundle_execution,
            }),
            idempotency_key: format!("runtime:{source_id}"),
            source_ids: vec![source_id],
            runtime: research_runtime,
            bundle_role: Some(research_role),
            budget: JobBudget {
                max_tokens: 4_096,
                max_sources: 4,
                max_wall_seconds: 30,
                max_attempts: 2,
            },
            scheduled_at: None,
        },
    )
    .await
    .unwrap();
    let executor = Arc::new(ScriptedExecutor {
        source_id,
        invocations: Mutex::new(Vec::new()),
    });
    let worker = runtime::spawn_worker(pool.clone(), layout.clone(), executor.clone(), None);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let change_sets = loop {
        let change_sets = jobs::list_change_sets(pool, actor, project.space.id, 10)
            .await
            .unwrap();
        if !change_sets.is_empty() {
            break change_sets;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "worker chain timed out"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    worker.shutdown().await;

    let research = jobs::get(pool, actor, job.id).await.unwrap();
    assert_eq!(research.status, "succeeded");
    let artifacts = jobs::artifacts(pool, actor, research.id).await.unwrap();
    assert_eq!(artifacts.len(), 2);
    assert_eq!(artifacts[0].kind, "dossier");
    assert_eq!(artifacts[1].kind, "candidate");
    assert_eq!(artifacts[1].payload["schema_version"], 1);
    assert_eq!(artifacts[1].payload["target_kind"], "lesson");
    assert_eq!(
        artifacts[1].payload["dossier_artifact_id"],
        artifacts[0].id.to_string()
    );
    assert_eq!(change_sets.len(), 1);
    assert_eq!(change_sets[0].status, "pending_user_review");
    assert_eq!(change_sets[0].candidate_artifact_id, Some(artifacts[1].id));
    assert_eq!(
        change_sets[0].citations[0]["source_id"],
        source_id.to_string()
    );
    let evolution = jobs::evolution_for_space(pool, actor, project.space.id, 10)
        .await
        .unwrap();
    assert_eq!(evolution.len(), 1);
    assert_eq!(evolution[0].candidate.id, artifacts[1].id);
    assert_eq!(
        evolution[0]
            .change_set
            .as_ref()
            .map(|change_set| change_set.id),
        Some(change_sets[0].id)
    );

    let all_jobs = jobs::list_for_space(pool, actor, project.space.id, 10)
        .await
        .unwrap();
    assert_eq!(all_jobs.len(), 2);
    assert!(all_jobs.iter().all(|row| row.status == "succeeded"));
    assert!(all_jobs
        .iter()
        .all(|row| row.on_behalf_of_user_id == Some(admin_id)));
    assert!(all_jobs.iter().any(|row| {
        row.bundle_role_id.as_deref() == Some("news-researcher")
            && row.prompt_contract_revision == "news-research-v1"
            && row.max_wall_seconds == 30
    }));
    assert!(all_jobs.iter().any(|row| {
        row.bundle_role_id.as_deref() == Some("news-distiller")
            && row.prompt_contract_revision == "news-distillation-v2"
            && row.max_wall_seconds == 20
            && row.input["collection_binding"]["collection_id"] == project.space.id.to_string()
            && row.input["collection_binding"]["collection_revision"] == 3
    }));
    let invocations = executor.invocations.lock().await;
    assert_eq!(
        invocations.as_slice(),
        [
            (
                "researcher".to_string(),
                vec![
                    "news.article-upsert".to_string(),
                    "news.claim-upsert".to_string(),
                    "wiki.read".to_string(),
                    "wiki.search".to_string(),
                ]
            ),
            (
                "gardener".to_string(),
                vec![
                    "news.briefing-upsert".to_string(),
                    "news.event-graph".to_string(),
                    "wiki.read".to_string(),
                    "wiki.search".to_string(),
                ]
            )
        ]
    );
    drop(invocations);

    let accepted = jobs::decide_change_set(
        pool,
        actor,
        change_sets[0].id,
        change_sets[0].revision,
        jobs::ChangeSetDecision::Accept,
        None,
    )
    .await
    .unwrap();
    let materializing = jobs::begin_materialization(pool, actor, accepted.id, accepted.revision)
        .await
        .unwrap();
    let object_id = Uuid::new_v4();
    let path = format!("notes/{object_id}.md");
    let repository = TenantVaultLayout::new(temp.path())
        .open_or_init(tenant_id)
        .unwrap();
    let states = repository
        .write_notes_batch(
            project.space.id,
            bundle_id,
            vec![VaultNoteWrite {
                relative_path: path.clone(),
                bytes: b"# Applied knowledge\n".to_vec(),
            }],
            materializing.expected_git_revision.as_deref(),
            "test: apply reviewed change set",
        )
        .unwrap();
    let applied = jobs::complete_materialization(
        pool,
        actor,
        materializing.id,
        materializing.revision,
        &states[0].git_revision,
        &[MaterializedObjectInput {
            id: object_id,
            path: path.clone(),
            content_hash: states[0].content_hash.clone(),
            expected_revision: None,
            originating_subject: None,
        }],
    )
    .await
    .unwrap();
    assert_eq!(applied.status, "applied");
    assert_eq!(applied.materialized_object_id, Some(object_id));
    let stored: (String, String) = sqlx::query_as(
        "SELECT path, content_hash FROM knowledge_objects WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(object_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(stored, (path, states[0].content_hash.clone()));

    let restart_job = jobs::enqueue(
        pool,
        actor,
        EnqueueKnowledgeJob {
            space_id: project.space.id,
            output_vault_id: vault.id,
            role: KnowledgeJobRole::Researcher,
            kind: KnowledgeJobKind::OnDemand,
            priority: 5,
            input: serde_json::json!({"question": "Can this job resume after worker shutdown?"}),
            idempotency_key: format!("restart:{source_id}"),
            source_ids: vec![source_id],
            runtime: RuntimeSnapshot {
                backend: "codex_exec".to_string(),
                model: "gpt-5.6-sol".to_string(),
                effort: "medium".to_string(),
                endpoint_id: None,
                model_source: "default".to_string(),
                local_base_url: String::new(),
                local_api_key_env: String::new(),
                prompt_contract_revision: "researcher-v2".to_string(),
                tool_policy_revision: "knowledge-read-v1".to_string(),
                role_profile_source: None,
                role_profile_ref: None,
            },
            bundle_role: None,
            budget: JobBudget {
                max_tokens: 4_096,
                max_sources: 4,
                max_wall_seconds: 30,
                max_attempts: 2,
            },
            scheduled_at: None,
        },
    )
    .await
    .unwrap();
    let blocking = runtime::spawn_worker(
        pool.clone(),
        layout.clone(),
        Arc::new(BlockingExecutor),
        None,
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let current = jobs::get(pool, actor, restart_job.id).await.unwrap();
        if current.status == "running" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "restart fixture did not lease"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    blocking.shutdown().await;
    let queued = jobs::get(pool, actor, restart_job.id).await.unwrap();
    assert_eq!(queued.status, "queued");
    assert_eq!(queued.attempt, 1);

    let resumed = runtime::spawn_worker(pool.clone(), layout.clone(), executor.clone(), None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let resumed_job = loop {
        let current = jobs::get(pool, actor, restart_job.id).await.unwrap();
        if current.status == "succeeded" {
            break current;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "restarted job did not complete"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    resumed.shutdown().await;
    assert_eq!(resumed_job.attempt, 2);

    let cancel_job = jobs::enqueue(
        pool,
        actor,
        EnqueueKnowledgeJob {
            space_id: project.space.id,
            output_vault_id: vault.id,
            role: KnowledgeJobRole::Researcher,
            kind: KnowledgeJobKind::OnDemand,
            priority: 5,
            input: serde_json::json!({"question": "Preserve partial output on cancellation"}),
            idempotency_key: format!("cancel-partial:{source_id}"),
            source_ids: vec![source_id],
            runtime: RuntimeSnapshot {
                backend: "codex_exec".to_string(),
                model: "gpt-5.6-sol".to_string(),
                effort: "medium".to_string(),
                endpoint_id: None,
                model_source: "default".to_string(),
                local_base_url: String::new(),
                local_api_key_env: String::new(),
                prompt_contract_revision: "researcher-v2".to_string(),
                tool_policy_revision: "knowledge-read-v1".to_string(),
                role_profile_source: None,
                role_profile_ref: None,
            },
            bundle_role: None,
            budget: JobBudget {
                max_tokens: 4_096,
                max_sources: 4,
                max_wall_seconds: 30,
                max_attempts: 2,
            },
            scheduled_at: None,
        },
    )
    .await
    .unwrap();
    let partial_executor = Arc::new(PartialBlockingExecutor {
        started: Notify::new(),
    });
    let cancelling_worker =
        runtime::spawn_worker(pool.clone(), layout, partial_executor.clone(), None);
    tokio::time::timeout(Duration::from_secs(5), partial_executor.started.notified())
        .await
        .expect("partial-output executor did not start");
    let running = jobs::get(pool, actor, cancel_job.id).await.unwrap();
    assert_eq!(running.status, "running");
    jobs::request_cancel(pool, actor, running.id, running.revision)
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let current = jobs::get(pool, actor, cancel_job.id).await.unwrap();
        if current.status == "cancelled" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "cancelled worker did not terminalize"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cancelling_worker.shutdown().await;
    let artifacts = jobs::artifacts(pool, actor, cancel_job.id).await.unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, "partial_dossier");
    assert!(artifacts[0].payload["output"]
        .as_str()
        .unwrap()
        .contains("Partial recovery dossier"));
    let event_kinds: Vec<String> = sqlx::query_scalar(
        "SELECT event_kind FROM knowledge_job_events WHERE tenant_id = $1 AND job_id = $2 ORDER BY created_at, id",
    )
    .bind(tenant_id)
    .bind(cancel_job.id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(event_kinds.iter().any(|kind| kind == "cancel_requested"));
    assert!(event_kinds.iter().any(|kind| kind == "cancelled"));

    drop(executor);
    harness.cleanup().await;
}

#[tokio::test]
async fn k13_outcome_backed_researcher_proposes_exact_existing_lesson_update_for_review() {
    if !pg_available().await {
        eprintln!("skipping K13-T1 Lesson revision worker test: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let temp = tempfile::tempdir().unwrap();
    let layout = Arc::new(TenantVaultLayout::new(temp.path()));
    let (tenant_id, admin_id) = tenant_and_admin(pool).await;
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        pool,
        actor,
        CreateProject {
            slug: "k13-lesson-revision-worker".to_string(),
            title: "K13 Lesson revision worker".to_string(),
            goal: "Prove verified outcomes become reviewed Lesson update proposals".to_string(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let bundle_id = "server-administrator";
    let vault = spaces::ensure_vault(
        pool,
        actor,
        project.space.id,
        EnsureVault {
            home_bundle_id: bundle_id.to_string(),
            knowledge_schema_id: "server.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let source_id = extracted_source_with(
        pool,
        layout.as_ref(),
        actor,
        project.space.id,
        vault.id,
        bundle_id,
        "Closed incident evidence",
        b"# Closed incident evidence\n\nA health check confirmed recovery.\n",
    )
    .await;
    let (lesson_id, path, revision): (Uuid, String, i64) = sqlx::query_as(
        "SELECT id, path, revision FROM knowledge_objects WHERE tenant_id = $1 AND source_id = $2",
    )
    .bind(tenant_id)
    .bind(source_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let mut properties = BTreeMap::new();
    properties.insert(
        "title".to_string(),
        serde_json::json!("Closed incident recovery Lesson"),
    );
    properties.insert("knowledge_kind".to_string(), serde_json::json!("lesson"));
    properties.insert("review_state".to_string(), serde_json::json!("reviewed"));
    properties.insert("source_ids".to_string(), serde_json::json!([source_id]));
    let lesson_body = "Run and record the health check before declaring recovery.";
    let lesson_bytes =
        gadgetron_knowledge::source::serialize_obsidian_note(&properties, lesson_body)
            .unwrap()
            .into_bytes();
    let repository = layout.open_or_init(tenant_id).unwrap();
    repository
        .write_note(
            project.space.id,
            bundle_id,
            &path,
            &lesson_bytes,
            "test: pin reviewed Lesson",
        )
        .unwrap();
    let lesson_hash = hex::encode(Sha256::digest(&lesson_bytes));
    let lesson = sources::update_note_hash(pool, actor, lesson_id, revision, &lesson_hash)
        .await
        .unwrap();
    let (outcome_id, created_at): (Uuid, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        r#"INSERT INTO knowledge_outcome_feedback
           (tenant_id, actor_user_id, consumer_bundle_id, feedback_id,
            experience_revision, subject_owner_bundle, subject_kind,
            subject_stable_id, subject_revision, operation_id,
            predicate_result, verification_summary, before_state, after_state,
            used_citations, feedback_json)
           VALUES ($1,$2,'server-administrator','k13-revision-outcome',
                   $3,'server-administrator','server-administrator.server-incident',
                   'incident-1','revision-1','close-incident',
                   'satisfied','Health check confirmed recovery','{}','{}',$4,$5)
           RETURNING id, created_at"#,
    )
    .bind(tenant_id)
    .bind(admin_id)
    .bind(format!("sha256:{}", "c".repeat(64)))
    .bind(serde_json::json!([{
        "citation_id": format!("{}:{}", lesson_id, lesson.revision),
        "source_revision": "1",
    }]))
    .bind(serde_json::json!({
        "authority": {"allowed_space_ids": [project.space.id.to_string()]}
    }))
    .fetch_one(pool)
    .await
    .unwrap();
    let outcome_snapshot = serde_json::json!({
        "id": outcome_id,
        "experience_revision": format!("sha256:{}", "c".repeat(64)),
        "consumer_bundle_id": "server-administrator",
        "subject_owner_bundle": "server-administrator",
        "subject_kind": "server-administrator.server-incident",
        "subject_stable_id": "incident-1",
        "subject_revision": "revision-1",
        "operation_id": "close-incident",
        "context_query_id": null,
        "context_revision": null,
        "predicate_result": "satisfied",
        "verification_summary": "Health check confirmed recovery",
        "used_citations": [{
            "citation_id": format!("{}:{}", lesson_id, lesson.revision),
            "source_revision": "1",
        }],
        "created_at": created_at,
    });
    let job = jobs::enqueue(
        pool,
        actor,
        EnqueueKnowledgeJob {
            space_id: project.space.id,
            output_vault_id: vault.id,
            role: KnowledgeJobRole::Researcher,
            kind: KnowledgeJobKind::OnDemand,
            priority: 5,
            input: serde_json::json!({
                "question": "Does the verified recovery outcome revise the existing Lesson?",
                "outcomes": [outcome_snapshot],
                "lesson_revision_target": {
                    "object_id": lesson_id,
                    "expected_revision": lesson.revision,
                    "content_hash": lesson_hash,
                    "title": "Closed incident recovery Lesson",
                    "body": lesson_body,
                    "source_ids": [source_id],
                    "originating_subject": null,
                },
            }),
            idempotency_key: format!("k13-lesson-revision:{lesson_id}:{}", lesson.revision),
            source_ids: vec![source_id],
            runtime: RuntimeSnapshot {
                backend: "codex_exec".to_string(),
                model: "gpt-5.6-sol".to_string(),
                effort: "high".to_string(),
                endpoint_id: None,
                model_source: "default".to_string(),
                local_base_url: String::new(),
                local_api_key_env: String::new(),
                prompt_contract_revision: "researcher-v2".to_string(),
                tool_policy_revision: "knowledge-read-v1".to_string(),
                role_profile_source: None,
                role_profile_ref: None,
            },
            bundle_role: None,
            budget: JobBudget {
                max_tokens: 4_096,
                max_sources: 4,
                max_wall_seconds: 30,
                max_attempts: 2,
            },
            scheduled_at: None,
        },
    )
    .await
    .unwrap();
    let executor = Arc::new(LessonRevisionExecutor {
        source_id,
        outcome_id,
        lesson_id,
        lesson_revision: lesson.revision,
        prompts: Mutex::new(Vec::new()),
    });
    let worker = runtime::spawn_worker(pool.clone(), layout.clone(), executor.clone(), None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let change_set = loop {
        if let Some(change_set) = jobs::list_change_sets(pool, actor, project.space.id, 10)
            .await
            .unwrap()
            .into_iter()
            .next()
        {
            break change_set;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Lesson revision worker chain timed out"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    worker.shutdown().await;

    let research = jobs::get(pool, actor, job.id).await.unwrap();
    assert_eq!(research.status, "succeeded");
    assert_eq!(change_set.status, "pending_user_review");
    let operations = change_set.operations.as_array().unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0]["op"], "update_note");
    assert_eq!(operations[0]["object_id"], lesson_id.to_string());
    assert_eq!(operations[0]["expected_revision"], lesson.revision);
    assert!(change_set.candidate_artifact_id.is_some());
    let prompts = executor.prompts.lock().await;
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0].1.contains(&lesson_id.to_string()));
    assert!(prompts[1].1.contains("exactly one update_note operation"));
    drop(prompts);

    harness.cleanup().await;
}
