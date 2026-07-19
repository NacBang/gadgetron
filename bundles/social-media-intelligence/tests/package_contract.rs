use std::{fs, path::Path};

use gadgetron_bundle_sdk::{
    BundleClass, BundlePackageManifest, DomainOntology, GadgetTier, NavigationSection,
    PermissionKind, UiContributionKind,
};
use sha2::{Digest, Sha256};

#[test]
fn package_is_independent_purgeable_and_zero_egress() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = include_str!("../package.template.toml").replace(
        "@ENTRY_SHA256@",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    let package = BundlePackageManifest::parse_toml(&source).unwrap();

    assert_eq!(package.bundle.id.as_str(), "social-media-intelligence");
    assert_eq!(package.bundle.version.to_string(), "0.1.0");
    assert_eq!(package.bundle.class, Some(BundleClass::Intelligence));
    assert!(package.runtime.egress.allow.is_empty());
    assert!(package
        .capabilities
        .gadgets
        .iter()
        .all(|gadget| gadget.name.as_str().starts_with("social.")));
    assert!(package.permissions.iter().any(|permission| {
        permission.kind == PermissionKind::KnowledgeCollection
            && permission.resources == ["knowledge:collection"]
    }));
    let purge = package
        .capabilities
        .gadgets
        .iter()
        .find(|gadget| gadget.name.as_str() == "social.source-purge")
        .unwrap();
    assert_eq!(purge.tier, GadgetTier::Destructive);
    assert!(!purge.effect.reversible);

    assert_eq!(package.capabilities.collection_profiles.len(), 1);
    let profile = &package.capabilities.collection_profiles[0];
    assert_eq!(profile.connector.as_str(), "core-social-api");
    assert_eq!(profile.allowlisted_domains, ["api.bsky.app"]);
    assert_eq!(profile.query_providers.len(), 2);
    assert!(profile
        .query_providers
        .iter()
        .all(|provider| !provider.requires_configuration));
    assert_eq!(
        profile.query_providers[0].query_label.as_deref(),
        Some("Keyword or phrase")
    );
    assert_eq!(
        profile.query_providers[1].query_label.as_deref(),
        Some("Handle or DID")
    );
    assert_eq!(package.capabilities.agent_roles.len(), 4);
    assert_eq!(
        package.capabilities.agent_roles[1]
            .followup_role
            .as_ref()
            .map(|role| role.as_str()),
        Some("social-distiller"),
    );
    assert_eq!(package.capabilities.jobs.len(), 4);
    assert_eq!(package.capabilities.domain_schemas.len(), 1);
    assert_eq!(package.capabilities.seed_assets.len(), 4);
    assert_eq!(package.capabilities.migrations.len(), 1);

    let navigation: Vec<_> = package
        .capabilities
        .ui_contributions
        .iter()
        .filter(|item| item.kind == UiContributionKind::Navigation)
        .collect();
    assert_eq!(navigation.len(), 6);
    assert!(navigation
        .iter()
        .all(|item| item.navigation_section == Some(NavigationSection::Knowledge)));

    let mut signed_files = vec![(
        package.capabilities.domain_schemas[0].schema_path.as_str(),
        package.capabilities.domain_schemas[0].sha256.as_str(),
    )];
    signed_files.extend(
        package
            .capabilities
            .migrations
            .iter()
            .map(|migration| (migration.path.as_str(), migration.sha256.as_str())),
    );
    for (path, digest) in signed_files {
        let bytes = fs::read(root.join(path)).unwrap();
        assert_eq!(hex::encode(Sha256::digest(bytes)), digest);
    }
    let schema = &package.capabilities.domain_schemas[0];
    let ontology = DomainOntology::parse_json(
        &fs::read(root.join(schema.schema_path.as_str())).unwrap(),
        schema.version,
    )
    .unwrap();
    assert!(ontology.type_by_id("Signal").is_some());
    assert!(ontology.type_by_id("ResponseDraft").is_some());
    for asset in &package.capabilities.seed_assets {
        let bytes = fs::read(root.join(asset.path.as_str())).unwrap();
        assert_eq!(hex::encode(Sha256::digest(bytes)), asset.sha256);
    }
}
