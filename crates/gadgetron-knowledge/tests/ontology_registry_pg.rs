use gadgetron_bundle_sdk::{BundleId, BundlePackageManifest};
use gadgetron_knowledge::{
    OntologyActivationCommand, OntologyKernel, OntologyMappingCommand, OntologyMappingDisposition,
    OntologyPackageRegistration, OntologyRegistry, OntologyRegistryError,
    OntologySchemaRegistration,
};
use gadgetron_testing::harness::pg::PgHarness;
use sha2::{Digest, Sha256};
use uuid::Uuid;

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

fn schema_descriptor(bytes: &[u8]) -> gadgetron_bundle_sdk::DomainSchemaDescriptor {
    let digest = hex::encode(Sha256::digest(bytes));
    let package = format!(
        r#"manifest_version = 1

[bundle]
id = "restaurant-research"
version = "1.0.0"
publisher = "example.publisher"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "container"
transport = "json_rpc_stdio"
entry = "unused"

[runtime.limits]
memory_mb = 64
open_files = 32
cpu_seconds = 10

[[capabilities.domain_schemas]]
id = "restaurant-domain"
version = 1
schema_path = "schema/domain.json"
sha256 = "{digest}"
"#
    );
    BundlePackageManifest::parse_toml(&package)
        .unwrap()
        .capabilities
        .domain_schemas
        .into_iter()
        .next()
        .unwrap()
}

#[tokio::test]
async fn immutable_registry_reuses_revision_tracks_packages_and_rejects_drift() {
    if !pg_available().await {
        eprintln!("skipping ontology registry test: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let registry = OntologyRegistry::new(harness.pool().clone());
    let owner = BundleId::new("restaurant-research").unwrap();
    let schema_bytes =
        br#"{"properties":{"entities":{"const":["Place"]},"relations":{"const":["applies_to"]}}}"#;
    let descriptor = schema_descriptor(schema_bytes);
    let schemas = [OntologySchemaRegistration {
        descriptor: &descriptor,
        bytes: schema_bytes,
    }];

    let first = registry
        .register_package(OntologyPackageRegistration {
            owner_bundle_id: &owner,
            package_version: "1.0.0",
            package_manifest_sha256: &"a".repeat(64),
            schemas: &schemas,
        })
        .await
        .unwrap();
    assert!(first[0].revision_created);
    assert!(first[0].provenance_created);
    assert!(first[0].revision.legacy_adapter);

    let repeated = registry
        .register_package(OntologyPackageRegistration {
            owner_bundle_id: &owner,
            package_version: "1.0.0",
            package_manifest_sha256: &"a".repeat(64),
            schemas: &schemas,
        })
        .await
        .unwrap();
    assert_eq!(first[0].revision.id, repeated[0].revision.id);
    assert!(!repeated[0].revision_created);
    assert!(!repeated[0].provenance_created);

    let patch = registry
        .register_package(OntologyPackageRegistration {
            owner_bundle_id: &owner,
            package_version: "1.0.1",
            package_manifest_sha256: &"b".repeat(64),
            schemas: &schemas,
        })
        .await
        .unwrap();
    assert!(!patch[0].revision_created);
    assert!(patch[0].provenance_created);

    let drifted_bytes =
        br#"{"properties":{"entities":{"const":["Venue"]},"relations":{"const":["applies_to"]}}}"#;
    let drifted_descriptor = schema_descriptor(drifted_bytes);
    let drifted = [OntologySchemaRegistration {
        descriptor: &drifted_descriptor,
        bytes: drifted_bytes,
    }];
    let error = registry
        .register_package(OntologyPackageRegistration {
            owner_bundle_id: &owner,
            package_version: "1.0.2",
            package_manifest_sha256: &"c".repeat(64),
            schemas: &drifted,
        })
        .await
        .expect_err("a versioned ontology identity cannot change digest");
    assert!(matches!(
        error,
        OntologyRegistryError::RevisionConflict { .. }
    ));

    let revision_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_ontology_revisions")
            .fetch_one(harness.pool())
            .await
            .unwrap();
    let provenance_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_ontology_package_provenance")
            .fetch_one(harness.pool())
            .await
            .unwrap();
    let stored_bytes: Vec<u8> =
        sqlx::query_scalar("SELECT schema_bytes FROM knowledge_ontology_revisions")
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(revision_count, 1);
    assert_eq!(provenance_count, 2);
    assert_eq!(stored_bytes, schema_bytes);

    let activation_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_ontology_activation_events")
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(
        activation_count, 0,
        "package registration is not activation"
    );

    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let vault_id = Uuid::new_v4();
    let object_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'ontology-kernel-test')")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1, $2, 'ontology@test.invalid', 'Ontology Curator', 'admin', 'test')",
    )
    .bind(user_id)
    .bind(tenant_id)
    .execute(harness.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO knowledge_spaces (id, tenant_id, kind, title, owner_user_id) \
         VALUES ($1, $2, 'personal', 'Personal Knowledge', $3)",
    )
    .bind(space_id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(harness.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO knowledge_vaults (id, tenant_id, space_id, home_bundle_id) \
         VALUES ($1, $2, $3, 'restaurant-research')",
    )
    .bind(vault_id)
    .bind(tenant_id)
    .bind(space_id)
    .execute(harness.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO knowledge_objects \
         (id, tenant_id, vault_id, canonical_kind, path, created_by) \
         VALUES ($1, $2, $3, 'domain_entity', 'places/copper-kettle.md', $4)",
    )
    .bind(object_id)
    .bind(tenant_id)
    .bind(vault_id)
    .bind(user_id)
    .execute(harness.pool())
    .await
    .unwrap();

    let kernel = OntologyKernel::new(harness.pool().clone());
    let before_activation = kernel
        .append_mapping(OntologyMappingCommand {
            tenant_id,
            recorded_by: user_id,
            object_id,
            object_revision: 1,
            expected_mapping_revision: 0,
            disposition: OntologyMappingDisposition::Active,
            ontology_revision_id: Some(first[0].revision.id),
            type_id: Some("Place"),
            confidence: Some(0.9),
            evidence: serde_json::json!({"source": "curator"}),
            reason: "Classify the reviewed place note",
        })
        .await
        .expect_err("a registered ontology is not tenant-active by default");
    assert!(matches!(
        before_activation,
        OntologyRegistryError::ActivationRequired
    ));

    let activation = kernel
        .activate(OntologyActivationCommand {
            tenant_id,
            actor_user_id: user_id,
            ontology_revision_id: first[0].revision.id,
            expected_activation_revision: 0,
            reason: "Use the reviewed Restaurant ontology",
        })
        .await
        .unwrap();
    assert!(activation.created);
    assert_eq!(activation.event.activation_revision, 1);

    let mapped = kernel
        .append_mapping(OntologyMappingCommand {
            tenant_id,
            recorded_by: user_id,
            object_id,
            object_revision: 1,
            expected_mapping_revision: 0,
            disposition: OntologyMappingDisposition::Active,
            ontology_revision_id: Some(first[0].revision.id),
            type_id: Some("Place"),
            confidence: Some(0.9),
            evidence: serde_json::json!({"source": "curator"}),
            reason: "Classify the reviewed place note",
        })
        .await
        .unwrap();
    assert_eq!(mapped.mapping_revision, 1);

    let unknown = kernel
        .append_mapping(OntologyMappingCommand {
            tenant_id,
            recorded_by: user_id,
            object_id,
            object_revision: 1,
            expected_mapping_revision: 1,
            disposition: OntologyMappingDisposition::Proposed,
            ontology_revision_id: Some(first[0].revision.id),
            type_id: Some("ImaginaryType"),
            confidence: Some(0.4),
            evidence: serde_json::json!({"source": "agent"}),
            reason: "Keep an uncertain classification explicit",
        })
        .await
        .expect_err("unknown types cannot be invented by an agent");
    assert!(matches!(unknown, OntologyRegistryError::UnknownType { .. }));

    let unmapped = kernel
        .append_mapping(OntologyMappingCommand {
            tenant_id,
            recorded_by: user_id,
            object_id,
            object_revision: 1,
            expected_mapping_revision: 1,
            disposition: OntologyMappingDisposition::Unmapped,
            ontology_revision_id: None,
            type_id: None,
            confidence: Some(0.4),
            evidence: serde_json::json!({"candidate_types": ["Place", "Branch"]}),
            reason: "Evidence is ambiguous; do not force a type",
        })
        .await
        .unwrap();
    assert_eq!(unmapped.mapping_revision, 2);

    let deactivation = kernel
        .deactivate(OntologyActivationCommand {
            tenant_id,
            actor_user_id: user_id,
            ontology_revision_id: first[0].revision.id,
            expected_activation_revision: 1,
            reason: "Pause new mappings while the ontology is reviewed",
        })
        .await
        .unwrap();
    assert_eq!(deactivation.event.activation_revision, 2);
    let mapping_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_ontology_mapping_events")
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(
        mapping_count, 2,
        "failed candidates append no mapping event"
    );

    harness.cleanup().await;
}
