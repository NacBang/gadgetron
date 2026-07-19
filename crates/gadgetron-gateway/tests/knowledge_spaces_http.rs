use std::{collections::HashMap, sync::Arc};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Extension,
};
use gadgetron_core::context::{QuotaSnapshot, Scope, TenantContext};
use gadgetron_core::error::GadgetronError;
use gadgetron_gateway::{
    server::AppState,
    web::{
        catalog::DescriptorCatalog, knowledge_spaces, projection::InProcessWorkbenchProjection,
        workbench::GatewayWorkbenchService,
    },
};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    audit::writer::AuditWriter,
    auth::validator::{KeyValidator, ValidatedKey},
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

async fn pg_available() -> bool {
    let admin_url = std::env::var("DATABASE_URL")
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

fn state(pool: sqlx::PgPool, vault_root: &std::path::Path) -> AppState {
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
            actions: None,
            approval_store: None,
            policy_evaluator: None,
            gadget_catalog: None,
            descriptor_catalog: Some(catalog),
            catalog_path: None,
            bundles_dir: None,
            bundle_signing: Default::default(),
            runtime_manager: None,
            gadget_modes: None,
            gadget_mode_reconfigurer: None,
            agent_brain: None,
            agent_config_base: None,
            vault_layout: Some(Arc::new(
                gadgetron_knowledge::vault::TenantVaultLayout::new(vault_root),
            )),
        })),
        penny_shared_surface: None,
        penny_assembler: None,
        agent_config: Arc::new(gadgetron_core::agent::config::AgentConfig::default()),
        google_oauth: None,
        activity_capture_store: None,
        candidate_coordinator: None,
        activity_bus: gadgetron_core::activity_bus::ActivityBus::new(),
        tool_catalog: None,
        gadget_dispatcher: None,
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

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response bytes"),
    )
    .expect("response JSON")
}

#[tokio::test]
async fn r2_1_http_project_space_and_physical_vault_round_trip() {
    if !pg_available().await {
        eprintln!("skipping R2.1 gateway fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'http-tenant')")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1, $2, 'admin@http.test', 'Admin', 'admin', 'test')",
    )
    .bind(user_id)
    .bind(tenant_id)
    .execute(harness.pool())
    .await
    .unwrap();

    let vault_root = tempfile::tempdir().unwrap();
    let app = knowledge_spaces::routes()
        .with_state(state(harness.pool().clone(), vault_root.path()))
        .layer(Extension(context(tenant_id, user_id)));

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/knowledge/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"slug":"http-project","title":"HTTP Project","goal":"prove API"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let created = response_json(create).await;
    let space_id = Uuid::parse_str(created["space"]["id"].as_str().unwrap()).unwrap();

    let ensure = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/spaces/{space_id}/vaults"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"home_bundle_id":"server-administrator","knowledge_schema_id":"server.knowledge","schema_version":1}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ensure.status(), StatusCode::OK);
    let provisioned = response_json(ensure).await;
    assert_eq!(provisioned["vault"]["space_id"], space_id.to_string());
    let physical = std::path::PathBuf::from(provisioned["physical"]["root"].as_str().unwrap());
    assert!(physical.join("_domain.json").is_file());
    assert!(physical.join("notes").is_dir());
    let note_path = format!("notes/{}.md", Uuid::new_v4());
    std::fs::write(
        physical.join(&note_path),
        "# HTTP runbook\n\nVerify the service before declaring recovery.\n",
    )
    .unwrap();
    let vault_id = Uuid::parse_str(provisioned["vault"]["id"].as_str().unwrap()).unwrap();

    let register = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/vaults/{vault_id}/objects"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"canonical_kind":"note","path":note_path}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(register.status(), StatusCode::OK);
    let object = response_json(register).await;
    let object_id = Uuid::parse_str(object["id"].as_str().unwrap()).unwrap();

    let objects = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/knowledge/spaces/{space_id}/objects?home_bundle_id=server-administrator&canonical_kind=note"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(objects.status(), StatusCode::OK);
    let objects = response_json(objects).await;
    assert_eq!(objects["returned"], 1);
    assert_eq!(objects["objects"][0]["id"], object_id.to_string());
    assert_eq!(objects["objects"][0]["space_id"], space_id.to_string());
    assert_eq!(
        objects["objects"][0]["home_bundle_id"],
        "server-administrator"
    );

    let target = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/knowledge/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"slug":"share-target","title":"Share Target","goal":"prove sharing"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(target.status(), StatusCode::OK);
    let target = response_json(target).await;
    let target_space_id = target["space"]["id"].as_str().unwrap();

    let share = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/objects/{object_id}/shares"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"target_space_id":"{target_space_id}","source_revision":1,"mode":"reference","follow_latest":true}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(share.status(), StatusCode::OK);
    let share = response_json(share).await;
    let share_id = Uuid::parse_str(share["id"].as_str().unwrap()).unwrap();
    let graph_bridge: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
             SELECT 1 FROM knowledge_graph_edges e
             JOIN knowledge_graph_generations g
               ON g.tenant_id = e.tenant_id AND g.id = e.generation_id AND g.state = 'active'
             WHERE e.tenant_id = $1 AND e.from_node_id = $2 AND e.to_node_id = $3
               AND e.relation_kind = 'bridge_to')"#,
    )
    .bind(tenant_id)
    .bind(format!("share:{share_id}"))
    .bind(format!("note:{object_id}"))
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert!(graph_bridge);

    let viewer_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1, $2, 'viewer@http.test', 'Viewer', 'member', 'test')",
    )
    .bind(viewer_id)
    .bind(tenant_id)
    .execute(harness.pool())
    .await
    .unwrap();
    let grant = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/spaces/{space_id}/grants"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"principal_kind":"user","principal_id":"{viewer_id}","role":"viewer"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(grant.status(), StatusCode::OK);

    let viewer_app = knowledge_spaces::routes()
        .with_state(state(harness.pool().clone(), vault_root.path()))
        .layer(Extension(context(tenant_id, viewer_id)));
    let hidden_target_share = viewer_app
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/objects/{object_id}/shares"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden_target_share.status(), StatusCode::OK);
    assert_eq!(response_json(hidden_target_share).await["returned"], 0);

    let shares = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/objects/{object_id}/shares"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(shares.status(), StatusCode::OK);
    let shares = response_json(shares).await;
    assert_eq!(shares["returned"], 1);
    assert_eq!(shares["shares"][0]["id"], share_id.to_string());

    let revoke = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/knowledge/shares/{share_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"expected_revision":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoke.status(), StatusCode::OK);
    let graph_bridge: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
             SELECT 1 FROM knowledge_graph_edges e
             JOIN knowledge_graph_generations g
               ON g.tenant_id = e.tenant_id AND g.id = e.generation_id AND g.state = 'active'
             WHERE e.tenant_id = $1 AND e.from_node_id = $2 AND e.relation_kind = 'bridge_to')"#,
    )
    .bind(tenant_id)
    .bind(format!("share:{share_id}"))
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert!(!graph_bridge);

    let shares = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/objects/{object_id}/shares"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(shares.status(), StatusCode::OK);
    assert_eq!(response_json(shares).await["returned"], 0);

    let list = app
        .oneshot(
            Request::builder()
                .uri("/knowledge/spaces")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let listed = response_json(list).await;
    assert_eq!(listed["returned"], 2);
    let source_space = listed["spaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|space| space["id"] == space_id.to_string())
        .unwrap();
    assert_eq!(source_space["effective_role"], "manager");

    harness.cleanup().await;
}
