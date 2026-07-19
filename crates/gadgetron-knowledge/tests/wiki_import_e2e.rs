//! `wiki.import` compatibility Gadget -> Source Ledger integration evidence.
//!
//! The Gadget keeps its base64 Markdown/plain signature, but a valid import
//! now requires an authenticated tenant user plus PostgreSQL. Original bytes,
//! attempts, and the extracted note are written to that user's Personal Space
//! `core` Vault. The legacy `imports/*.md` wiki remains readable only as a
//! compatibility surface for pre-existing pages.

use std::sync::Arc;

use base64::Engine as _;
use gadgetron_core::agent::tools::{GadgetDispatchContext, GadgetError, GadgetProvider};
use gadgetron_knowledge::config::{KnowledgeConfig, WikiConfig};
use gadgetron_knowledge::gadget::KnowledgeGadgetProvider;
use gadgetron_knowledge::vault::TenantVaultLayout;
use gadgetron_knowledge::wiki::Wiki;
use gadgetron_knowledge::WikiKeywordIndex;
use gadgetron_knowledge::{KnowledgeService, KnowledgeServiceBuilder, LlmWikiStore};
use gadgetron_testing::harness::pg::PgHarness;
use tempfile::TempDir;
use uuid::Uuid;

async fn pg_available() -> bool {
    if std::env::var("GADGETRON_SKIP_POSTGRES_TESTS").is_ok() {
        return false;
    }
    let admin_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .is_ok()
}

fn build_wiki(dir: &TempDir) -> Arc<Wiki> {
    let cfg = WikiConfig {
        root: dir.path().join("wiki"),
        autocommit: true,
        git_author_name: "Wiki Import Test".into(),
        git_author_email: "wiki-import@test.local".into(),
        max_page_bytes: 1024 * 1024,
    };
    Arc::new(Wiki::open(cfg).expect("wiki open"))
}

fn build_service(wiki: Arc<Wiki>) -> Arc<KnowledgeService> {
    let store = Arc::new(LlmWikiStore::new(wiki).expect("llm-wiki store"));
    let keyword = Arc::new(WikiKeywordIndex::new().expect("keyword index"));
    KnowledgeServiceBuilder::new()
        .canonical_store(store)
        .add_index(keyword)
        .build()
        .expect("service build")
}

fn encoded(body: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(body.as_bytes())
}

fn import_args(body: &str, path: &str, overwrite: bool) -> serde_json::Value {
    serde_json::json!({
        "bytes": encoded(body),
        "content_type": "text/markdown",
        "target_path": path,
        "overwrite": overwrite,
        "source_uri": "https://example.invalid/operator-note",
    })
}

async fn insert_tenant_user(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'wiki-import-tenant')")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1, $2, $3, 'Import User', 'admin', 'test')",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(format!("wiki-import-{user_id}@test.invalid"))
    .execute(pool)
    .await
    .unwrap();
    (tenant_id, user_id)
}

fn knowledge_config(dir: &TempDir) -> KnowledgeConfig {
    let raw = format!(
        r#"
[knowledge]
wiki_path = "{}"
vault_path = "{}"
wiki_autocommit = true
"#,
        dir.path().join("wiki").display(),
        dir.path().join("vaults").display(),
    );
    KnowledgeConfig::extract_from_toml_str(&raw)
        .expect("valid knowledge TOML")
        .expect("knowledge section")
}

fn receipt_uuid(result: &gadgetron_core::agent::tools::GadgetResult, field: &str) -> Uuid {
    Uuid::parse_str(result.content[field].as_str().expect("receipt UUID string"))
        .expect("valid receipt UUID")
}

#[tokio::test]
async fn wiki_import_uses_personal_source_ledger_with_dedup_and_explicit_overwrite() {
    if !pg_available().await {
        eprintln!("skipping wiki_import Source Ledger fixture: PostgreSQL unavailable");
        return;
    }

    let harness = PgHarness::new().await;
    let (tenant_id, user_id) = insert_tenant_user(harness.pool()).await;
    let dir = tempfile::tempdir().unwrap();
    let legacy_path = dir.path().join("wiki/imports/legacy.md");
    std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
    let legacy = "---\nsource = \"imported\"\n---\n# Legacy\n\nKeep me readable.\n";
    std::fs::write(&legacy_path, legacy).unwrap();

    let config = knowledge_config(&dir);
    let vault_layout = TenantVaultLayout::new(config.effective_vault_path());
    let provider = KnowledgeGadgetProvider::new(config, Some(harness.pool().clone())).unwrap();
    let context = GadgetDispatchContext::new(
        tenant_id.to_string(),
        user_id.to_string(),
        Uuid::new_v4().to_string(),
    );
    let first_body = "# Operator Note\n\nFirst imported revision.";

    let unpinned_service = GadgetDispatchContext::new(
        tenant_id.to_string(),
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
    );
    let service_error = provider
        .call_with_context(
            &unpinned_service,
            "wiki.import",
            import_args(first_body, "notes/unpinned-service--a11ce000.md", false),
        )
        .await
        .expect_err("service actor without a pinned tenant user must fail closed");
    assert!(
        matches!(service_error, GadgetError::Denied { .. }),
        "unexpected service actor error: {service_error:?}"
    );

    let first = provider
        .call_with_context(
            &context,
            "wiki.import",
            import_args(first_body, "notes/operator-note--a11ce001.md", false),
        )
        .await
        .expect("first Source import");
    assert_eq!(first.content["path"], "notes/operator-note--a11ce001.md");
    assert_eq!(first.content["canonical_plug"], "source-ledger");
    assert_eq!(first.content["blob_existed"], false);
    assert_eq!(first.content["object_revision"], 1);
    let first_source_id = receipt_uuid(&first, "source_id");
    let object_id = receipt_uuid(&first, "object_id");

    let (space_id, vault_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT s.id, v.id FROM knowledge_spaces s \
         JOIN knowledge_vaults v ON v.tenant_id = s.tenant_id AND v.space_id = s.id \
         WHERE s.tenant_id = $1 AND s.kind = 'personal' AND s.owner_user_id = $2 \
           AND v.home_bundle_id = 'core'",
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_one(harness.pool())
    .await
    .expect("Personal Space core Vault");
    let (status, stored_object, final_uri): (String, Option<Uuid>, Option<String>) =
        sqlx::query_as(
            "SELECT status, extracted_object_id, final_uri FROM knowledge_sources \
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(first_source_id)
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(status, "extracted");
    assert_eq!(stored_object, Some(object_id));
    assert_eq!(
        final_uri.as_deref(),
        Some("https://example.invalid/operator-note")
    );
    let attempts: Vec<String> = sqlx::query_scalar(
        "SELECT phase FROM knowledge_source_attempts WHERE tenant_id = $1 AND source_id = $2 \
         ORDER BY created_at, phase",
    )
    .bind(tenant_id)
    .bind(first_source_id)
    .fetch_all(harness.pool())
    .await
    .unwrap();
    assert_eq!(attempts.len(), 2);
    assert!(attempts.iter().any(|phase| phase == "upload"));
    assert!(attempts.iter().any(|phase| phase == "extract"));

    let note_path = vault_layout
        .domain_root(tenant_id, space_id, "core")
        .unwrap()
        .join("notes/operator-note--a11ce001.md");
    let first_note = std::fs::read_to_string(&note_path).expect("Source-linked note");
    assert!(first_note.contains(&first_source_id.to_string()));
    assert!(first_note.contains("First imported revision."));

    let duplicate = provider
        .call_with_context(
            &context,
            "wiki.import",
            import_args(first_body, "notes/operator-note-copy--a11ce002.md", false),
        )
        .await
        .expect("same bytes at a distinct path");
    assert_eq!(duplicate.content["blob_existed"], true);
    assert_ne!(receipt_uuid(&duplicate, "source_id"), first_source_id);
    assert_ne!(receipt_uuid(&duplicate, "object_id"), object_id);
    let blob_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_blobs WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    let source_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_sources WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(
        blob_count, 1,
        "identical original bytes reuse one tenant blob"
    );
    assert_eq!(
        source_count, 2,
        "each import remains a distinct Source event"
    );

    let conflict = provider
        .call_with_context(
            &context,
            "wiki.import",
            import_args(first_body, "notes/operator-note--a11ce001.md", false),
        )
        .await
        .expect_err("existing exact path requires overwrite=true");
    assert!(matches!(conflict, GadgetError::InvalidArgs(_)));
    let source_count_after_conflict: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_sources WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(
        source_count_after_conflict, 2,
        "conflict creates no Source row"
    );

    let dedup_overwrite = provider
        .call_with_context(
            &context,
            "wiki.import",
            import_args(first_body, "notes/operator-note--a11ce001.md", true),
        )
        .await
        .expect("same bytes can explicitly advance the stable note");
    assert_eq!(dedup_overwrite.content["blob_existed"], true);
    assert_eq!(receipt_uuid(&dedup_overwrite, "object_id"), object_id);
    assert_eq!(dedup_overwrite.content["object_revision"], 2);
    assert_ne!(receipt_uuid(&dedup_overwrite, "source_id"), first_source_id);

    let second_body = "# Operator Note\n\nExplicit replacement revision.";
    let overwritten = provider
        .call_with_context(
            &context,
            "wiki.import",
            import_args(second_body, "notes/operator-note--a11ce001.md", true),
        )
        .await
        .expect("explicit overwrite");
    assert_eq!(receipt_uuid(&overwritten, "object_id"), object_id);
    assert_eq!(overwritten.content["object_revision"], 3);
    assert_ne!(receipt_uuid(&overwritten, "source_id"), first_source_id);
    let overwritten_note = std::fs::read_to_string(&note_path).unwrap();
    assert!(overwritten_note.contains("Explicit replacement revision."));
    assert!(!overwritten_note.contains("First imported revision."));

    let legacy_get = provider
        .call("wiki.get", serde_json::json!({"name": "imports/legacy"}))
        .await
        .expect("pre-existing legacy import remains readable");
    assert!(legacy_get.content["content"]
        .as_str()
        .unwrap()
        .contains("Keep me readable."));
    assert_eq!(std::fs::read_to_string(&legacy_path).unwrap(), legacy);

    let (current_source_id, revision): (Option<Uuid>, i64) = sqlx::query_as(
        "SELECT source_id, revision FROM knowledge_objects \
         WHERE tenant_id = $1 AND vault_id = $2 AND id = $3",
    )
    .bind(tenant_id)
    .bind(vault_id)
    .bind(object_id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(
        current_source_id,
        Some(receipt_uuid(&overwritten, "source_id"))
    );
    assert_eq!(revision, 3);

    harness.cleanup().await;
}

#[tokio::test]
async fn wiki_import_fails_closed_without_user_context_or_source_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let provider = KnowledgeGadgetProvider::from_service(build_service(build_wiki(&dir)), None, 10);
    let args = import_args("# No Context", "notes/no-context--a11ce003.md", false);
    let no_context = provider
        .call("wiki.import", args.clone())
        .await
        .expect_err("contextless import must fail closed");
    assert!(matches!(no_context, GadgetError::Denied { .. }));

    let context = GadgetDispatchContext::new(
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
    );
    let no_ledger = provider
        .call_with_context(&context, "wiki.import", args)
        .await
        .expect_err("provider without PostgreSQL must fail closed");
    assert!(matches!(no_ledger, GadgetError::Denied { .. }));
}

#[tokio::test]
async fn wiki_import_rejects_malformed_base64_before_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let provider = KnowledgeGadgetProvider::from_service(build_service(build_wiki(&dir)), None, 10);
    let err = provider
        .call(
            "wiki.import",
            serde_json::json!({
                "bytes": "not base64!",
                "content_type": "text/markdown",
            }),
        )
        .await
        .expect_err("malformed base64 must error");
    assert!(matches!(err, GadgetError::InvalidArgs(ref message) if message.contains("base64")));
}

#[tokio::test]
async fn wiki_import_rejects_unsupported_content_type_before_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let provider = KnowledgeGadgetProvider::from_service(build_service(build_wiki(&dir)), None, 10);
    let err = provider
        .call(
            "wiki.import",
            serde_json::json!({
                "bytes": encoded("%PDF-1.7"),
                "content_type": "application/pdf",
            }),
        )
        .await
        .expect_err("PDF uses the Source upload surface");
    assert!(matches!(err, GadgetError::InvalidArgs(_)));
}
