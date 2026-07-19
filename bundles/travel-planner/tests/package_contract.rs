use std::{fs, path::Path};

use gadgetron_bundle_sdk::{BundlePackageManifest, NavigationSection, UiContributionKind};
use sha2::{Digest, Sha256};

#[test]
fn signed_template_is_independent_and_pins_its_owned_schema() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = include_str!("../package.template.toml").replace(
        "@ENTRY_SHA256@",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    let package = BundlePackageManifest::parse_toml(&source).unwrap();
    assert_eq!(package.bundle.id.as_str(), "travel-planner");
    assert_eq!(package.bundle.version.to_string(), "0.2.0");
    assert_eq!(
        package.bundle.class,
        Some(gadgetron_bundle_sdk::BundleClass::Operational)
    );
    assert_eq!(
        package.capabilities.provides[0].id.as_str(),
        "gadgetron.operation.travel-planning"
    );
    assert_eq!(package.dependencies.optional.len(), 2);
    assert_eq!(
        package
            .dependencies
            .optional
            .iter()
            .map(|dependency| dependency.capability.as_str())
            .collect::<Vec<_>>(),
        vec![
            "gadgetron.intelligence.restaurant-context",
            "gadgetron.intelligence.travel-context",
        ]
    );
    let navigation_sections: Vec<_> = package
        .capabilities
        .ui_contributions
        .iter()
        .filter(|item| item.kind == UiContributionKind::Navigation)
        .map(|item| item.navigation_section)
        .collect();
    assert_eq!(
        navigation_sections,
        vec![
            Some(NavigationSection::Planning),
            Some(NavigationSection::Planning),
            Some(NavigationSection::Planning)
        ]
    );
    assert_eq!(package.capabilities.migrations.len(), 3);
    assert!(package
        .capabilities
        .gadgets
        .iter()
        .all(|gadget| gadget.name.as_str().starts_with("travel.")));
    assert!(package
        .capabilities
        .gadgets
        .iter()
        .any(|gadget| gadget.name.as_str() == "travel.restaurant-attach"));
    assert!(package
        .capabilities
        .gadgets
        .iter()
        .any(|gadget| gadget.name.as_str() == "travel.knowledge-context"));
    assert!(package.permissions.iter().any(|permission| {
        permission.kind == gadgetron_bundle_sdk::PermissionKind::KnowledgeRead
            && permission.resources == ["knowledge:context"]
    }));
    assert!(package.permissions.iter().any(|permission| {
        permission.kind == gadgetron_bundle_sdk::PermissionKind::KnowledgeFeedback
            && permission.resources == ["knowledge:feedback"]
    }));
    assert_eq!(package.capabilities.workspaces.len(), 3);

    for migration in &package.capabilities.migrations {
        assert!(migration.legacy_sqlx_version.is_none());
        let bytes = fs::read(root.join(migration.path.as_str())).unwrap();
        assert_eq!(hex::encode(Sha256::digest(bytes)), migration.sha256);
    }
}
