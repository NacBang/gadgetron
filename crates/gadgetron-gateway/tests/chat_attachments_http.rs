use std::{collections::HashMap, sync::Arc};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Extension,
};
use gadgetron_core::agent::tools::{GadgetDispatchContext, GadgetProvider};
use gadgetron_core::context::{QuotaSnapshot, Scope, TenantContext};
use gadgetron_core::error::GadgetronError;
use gadgetron_gateway::{
    server::AppState,
    web::{
        catalog::DescriptorCatalog, knowledge_sources, projection::InProcessWorkbenchProjection,
        workbench::GatewayWorkbenchService,
    },
};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    audit::writer::AuditWriter,
    auth::validator::{KeyValidator, ValidatedKey},
    knowledge_sources::{self as sources, CreateSource},
    knowledge_spaces::{self as spaces, CreateProject, EnsureVault, SpaceActor},
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

fn app(state: AppState, context: TenantContext) -> axum::Router {
    knowledge_sources::routes()
        .merge(knowledge_sources::upload_routes())
        .with_state(state)
        .layer(Extension(context))
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("response bytes"),
    )
    .expect("response JSON")
}

fn multipart_file(boundary: &str, markdown: &str, conversation_id: Option<Uuid>) -> Vec<u8> {
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nBounded runbook\r\n"
    );
    if let Some(conversation_id) = conversation_id {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"conversation_id\"\r\n\r\n{conversation_id}\r\n"
        ));
    }
    body.push_str(&format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"bounded.md\"\r\nContent-Type: text/markdown\r\n\r\n{markdown}\r\n--{boundary}--\r\n"
    ));
    body.into_bytes()
}

async fn insert_actor(pool: &sqlx::PgPool, tenant_id: Uuid, user_id: Uuid, suffix: &str) {
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(tenant_id)
        .bind(format!("chat-attachment-{suffix}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1, $2, $3, 'Admin', 'admin', 'test')",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(format!("{suffix}@chat-attachment.test"))
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn chat_attachment_retention_acl_promotion_and_purge_contract() {
    if !pg_available().await {
        eprintln!("skipping chat attachment fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let other_tenant = Uuid::new_v4();
    let other_user = Uuid::new_v4();
    insert_actor(harness.pool(), tenant_id, user_id, "owner").await;
    insert_actor(harness.pool(), other_tenant, other_user, "foreign").await;

    let actor = SpaceActor { tenant_id, user_id };
    let project = spaces::create_project(
        harness.pool(),
        actor,
        CreateProject {
            slug: "explicit-target".into(),
            title: "Explicit target".into(),
            goal: "No implicit promotion".into(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let project_vault = spaces::ensure_vault(
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
    let state = state(harness.pool().clone(), vault_root.path());
    let owner_app = app(state.clone(), context(tenant_id, user_id));
    let conversation_id = Uuid::new_v4();
    let boundary = "gadgetron-chat-attachment";
    let markdown = "# Bounded runbook\n\nDrain before restart.\n";

    let chat_upload = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/knowledge/conversations/{conversation_id}/attachments/upload"
                ))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_file(boundary, markdown, None)))
                .unwrap(),
        )
        .await
        .unwrap();
    let chat_status = chat_upload.status();
    let chat = response_json(chat_upload).await;
    assert_eq!(chat_status, StatusCode::OK, "{chat}");
    assert_eq!(chat["source"]["source_kind"], "chat_attachment");
    assert_eq!(
        chat["source"]["conversation_id"],
        conversation_id.to_string()
    );
    assert!(chat["source"]["requested_uri"].is_null());
    assert_eq!(chat["source"]["status"], "extracted");
    let chat_source_id = Uuid::parse_str(chat["source"]["id"].as_str().unwrap()).unwrap();
    let chat_revision = chat["source"]["revision"].as_i64().unwrap();
    let chat_blob_id = Uuid::parse_str(chat["source"]["blob_id"].as_str().unwrap()).unwrap();
    let chat_object_id = Uuid::parse_str(chat["object"]["id"].as_str().unwrap()).unwrap();
    let chat_object_revision = chat["object"]["revision"].as_i64().unwrap();
    let chat_locator = chat["object"]["path"].as_str().unwrap().to_string();

    let knowledge_config =
        gadgetron_knowledge::config::KnowledgeConfig::extract_from_toml_str(&format!(
            "[knowledge]\nwiki_path = {:?}\nvault_path = {:?}\nwiki_autocommit = true\n",
            vault_root.path().join("wiki"),
            vault_root.path(),
        ))
        .unwrap()
        .unwrap();
    let knowledge = gadgetron_knowledge::KnowledgeGadgetProvider::new(
        knowledge_config,
        Some(harness.pool().clone()),
    )
    .unwrap();
    let read_args = serde_json::json!({
        "conversation_id": conversation_id,
        "source_id": chat_source_id,
        "source_revision": chat_revision,
        "object_id": chat_object_id,
        "object_revision": chat_object_revision,
        "locator": chat_locator,
    });
    let read = knowledge
        .call_with_context(
            &GadgetDispatchContext::new(
                tenant_id.to_string(),
                user_id.to_string(),
                "chat-attachment-read",
            ),
            "source.get",
            read_args.clone(),
        )
        .await
        .unwrap();
    assert_eq!(read.content["locator"], chat_locator);
    assert_eq!(read.content["source_metadata"]["format"], "text/markdown");
    assert!(
        read.content["content"]
            .as_str()
            .unwrap()
            .contains("Drain before restart."),
        "{}",
        read.content
    );
    assert!(knowledge
        .call_with_context(
            &GadgetDispatchContext::new(
                other_tenant.to_string(),
                other_user.to_string(),
                "foreign-chat-attachment-read",
            ),
            "source.get",
            read_args,
        )
        .await
        .is_err());

    let personal_kind: String = sqlx::query_scalar(
        r#"SELECT s.kind FROM knowledge_sources src
           JOIN knowledge_vaults v ON v.id = src.vault_id AND v.tenant_id = src.tenant_id
           JOIN knowledge_spaces s ON s.id = v.space_id AND s.tenant_id = v.tenant_id
           WHERE src.id = $1"#,
    )
    .bind(chat_source_id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(personal_kind, "personal");
    let implicit_project_sources: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM knowledge_sources src
           JOIN knowledge_vaults v ON v.id = src.vault_id AND v.tenant_id = src.tenant_id
           WHERE src.tenant_id = $1 AND v.space_id = $2"#,
    )
    .bind(tenant_id)
    .bind(project.space.id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(
        implicit_project_sources, 0,
        "chat-only must not auto-promote"
    );

    let explicit_upload = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/knowledge/vaults/{}/sources/upload",
                    project_vault.id
                ))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_file(
                    boundary,
                    markdown,
                    Some(conversation_id),
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let explicit_status = explicit_upload.status();
    let explicit = response_json(explicit_upload).await;
    assert_eq!(explicit_status, StatusCode::OK, "{explicit}");
    assert_eq!(explicit["source"]["source_kind"], "upload");
    assert_eq!(explicit["source"]["vault_id"], project_vault.id.to_string());
    assert_eq!(
        explicit["source"]["conversation_id"],
        conversation_id.to_string()
    );
    assert_eq!(explicit["blob_existed"], true);

    let pending = sources::create_pending_source(
        harness.pool(),
        actor,
        CreateSource {
            vault_id: project_vault.id,
            conversation_id: Some(conversation_id),
            source_kind: "upload".into(),
            title: "Not ready".into(),
            original_name: "not-ready.md".into(),
            requested_uri: None,
        },
    )
    .await
    .unwrap();
    let ready = sources::list_ready_conversation_sources(harness.pool(), actor, conversation_id)
        .await
        .unwrap();
    assert!(ready.iter().all(|(source, _)| source.status == "extracted"));
    assert!(ready.iter().all(|(source, _)| source.id != pending.id));
    sources::delete_source(harness.pool(), actor, pending.id, pending.revision)
        .await
        .unwrap();

    let promote = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/knowledge/conversations/{conversation_id}/attachments/{chat_source_id}/promote"
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "vault_id": project_vault.id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let promote_status = promote.status();
    let promoted = response_json(promote).await;
    assert_eq!(promote_status, StatusCode::OK, "{promoted}");
    assert_eq!(promoted["source"]["source_kind"], "upload");
    assert_eq!(promoted["source"]["blob_id"], chat_blob_id.to_string());
    assert_eq!(promoted["blob_existed"], true);

    let foreign_app = app(state.clone(), context(other_tenant, other_user));
    let hidden_list = foreign_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/knowledge/conversations/{conversation_id}/attachments"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden_list.status(), StatusCode::OK);
    assert_eq!(response_json(hidden_list).await["returned"], 0);
    let hidden_source = foreign_app
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/sources/{chat_source_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden_source.status(), StatusCode::NOT_FOUND);

    let purge = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/knowledge/conversations/{conversation_id}/attachments/{chat_source_id}"
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "expected_revision": chat_revision }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(purge.status(), StatusCode::OK);
    let purged: (String, Option<Uuid>, Option<String>) =
        sqlx::query_as("SELECT status, blob_id, content_hash FROM knowledge_sources WHERE id = $1")
            .bind(chat_source_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(purged, ("deleted".to_string(), None, None));
    let blob_deleted: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM knowledge_blobs WHERE id = $1")
            .bind(chat_blob_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert!(
        blob_deleted.is_none(),
        "versioned references retain the deduped blob"
    );

    let second_chat = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/knowledge/conversations/{conversation_id}/attachments/upload"
                ))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_file(
                    boundary,
                    "# Ephemeral attachment\n\nPurge these bytes.\n",
                    None,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_chat.status(), StatusCode::OK);
    let second_chat = response_json(second_chat).await;
    let second_blob_id =
        Uuid::parse_str(second_chat["source"]["blob_id"].as_str().unwrap()).unwrap();
    let purge_conversation = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/knowledge/conversations/{conversation_id}/attachments"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(purge_conversation.status(), StatusCode::OK);
    assert_eq!(response_json(purge_conversation).await["purged"], 1);
    let second_blob_deleted: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM knowledge_blobs WHERE id = $1")
            .bind(second_blob_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert!(
        second_blob_deleted.is_some(),
        "unreferenced chat-only bytes are purged"
    );

    let remaining = owner_app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/knowledge/conversations/{conversation_id}/attachments"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let remaining = response_json(remaining).await;
    assert_eq!(remaining["returned"], 2);
    assert!(remaining["sources"]
        .as_array()
        .unwrap()
        .iter()
        .all(|source| source["source_kind"] == "upload"));
}
