use std::{fs, path::Path};

use gadgetron_bundle_sdk::{BundlePackageManifest, NavigationSection, UiContributionKind};
use sha2::{Digest, Sha256};

#[test]
fn package_is_independent_cited_and_zero_egress() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = include_str!("../package.template.toml").replace(
        "@ENTRY_SHA256@",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    let package = BundlePackageManifest::parse_toml(&source).unwrap();

    assert_eq!(package.bundle.id.as_str(), "restaurant-research");
    assert_eq!(package.bundle.version.to_string(), "0.1.4");
    assert_eq!(
        package.bundle.class,
        Some(gadgetron_bundle_sdk::BundleClass::Intelligence)
    );
    assert_eq!(
        package.capabilities.provides[0].id.as_str(),
        "gadgetron.intelligence.restaurant-context"
    );
    assert!(package.runtime.egress.allow.is_empty());
    assert!(package
        .capabilities
        .gadgets
        .iter()
        .all(|gadget| gadget.name.as_str().starts_with("restaurant.")));
    assert!(package
        .capabilities
        .gadgets
        .iter()
        .all(|gadget| gadget.name.as_str() != "restaurant.attach-to-trip"));
    assert_eq!(package.capabilities.jobs.len(), 1);
    assert_eq!(package.capabilities.collection_profiles.len(), 1);
    assert_eq!(package.capabilities.agent_roles.len(), 1);
    assert_eq!(package.capabilities.domain_schemas.len(), 1);
    assert_eq!(package.capabilities.seed_assets.len(), 2);
    assert_eq!(package.capabilities.migrations.len(), 1);

    let navigation = package
        .capabilities
        .ui_contributions
        .iter()
        .find(|item| item.kind == UiContributionKind::Navigation)
        .unwrap();
    assert_eq!(
        navigation.navigation_section,
        Some(NavigationSection::Planning)
    );

    for (path, digest) in [
        (
            package.capabilities.domain_schemas[0].schema_path.as_str(),
            package.capabilities.domain_schemas[0].sha256.as_str(),
        ),
        (
            package.capabilities.migrations[0].path.as_str(),
            package.capabilities.migrations[0].sha256.as_str(),
        ),
    ] {
        let bytes = fs::read(root.join(path)).unwrap();
        assert_eq!(hex::encode(Sha256::digest(bytes)), digest);
    }

    for asset in &package.capabilities.seed_assets {
        let bytes = fs::read(root.join(asset.path.as_str())).unwrap();
        assert_eq!(hex::encode(Sha256::digest(bytes)), asset.sha256);
    }
}
