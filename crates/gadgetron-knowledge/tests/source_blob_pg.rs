use gadgetron_core::ingest::{BlobMetadata, BlobStore};
use gadgetron_knowledge::source::FilesystemBlobStore;
use gadgetron_testing::harness::pg::PgHarness;
use uuid::Uuid;

async fn pg_available() -> bool {
    let admin_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .is_ok()
}

#[tokio::test]
async fn r2_2_blob_dedup_is_tenant_scoped_and_checksum_verified() {
    if !pg_available().await {
        eprintln!("skipping R2.2 blob fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    for (tenant, user, name) in [(tenant_a, user_a, "blob-a"), (tenant_b, user_b, "blob-b")] {
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
            .bind(tenant)
            .bind(name)
            .execute(harness.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
             VALUES ($1, $2, $3, 'Blob User', 'admin', 'test')",
        )
        .bind(user)
        .bind(tenant)
        .bind(format!("{name}@test.invalid"))
        .execute(harness.pool())
        .await
        .unwrap();
    }
    let root = tempfile::tempdir().unwrap();
    let store = FilesystemBlobStore::new(harness.pool().clone(), root.path());
    let bytes = b"same original bytes";
    let meta = |tenant: Uuid, user: Uuid| BlobMetadata {
        tenant_id: tenant.to_string(),
        content_type: "text/plain".into(),
        filename: "source.txt".into(),
        byte_size: bytes.len() as u64,
        imported_by: user.to_string(),
    };
    let first = store.put(bytes, &meta(tenant_a, user_a)).await.unwrap();
    let duplicate = store.put(bytes, &meta(tenant_a, user_a)).await.unwrap();
    let isolated = store.put(bytes, &meta(tenant_b, user_b)).await.unwrap();
    assert!(!first.existed);
    assert!(duplicate.existed);
    assert_eq!(first.id, duplicate.id);
    assert_ne!(first.id, isolated.id);
    assert_ne!(first.storage_uri, isolated.storage_uri);
    assert_eq!(store.get(&first.id).await.unwrap(), bytes);
    assert_eq!(store.get(&isolated.id).await.unwrap(), bytes);

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_blobs")
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(rows, 2);
    assert!(root
        .path()
        .join("tenants")
        .join(tenant_a.to_string())
        .join("blobs")
        .is_dir());
    assert!(root
        .path()
        .join("tenants")
        .join(tenant_b.to_string())
        .join("blobs")
        .is_dir());
    harness.cleanup().await;
}
