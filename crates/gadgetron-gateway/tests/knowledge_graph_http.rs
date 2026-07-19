use std::{collections::HashMap, sync::Arc};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Extension,
};
use gadgetron_bundle_sdk::{
    BundleId, CapabilityId, CitationUseRef, ContextUseRef, IntelligenceBudget,
    IntelligenceQueryDraft, OutcomeFeedbackDraft, OutcomePredicateResult, SubjectRevisionRef,
};
use gadgetron_core::error::GadgetronError;
use gadgetron_core::{
    agent::tools::{
        GadgetDispatchContext, GadgetDispatcher, GadgetError, GadgetProvider, GadgetResult,
    },
    context::{QuotaSnapshot, Scope, TenantContext},
};
use gadgetron_gateway::{
    server::AppState,
    web::{
        action_service::InProcessWorkbenchActionService,
        catalog::DescriptorCatalog,
        intelligence_context::{IntelligenceActorBinding, IntelligenceContextService},
        knowledge_graph, knowledge_jobs as knowledge_jobs_http, knowledge_sources,
        projection::InProcessWorkbenchProjection,
        replay_cache::{InMemoryReplayCache, DEFAULT_REPLAY_TTL},
        workbench::{self, GatewayWorkbenchService, WorkbenchActionService},
    },
};
use gadgetron_knowledge::{
    config::KnowledgeConfig, gadget::KnowledgeGadgetProvider, vault::TenantVaultLayout,
};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    audit::writer::AuditWriter,
    auth::validator::{KeyValidator, ValidatedKey},
    knowledge_graph::{self as graph, GraphNodeInput, GraphSnapshotInput, ReconcileMode},
    knowledge_jobs::{
        self as jobs, ArtifactInput, ChangeSetInput, EnqueueKnowledgeJob, JobBudget,
        KnowledgeJobKind, KnowledgeJobRole, RuntimeSnapshot,
    },
    knowledge_spaces::{
        self as spaces, CreateProject, EnsureVault, PrincipalKind, SpaceActor, SpaceRole,
    },
    quota::enforcer::InMemoryQuotaEnforcer,
};
use tower::ServiceExt;
use uuid::Uuid;

struct UnusedValidator;

#[async_trait]
impl KeyValidator for UnusedValidator {
    async fn validate(&self, _key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
        Err(GadgetronError::Forbidden)
    }
    async fn invalidate(&self, _key_hash: &str) {}
}

struct KnowledgeDispatcher {
    provider: KnowledgeGadgetProvider,
}

#[async_trait]
impl GadgetDispatcher for KnowledgeDispatcher {
    async fn dispatch_gadget(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<GadgetResult, GadgetError> {
        self.provider.call(name, args).await
    }

    async fn dispatch_gadget_with_context(
        &self,
        context: GadgetDispatchContext,
        name: &str,
        args: serde_json::Value,
    ) -> Result<GadgetResult, GadgetError> {
        self.provider.call_with_context(&context, name, args).await
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

fn state(
    pool: sqlx::PgPool,
    vault_root: &std::path::Path,
    dispatcher: Arc<dyn GadgetDispatcher>,
) -> AppState {
    let (audit_writer, _rx) = AuditWriter::new(64);
    let catalog = Arc::new(ArcSwap::from_pointee(
        DescriptorCatalog::seed_p2b().into_snapshot(),
    ));
    let projection = Arc::new(InProcessWorkbenchProjection {
        knowledge: None,
        gateway_version: "test",
        descriptor_catalog: catalog.clone(),
        dynamic_workbench: None,
    });
    let actions: Arc<dyn WorkbenchActionService> =
        Arc::new(InProcessWorkbenchActionService::new_with_dispatcher(
            catalog.clone(),
            InMemoryReplayCache::new(DEFAULT_REPLAY_TTL),
            None,
            Some(dispatcher.clone()),
        ));
    let agent_config = gadgetron_core::agent::config::AgentConfig::default();
    let agent_brain = Arc::new(ArcSwap::from_pointee(agent_config.clone()));
    let policy_evaluator = Arc::new(gadgetron_xaas::policy::PgPolicyEvaluator::new(
        pool.clone(),
        gadgetron_core::agent::config::GadgetsConfig::default(),
    ));
    AppState {
        key_validator: Arc::new(UnusedValidator),
        quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
        audit_writer: Arc::new(audit_writer),
        providers: Arc::new(HashMap::new()),
        router: None,
        pg_pool: Some(pool),
        no_db: false,
        tui_tx: None,
        workbench: Some(Arc::new(GatewayWorkbenchService {
            projection,
            actions: Some(actions),
            approval_store: None,
            policy_evaluator: Some(policy_evaluator),
            gadget_catalog: None,
            descriptor_catalog: Some(catalog),
            catalog_path: None,
            bundles_dir: None,
            bundle_signing: Default::default(),
            runtime_manager: None,
            gadget_modes: None,
            gadget_mode_reconfigurer: None,
            agent_brain: Some(agent_brain),
            agent_config_base: None,
            vault_layout: Some(Arc::new(
                gadgetron_knowledge::vault::TenantVaultLayout::new(vault_root),
            )),
        })),
        penny_shared_surface: None,
        penny_assembler: None,
        agent_config: Arc::new(agent_config),
        google_oauth: None,
        activity_capture_store: None,
        candidate_coordinator: None,
        activity_bus: gadgetron_core::activity_bus::ActivityBus::new(),
        tool_catalog: None,
        gadget_dispatcher: Some(dispatcher),
        tool_audit_sink: Arc::new(gadgetron_core::audit::NoopGadgetAuditEventSink),
        billing_failures: Arc::new(gadgetron_xaas::billing::BillingFailureCounter::new()),
        chat_jobs: Arc::new(gadgetron_gateway::chat_jobs::JobStore::new()),
    }
}

fn context(tenant_id: Uuid, user_id: Uuid) -> TenantContext {
    TenantContext {
        tenant_id,
        api_key_id: Uuid::nil(),
        scopes: vec![Scope::Management],
        quota_snapshot: Arc::new(QuotaSnapshot {
            daily_limit_cents: i64::MAX,
            daily_used_cents: 0,
            monthly_limit_cents: i64::MAX,
            monthly_used_cents: 0,
        }),
        request_id: Uuid::new_v4(),
        started_at: std::time::Instant::now(),
        actor_user_id: Some(user_id),
        actor_api_key_id: None,
    }
}

fn intelligence_query(query_id: &str, target_id: &str) -> IntelligenceQueryDraft {
    IntelligenceQueryDraft::new(
        query_id,
        SubjectRevisionRef::new(
            BundleId::new("server-administrator").unwrap(),
            CapabilityId::new("server.target").unwrap(),
            target_id,
            "1",
        )
        .unwrap(),
        "How should cooling recovery be verified before closing the incident?",
        86_400,
        IntelligenceBudget::new(8, 100, 65_536, 8_000, 10).unwrap(),
    )
    .unwrap()
}

fn app(state: AppState, context: TenantContext) -> axum::Router {
    knowledge_sources::routes()
        .merge(knowledge_sources::upload_routes())
        .merge(knowledge_graph::routes())
        .merge(knowledge_jobs_http::routes())
        .nest("/api/v1/web/workbench", workbench::workbench_routes())
        .with_state(state)
        .layer(Extension(context))
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn multipart(boundary: &str, title: &str, body: &str) -> Vec<u8> {
    format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\n{title}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.md\"\r\n\
         Content-Type: text/markdown\r\n\r\n{body}\r\n--{boundary}--\r\n"
    )
    .into_bytes()
}

async fn upload(app: &axum::Router, vault_id: Uuid, title: &str, body: &str) -> serde_json::Value {
    let boundary = format!("graph-{}", Uuid::new_v4());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/vaults/{vault_id}/sources/upload"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart(&boundary, title, body)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json(response).await
}

async fn note(app: &axum::Router, object_id: &str) -> serde_json::Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/objects/{object_id}/note"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json(response).await
}

async fn put_note(
    app: &axum::Router,
    object_id: &str,
    current: &serde_json::Value,
    properties: serde_json::Value,
    body: &str,
) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/knowledge/objects/{object_id}/note"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "expected_revision": current["revision"],
                        "expected_git_revision": current["git_revision"],
                        "properties": properties,
                        "body": body,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        json(response).await
    );
}

async fn post_json(
    app: &axum::Router,
    path: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn put_json(
    app: &axum::Router,
    path: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn get(app: &axum::Router, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn assert_hidden_get(
    app: &axum::Router,
    case: &str,
    label: &str,
    path: &str,
    expected: StatusCode,
) {
    let response = get(app, path).await;
    assert_eq!(response.status(), expected, "{case}: {label}");
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body = String::from_utf8_lossy(&body);
    for protected in [
        "Alpha",
        "Beta",
        "J10 trust-boundary",
        "Visible review proposal",
    ] {
        assert!(
            !body.contains(protected),
            "{case}: {label} leaked protected content {protected}"
        );
    }
}

async fn search(app: &axum::Router, query: &str) -> serde_json::Value {
    let response = post_json(
        app,
        "/api/v1/web/workbench/actions/knowledge-search",
        serde_json::json!({"args":{"query":query}}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    json(response).await["result"]["payload"].clone()
}

async fn insert_outcome(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    actor_id: Uuid,
    feedback_id: &str,
    predicate: &str,
    allowed_space_id: Uuid,
) -> Uuid {
    insert_outcome_with_citations(
        pool,
        tenant_id,
        actor_id,
        feedback_id,
        predicate,
        allowed_space_id,
        serde_json::json!([]),
    )
    .await
}

async fn insert_outcome_with_citations(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    actor_id: Uuid,
    feedback_id: &str,
    predicate: &str,
    allowed_space_id: Uuid,
    used_citations: serde_json::Value,
) -> Uuid {
    sqlx::query_scalar(
        r#"INSERT INTO knowledge_outcome_feedback
           (tenant_id, actor_user_id, consumer_bundle_id, feedback_id,
            experience_revision, subject_owner_bundle, subject_kind,
            subject_stable_id, subject_revision, operation_id,
            predicate_result, verification_summary, before_state, after_state,
            used_citations, feedback_json)
           VALUES ($1,$2,'server-administrator',$3,$4,
                   'server-administrator','server-administrator.server-incident',
                   'incident-outcome','revision-1','close-incident',$5,
                   'Incident closure was checked','{}','{}',$6,$7)
           RETURNING id"#,
    )
    .bind(tenant_id)
    .bind(actor_id)
    .bind(feedback_id)
    .bind(format!("sha256:{}", "a".repeat(64)))
    .bind(predicate)
    .bind(used_citations)
    .bind(serde_json::json!({
        "authority": {"allowed_space_ids": [allowed_space_id.to_string()]}
    }))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn propose_gardener_change_set(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    vault_id: Uuid,
    source_id: Uuid,
    key: &str,
    operations: serde_json::Value,
) -> jobs::KnowledgeChangeSetRow {
    let job = jobs::enqueue(
        pool,
        actor,
        EnqueueKnowledgeJob {
            space_id,
            output_vault_id: vault_id,
            role: KnowledgeJobRole::Gardener,
            kind: KnowledgeJobKind::OnDemand,
            priority: 0,
            input: serde_json::json!({"question":"Review a canonical knowledge change"}),
            idempotency_key: format!("j10:{key}:{space_id}"),
            source_ids: vec![source_id],
            runtime: RuntimeSnapshot {
                backend: "codex_exec".into(),
                model: "gpt-5.6-sol".into(),
                effort: "high".into(),
                endpoint_id: None,
                model_source: "default".into(),
                local_base_url: String::new(),
                local_api_key_env: String::new(),
                prompt_contract_revision: "gardener-v1".into(),
                tool_policy_revision: "knowledge-read-v1".into(),
                role_profile_source: None,
                role_profile_ref: None,
            },
            bundle_role: None,
            budget: JobBudget {
                max_tokens: 2_048,
                max_sources: 4,
                max_wall_seconds: 30,
                max_attempts: 2,
            },
            scheduled_at: None,
        },
    )
    .await
    .unwrap();
    let worker_id = format!("j10-{key}");
    let leased = jobs::lease_next(pool, &worker_id, 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(leased.id, job.id);
    let change_set = jobs::append_change_set(
        pool,
        job.id,
        &worker_id,
        ChangeSetInput {
            candidate_artifact_id: None,
            title: format!("J10 {key}"),
            summary: "Reviewed canonical knowledge evolution".into(),
            operations,
            citations: serde_json::json!([{
                "source_id": source_id,
                "locator": "reviewed note set",
                "claim": "The proposed canonical change is grounded in a pinned source."
            }]),
            expected_git_revision: None,
            materialization_key: format!("j10:{key}:{space_id}"),
        },
    )
    .await
    .unwrap();
    jobs::complete(pool, job.id, &worker_id, 0, 0)
        .await
        .unwrap();
    change_set
}

// Keep one call binding every projection to the same ACL fixture and expected
// storage status; splitting these values would weaken the negative matrix.
#[allow(clippy::too_many_arguments)]
async fn assert_graph_and_job_hidden(
    app: &axum::Router,
    case: &str,
    space_id: Uuid,
    from_node_id: &str,
    to_node_id: &str,
    job_id: Uuid,
    source_id: &str,
    object_id: &str,
    change_set_id: Uuid,
    storage_status: StatusCode,
) {
    let workbench = "/api/v1/web/workbench/knowledge";
    for (label, path) in [
        (
            "Vault list",
            format!("{workbench}/spaces/{space_id}/vaults"),
        ),
        (
            "Source list",
            format!("{workbench}/spaces/{space_id}/sources"),
        ),
        ("Source detail", format!("{workbench}/sources/{source_id}")),
        (
            "Source bytes",
            format!("{workbench}/sources/{source_id}/blob"),
        ),
        (
            "Library list",
            format!("{workbench}/spaces/{space_id}/objects?canonical_kind=note"),
        ),
        ("Note body", format!("{workbench}/objects/{object_id}/note")),
    ] {
        assert_hidden_get(app, case, label, &path, storage_status).await;
    }

    for (label, path) in [
        (
            "Review list",
            format!("{workbench}/spaces/{space_id}/change-sets"),
        ),
        (
            "Review detail",
            format!("{workbench}/change-sets/{change_set_id}"),
        ),
        (
            "Evolution trace",
            format!("{workbench}/spaces/{space_id}/evolution"),
        ),
    ] {
        assert_hidden_get(app, case, label, &path, StatusCode::NOT_FOUND).await;
    }

    let path = post_json(
        app,
        "/knowledge/graph/path",
        serde_json::json!({
            "from_node_id": from_node_id,
            "to_node_id": to_node_id,
            "space_ids": [space_id]
        }),
    )
    .await;
    assert_eq!(path.status(), StatusCode::NOT_FOUND, "{case}: graph path");

    let diagnostics = post_json(
        app,
        "/knowledge/graph/diagnostics",
        serde_json::json!({"space_ids":[space_id]}),
    )
    .await;
    assert_eq!(
        diagnostics.status(),
        StatusCode::NOT_FOUND,
        "{case}: graph diagnostics"
    );

    let job_list = get(app, &format!("/knowledge/spaces/{space_id}/jobs")).await;
    assert_eq!(job_list.status(), StatusCode::NOT_FOUND, "{case}: job list");

    let job_detail = get(app, &format!("/knowledge/jobs/{job_id}")).await;
    assert_eq!(
        job_detail.status(),
        StatusCode::NOT_FOUND,
        "{case}: job detail"
    );
}

#[tokio::test]
async fn r2_3_generation_queries_acl_incremental_and_deterministic_rebuild() {
    if !pg_available().await {
        eprintln!("skipping R2.3 graph fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    let admin_id = Uuid::new_v4();
    let viewer_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'graph-tenant')")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    for (user, email, role) in [
        (admin_id, "admin@graph.test", "admin"),
        (viewer_id, "viewer@graph.test", "member"),
    ] {
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
             VALUES ($1, $2, $3, $3, $4, 'test')",
        )
        .bind(user)
        .bind(tenant_id)
        .bind(email)
        .bind(role)
        .execute(harness.pool())
        .await
        .unwrap();
    }
    let foreign_tenant_id = Uuid::new_v4();
    let foreign_user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'graph-foreign')")
        .bind(foreign_tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1, $2, 'foreign@graph.test', 'Foreign', 'admin', 'test')",
    )
    .bind(foreign_user_id)
    .bind(foreign_tenant_id)
    .execute(harness.pool())
    .await
    .unwrap();
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let visible_project = spaces::create_project(
        harness.pool(),
        actor,
        CreateProject {
            slug: "visible-graph".into(),
            title: "Visible Graph".into(),
            goal: "R2.3 visible".into(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let hidden_project = spaces::create_project(
        harness.pool(),
        actor,
        CreateProject {
            slug: "hidden-graph".into(),
            title: "Hidden Graph".into(),
            goal: "R2.3 hidden".into(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let viewer_grant = spaces::upsert_grant(
        harness.pool(),
        actor,
        visible_project.space.id,
        PrincipalKind::User,
        &viewer_id.to_string(),
        SpaceRole::Viewer,
        None,
    )
    .await
    .unwrap();
    let visible_vault = spaces::ensure_vault(
        harness.pool(),
        actor,
        visible_project.space.id,
        EnsureVault {
            home_bundle_id: "server-administrator".into(),
            knowledge_schema_id: "server.knowledge".into(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let hidden_vault = spaces::ensure_vault(
        harness.pool(),
        actor,
        hidden_project.space.id,
        EnsureVault {
            home_bundle_id: "travel-planner".into(),
            knowledge_schema_id: "travel.knowledge".into(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let vault_root = tempfile::tempdir().unwrap();
    let wiki_root = tempfile::tempdir().unwrap();
    let config = KnowledgeConfig::extract_from_toml_str(&format!(
        "[knowledge]\nwiki_path = {:?}\nvault_path = {:?}\n",
        wiki_root.path().join("wiki"),
        vault_root.path()
    ))
    .unwrap()
    .unwrap();
    let dispatcher: Arc<dyn GadgetDispatcher> = Arc::new(KnowledgeDispatcher {
        provider: KnowledgeGadgetProvider::new(config, Some(harness.pool().clone())).unwrap(),
    });
    let state = state(harness.pool().clone(), vault_root.path(), dispatcher);
    let admin_app = app(state.clone(), context(tenant_id, admin_id));
    let viewer_app = app(state.clone(), context(tenant_id, viewer_id));
    let foreign_app = app(state, context(foreign_tenant_id, foreign_user_id));

    let alpha = upload(
        &admin_app,
        visible_vault.id,
        "Alpha",
        "# Alpha\n\nSee [[Beta]] and [[Missing target]].\n",
    )
    .await;
    let beta = upload(&admin_app, visible_vault.id, "Beta", "# Beta\n").await;
    let hidden = upload(&admin_app, hidden_vault.id, "Hidden", "# Hidden\n").await;
    let alpha_object = alpha["object"]["id"].as_str().unwrap();
    let beta_object = beta["object"]["id"].as_str().unwrap();
    let hidden_object = hidden["object"]["id"].as_str().unwrap();
    let alpha_source = alpha["source"]["id"].as_str().unwrap();
    let beta_source = beta["source"]["id"].as_str().unwrap();
    let beta_source_revision = beta["source"]["revision"].as_i64().unwrap();
    let beta_path = beta["object"]["path"]
        .as_str()
        .unwrap()
        .strip_suffix(".md")
        .unwrap();

    let change_set = propose_gardener_change_set(
        harness.pool(),
        actor,
        visible_project.space.id,
        visible_vault.id,
        Uuid::parse_str(beta_source).unwrap(),
        "trust-boundary",
        serde_json::json!([{
            "op": "create_note",
            "title": "Visible review proposal",
            "body": "# Visible review proposal\n\nGrounded in Beta.\n"
        }]),
    )
    .await;
    let job = jobs::get(
        harness.pool(),
        actor,
        change_set.job_id.expect("Gardener change set job"),
    )
    .await
    .unwrap();

    let alpha_note = note(&admin_app, alpha_object).await;
    let mut alpha_properties = alpha_note["properties"].clone();
    alpha_properties["links_to"] = serde_json::json!([hidden_object]);
    alpha_properties["supports"] = serde_json::json!([format!("[[{beta_path}]]")]);
    put_note(
        &admin_app,
        alpha_object,
        &alpha_note,
        alpha_properties,
        "# Alpha\n\nSee [[Beta]] and [[Missing target]].\n",
    )
    .await;
    let beta_note = note(&admin_app, beta_object).await;
    let mut beta_properties = beta_note["properties"].clone();
    beta_properties["contradicts"] = serde_json::json!([alpha_object]);
    put_note(
        &admin_app,
        beta_object,
        &beta_note,
        beta_properties,
        "# Beta\n",
    )
    .await;

    // Canonical mutations reconcile automatically. Remove only the derived
    // index so the explicit full-rebuild path proves clean recovery.
    sqlx::query("DELETE FROM knowledge_graph_generations WHERE tenant_id = $1")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();

    let rebuilt = post_json(
        &admin_app,
        "/knowledge/graph/rebuild",
        serde_json::json!({"mode":"full"}),
    )
    .await;
    assert_eq!(rebuilt.status(), StatusCode::OK);
    let rebuilt = json(rebuilt).await;
    assert_eq!(rebuilt["result"]["changed"], true);
    assert_eq!(rebuilt["result"]["mode"], "full");
    let generation_id = rebuilt["result"]["generation"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let digest = rebuilt["result"]["generation"]["input_digest"]
        .as_str()
        .unwrap()
        .to_string();

    // A failed replacement may not disturb the active generation. The
    // unknown Space passes in-memory shape validation but fails inside the
    // database transaction after the building generation is inserted.
    let failed_rebuild = graph::materialize(
        harness.pool(),
        tenant_id,
        admin_id,
        ReconcileMode::Full,
        GraphSnapshotInput {
            nodes: vec![GraphNodeInput {
                stable_node_id: "note:transaction-rollback-probe".into(),
                space_id: Uuid::new_v4(),
                vault_id: None,
                node_kind: "note".into(),
                canonical_id: None,
                canonical_revision: 0,
                home_bundle_id: "core".into(),
                title: "Rollback probe".into(),
                status: "active".into(),
                freshness: "current".into(),
                content_hash: None,
                metadata: serde_json::json!({}),
            }],
            edges: vec![],
        },
    )
    .await;
    assert!(failed_rebuild.is_err());
    assert_eq!(
        graph::active_generation(harness.pool(), tenant_id)
            .await
            .unwrap()
            .id
            .to_string(),
        generation_id
    );

    let alpha_node = format!("note:{alpha_object}");
    let beta_node = format!("note:{beta_object}");
    let hidden_node = format!("note:{hidden_object}");
    let neighborhood = post_json(
        &viewer_app,
        "/knowledge/graph/neighborhood",
        serde_json::json!({
            "center_node_id": alpha_node,
            "depth": 2,
            "space_ids": [visible_project.space.id]
        }),
    )
    .await;
    assert_eq!(neighborhood.status(), StatusCode::OK);
    let neighborhood = json(neighborhood).await;
    assert!(neighborhood["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["stable_node_id"] == beta_node));
    assert!(!neighborhood["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["stable_node_id"] == hidden_node));
    assert!(!neighborhood["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| edge["to_node_id"] == hidden_node));
    assert!(neighborhood["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| edge["from_node_id"] == alpha_node
            && edge["to_node_id"] == beta_node
            && edge["relation_kind"] == "supports"
            && edge["evidence"]["kind"] == "yaml_property"));

    let hidden_direct = viewer_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/graph/nodes/{hidden_node}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden_direct.status(), StatusCode::NOT_FOUND);
    let hidden_scope = post_json(
        &viewer_app,
        "/knowledge/graph/neighborhood",
        serde_json::json!({
            "center_node_id": alpha_node,
            "space_ids": [visible_project.space.id, hidden_project.space.id]
        }),
    )
    .await;
    assert_eq!(hidden_scope.status(), StatusCode::NOT_FOUND);

    let backlinks = post_json(
        &viewer_app,
        "/knowledge/graph/backlinks",
        serde_json::json!({
            "node_id": beta_node,
            "space_ids": [visible_project.space.id]
        }),
    )
    .await;
    assert_eq!(backlinks.status(), StatusCode::OK);
    assert!(json(backlinks).await["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| edge["from_node_id"] == alpha_node && edge["relation_kind"] == "links_to"));

    let path = post_json(
        &viewer_app,
        "/knowledge/graph/path",
        serde_json::json!({
            "from_node_id": alpha_node,
            "to_node_id": beta_node,
            "space_ids": [visible_project.space.id]
        }),
    )
    .await;
    assert_eq!(path.status(), StatusCode::OK);
    assert_eq!(json(path).await["paths"][0]["node_ids"][1], beta_node);

    let diagnostics = post_json(
        &viewer_app,
        "/knowledge/graph/diagnostics",
        serde_json::json!({"space_ids":[visible_project.space.id]}),
    )
    .await;
    assert_eq!(diagnostics.status(), StatusCode::OK);
    let diagnostics = json(diagnostics).await;
    assert!(!diagnostics["broken_edges"].as_array().unwrap().is_empty());
    assert!(!diagnostics["stale_nodes"].as_array().unwrap().is_empty());
    assert!(!diagnostics["contradiction_edges"]
        .as_array()
        .unwrap()
        .is_empty());

    sqlx::query(
        "UPDATE knowledge_graph_nodes SET node_kind = 'lesson', metadata = \
         jsonb_build_object('review_state', 'verified') \
         WHERE tenant_id = $1 AND stable_node_id = $2",
    )
    .bind(tenant_id)
    .bind(&alpha_node)
    .execute(harness.pool())
    .await
    .unwrap();
    let library = spaces::list_objects(
        harness.pool(),
        actor,
        visible_project.space.id,
        Some("server-administrator"),
        Some("note"),
    )
    .await
    .unwrap();
    let alpha_library = library
        .iter()
        .find(|object| object.id.to_string() == alpha_object)
        .unwrap();
    assert_eq!(alpha_library.canonical_kind, "note");
    assert_eq!(alpha_library.knowledge_kind, "lesson");
    assert_eq!(alpha_library.review_state.as_deref(), Some("verified"));
    assert_eq!(alpha_library.freshness, "stale");

    let workbench = "/api/v1/web/workbench/knowledge";
    let visible_library = get(
        &viewer_app,
        &format!(
            "{workbench}/spaces/{}/objects?canonical_kind=note",
            visible_project.space.id
        ),
    )
    .await;
    assert_eq!(visible_library.status(), StatusCode::OK);
    let visible_library = json(visible_library).await;
    let alpha_projection = visible_library["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|object| object["id"] == alpha_object)
        .unwrap();
    assert_eq!(alpha_projection["knowledge_kind"], "lesson");
    assert_eq!(alpha_projection["review_state"], "verified");

    let visible_sources = get(
        &viewer_app,
        &format!("{workbench}/spaces/{}/sources", visible_project.space.id),
    )
    .await;
    assert_eq!(visible_sources.status(), StatusCode::OK);
    assert_eq!(json(visible_sources).await["returned"], 2);
    for path in [
        format!("{workbench}/spaces/{}/vaults", visible_project.space.id),
        format!("{workbench}/sources/{alpha_source}"),
        format!("{workbench}/objects/{alpha_object}/note"),
        format!("{workbench}/spaces/{}/evolution", visible_project.space.id),
    ] {
        assert_eq!(get(&viewer_app, &path).await.status(), StatusCode::OK);
    }
    let visible_blob = get(
        &viewer_app,
        &format!("{workbench}/sources/{alpha_source}/blob"),
    )
    .await;
    assert_eq!(visible_blob.status(), StatusCode::OK);
    assert!(String::from_utf8_lossy(
        &to_bytes(visible_blob.into_body(), 1024 * 1024)
            .await
            .unwrap()
    )
    .contains("# Alpha"));

    let visible_reviews = get(
        &viewer_app,
        &format!(
            "{workbench}/spaces/{}/change-sets",
            visible_project.space.id
        ),
    )
    .await;
    assert_eq!(visible_reviews.status(), StatusCode::OK);
    let visible_reviews = json(visible_reviews).await;
    assert_eq!(visible_reviews["returned"], 1);
    assert_eq!(
        visible_reviews["change_sets"][0]["id"],
        change_set.id.to_string()
    );
    let visible_review = get(
        &viewer_app,
        &format!("{workbench}/change-sets/{}", change_set.id),
    )
    .await;
    assert_eq!(visible_review.status(), StatusCode::OK);
    assert_eq!(json(visible_review).await["title"], "J10 trust-boundary");

    let visible_search = search(&viewer_app, "Alpha").await;
    assert!(visible_search["hits"]
        .as_array()
        .unwrap()
        .iter()
        .any(|hit| hit["page_name"].as_str().unwrap().contains(alpha_object)));

    let job_list = get(
        &viewer_app,
        &format!("/knowledge/spaces/{}/jobs", visible_project.space.id),
    )
    .await;
    assert_eq!(job_list.status(), StatusCode::OK);
    let job_list = json(job_list).await;
    assert_eq!(job_list["returned"], 1);
    assert_eq!(job_list["jobs"][0]["id"], job.id.to_string());

    let job_detail = get(&viewer_app, &format!("/knowledge/jobs/{}", job.id)).await;
    assert_eq!(job_detail.status(), StatusCode::OK);
    assert_eq!(json(job_detail).await["job"]["id"], job.id.to_string());

    spaces::revoke_grant(
        harness.pool(),
        actor,
        visible_project.space.id,
        viewer_grant.id,
        viewer_grant.revision,
    )
    .await
    .unwrap();
    assert!(search(&viewer_app, "Alpha").await["hits"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(search(&foreign_app, "Alpha").await["hits"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_graph_and_job_hidden(
        &viewer_app,
        "revoked grant",
        visible_project.space.id,
        &alpha_node,
        &beta_node,
        job.id,
        alpha_source,
        alpha_object,
        change_set.id,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_graph_and_job_hidden(
        &foreign_app,
        "foreign tenant",
        visible_project.space.id,
        &alpha_node,
        &beta_node,
        job.id,
        alpha_source,
        alpha_object,
        change_set.id,
        StatusCode::NOT_FOUND,
    )
    .await;

    let noop = post_json(
        &admin_app,
        "/knowledge/graph/rebuild",
        serde_json::json!({"mode":"incremental"}),
    )
    .await;
    let noop = json(noop).await;
    assert_eq!(noop["result"]["changed"], false);
    assert_eq!(noop["result"]["generation"]["id"], generation_id);
    assert_eq!(noop["result"]["generation"]["graph_revision"], 1);

    let alpha_note = note(&admin_app, alpha_object).await;
    let alpha_properties = alpha_note["properties"].clone();
    put_note(
        &admin_app,
        alpha_object,
        &alpha_note,
        alpha_properties,
        "# Alpha\n\nSee [[Beta]]. The missing link was removed.\n",
    )
    .await;
    let generation = admin_app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/knowledge/graph/generation")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(generation.status(), StatusCode::OK);
    let generation = json(generation).await;
    assert_eq!(generation["id"], generation_id);
    assert_eq!(generation["graph_revision"], 2);
    let updated_digest = generation["input_digest"].as_str().unwrap().to_string();
    assert_ne!(updated_digest, digest);

    let incremental_noop = post_json(
        &admin_app,
        "/knowledge/graph/rebuild",
        serde_json::json!({"mode":"incremental"}),
    )
    .await;
    assert_eq!(incremental_noop.status(), StatusCode::OK);
    let incremental_noop = json(incremental_noop).await;
    assert_eq!(incremental_noop["result"]["mode"], "noop");
    assert_eq!(incremental_noop["result"]["changed"], false);
    assert_eq!(
        incremental_noop["result"]["generation"]["graph_revision"],
        2
    );

    // Source removal must not trip the node FK: automatic delta first turns
    // resolved citation edges into broken edges, then removes the Source node.
    let deleted_source = admin_app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/knowledge/sources/{beta_source}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"expected_revision": beta_source_revision}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_source.status(), StatusCode::OK);
    let after_delete = admin_app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/knowledge/graph/generation")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let after_delete = json(after_delete).await;
    assert_eq!(after_delete["graph_revision"], 3);
    let updated_digest = after_delete["input_digest"].as_str().unwrap().to_string();
    let source_node_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM knowledge_graph_nodes WHERE tenant_id = $1 AND stable_node_id = $2)",
    )
    .bind(tenant_id)
    .bind(format!("source:{beta_source}"))
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert!(!source_node_exists);
    let broken_beta_citation: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM knowledge_graph_edges WHERE tenant_id = $1 AND target_ref = $2 AND status = 'broken')",
    )
    .bind(tenant_id)
    .bind(beta_source)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert!(broken_beta_citation);

    let ids_before: Vec<String> = sqlx::query_scalar(
        "SELECT stable_node_id FROM knowledge_graph_nodes WHERE tenant_id = $1 ORDER BY stable_node_id",
    )
    .bind(tenant_id)
    .fetch_all(harness.pool())
    .await
    .unwrap();
    let edges_before: Vec<String> = sqlx::query_scalar(
        "SELECT stable_edge_id FROM knowledge_graph_edges WHERE tenant_id = $1 ORDER BY stable_edge_id",
    )
    .bind(tenant_id)
    .fetch_all(harness.pool())
    .await
    .unwrap();
    sqlx::query("DELETE FROM knowledge_graph_generations WHERE tenant_id = $1")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    let restored = post_json(
        &admin_app,
        "/knowledge/graph/rebuild",
        serde_json::json!({"mode":"full"}),
    )
    .await;
    assert_eq!(restored.status(), StatusCode::OK);
    let restored = json(restored).await;
    assert_ne!(restored["result"]["generation"]["id"], generation_id);
    assert_eq!(
        restored["result"]["generation"]["input_digest"],
        updated_digest
    );
    let ids_after: Vec<String> = sqlx::query_scalar(
        "SELECT stable_node_id FROM knowledge_graph_nodes WHERE tenant_id = $1 ORDER BY stable_node_id",
    )
    .bind(tenant_id)
    .fetch_all(harness.pool())
    .await
    .unwrap();
    let edges_after: Vec<String> = sqlx::query_scalar(
        "SELECT stable_edge_id FROM knowledge_graph_edges WHERE tenant_id = $1 ORDER BY stable_edge_id",
    )
    .bind(tenant_id)
    .fetch_all(harness.pool())
    .await
    .unwrap();
    assert_eq!(ids_before, ids_after);
    assert_eq!(edges_before, edges_after);

    harness.cleanup().await;
}

#[tokio::test]
async fn k14_exact_duplicate_group_creates_and_applies_user_merge_change_set() {
    if !pg_available().await {
        eprintln!("skipping K1.4 cleanup inbox journey: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    let admin_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'k14-cleanup')")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1, $2, 'k14@example.test', 'K14 Admin', 'admin', 'test')",
    )
    .bind(admin_id)
    .bind(tenant_id)
    .execute(harness.pool())
    .await
    .unwrap();
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        harness.pool(),
        actor,
        CreateProject {
            slug: "cleanup-inbox".into(),
            title: "Cleanup inbox".into(),
            goal: "Merge exact duplicate notes through review".into(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let vault = spaces::ensure_vault(
        harness.pool(),
        actor,
        project.space.id,
        EnsureVault {
            home_bundle_id: "computer-science-research".into(),
            knowledge_schema_id: "cs.knowledge".into(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let vault_root = tempfile::tempdir().unwrap();
    let wiki_root = tempfile::tempdir().unwrap();
    let config = KnowledgeConfig::extract_from_toml_str(&format!(
        "[knowledge]\nwiki_path = {:?}\nvault_path = {:?}\n",
        wiki_root.path().join("wiki"),
        vault_root.path()
    ))
    .unwrap()
    .unwrap();
    let dispatcher: Arc<dyn GadgetDispatcher> = Arc::new(KnowledgeDispatcher {
        provider: KnowledgeGadgetProvider::new(config, Some(harness.pool().clone())).unwrap(),
    });
    let app = app(
        state(harness.pool().clone(), vault_root.path(), dispatcher),
        context(tenant_id, admin_id),
    );

    let first = upload(
        &app,
        vault.id,
        "Retry checklist",
        "# Retry checklist\n\nCheck the queue before retrying.\n",
    )
    .await;
    let second = upload(
        &app,
        vault.id,
        "  retry   CHECKLIST ",
        "# Retry checklist\n\nRecord the final worker state.\n",
    )
    .await;
    let first_id = first["object"]["id"].as_str().unwrap();
    let second_id = second["object"]["id"].as_str().unwrap();
    let groups = get(
        &app,
        &format!("/knowledge/spaces/{}/duplicate-groups", project.space.id),
    )
    .await;
    assert_eq!(groups.status(), StatusCode::OK);
    let groups = json(groups).await;
    assert_eq!(groups["returned"], 1);
    assert_eq!(groups["groups"][0]["confidence"], "exact");
    assert!(groups["groups"][0]["match_reasons"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("normalized_title")));

    let proposed = post_json(
        &app,
        &format!("/knowledge/spaces/{}/merge-change-sets", project.space.id),
        serde_json::json!({
            "idempotency_key": Uuid::new_v4(),
            "sources": [
                {"object_id": first_id, "expected_revision": 1},
                {"object_id": second_id, "expected_revision": 1}
            ],
            "master_object_id": first_id,
            "field_sources": {"title": second_id},
            "body_strategy": "keep_both"
        }),
    )
    .await;
    let proposed_status = proposed.status();
    let proposed = json(proposed).await;
    assert_eq!(proposed_status, StatusCode::OK, "{proposed}");
    assert_eq!(proposed["origin"], "user");
    assert_eq!(proposed["job_id"], serde_json::Value::Null);
    assert_eq!(proposed["status"], "pending_user_review");
    assert_eq!(proposed["operations"][0]["op"], "merge_notes");

    let accepted = post_json(
        &app,
        &format!(
            "/knowledge/change-sets/{}/accept",
            proposed["id"].as_str().unwrap()
        ),
        serde_json::json!({"expected_revision": proposed["revision"]}),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    let accepted = json(accepted).await;
    assert_eq!(accepted["status"], "applied");
    let merged = note(&app, accepted["materialized_object_id"].as_str().unwrap()).await;
    assert_eq!(merged["properties"]["canonical_change"], "merge");
    assert!(merged["body"]
        .as_str()
        .unwrap()
        .contains("Check the queue before retrying."));
    assert!(merged["body"]
        .as_str()
        .unwrap()
        .contains("Record the final worker state."));

    harness.cleanup().await;
}

#[tokio::test]
async fn r4_5_reviewed_merge_and_split_preserve_revisions_and_graph_relations() {
    if !pg_available().await {
        eprintln!("skipping J10 merge/split journey: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    let admin_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'j10-merge-split')")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1, $2, 'j10@example.test', 'J10 Admin', 'admin', 'test')",
    )
    .bind(admin_id)
    .bind(tenant_id)
    .execute(harness.pool())
    .await
    .unwrap();
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        harness.pool(),
        actor,
        CreateProject {
            slug: "reviewed-evolution".into(),
            title: "Reviewed Evolution".into(),
            goal: "Prove canonical merge and split".into(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let vault = spaces::ensure_vault(
        harness.pool(),
        actor,
        project.space.id,
        EnsureVault {
            home_bundle_id: "computer-science-research".into(),
            knowledge_schema_id: "cs.knowledge".into(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let vault_root = tempfile::tempdir().unwrap();
    let wiki_root = tempfile::tempdir().unwrap();
    let config = KnowledgeConfig::extract_from_toml_str(&format!(
        "[knowledge]\nwiki_path = {:?}\nvault_path = {:?}\n",
        wiki_root.path().join("wiki"),
        vault_root.path()
    ))
    .unwrap()
    .unwrap();
    let dispatcher: Arc<dyn GadgetDispatcher> = Arc::new(KnowledgeDispatcher {
        provider: KnowledgeGadgetProvider::new(config, Some(harness.pool().clone())).unwrap(),
    });
    let app = app(
        state(harness.pool().clone(), vault_root.path(), dispatcher),
        context(tenant_id, admin_id),
    );

    let first = upload(
        &app,
        vault.id,
        "First scheduler note",
        "# First scheduler note\n\nQueues preserve work ordering.\n",
    )
    .await;
    let second = upload(
        &app,
        vault.id,
        "Second scheduler note",
        "# Second scheduler note\n\nLeases prevent duplicate workers.\n",
    )
    .await;
    let first_id: Uuid = first["object"]["id"].as_str().unwrap().parse().unwrap();
    let second_id: Uuid = second["object"]["id"].as_str().unwrap().parse().unwrap();
    let source_id: Uuid = first["source"]["id"].as_str().unwrap().parse().unwrap();
    let source_revisions: Vec<i64> = sqlx::query_scalar(
        "SELECT revision FROM knowledge_objects WHERE tenant_id = $1 AND id = ANY($2) ORDER BY id",
    )
    .bind(tenant_id)
    .bind([first_id, second_id])
    .fetch_all(harness.pool())
    .await
    .unwrap();
    assert_eq!(source_revisions, vec![1, 1]);

    let merge = propose_gardener_change_set(
        harness.pool(),
        actor,
        project.space.id,
        vault.id,
        source_id,
        "merge",
        serde_json::json!([{
            "op": "merge_notes",
            "sources": [
                {"object_id": first_id, "expected_revision": 1},
                {"object_id": second_id, "expected_revision": 1}
            ],
            "title": "Durable worker coordination",
            "body": "Queues preserve ordering while leases prevent duplicate workers."
        }]),
    )
    .await;
    assert_eq!(merge.status, "pending_user_review");
    assert_eq!(merge.citations[0]["source_id"], source_id.to_string());
    let merged = post_json(
        &app,
        &format!("/knowledge/change-sets/{}/accept", merge.id),
        serde_json::json!({
            "expected_revision": merge.revision,
            "rationale": "The two notes describe one coordination mechanism."
        }),
    )
    .await;
    assert_eq!(merged.status(), StatusCode::OK);
    let merged = json(merged).await;
    assert_eq!(merged["status"], "applied");
    assert_eq!(
        merged["operations"][0]["sources"][0]["expected_revision"],
        1
    );
    let merged_id = merged["materialized_object_id"].as_str().unwrap();
    let merged_note = note(&app, merged_id).await;
    assert_eq!(merged_note["properties"]["canonical_change"], "merge");
    assert_eq!(
        merged_note["properties"]["change_set"],
        merge.id.to_string()
    );
    assert_eq!(
        merged_note["properties"]["source_revisions"][first_id.to_string()],
        1
    );
    assert_eq!(
        merged_note["properties"]["source_revisions"][second_id.to_string()],
        1
    );
    let merge_edges: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_graph_edges e \
         JOIN knowledge_graph_generations g \
           ON g.tenant_id = e.tenant_id AND g.id = e.generation_id AND g.state = 'active' \
         WHERE e.tenant_id = $1 AND e.from_node_id = $2 \
           AND e.relation_kind IN ('derived_from', 'supersedes') \
           AND e.to_node_id = ANY($3)",
    )
    .bind(tenant_id)
    .bind(format!("note:{merged_id}"))
    .bind(&[format!("note:{first_id}"), format!("note:{second_id}")])
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(merge_edges, 4);

    let split = propose_gardener_change_set(
        harness.pool(),
        actor,
        project.space.id,
        vault.id,
        source_id,
        "split",
        serde_json::json!([{
            "op": "split_note",
            "source_object_id": merged_id,
            "expected_revision": 1,
            "outputs": [{
                "title": "Queue ordering",
                "body": "Queues preserve work ordering."
            }, {
                "title": "Worker leases",
                "body": "Leases prevent duplicate workers."
            }]
        }]),
    )
    .await;
    let split = post_json(
        &app,
        &format!("/knowledge/change-sets/{}/accept", split.id),
        serde_json::json!({"expected_revision":split.revision}),
    )
    .await;
    assert_eq!(split.status(), StatusCode::OK);
    let split = json(split).await;
    assert_eq!(split["status"], "applied");
    let split_objects = split["materialization_receipt"]["objects"]
        .as_array()
        .unwrap();
    assert_eq!(split_objects.len(), 2);
    for object in split_objects {
        let child_id = object["id"].as_str().unwrap();
        let child = note(&app, child_id).await;
        assert_eq!(child["properties"]["canonical_change"], "split");
        assert_eq!(child["properties"]["derived_from"], merged_id);
        assert_eq!(child["properties"]["source_revision"], 1);
    }
    let split_node_ids = split_objects
        .iter()
        .map(|object| format!("note:{}", object["id"].as_str().unwrap()))
        .collect::<Vec<_>>();
    let split_edges: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_graph_edges e \
         JOIN knowledge_graph_generations g \
           ON g.tenant_id = e.tenant_id AND g.id = e.generation_id AND g.state = 'active' \
         WHERE e.tenant_id = $1 AND e.relation_kind = 'derived_from' AND e.to_node_id = $2 \
           AND e.from_node_id = ANY($3)",
    )
    .bind(tenant_id)
    .bind(format!("note:{merged_id}"))
    .bind(&split_node_ids)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(split_edges, 2);

    let object_count_before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_objects WHERE tenant_id = $1 AND vault_id = $2",
    )
    .bind(tenant_id)
    .bind(vault.id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    let stale = propose_gardener_change_set(
        harness.pool(),
        actor,
        project.space.id,
        vault.id,
        source_id,
        "stale-merge",
        serde_json::json!([{
            "op": "merge_notes",
            "sources": [
                {"object_id": first_id, "expected_revision": 999},
                {"object_id": second_id, "expected_revision": 1}
            ],
            "title": "Stale merge",
            "body": "This must not be written."
        }]),
    )
    .await;
    let stale = post_json(
        &app,
        &format!("/knowledge/change-sets/{}/accept", stale.id),
        serde_json::json!({"expected_revision":stale.revision}),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::OK);
    let stale = json(stale).await;
    assert_eq!(stale["status"], "failed_retryable");
    assert!(stale["materialized_object_id"].is_null());
    let object_count_after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_objects WHERE tenant_id = $1 AND vault_id = $2",
    )
    .bind(tenant_id)
    .bind(vault.id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(object_count_after, object_count_before);

    harness.cleanup().await;
}

#[tokio::test]
async fn k12_review_edit_reject_and_retry_apply_revalidate_vault_state() {
    if !pg_available().await {
        eprintln!("skipping K12 review recovery journey: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    let admin_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'k12-review-recovery')")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1, $2, 'k12@example.test', 'K12 Admin', 'admin', 'test')",
    )
    .bind(admin_id)
    .bind(tenant_id)
    .execute(harness.pool())
    .await
    .unwrap();
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        harness.pool(),
        actor,
        CreateProject {
            slug: "review-recovery".into(),
            title: "Review Recovery".into(),
            goal: "Prove human-reviewed Knowledge materialization".into(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let vault = spaces::ensure_vault(
        harness.pool(),
        actor,
        project.space.id,
        EnsureVault {
            home_bundle_id: "server-administrator".into(),
            knowledge_schema_id: "server.knowledge".into(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let vault_root = tempfile::tempdir().unwrap();
    let wiki_root = tempfile::tempdir().unwrap();
    let config = KnowledgeConfig::extract_from_toml_str(&format!(
        "[knowledge]\nwiki_path = {:?}\nvault_path = {:?}\n",
        wiki_root.path().join("wiki"),
        vault_root.path()
    ))
    .unwrap()
    .unwrap();
    let dispatcher: Arc<dyn GadgetDispatcher> = Arc::new(KnowledgeDispatcher {
        provider: KnowledgeGadgetProvider::new(config, Some(harness.pool().clone())).unwrap(),
    });
    let app = app(
        state(harness.pool().clone(), vault_root.path(), dispatcher),
        context(tenant_id, admin_id),
    );

    let first = upload(
        &app,
        vault.id,
        "Cooling recovery",
        "# Cooling recovery\n\nCheck the loop.\n",
    )
    .await;
    let second = upload(
        &app,
        vault.id,
        "Worker retry",
        "# Worker retry\n\nRetry only unchanged work.\n",
    )
    .await;
    let first_id = first["object"]["id"].as_str().unwrap();
    let second_id = second["object"]["id"].as_str().unwrap();
    let source_id: Uuid = first["source"]["id"].as_str().unwrap().parse().unwrap();

    let edited_proposal = propose_gardener_change_set(
        harness.pool(),
        actor,
        project.space.id,
        vault.id,
        source_id,
        "edit-accept",
        serde_json::json!([{
            "op": "update_note",
            "object_id": first_id,
            "expected_revision": 1,
            "title": "Cooling recovery",
            "body": "# Cooling recovery\n\nCheck the loop before declaring recovery.\n"
        }]),
    )
    .await;
    let edited = put_json(
        &app,
        &format!("/knowledge/change-sets/{}", edited_proposal.id),
        serde_json::json!({
            "expected_revision": edited_proposal.revision,
            "title": edited_proposal.title,
            "summary": "Reviewer clarified the required verification.",
            "operations": [{
                "op": "update_note",
                "object_id": first_id,
                "expected_revision": 1,
                "title": "Cooling recovery",
                "body": "# Cooling recovery\n\nVerify the loop twice before declaring recovery.\n"
            }],
            "citations": edited_proposal.citations,
        }),
    )
    .await;
    assert_eq!(edited.status(), StatusCode::OK);
    let edited = json(edited).await;
    assert_eq!(edited["status"], "pending_user_review");
    let applied = post_json(
        &app,
        &format!("/knowledge/change-sets/{}/accept", edited_proposal.id),
        serde_json::json!({"expected_revision": edited["revision"]}),
    )
    .await;
    assert_eq!(applied.status(), StatusCode::OK);
    let applied = json(applied).await;
    assert_eq!(applied["status"], "applied");
    assert!(applied["applied_git_revision"].as_str().is_some());
    assert!(note(&app, first_id).await["body"]
        .as_str()
        .unwrap()
        .contains("Verify the loop twice"));

    let retry_proposal = propose_gardener_change_set(
        harness.pool(),
        actor,
        project.space.id,
        vault.id,
        source_id,
        "retry-head",
        serde_json::json!([{
            "op": "update_note",
            "object_id": second_id,
            "expected_revision": 1,
            "title": "Worker retry",
            "body": "# Worker retry\n\nRetry unchanged work after an unrelated Git advance.\n"
        }]),
    )
    .await;
    let pinned = put_json(
        &app,
        &format!("/knowledge/change-sets/{}", retry_proposal.id),
        serde_json::json!({
            "expected_revision": retry_proposal.revision,
            "title": retry_proposal.title,
            "summary": retry_proposal.summary,
            "operations": retry_proposal.operations,
            "citations": retry_proposal.citations,
        }),
    )
    .await;
    assert_eq!(pinned.status(), StatusCode::OK);
    let pinned = json(pinned).await;
    upload(
        &app,
        vault.id,
        "Unrelated note",
        "# Unrelated note\n\nThis advances only the Vault Git head.\n",
    )
    .await;
    let failed = post_json(
        &app,
        &format!("/knowledge/change-sets/{}/accept", retry_proposal.id),
        serde_json::json!({"expected_revision": pinned["revision"]}),
    )
    .await;
    assert_eq!(failed.status(), StatusCode::OK);
    let failed = json(failed).await;
    assert_eq!(failed["status"], "failed_retryable");
    let retried = post_json(
        &app,
        &format!("/knowledge/change-sets/{}/retry-apply", retry_proposal.id),
        serde_json::json!({"expected_revision": failed["revision"]}),
    )
    .await;
    assert_eq!(retried.status(), StatusCode::OK);
    let retried = json(retried).await;
    assert_eq!(retried["status"], "applied");
    assert!(retried["applied_git_revision"].as_str().is_some());
    assert!(note(&app, second_id).await["body"]
        .as_str()
        .unwrap()
        .contains("unrelated Git advance"));

    let conflict_proposal = propose_gardener_change_set(
        harness.pool(),
        actor,
        project.space.id,
        vault.id,
        source_id,
        "target-conflict",
        serde_json::json!([{
            "op": "update_note",
            "object_id": first_id,
            "expected_revision": 2,
            "title": "Cooling recovery",
            "body": "# Cooling recovery\n\nThis proposal must be reviewed against the current note.\n"
        }]),
    )
    .await;
    let current = note(&app, first_id).await;
    put_note(
        &app,
        first_id,
        &current,
        current["properties"].clone(),
        "# Cooling recovery\n\nA human changed the target after the proposal.\n",
    )
    .await;
    let failed = post_json(
        &app,
        &format!("/knowledge/change-sets/{}/accept", conflict_proposal.id),
        serde_json::json!({"expected_revision": conflict_proposal.revision}),
    )
    .await;
    assert_eq!(failed.status(), StatusCode::OK);
    let failed = json(failed).await;
    assert_eq!(failed["status"], "failed_retryable");
    let review_again = post_json(
        &app,
        &format!(
            "/knowledge/change-sets/{}/retry-apply",
            conflict_proposal.id
        ),
        serde_json::json!({"expected_revision": failed["revision"]}),
    )
    .await;
    assert_eq!(review_again.status(), StatusCode::OK);
    let review_again = json(review_again).await;
    assert_eq!(review_again["status"], "pending_user_review");
    assert_eq!(review_again["operations"][0]["expected_revision"], 3);
    assert_eq!(
        review_again["materialization_receipt"]["recovery"],
        "review_required"
    );
    assert!(review_again["materialization_receipt"]["error"]
        .as_str()
        .unwrap()
        .contains("Review the refreshed diff"));

    let rejected = post_json(
        &app,
        &format!("/knowledge/change-sets/{}/reject", conflict_proposal.id),
        serde_json::json!({
            "expected_revision": review_again["revision"],
            "rationale": "The human edit supersedes this proposal."
        }),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::OK);
    let rejected = json(rejected).await;
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(
        rejected["decision_rationale"],
        "The human edit supersedes this proposal."
    );

    harness.cleanup().await;
}

#[tokio::test]
async fn researcher_and_gardener_http_accept_only_visible_satisfied_outcomes() {
    if !pg_available().await {
        eprintln!("skipping CORE-T3 Outcome HTTP fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    let admin_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'core-t3-outcome')")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO users (id, tenant_id, email, display_name, role, password_hash)
           VALUES ($1,$2,$3,'CORE-T3 Admin','admin','test')"#,
    )
    .bind(admin_id)
    .bind(tenant_id)
    .bind(format!("core-t3-outcome-{tenant_id}@example.test"))
    .execute(harness.pool())
    .await
    .unwrap();
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        harness.pool(),
        actor,
        CreateProject {
            slug: "core-t3-outcome".into(),
            title: "CORE-T3 Outcome".into(),
            goal: "Validate pinned incident outcomes".into(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let vault = spaces::ensure_vault(
        harness.pool(),
        actor,
        project.space.id,
        EnsureVault {
            home_bundle_id: "server-administrator".into(),
            knowledge_schema_id: "server.knowledge".into(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let visible = insert_outcome(
        harness.pool(),
        tenant_id,
        admin_id,
        "core-t3-visible",
        "satisfied",
        project.space.id,
    )
    .await;
    let failed = insert_outcome(
        harness.pool(),
        tenant_id,
        admin_id,
        "core-t3-failed",
        "failed",
        project.space.id,
    )
    .await;
    let hidden = insert_outcome(
        harness.pool(),
        tenant_id,
        admin_id,
        "core-t3-hidden",
        "satisfied",
        Uuid::new_v4(),
    )
    .await;

    let vault_root = tempfile::tempdir().unwrap();
    let wiki_root = tempfile::tempdir().unwrap();
    let config = KnowledgeConfig::extract_from_toml_str(&format!(
        "[knowledge]\nwiki_path = {:?}\nvault_path = {:?}\n",
        wiki_root.path().join("wiki"),
        vault_root.path()
    ))
    .unwrap()
    .unwrap();
    let dispatcher: Arc<dyn GadgetDispatcher> = Arc::new(KnowledgeDispatcher {
        provider: KnowledgeGadgetProvider::new(config, Some(harness.pool().clone())).unwrap(),
    });
    let app = app(
        state(harness.pool().clone(), vault_root.path(), dispatcher),
        context(tenant_id, admin_id),
    );
    let source = upload(
        &app,
        vault.id,
        "Incident evidence",
        "# Incident evidence\n\nThe closure checks completed successfully.\n",
    )
    .await;
    let source_id = Uuid::parse_str(source["source"]["id"].as_str().unwrap()).unwrap();
    for role in ["researcher", "gardener"] {
        let response = post_json(
            &app,
            &format!("/knowledge/spaces/{}/jobs", project.space.id),
            serde_json::json!({
                "role": role,
                "output_vault_id": vault.id,
                "question": format!("Review the verified incident as a {role}"),
                "source_ids": [source_id],
                "outcome_ids": [visible]
            }),
        )
        .await;
        let status = response.status();
        let row = json(response).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{role} should accept Outcome: {row}"
        );
        assert_eq!(row["input"]["outcomes"][0]["id"], visible.to_string());
        assert_eq!(row["input"]["outcomes"][0]["predicate_result"], "satisfied");
    }
    for (label, outcome_id) in [("failed", failed), ("hidden", hidden)] {
        let response = post_json(
            &app,
            &format!("/knowledge/spaces/{}/jobs", project.space.id),
            serde_json::json!({
                "role": "researcher",
                "output_vault_id": vault.id,
                "question": format!("Reject the {label} Outcome"),
                "source_ids": [source_id],
                "outcome_ids": [outcome_id]
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{label}");
        let body = json(response).await;
        assert_eq!(body["error"]["code"], "knowledge_invalid_input");
    }

    harness.cleanup().await;
}

#[tokio::test]
async fn outcome_backed_lesson_revision_pins_exact_reviewed_lesson_and_provenance() {
    if !pg_available().await {
        eprintln!("skipping K13-T1 Lesson revision HTTP fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    let admin_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'k13-lesson-revision')")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO users (id, tenant_id, email, display_name, role, password_hash)
           VALUES ($1,$2,$3,'K13 Admin','admin','test')"#,
    )
    .bind(admin_id)
    .bind(tenant_id)
    .bind(format!("k13-lesson-revision-{tenant_id}@example.test"))
    .execute(harness.pool())
    .await
    .unwrap();
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        harness.pool(),
        actor,
        CreateProject {
            slug: "k13-lesson-revision".into(),
            title: "K13 Lesson revision".into(),
            goal: "Prove reviewed Lesson revision stays in Review".into(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let vault = spaces::ensure_vault(
        harness.pool(),
        actor,
        project.space.id,
        EnsureVault {
            home_bundle_id: "server-administrator".into(),
            knowledge_schema_id: "server.knowledge".into(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let vault_root = tempfile::tempdir().unwrap();
    let wiki_root = tempfile::tempdir().unwrap();
    let config = KnowledgeConfig::extract_from_toml_str(&format!(
        "[knowledge]\nwiki_path = {:?}\nvault_path = {:?}\n",
        wiki_root.path().join("wiki"),
        vault_root.path()
    ))
    .unwrap()
    .unwrap();
    let dispatcher: Arc<dyn GadgetDispatcher> = Arc::new(KnowledgeDispatcher {
        provider: KnowledgeGadgetProvider::new(config, Some(harness.pool().clone())).unwrap(),
    });
    let app = app(
        state(harness.pool().clone(), vault_root.path(), dispatcher),
        context(tenant_id, admin_id),
    );
    let uploaded = upload(
        &app,
        vault.id,
        "Closed incident evidence",
        "# Closed incident evidence\n\nThe service recovered after the runbook step.\n",
    )
    .await;
    let lesson_id = uploaded["object"]["id"].as_str().unwrap().to_string();
    let source_id = uploaded["source"]["id"].as_str().unwrap().to_string();
    let source_revision = uploaded["source"]["revision"].as_i64().unwrap();
    let current = note(&app, &lesson_id).await;
    let mut properties = current["properties"].clone();
    properties["knowledge_kind"] = serde_json::json!("lesson");
    properties["review_state"] = serde_json::json!("reviewed");
    properties["source_ids"] = serde_json::json!([source_id]);
    put_note(
        &app,
        &lesson_id,
        &current,
        properties,
        "# Closed incident lesson\n\nRe-run the validated recovery step, then verify health.\n",
    )
    .await;
    let lesson = note(&app, &lesson_id).await;
    let lesson_revision = lesson["revision"].as_i64().unwrap();
    let cited_outcome = insert_outcome_with_citations(
        harness.pool(),
        tenant_id,
        admin_id,
        "k13-exact-reviewed-lesson",
        "satisfied",
        project.space.id,
        serde_json::json!([{
            "citation_id": format!("{lesson_id}:{lesson_revision}"),
            "source_revision": source_revision.to_string(),
        }]),
    )
    .await;
    let response = post_json(
        &app,
        &format!("/knowledge/spaces/{}/jobs", project.space.id),
        serde_json::json!({
            "role": "researcher",
            "output_vault_id": vault.id,
            "question": "Pin the reviewed Lesson revision",
            "outcome_ids": [cited_outcome],
            "lesson_revision": {
                "object_id": lesson_id,
                "expected_revision": lesson_revision,
            },
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let job = json(response).await;
    assert_eq!(
        job["input"]["lesson_revision_target"]["object_id"],
        lesson_id
    );
    assert_eq!(
        job["input"]["lesson_revision_target"]["expected_revision"],
        lesson_revision
    );
    assert_eq!(
        job["input"]["lesson_revision_target"]["source_ids"][0],
        source_id
    );
    let job_id: Uuid = job["id"].as_str().unwrap().parse().unwrap();
    let pinned_sources = jobs::sources(harness.pool(), actor, job_id).await.unwrap();
    assert_eq!(pinned_sources.len(), 1);
    assert_eq!(pinned_sources[0].source_id.to_string(), source_id);

    let uncited_outcome = insert_outcome_with_citations(
        harness.pool(),
        tenant_id,
        admin_id,
        "k13-wrong-reviewed-lesson",
        "satisfied",
        project.space.id,
        serde_json::json!([{
            "citation_id": format!("{}:{}", Uuid::new_v4(), lesson_revision),
            "source_revision": source_revision.to_string(),
        }]),
    )
    .await;
    let rejected = post_json(
        &app,
        &format!("/knowledge/spaces/{}/jobs", project.space.id),
        serde_json::json!({
            "role": "researcher",
            "output_vault_id": vault.id,
            "question": "Reject an Outcome without the exact Lesson citation",
            "outcome_ids": [uncited_outcome],
            "lesson_revision": {
                "object_id": lesson_id,
                "expected_revision": lesson_revision,
            },
        }),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json(rejected).await["error"]["code"],
        "knowledge_invalid_input"
    );

    harness.cleanup().await;
}

#[tokio::test]
async fn a22_learning_feedback_reuses_success_and_requires_review_for_contradictory_revision() {
    if !pg_available().await {
        eprintln!("skipping A22-T1 learning feedback fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let tenant_id = Uuid::new_v4();
    let admin_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'a22-learning-feedback')")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO users (id, tenant_id, email, display_name, role, password_hash)
           VALUES ($1,$2,$3,'A22 Admin','admin','test')"#,
    )
    .bind(admin_id)
    .bind(tenant_id)
    .bind(format!("a22-learning-feedback-{tenant_id}@example.test"))
    .execute(pool)
    .await
    .unwrap();
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        pool,
        actor,
        CreateProject {
            slug: "a22-learning-feedback".into(),
            title: "A22 learning feedback".into(),
            goal: "Prove bounded learning from verified outcomes and contradictions".into(),
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
            home_bundle_id: "server-administrator".into(),
            knowledge_schema_id: "server.knowledge".into(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();

    let vault_root = tempfile::tempdir().unwrap();
    let wiki_root = tempfile::tempdir().unwrap();
    let layout = Arc::new(TenantVaultLayout::new(vault_root.path()));
    let config = KnowledgeConfig::extract_from_toml_str(&format!(
        "[knowledge]\nwiki_path = {:?}\nvault_path = {:?}\n",
        wiki_root.path().join("wiki"),
        vault_root.path()
    ))
    .unwrap()
    .unwrap();
    let dispatcher: Arc<dyn GadgetDispatcher> = Arc::new(KnowledgeDispatcher {
        provider: KnowledgeGadgetProvider::new(config, Some(pool.clone())).unwrap(),
    });
    let app = app(
        state(pool.clone(), vault_root.path(), dispatcher),
        context(tenant_id, admin_id),
    );
    let uploaded_lesson = upload(
        &app,
        vault.id,
        "Cooling recovery runbook",
        "# Cooling recovery runbook\n\nAfter cooling recovery, verify health for five minutes before closing the incident. A five-minute window is insufficient when sensor telemetry is delayed.\n",
    )
    .await;
    let lesson_id = uploaded_lesson["object"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let lesson_source_id =
        Uuid::parse_str(uploaded_lesson["source"]["id"].as_str().unwrap()).unwrap();
    let current = note(&app, &lesson_id).await;
    let mut properties = current["properties"].clone();
    properties["knowledge_kind"] = serde_json::json!("lesson");
    properties["review_state"] = serde_json::json!("reviewed");
    properties["source_ids"] = serde_json::json!([lesson_source_id]);
    put_note(
        &app,
        &lesson_id,
        &current,
        properties,
        "# Cooling recovery lesson\n\nVerify health for five minutes before closing the incident.\n",
    )
    .await;
    let reviewed_lesson = note(&app, &lesson_id).await;
    let reviewed_revision = reviewed_lesson["revision"].as_i64().unwrap();

    let binding = IntelligenceActorBinding {
        tenant_id,
        actor_id: admin_id,
        authority_actor_id: None,
        acting_space_id: Some(project.space.id),
    };
    let consumer = BundleId::new("server-administrator").unwrap();
    let intelligence = IntelligenceContextService::new(pool.clone(), layout);
    let context_pack = intelligence
        .resolve(
            binding,
            &consumer,
            intelligence_query("a22-before-recovery", "cooling-edge-1"),
        )
        .await
        .unwrap();
    let lesson_citation = context_pack
        .citations
        .iter()
        .find(|citation| citation.citation_id == format!("{lesson_id}:{reviewed_revision}"))
        .cloned()
        .expect("the reviewed Lesson must be available to the acting Space");
    let receipt = intelligence
        .record_feedback(
            binding,
            &consumer,
            OutcomeFeedbackDraft::new(
                "a22-verified-cooling-recovery",
                context_pack.subject.clone(),
                "a22-repair-succeeded",
                Some(ContextUseRef::new(
                    context_pack.query_id.clone(),
                    context_pack.context_revision.clone(),
                )),
                serde_json::json!({"health":"degraded"}),
                serde_json::json!({"health":"healthy"}),
                OutcomePredicateResult::Satisfied,
                "Cooling recovery remained healthy through the verification window",
                vec![CitationUseRef::new(
                    lesson_citation.citation_id.clone(),
                    lesson_citation.source_revision.clone(),
                )],
            ),
        )
        .await
        .unwrap();
    let after_success = intelligence
        .resolve(
            binding,
            &consumer,
            intelligence_query("a22-after-recovery", "cooling-edge-1"),
        )
        .await
        .unwrap();
    let reused = after_success
        .citations
        .iter()
        .find(|citation| citation.source_id == "a22-verified-cooling-recovery")
        .expect("a verified success must become reusable experience");
    assert_eq!(reused.source_revision, receipt.experience_revision);
    assert!(reused
        .passage
        .contains("remained healthy through the verification window"));
    let satisfied_outcome_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM knowledge_outcome_feedback WHERE tenant_id = $1 AND feedback_id = $2",
    )
    .bind(tenant_id)
    .bind("a22-verified-cooling-recovery")
    .fetch_one(pool)
    .await
    .unwrap();
    let recorded_feedback: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_outcome_feedback WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        recorded_feedback, 1,
        "only the verified success was recorded"
    );

    // A legacy or externally imported failure cannot start a revision job. The
    // Server Administrator unit gate proves new failed operations never create
    // this row in the first place.
    let failed_outcome_id = insert_outcome(
        pool,
        tenant_id,
        admin_id,
        "a22-rejected-failure",
        "failed",
        project.space.id,
    )
    .await;
    let rejected = post_json(
        &app,
        &format!("/knowledge/spaces/{}/jobs", project.space.id),
        serde_json::json!({
            "role": "researcher",
            "output_vault_id": vault.id,
            "question": "Do not learn from the failed cooling repair",
            "outcome_ids": [failed_outcome_id],
            "lesson_revision": {
                "object_id": lesson_id,
                "expected_revision": reviewed_revision,
            },
        }),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json(rejected).await["error"]["code"],
        "knowledge_invalid_input"
    );
    let still_reviewed = note(&app, &lesson_id).await;
    assert_eq!(still_reviewed["revision"], reviewed_revision);
    assert_eq!(still_reviewed["properties"]["review_state"], "reviewed");

    let response = post_json(
        &app,
        &format!("/knowledge/spaces/{}/jobs", project.space.id),
        serde_json::json!({
            "role": "researcher",
            "output_vault_id": vault.id,
            "question": "Revise the cooling Lesson using the verified outcome and counterexample",
            "outcome_ids": [satisfied_outcome_id],
            "lesson_revision": {
                "object_id": lesson_id,
                "expected_revision": reviewed_revision,
            },
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let researcher_job = json(response).await;
    let researcher_job_id = Uuid::parse_str(researcher_job["id"].as_str().unwrap()).unwrap();
    let pinned_source_ids = jobs::sources(pool, actor, researcher_job_id)
        .await
        .unwrap()
        .into_iter()
        .map(|source| source.source_id)
        .collect::<Vec<_>>();
    assert!(pinned_source_ids.contains(&lesson_source_id));
    let worker_id = "a22-researcher";
    let leased = jobs::lease_next(pool, worker_id, 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(leased.id, researcher_job_id);
    let citations = serde_json::json!([{
        "source_id": lesson_source_id,
        "locator": "verification window",
        "claim": "The reviewed runbook requires a health verification window.",
        "stance": "supports"
    }, {
        "source_id": lesson_source_id,
        "locator": "delayed telemetry",
        "claim": "Delayed telemetry can make a fixed five-minute window insufficient.",
        "stance": "contradicts"
    }]);
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
            "score": 0.64,
            "reason": "The verified outcome and explicit counterexample need human review"
        })
    });
    let candidate = serde_json::json!({
        "schema_version": 1,
        "target_kind": "lesson",
        "claim": "Verify cooling recovery long enough to observe current telemetry",
        "claims": [{
            "id": "verified-window",
            "statement": "The verified recovery remained healthy through the observation window.",
            "source_ids": [lesson_source_id]
        }, {
            "id": "delayed-telemetry",
            "statement": "A fixed five-minute window can fail when telemetry arrives late.",
            "source_ids": [lesson_source_id]
        }],
        "supporting_claim_ids": ["verified-window"],
        "contradicting_claim_ids": ["delayed-telemetry"],
        "applicability": ["Cooling recovery with current health telemetry"],
        "limitations": ["Extend the window when sensor telemetry is delayed or incomplete"],
        "freshness": {
            "status": "current",
            "reason": "Pinned reviewed Lesson, counterexample, and verified Outcome"
        },
        "confidence": 0.64,
        "importance": importance,
        "verified_outcome_ids": [satisfied_outcome_id]
    });
    let artifact = jobs::append_artifact(
        pool,
        researcher_job_id,
        worker_id,
        ArtifactInput {
            kind: "candidate".into(),
            title: "Cooling recovery Lesson revision".into(),
            summary: "Narrow the verification guidance around delayed telemetry.".into(),
            payload: candidate,
            citations: citations.clone(),
        },
    )
    .await
    .unwrap();
    jobs::complete(pool, researcher_job_id, worker_id, 0, 2)
        .await
        .unwrap();

    let gardener = jobs::enqueue(
        pool,
        actor,
        EnqueueKnowledgeJob {
            space_id: project.space.id,
            output_vault_id: vault.id,
            role: KnowledgeJobRole::Gardener,
            kind: KnowledgeJobKind::OnDemand,
            priority: 0,
            input: serde_json::json!({
                "question": "Prepare the reviewed cooling Lesson revision",
                "candidate_artifact_id": artifact.id,
                "lesson_revision_target": researcher_job["input"]["lesson_revision_target"].clone(),
                "outcomes": researcher_job["input"]["outcomes"].clone(),
            }),
            idempotency_key: format!("a22-gardener:{}", artifact.id),
            source_ids: vec![lesson_source_id],
            runtime: RuntimeSnapshot {
                backend: "codex_exec".into(),
                model: "gpt-5.6-sol".into(),
                effort: "high".into(),
                endpoint_id: None,
                model_source: "default".into(),
                local_base_url: String::new(),
                local_api_key_env: String::new(),
                prompt_contract_revision: "gardener-v2".into(),
                tool_policy_revision: "knowledge-read-v1".into(),
                role_profile_source: None,
                role_profile_ref: None,
            },
            bundle_role: None,
            budget: JobBudget {
                max_tokens: 2_048,
                max_sources: 4,
                max_wall_seconds: 30,
                max_attempts: 2,
            },
            scheduled_at: None,
        },
    )
    .await
    .unwrap();
    let gardener_worker = "a22-gardener";
    let leased = jobs::lease_next(pool, gardener_worker, 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(leased.id, gardener.id);
    let change_set = jobs::append_change_set(
        pool,
        gardener.id,
        gardener_worker,
        ChangeSetInput {
            candidate_artifact_id: Some(artifact.id),
            title: "Review cooling recovery guidance".into(),
            summary: "A verified success supports the Lesson, while delayed telemetry narrows its scope."
                .into(),
            operations: serde_json::json!([{
                "op": "update_note",
                "object_id": lesson_id,
                "expected_revision": reviewed_revision,
                "title": "Cooling recovery lesson",
                "body": "Verify cooling recovery long enough to observe current telemetry. Extend the window when sensor telemetry is delayed or incomplete."
            }]),
            citations,
            expected_git_revision: None,
            materialization_key: format!("a22-learning-feedback:{lesson_id}:{reviewed_revision}"),
        },
    )
    .await
    .unwrap();
    jobs::complete(pool, gardener.id, gardener_worker, 0, 2)
        .await
        .unwrap();

    let before_review = note(&app, &lesson_id).await;
    assert_eq!(before_review["revision"], reviewed_revision);
    assert_eq!(before_review["body"], reviewed_lesson["body"]);
    let evolution = get(
        &app,
        &format!(
            "/api/v1/web/workbench/knowledge/spaces/{}/evolution",
            project.space.id
        ),
    )
    .await;
    assert_eq!(evolution.status(), StatusCode::OK);
    let evolution = json(evolution).await;
    assert_eq!(
        evolution["traces"][0]["candidate"]["payload"]["confidence"],
        0.64
    );
    assert_eq!(
        evolution["traces"][0]["candidate"]["payload"]["contradicting_claim_ids"][0],
        "delayed-telemetry"
    );
    assert_eq!(
        evolution["traces"][0]["change_set"]["status"],
        "pending_user_review"
    );

    let accepted = post_json(
        &app,
        &format!("/knowledge/change-sets/{}/accept", change_set.id),
        serde_json::json!({"expected_revision": change_set.revision}),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(json(accepted).await["status"], "applied");
    let promoted = note(&app, &lesson_id).await;
    assert!(promoted["revision"].as_i64().unwrap() > reviewed_revision);
    assert_eq!(promoted["properties"]["review_state"], "verified");
    assert!((promoted["properties"]["confidence"].as_f64().unwrap() - 0.64).abs() < 0.000_001);
    assert_eq!(
        promoted["properties"]["applicability"][0],
        "Cooling recovery with current health telemetry"
    );
    assert_eq!(
        promoted["properties"]["limitations"][0],
        "Extend the window when sensor telemetry is delayed or incomplete"
    );
    assert_eq!(
        promoted["properties"]["outcome_of"][0],
        satisfied_outcome_id.to_string()
    );
    let promoted_sources = promoted["properties"]["source_ids"].as_array().unwrap();
    assert!(promoted_sources.contains(&serde_json::json!(lesson_source_id)));
    assert!(promoted["body"]
        .as_str()
        .unwrap()
        .contains("sensor telemetry is delayed"));

    harness.cleanup().await;
}
