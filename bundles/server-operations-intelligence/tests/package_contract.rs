use std::{fs, path::Path};

use gadgetron_bundle_sdk::{BundleClass, BundlePackageManifest, DomainOntology};
use sha2::{Digest, Sha256};

#[test]
fn package_is_native_zero_authority_intelligence() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = include_str!("../package.template.toml").replace(
        "@ENTRY_SHA256@",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    let package = BundlePackageManifest::parse_toml(&source).unwrap();

    assert_eq!(package.bundle.id.as_str(), "server-operations-intelligence");
    assert_eq!(package.bundle.version.to_string(), "0.1.2");
    assert_eq!(package.bundle.class, Some(BundleClass::Intelligence));
    assert_eq!(
        package.capabilities.provides[0].id.as_str(),
        "gadgetron.intelligence.server-operations-context"
    );
    assert_eq!(package.permissions.len(), 2);
    assert!(package.runtime.egress.allow.is_empty());
    assert_eq!(package.capabilities.gadgets.len(), 3);
    assert!(package.capabilities.workspaces.is_empty());
    assert!(package.capabilities.ui_contributions.is_empty());
    assert_eq!(package.capabilities.collection_profiles.len(), 1);
    assert_eq!(package.capabilities.agent_roles.len(), 5);
    assert_eq!(package.capabilities.event_jobs.len(), 2);
    assert!(package.capabilities.event_jobs.iter().any(|event| {
        event.id.as_str() == "server-incident-enrichment"
            && event.event_kind.as_str() == "server-incident-updated"
            && event.subject_owner_bundle.as_str() == "server-administrator"
    }));
    assert_eq!(package.capabilities.row_enrichments.len(), 1);
    let incident = &package.capabilities.row_enrichments[0];
    assert_eq!(incident.target_bundle.as_str(), "server-administrator");
    assert_eq!(incident.target_workspace.as_str(), "alerts");
    assert_eq!(
        incident.target_data_capability.as_str(),
        "server.incidents-list"
    );
    assert_eq!(incident.row_join_key_field, "incident_id");
    assert_eq!(incident.row_revision_field, "revision");

    let schema = &package.capabilities.domain_schemas[0];
    let schema_bytes = fs::read(root.join(schema.schema_path.as_str())).unwrap();
    assert_eq!(hex::encode(Sha256::digest(&schema_bytes)), schema.sha256);
    assert!(
        !DomainOntology::parse_json(&schema_bytes, schema.version)
            .unwrap()
            .legacy_adapter
    );
    for asset in &package.capabilities.seed_assets {
        let bytes = fs::read(root.join(asset.path.as_str())).unwrap();
        assert_eq!(hex::encode(Sha256::digest(bytes)), asset.sha256);
    }
    for migration in &package.capabilities.migrations {
        let bytes = fs::read(root.join(migration.path.as_str())).unwrap();
        assert_eq!(hex::encode(Sha256::digest(bytes)), migration.sha256);
    }
}
