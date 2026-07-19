use std::{collections::BTreeMap, collections::BTreeSet, fs, path::Path};

use gadgetron_bundle_sdk::{
    BundlePackageManifest, GadgetTier, NavigationSection, TargetRegistryKind, UiContributionKind,
};
use sha2::{Digest, Sha256, Sha384};

#[test]
fn package_template_pins_byte_identical_legacy_migration_adopters() {
    let bundle_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = bundle_root.join("../..");
    let source = include_str!("../package.template.toml").replace(
        "@ENTRY_SHA256@",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    let package = BundlePackageManifest::parse_toml(&source).unwrap();
    assert_eq!(package.bundle.id.as_str(), "server-administrator");
    assert_eq!(package.bundle.version.to_string(), "0.4.27");
    assert_eq!(
        package.bundle.class,
        Some(gadgetron_bundle_sdk::BundleClass::Operational)
    );
    assert_eq!(
        package.capabilities.provides[0].id.as_str(),
        "gadgetron.operation.server-administration"
    );
    assert_eq!(package.dependencies.optional.len(), 1);
    assert_eq!(
        package.dependencies.optional[0].capability.as_str(),
        "gadgetron.intelligence.server-operations-context"
    );
    assert!(package.permissions.iter().any(|permission| {
        permission.kind == gadgetron_bundle_sdk::PermissionKind::KnowledgeRead
            && permission.resources == ["knowledge:context"]
    }));
    assert!(package.permissions.iter().any(|permission| {
        permission.kind == gadgetron_bundle_sdk::PermissionKind::KnowledgeFeedback
            && permission.resources == ["knowledge:feedback"]
    }));
    assert!(package
        .capabilities
        .gadgets
        .iter()
        .any(|gadget| gadget.name.as_str() == "server.knowledge-context"));
    for name in [
        "server.profiles-list",
        "server.profile-revision-create",
        "server.clusters-list",
        "server.cluster-upsert",
        "server.enrollments-list",
        "server.enrollment-start",
        "server.enrollment-rollout-plan",
        "server.enrollment-rollout-apply",
        "server.enrollment-transition",
        "server.validation-record",
        "server.validation-results-list",
        "server.incidents-list",
        "server.incident-context",
        "server.incident-distill",
        "server.monitoring-observe",
    ] {
        assert!(
            package
                .capabilities
                .gadgets
                .iter()
                .any(|gadget| gadget.name.as_str() == name),
            "missing cluster enrollment Gadget {name}"
        );
    }
    for table in [
        "postgres:table:server_profile_revisions",
        "postgres:table:server_cluster_revisions",
        "postgres:table:server_clusters",
        "postgres:table:server_enrollments",
        "postgres:table:server_validation_results",
        "postgres:table:server_incidents",
        "postgres:table:server_incident_signals",
        "postgres:table:server_incident_events",
        "postgres:table:server_incident_enrichment_dispatches",
    ] {
        assert!(package.permissions.iter().any(|permission| {
            permission.id.as_str() == "operations-read"
                && permission
                    .resources
                    .iter()
                    .any(|resource| resource == table)
        }));
        assert!(package.permissions.iter().any(|permission| {
            permission.id.as_str() == "operations-write"
                && permission
                    .resources
                    .iter()
                    .any(|resource| resource == table)
        }));
    }
    let live = package
        .capabilities
        .gadgets
        .iter()
        .find(|gadget| gadget.name.as_str() == "server.telemetry-live")
        .expect("live telemetry Gadget is signed");
    assert_eq!(live.tier, GadgetTier::Read);
    assert_eq!(live.input_schema["x_gadgetron_live_telemetry"], true);
    let history = package
        .capabilities
        .gadgets
        .iter()
        .find(|gadget| gadget.name.as_str() == "server.metric-series")
        .expect("metric history Gadget is signed");
    assert_eq!(history.input_schema["x_gadgetron_metric_history"], true);
    let navigation_sections: Vec<_> = package
        .capabilities
        .ui_contributions
        .iter()
        .filter(|item| item.kind == UiContributionKind::Navigation)
        .map(|item| (item.id.as_str(), item.navigation_section))
        .collect();
    assert_eq!(
        navigation_sections,
        vec![
            ("fleet-navigation", Some(NavigationSection::Operations)),
            ("fleet-map-navigation", Some(NavigationSection::Operations),),
            ("servers-navigation", Some(NavigationSection::Operations)),
            ("topology-navigation", Some(NavigationSection::Operations)),
            ("metrics-navigation", Some(NavigationSection::Operations)),
            (
                "raw-telemetry-navigation",
                Some(NavigationSection::Diagnostics),
            ),
            ("alerts-navigation", Some(NavigationSection::Operations)),
            ("logs-navigation", Some(NavigationSection::Diagnostics)),
        ]
    );
    assert_eq!(
        package
            .capabilities
            .workspaces
            .iter()
            .find(|workspace| workspace.id.as_str() == "fleet")
            .map(|workspace| workspace.label.as_str()),
        Some("Overview")
    );
    let incidents = package
        .capabilities
        .workspaces
        .iter()
        .find(|workspace| workspace.id.as_str() == "alerts")
        .expect("Incidents workspace is signed");
    assert_eq!(
        incidents.renderer,
        gadgetron_bundle_sdk::WorkspaceRenderer::Cards
    );
    assert_eq!(incidents.data_capability.as_str(), "server.incidents-list");
    assert_eq!(
        incidents
            .action_gadgets
            .iter()
            .map(|gadget| gadget.as_str())
            .collect::<Vec<_>>(),
        vec!["server.incident-context", "server.incident-distill"]
    );
    let incident_distill = package
        .capabilities
        .gadgets
        .iter()
        .find(|gadget| gadget.name.as_str() == "server.incident-distill")
        .expect("incident Distill action is signed");
    assert_eq!(incident_distill.tier, GadgetTier::Write);
    assert_eq!(
        incident_distill.input_schema["x_gadgetron_row_action"],
        true
    );
    assert_eq!(
        incident_distill.input_schema["x_gadgetron_row_action_when"],
        serde_json::json!({"field": "status", "equals": "closed"})
    );
    assert_eq!(
        incident_distill.input_schema["properties"]["revision"]["pattern"],
        "^[0-9a-f]{64}$"
    );
    assert!(package.capabilities.ui_contributions.iter().any(|item| {
        item.kind == UiContributionKind::SubjectContext
            && item
                .gadget
                .as_ref()
                .is_some_and(|gadget| gadget.as_str() == "server.incident-context")
    }));
    let monitoring_repair = package
        .capabilities
        .gadgets
        .iter()
        .find(|gadget| gadget.name.as_str() == "server.monitoring-repair")
        .expect("monitoring repair is signed");
    assert_eq!(
        monitoring_repair.input_schema["properties"]["incident_id"]["format"],
        "uuid"
    );
    let monitoring_observe = package
        .capabilities
        .gadgets
        .iter()
        .find(|gadget| gadget.name.as_str() == "server.monitoring-observe")
        .expect("monitoring observation is signed");
    assert_eq!(monitoring_observe.tier, GadgetTier::Write);
    let enrollment_transition = package
        .capabilities
        .gadgets
        .iter()
        .find(|gadget| gadget.name.as_str() == "server.enrollment-transition")
        .expect("enrollment transition is signed");
    assert_eq!(
        enrollment_transition.input_schema["properties"]["incident_id"]["format"],
        "uuid"
    );
    assert_eq!(
        enrollment_transition.output_schema["properties"]["operation_kind"]["type"],
        serde_json::json!(["string", "null"])
    );
    assert_eq!(
        enrollment_transition
            .effect
            .outcome_gadget
            .as_ref()
            .map(|gadget| gadget.as_str()),
        Some("server.operation-outcomes-list")
    );
    let servers = package
        .capabilities
        .workspaces
        .iter()
        .find(|workspace| workspace.id.as_str() == "servers")
        .expect("Servers workspace is signed");
    assert_eq!(
        servers
            .action_gadgets
            .iter()
            .map(|gadget| gadget.as_str())
            .collect::<Vec<_>>(),
        vec![
            "server.subject-context",
            "server.monitoring-state",
            "server.monitoring-repair",
        ]
    );
    assert_eq!(
        package
            .capabilities
            .ui_contributions
            .iter()
            .find(|item| item.id.as_str() == "servers-main")
            .and_then(|item| item.target_registry),
        Some(TargetRegistryKind::Ssh)
    );
    assert_eq!(
        package
            .capabilities
            .target_profiles
            .iter()
            .map(|profile| (profile.id.as_str(), profile.default))
            .collect::<Vec<_>>(),
        vec![("server", true), ("gadgetini", false)]
    );
    assert_eq!(
        package
            .capabilities
            .jobs
            .iter()
            .find(|job| job.id.as_str() == "server-duty-cycle")
            .and_then(|job| job.target_profile.as_ref())
            .map(|profile| profile.as_str()),
        Some("server")
    );
    assert!(package
        .capabilities
        .jobs
        .iter()
        .find(|job| job.id.as_str() == "server-duty-cycle")
        .is_some_and(|job| job
            .gadget_allowlist
            .iter()
            .any(|gadget| gadget.as_str() == "server.monitoring-observe")));
    assert!(package
        .capabilities
        .jobs
        .iter()
        .any(|job| job.id.as_str() == "server-enrollment"));
    assert_eq!(package.capabilities.migrations.len(), 28);
    assert!(package.capabilities.knowledge_events.iter().any(|event| {
        event.id.as_str() == "incident-closed-knowledge"
            && event.snapshot_resource.as_str()
                == "postgres:table:server_incident_knowledge_snapshots"
            && event.acting_space_id_field.as_deref() == Some("acting_space_id")
    }));
    assert!(package
        .capabilities
        .migrations
        .iter()
        .any(|migration| migration.id.as_str() == "incident-knowledge-snapshot"));
    assert!(package
        .capabilities
        .migrations
        .iter()
        .any(|migration| migration.id.as_str() == "incident-enrichment-dispatch"));
    let legacy_sha384 = BTreeMap::from([
        (
            20260421000001_i64,
            "7244a1f123588fdfdce04d1852366747778692984454dcd835dcd23c3312b8e97b6310fa6ef334ce362e1005c3a22ec6",
        ),
        (
            20260423000001,
            "bf21d45acf6fa622ff462965f3fea5c2aeae409c2bc5f59d417b0143a64e3e285cd67be33d3a1783d1dd918755fd5373",
        ),
        (
            20260423000002,
            "d3ae96b1d0f3ace1bef371ed98b41354bd24c4675c0662cecddfdb7fcdd27ad5e28c97ca8877189e9f975c0e7cafb3d0",
        ),
        (
            20260423000003,
            "d00a2ec0364fb1968cf86bd0c50b23d785f4846a37c06ca033ad73cadd4417a9229c30cdaa1662d6dd6fabeb5e4c4987",
        ),
        (
            20260503000003,
            "8dd5bf7448588a731563a187a9ecf26868cf481457f46797249de2d7fa4f1d7ec3007ff4689ecd48767508591297c591",
        ),
        (
            20260611000001,
            "218640b6a6648bbf202ddabfca188dbea9d80ae9c28cdc5cb984cb5d80e29bb3c9cb85c90ddeb255ac3ccbed3743c76e",
        ),
        (
            20260616000001,
            "116afc8705be7ea8466fa43dc4e07c67abc2d0685834402214bf78f25df06e56556894ac8b52895ef02a1d743737f4b1",
        ),
    ]);

    let mut versions = BTreeSet::new();
    for descriptor in &package.capabilities.migrations {
        let bundle_path = bundle_root.join(descriptor.path.as_str());
        let filename = bundle_path.file_name().unwrap().to_str().unwrap();
        let version = filename.split_once('_').unwrap().0.parse::<i64>().unwrap();
        assert!(versions.insert(version), "duplicate migration version");
        assert_eq!(descriptor.revision, version as u64);

        let adopted = fs::read(&bundle_path).unwrap();
        if let Some(legacy_version) = descriptor.legacy_sqlx_version {
            assert_eq!(legacy_version, version);
            assert_eq!(
                hex::encode(Sha384::digest(&adopted)),
                legacy_sha384[&version],
                "adopter must preserve the published legacy SQLx checksum for {filename}"
            );
        } else {
            assert!(
                version >= 20260711000001,
                "only new Bundle-native migrations omit legacy adoption"
            );
        }
        assert_eq!(
            hex::encode(Sha256::digest(&adopted)),
            descriptor.sha256,
            "signed package digest drift for {filename}"
        );
        assert!(
            !workspace_root
                .join("crates/gadgetron-xaas/migrations")
                .join(filename)
                .exists(),
            "Bundle-owned migration must not remain in the Core stream: {filename}"
        );
    }
}
