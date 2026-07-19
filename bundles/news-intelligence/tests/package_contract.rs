use std::{fs, path::Path};

use gadgetron_bundle_sdk::{
    BundleClass, BundlePackageManifest, DomainOntology, NavigationSection, PermissionKind,
    UiContributionKind,
};
use sha2::{Digest, Sha256};

#[test]
fn package_is_independent_cited_and_zero_egress() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = include_str!("../package.template.toml").replace(
        "@ENTRY_SHA256@",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    let package = BundlePackageManifest::parse_toml(&source).unwrap();

    assert_eq!(package.bundle.id.as_str(), "news-intelligence");
    assert_eq!(package.bundle.version.to_string(), "0.1.0");
    assert_eq!(package.bundle.class, Some(BundleClass::Intelligence));
    assert!(package.runtime.egress.allow.is_empty());
    assert!(package
        .capabilities
        .gadgets
        .iter()
        .all(|gadget| gadget.name.as_str().starts_with("news.")));
    let article_upsert = package
        .capabilities
        .gadgets
        .iter()
        .find(|gadget| gadget.name.as_str() == "news.article-upsert")
        .unwrap();
    let article_properties = article_upsert.input_schema["properties"]
        .as_object()
        .unwrap();
    assert!(!article_properties.contains_key("article_id"));
    assert!(!article_properties.contains_key("revision"));
    assert!(package.permissions.iter().any(|permission| {
        permission.kind == PermissionKind::KnowledgeCollection
            && permission.resources == ["knowledge:collection"]
    }));
    assert_eq!(package.capabilities.collection_profiles.len(), 1);
    assert_eq!(package.capabilities.agent_roles.len(), 3);
    assert_eq!(
        package.capabilities.agent_roles[1]
            .followup_role
            .as_ref()
            .map(|role| role.as_str()),
        Some("news-distiller"),
    );
    assert_eq!(package.capabilities.jobs.len(), 3);
    assert_eq!(package.capabilities.domain_schemas.len(), 1);
    assert_eq!(package.capabilities.seed_assets.len(), 3);
    assert_eq!(package.capabilities.migrations.len(), 2);

    let navigation: Vec<_> = package
        .capabilities
        .ui_contributions
        .iter()
        .filter(|item| item.kind == UiContributionKind::Navigation)
        .collect();
    assert_eq!(navigation.len(), 4);
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
    assert!(ontology.type_by_id("NewsEvent").is_some());
    for asset in &package.capabilities.seed_assets {
        let bytes = fs::read(root.join(asset.path.as_str())).unwrap();
        assert_eq!(hex::encode(Sha256::digest(bytes)), asset.sha256);
    }
}
