use std::{collections::HashMap, sync::Arc, time::Duration};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Extension,
};
use gadgetron_core::{
    context::{QuotaSnapshot, Scope, TenantContext},
    error::GadgetronError,
};
use gadgetron_gateway::{
    server::AppState,
    web::{
        catalog::DescriptorCatalog,
        knowledge_sources,
        projection::InProcessWorkbenchProjection,
        safe_fetch::{
            checked_content_length, checked_http_status, checked_redirect, checked_stream_size,
            fetch_public_https, SafeFetchPolicy,
        },
        workbench::GatewayWorkbenchService,
    },
};
use gadgetron_knowledge::source::MAX_SOURCE_BYTES;
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    audit::writer::AuditWriter,
    auth::validator::{KeyValidator, ValidatedKey},
    knowledge_spaces::{self as spaces, CreateProject, EnsureVault, SpaceActor},
    quota::enforcer::InMemoryQuotaEnforcer,
};
use reqwest::Url;
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

fn multipart_file(boundary: &str, filename: &str, content_type: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nNegative fixture\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
         Content-Type: {content_type}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

async fn upload(
    app: &axum::Router,
    vault_id: Uuid,
    boundary: &str,
    filename: &str,
    content_type: &str,
    bytes: &[u8],
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/vaults/{vault_id}/sources/upload"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_file(
                    boundary,
                    filename,
                    content_type,
                    bytes,
                )))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn fetch(app: &axum::Router, vault_id: Uuid, url: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/vaults/{vault_id}/sources/fetch"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"url": url, "title": "blocked fetch"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn safe_fetch_negative_contract_is_network_independent() {
    for address in ["127.0.0.1", "10.0.0.1"] {
        let error = fetch_public_https(
            &format!("https://{address}/metadata"),
            SafeFetchPolicy {
                max_bytes: 1024,
                max_redirects: 3,
                allowed_content_types: &["text/plain"],
                allowed_domains: None,
                timeout: Duration::from_secs(1),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "fetch_ssrf_blocked", "{address}");
    }

    let mut loop_url = Url::parse("https://example.com/loop").unwrap();
    for redirects in 0..3 {
        loop_url = checked_redirect(&loop_url, "/loop", redirects, 3).unwrap();
    }
    let redirect_error = checked_redirect(&loop_url, "/loop", 3, 3).unwrap_err();
    assert_eq!(redirect_error.code(), "fetch_redirect_blocked");

    let http_error = checked_http_status(StatusCode::NOT_FOUND).unwrap_err();
    assert_eq!(http_error.code(), "fetch_http_status");
    assert_eq!(http_error.http_status(), Some(404));

    let length_error = checked_content_length(Some(1025), 1024).unwrap_err();
    assert_eq!(length_error.code(), "fetch_too_large");
    let stream_error = checked_stream_size(768, 257, 1024).unwrap_err();
    assert_eq!(stream_error.code(), "fetch_too_large");
}

#[tokio::test]
async fn source_ingest_negative_http_contract() {
    if !pg_available().await {
        eprintln!("skipping Source ingest negative fixture: pgvector/PostgreSQL unavailable");
        return;
    }

    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let other_tenant_id = Uuid::new_v4();
    let other_user_id = Uuid::new_v4();
    for (tenant, user, suffix) in [
        (tenant_id, user_id, "owner"),
        (other_tenant_id, other_user_id, "other"),
    ] {
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
            .bind(tenant)
            .bind(format!("source-negative-{suffix}"))
            .execute(harness.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
             VALUES ($1, $2, $3, 'Admin', 'admin', 'test')",
        )
        .bind(user)
        .bind(tenant)
        .bind(format!("{suffix}@source-negative.test"))
        .execute(harness.pool())
        .await
        .unwrap();
    }

    let actor = SpaceActor { tenant_id, user_id };
    let project = spaces::create_project(
        harness.pool(),
        actor,
        CreateProject {
            slug: "source-negative".into(),
            title: "Source Negative".into(),
            goal: "K0.3 negative contract".into(),
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
    let state = state(harness.pool().clone(), vault_root.path());
    let owner_app = app(state.clone(), context(tenant_id, user_id));
    let foreign_app = app(state, context(other_tenant_id, other_user_id));
    let boundary = "gadgetron-k03-negative";

    for url in [
        "https://127.0.0.1/latest/meta-data",
        "https://10.0.0.1/latest/meta-data",
    ] {
        let response = fetch(&owner_app, vault.id, url).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "fetch_ssrf_blocked"
        );
    }

    let oversized = vec![b'x'; MAX_SOURCE_BYTES + 1];
    let response = upload(
        &owner_app,
        vault.id,
        boundary,
        "oversized.txt",
        "text/plain",
        &oversized,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "knowledge_invalid_input"
    );

    let response = upload(
        &owner_app,
        vault.id,
        boundary,
        "disguised.txt",
        "application/x-executable",
        b"plain text",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "knowledge_invalid_input"
    );

    let response = upload(
        &owner_app,
        vault.id,
        boundary,
        "empty.txt",
        "text/plain",
        b"",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "knowledge_invalid_input"
    );

    let spoofed = upload(
        &owner_app,
        vault.id,
        boundary,
        "runbook.md",
        "application/pdf",
        b"# this is not a PDF",
    )
    .await;
    assert_eq!(spoofed.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response_json(spoofed).await["error"]["code"], "invalid_pdf");

    let truncated = upload(
        &owner_app,
        vault.id,
        boundary,
        "truncated.pdf",
        "application/pdf",
        b"%PDF",
    )
    .await;
    assert_eq!(truncated.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let truncated = response_json(truncated).await;
    assert_eq!(truncated["error"]["code"], "invalid_pdf");
    let source_id = Uuid::parse_str(truncated["error"]["source_id"].as_str().unwrap()).unwrap();

    let hidden = foreign_app
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/sources/{source_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    let detail = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/sources/{source_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = response_json(detail).await;
    let revision = detail["source"]["revision"].as_i64().unwrap();
    let attempt_count = detail["source"]["attempt_count"].as_i64().unwrap();

    let stale_retry = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/sources/{source_id}/retry"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"expected_revision": revision - 1}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_retry.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(stale_retry).await["error"]["code"],
        "knowledge_revision_conflict"
    );

    let unchanged = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/sources/{source_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let unchanged = response_json(unchanged).await;
    assert_eq!(unchanged["source"]["revision"], revision);
    assert_eq!(unchanged["source"]["attempt_count"], attempt_count);

    let retry = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/sources/{source_id}/retry"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"expected_revision": revision}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response_json(retry).await["error"]["code"], "invalid_pdf");

    let retried = owner_app
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/sources/{source_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let retried = response_json(retried).await;
    assert_eq!(retried["source"]["attempt_count"], attempt_count + 1);
    assert!(retried["source"]["revision"].as_i64().unwrap() > revision);

    harness.cleanup().await;
}
