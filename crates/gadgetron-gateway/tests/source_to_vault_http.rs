use std::{collections::HashMap, sync::Arc};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Extension,
};
use base64::Engine as _;
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

fn multipart_markdown(boundary: &str, title: &str, markdown: &str) -> Vec<u8> {
    multipart_file(
        boundary,
        title,
        "runbook.md",
        "text/markdown",
        markdown.as_bytes(),
    )
}

fn multipart_file(
    boundary: &str,
    title: &str,
    filename: &str,
    content_type: &str,
    bytes: &[u8],
) -> Vec<u8> {
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\n{title}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
         Content-Type: {content_type}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[tokio::test]
async fn r2_2_upload_dedup_note_revision_external_reconcile_and_tenant_negative() {
    if !pg_available().await {
        eprintln!("skipping R2.2 gateway fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let other_tenant = Uuid::new_v4();
    let other_user = Uuid::new_v4();
    for (tenant, user, suffix) in [
        (tenant_id, user_id, "owner"),
        (other_tenant, other_user, "other"),
    ] {
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
            .bind(tenant)
            .bind(format!("source-{suffix}"))
            .execute(harness.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
             VALUES ($1, $2, $3, 'Admin', 'admin', 'test')",
        )
        .bind(user)
        .bind(tenant)
        .bind(format!("{suffix}@source.test"))
        .execute(harness.pool())
        .await
        .unwrap();
    }
    let actor = SpaceActor { tenant_id, user_id };
    let project = spaces::create_project(
        harness.pool(),
        actor,
        CreateProject {
            slug: "source-project".into(),
            title: "Source Project".into(),
            goal: "R2.2".into(),
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
    let domain_root = vault_root
        .path()
        .join("tenants")
        .join(tenant_id.to_string())
        .join("vault/spaces")
        .join(project.space.id.to_string())
        .join("domains/server-administrator");
    let boundary = "gadgetron-r2-2-boundary";
    let markdown = "# GPU reset runbook\n\nUse a bounded drain before reset.\n";

    let create_note = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/vaults/{}/notes", vault.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"title":"Manual cooling note"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_note.status(), StatusCode::OK);
    let created_note = response_json(create_note).await;
    assert_eq!(created_note["revision"], 1);
    assert_eq!(created_note["properties"]["title"], "Manual cooling note");
    assert_eq!(
        created_note["properties"]["space_id"],
        project.space.id.to_string()
    );
    assert_eq!(
        created_note["properties"]["home_bundle_id"],
        "server-administrator"
    );
    assert!(created_note["body"]
        .as_str()
        .unwrap()
        .contains("Manual cooling note"));

    let listed_notes = spaces::list_objects(
        harness.pool(),
        actor,
        project.space.id,
        Some("server-administrator"),
        Some("note"),
    )
    .await
    .unwrap();
    assert_eq!(listed_notes.len(), 1);
    assert_eq!(
        listed_notes[0].title.as_deref(),
        Some("Manual cooling note")
    );
    assert!(listed_notes[0]
        .path
        .starts_with("notes/manual-cooling-note--"));
    let manual_id = Uuid::parse_str(created_note["object_id"].as_str().unwrap()).unwrap();
    let legacy = format!(
        "---\nid = \"{manual_id}\"\nspace_id = \"{}\"\nhome_bundle_id = \"server-administrator\"\nlegacy_owner = \"operator\"\n---\n# Legacy note\n",
        project.space.id
    );
    std::fs::write(domain_root.join(&listed_notes[0].path), legacy).unwrap();
    let legacy_note = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/objects/{manual_id}/note"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(legacy_note.status(), StatusCode::OK);
    let legacy_note = response_json(legacy_note).await;
    assert_eq!(legacy_note["frontmatter_format"], "legacy_toml");
    assert_eq!(legacy_note["properties"]["legacy_owner"], "operator");

    let upload = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/vaults/{}/sources/upload", vault.id))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_markdown(
                    boundary,
                    "GPU reset runbook",
                    markdown,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        upload.status(),
        StatusCode::OK,
        "{}",
        response_json(upload).await
    );
    let upload = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/vaults/{}/sources/upload", vault.id))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_markdown(
                    boundary,
                    "GPU reset runbook copy",
                    markdown,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload.status(), StatusCode::OK);
    let second = response_json(upload).await;
    assert_eq!(second["blob_existed"], true);

    let listed = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/spaces/{}/sources", project.space.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert_eq!(listed["returned"], 2);
    assert_eq!(listed["sources"].as_array().unwrap().len(), 2);

    let sources: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, blob_id FROM knowledge_sources WHERE tenant_id = $1 ORDER BY created_at",
    )
    .bind(tenant_id)
    .fetch_all(harness.pool())
    .await
    .unwrap();
    assert_eq!(sources.len(), 2);
    assert_ne!(sources[0].0, sources[1].0);
    assert_eq!(sources[0].1, sources[1].1);
    let blob_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_blobs WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(blob_count, 1);

    let second_source_id = Uuid::parse_str(second["source"]["id"].as_str().unwrap()).unwrap();
    let source_id = sources
        .iter()
        .map(|(id, _)| *id)
        .find(|id| *id != second_source_id)
        .unwrap();
    let object_id: Uuid =
        sqlx::query_scalar("SELECT extracted_object_id FROM knowledge_sources WHERE id = $1")
            .bind(source_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    let note_get = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/objects/{object_id}/note"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let note_status = note_get.status();
    let note = response_json(note_get).await;
    assert_eq!(note_status, StatusCode::OK, "{note}");
    assert_eq!(note["frontmatter_format"], "yaml");
    assert_eq!(note["properties"]["id"], object_id.to_string());
    assert_eq!(note["revision"], 1);
    let object_path: String =
        sqlx::query_scalar("SELECT path FROM knowledge_objects WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(object_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert!(object_path.starts_with("notes/gpu-reset-runbook--"));

    let stale = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/knowledge/objects/{object_id}/note"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "expected_revision": 9,
                        "expected_git_revision": note["git_revision"],
                        "properties": note["properties"],
                        "body": "stale"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let edit = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/knowledge/objects/{object_id}/note"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "expected_revision": 1,
                        "expected_git_revision": note["git_revision"],
                        "properties": note["properties"],
                        "body": "# GPU reset runbook\n\nManager-reviewed edit.\n"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit.status(), StatusCode::OK);
    let edited = response_json(edit).await;
    assert_eq!(edited["revision"], 2);

    let note_path = domain_root.join(object_path);
    let external = std::fs::read_to_string(&note_path)
        .unwrap()
        .replacen("---\n", "---\noperator_note: preserved\n", 1)
        .replace("Manager-reviewed edit.", "Externally edited in Obsidian.");
    std::fs::write(&note_path, external).unwrap();
    let overwritten = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/knowledge/objects/{object_id}/note"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "expected_revision": edited["revision"],
                        "expected_git_revision": edited["git_revision"],
                        "properties": edited["properties"],
                        "body": "# GPU reset runbook\n\nThis must not overwrite the external edit.\n"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(overwritten.status(), StatusCode::CONFLICT);
    let reconciled = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/objects/{object_id}/note"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reconciled.status(), StatusCode::OK);
    let reconciled = response_json(reconciled).await;
    assert_eq!(reconciled["revision"], 3);
    assert_eq!(reconciled["external_edit_reconciled"], false);
    assert_eq!(reconciled["properties"]["operator_note"], "preserved");
    assert!(reconciled["body"]
        .as_str()
        .unwrap()
        .contains("Externally edited in Obsidian"));
    let merged = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/knowledge/objects/{object_id}/note"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "expected_revision": reconciled["revision"],
                        "expected_git_revision": reconciled["git_revision"],
                        "properties": reconciled["properties"],
                        "body": "# GPU reset runbook\n\nMerged after reloading the Obsidian edit.\n"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(merged.status(), StatusCode::OK);
    let merged = response_json(merged).await;
    assert_eq!(merged["revision"], 4);
    assert_eq!(merged["properties"]["operator_note"], "preserved");
    assert!(std::fs::read_to_string(&note_path)
        .unwrap()
        .contains("operator_note: preserved"));

    let second_object_id = second["object"]["id"].as_str().unwrap();
    let deleted_second_note = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/knowledge/objects/{second_object_id}/note"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "expected_revision": second["object"]["revision"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_second_note.status(), StatusCode::OK);
    let second_detail = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/sources/{second_source_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_detail.status(), StatusCode::OK);
    let second_detail = response_json(second_detail).await;
    assert_eq!(second_detail["source"]["status"], "extracted");
    assert!(second_detail["extraction"].is_null());
    assert_eq!(second_detail["attempts"].as_array().unwrap().len(), 2);

    let blob = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/sources/{source_id}/blob"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blob.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(blob.into_body(), 1024 * 1024).await.unwrap(),
        markdown
    );

    let foreign_app = app(state.clone(), context(other_tenant, other_user));
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

    let blocked = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/vaults/{}/sources/fetch", vault.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://127.0.0.1/latest/meta-data","title":"blocked"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let blocked = response_json(blocked).await;
    assert_eq!(blocked["error"]["code"], "fetch_ssrf_blocked");

    let html = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/knowledge/vaults/{}/sources/upload",
                    vault.id
                ))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_file(
                    boundary,
                    "HTML article",
                    "article.html",
                    "text/html",
                    b"<html><body><h1>Cooling study</h1><script>ignore()</script><p>Validated airflow.</p></body></html>",
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(html.status(), StatusCode::OK);
    let html = response_json(html).await;
    let html_object = html["object"]["id"].as_str().unwrap();
    let html_note = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/objects/{html_object}/note"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html_note = response_json(html_note).await;
    assert!(html_note["body"]
        .as_str()
        .unwrap()
        .contains("Validated airflow"));
    assert!(!html_note["body"].as_str().unwrap().contains("<script>"));

    let pdf = base64::engine::general_purpose::STANDARD
        .decode("JVBERi0xLjQKMSAwIG9iago8PCAvVHlwZSAvQ2F0YWxvZyAvUGFnZXMgMiAwIFIgPj4KZW5kb2JqCjIgMCBvYmoKPDwgL1R5cGUgL1BhZ2VzIC9LaWRzIFszIDAgUiA2IDAgUl0gL0NvdW50IDIgPj4KZW5kb2JqCjMgMCBvYmoKPDwgL1R5cGUgL1BhZ2UgL1BhcmVudCAyIDAgUiAvTWVkaWFCb3ggWzAgMCA2MTIgNzkyXSAvUmVzb3VyY2VzIDw8IC9Gb250IDw8IC9GMSA0IDAgUiA+PiA+PiAvQ29udGVudHMgNSAwIFIgPj4KZW5kb2JqCjQgMCBvYmoKPDwgL1R5cGUgL0ZvbnQgL1N1YnR5cGUgL1R5cGUxIC9CYXNlRm9udCAvSGVsdmV0aWNhID4+CmVuZG9iago1IDAgb2JqCjw8IC9MZW5ndGggNDUgPj4Kc3RyZWFtCkJUCi9GMSAyNCBUZgo3MiA3MjAgVGQKKFBhZ2UgT25lIFRleHQpIFRqCkVUCmVuZHN0cmVhbQplbmRvYmoKNiAwIG9iago8PCAvVHlwZSAvUGFnZSAvUGFyZW50IDIgMCBSIC9NZWRpYUJveCBbMCAwIDYxMiA3OTJdIC9SZXNvdXJjZXMgPDwgL0ZvbnQgPDwgL0YxIDQgMCBSID4+ID4+IC9Db250ZW50cyA3IDAgUiA+PgplbmRvYmoKNyAwIG9iago8PCAvTGVuZ3RoIDQ4ID4+CnN0cmVhbQpCVAovRjEgMjQgVGYKNzIgNzIwIFRkCihQYWdlIFR3byBDb250ZW50KSBUagpFVAplbmRzdHJlYW0KZW5kb2JqCnhyZWYKMCA4CjAwMDAwMDAwMDAgNjU1MzUgZiAKMDAwMDAwMDAwOSAwMDAwMCBuIAowMDAwMDAwMDU4IDAwMDAwIG4gCjAwMDAwMDAxMjEgMDAwMDAgbiAKMDAwMDAwMDI0NyAwMDAwMCBuIAowMDAwMDAwMzE3IDAwMDAwIG4gCjAwMDAwMDA0MTEgMDAwMDAgbiAKMDAwMDAwMDUzNyAwMDAwMCBuIAp0cmFpbGVyCjw8IC9TaXplIDggL1Jvb3QgMSAwIFIgPj4Kc3RhcnR4cmVmCjYzNAolJUVPRgo=")
        .unwrap();
    let pdf_upload = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/vaults/{}/sources/upload", vault.id))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_file(
                    boundary,
                    "Two page report",
                    "two-page.pdf",
                    "application/pdf",
                    &pdf,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pdf_upload.status(), StatusCode::OK);
    let pdf_upload = response_json(pdf_upload).await;
    let pdf_source = pdf_upload["source"]["id"].as_str().unwrap();
    let pdf_object = pdf_upload["object"]["id"].as_str().unwrap();
    let pdf_note = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/objects/{pdf_object}/note"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pdf_note.status(), StatusCode::OK);
    let pdf_note = response_json(pdf_note).await;
    assert_eq!(pdf_note["properties"]["extraction"]["page_count"], 2);
    assert_eq!(pdf_note["properties"]["extraction"]["pages"][0]["page"], 2);
    assert!(
        pdf_note["properties"]["extraction"]["pages"][0]["byte_offset"]
            .as_u64()
            .is_some()
    );
    let pdf_detail = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/sources/{pdf_source}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pdf_detail.status(), StatusCode::OK);
    let pdf_detail = response_json(pdf_detail).await;
    assert_eq!(
        pdf_detail["extraction"],
        pdf_note["properties"]["extraction"]
    );

    let malformed = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/vaults/{}/sources/upload", vault.id))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_file(
                    boundary,
                    "Malformed PDF",
                    "broken.pdf",
                    "application/pdf",
                    b"%PDF-not-a-document",
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let malformed = response_json(malformed).await;
    let malformed_source = malformed["error"]["source_id"].as_str().unwrap();
    let detail = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/sources/{malformed_source}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let detail = response_json(detail).await;
    assert_eq!(detail["source"]["status"], "failed");
    assert_eq!(detail["source"]["attempt_count"], 1);
    assert_eq!(detail["attempts"].as_array().unwrap().len(), 2);
    let retry = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/knowledge/sources/{malformed_source}/retry"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "expected_revision": detail["source"]["revision"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let detail = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/sources/{malformed_source}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let detail = response_json(detail).await;
    assert_eq!(detail["source"]["attempt_count"], 2);
    assert_eq!(detail["attempts"].as_array().unwrap().len(), 4);

    let deleted_source = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/knowledge/sources/{source_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"expected_revision":3}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_source.status(), StatusCode::OK);
    let note_survives_source_delete = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/objects/{object_id}/note"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(note_survives_source_delete.status(), StatusCode::OK);
    let note_survives_source_delete = response_json(note_survives_source_delete).await;

    let deleted_note = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/knowledge/objects/{object_id}/note"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "expected_revision": note_survives_source_delete["revision"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_note.status(), StatusCode::OK);
    assert!(note_path
        .parent()
        .unwrap()
        .join("_archived")
        .join(note_path.file_name().unwrap())
        .is_file());
    let hidden_note = owner_app
        .oneshot(
            Request::builder()
                .uri(format!("/knowledge/objects/{object_id}/note"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden_note.status(), StatusCode::NOT_FOUND);

    harness.cleanup().await;
}
