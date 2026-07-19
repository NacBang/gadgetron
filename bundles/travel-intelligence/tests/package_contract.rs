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

    assert_eq!(package.bundle.id.as_str(), "travel-intelligence");
    assert_eq!(package.bundle.version.to_string(), "0.1.0");
    assert_eq!(package.bundle.class, Some(BundleClass::Intelligence));
    assert_eq!(
        package.capabilities.provides[0].id.as_str(),
        "gadgetron.intelligence.travel-context"
    );
    assert!(package.permissions.is_empty());
    assert!(package.runtime.egress.allow.is_empty());
    assert!(package.capabilities.gadgets.is_empty());
    assert!(package.capabilities.workspaces.is_empty());
    assert!(package.capabilities.ui_contributions.is_empty());
    assert_eq!(package.capabilities.collection_profiles.len(), 1);
    assert_eq!(package.capabilities.agent_roles.len(), 3);

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
}
