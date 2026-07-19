//! Architectural ratchet for the Core/Bundle package boundary.
//!
//! Existing violations are explicit release debt, not accepted architecture.
//! Any added edge fails this test; removing an edge requires deleting its
//! matching baseline entry so the evidence stays honest.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct BoundaryBaseline {
    schema_version: u32,
    temporary_violations: Vec<BoundaryViolation>,
}

#[derive(Debug, Deserialize)]
struct BoundaryViolation {
    from: String,
    to: String,
    reason: String,
    removal_track: String,
}

#[test]
fn core_bundle_dependency_boundary_matches_the_explicit_debt_baseline() {
    let root = workspace_root();
    let workspace = read_toml(&root.join("Cargo.toml"));
    let members = workspace["workspace"]["members"]
        .as_array()
        .expect("workspace.members must be an array");

    let mut violations = BTreeSet::new();
    for member in members {
        let relative = member.as_str().expect("workspace member must be a path");
        let manifest = read_toml(&root.join(relative).join("Cargo.toml"));
        let Some(package_name) = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
        else {
            panic!("workspace member {relative} has no package.name");
        };

        let dependencies = direct_dependencies(&manifest);
        if package_name == "gadgetron-bundle-sdk" {
            let platform_dependencies: Vec<_> = dependencies
                .iter()
                .filter(|dependency| dependency.starts_with("gadgetron-"))
                .collect();
            assert!(
                platform_dependencies.is_empty(),
                "the public Bundle SDK must remain a leaf crate; found {platform_dependencies:?}"
            );
            continue;
        }

        if package_name == "gadgetron-bundle-runtime" {
            let platform_dependencies: Vec<_> = dependencies
                .iter()
                .filter(|dependency| dependency.starts_with("gadgetron-"))
                .cloned()
                .collect();
            assert_eq!(
                platform_dependencies,
                vec!["gadgetron-bundle-sdk".to_string()],
                "the public Bundle runtime support may depend only on the public SDK inside the platform"
            );
            continue;
        }

        if package_name == "gadgetron-bundle-host" {
            let platform_dependencies: Vec<_> = dependencies
                .iter()
                .filter(|dependency| dependency.starts_with("gadgetron-"))
                .cloned()
                .collect();
            assert_eq!(
                platform_dependencies,
                vec!["gadgetron-bundle-sdk".to_string()],
                "the Core-owned Bundle host may depend only on the public SDK inside the platform"
            );
            continue;
        }

        if package_name == "gadgetron-bundle-supervisor" {
            let platform_dependencies: Vec<_> = dependencies
                .iter()
                .filter(|dependency| dependency.starts_with("gadgetron-"))
                .cloned()
                .collect();
            assert_eq!(
                platform_dependencies,
                vec![
                    "gadgetron-bundle-host".to_string(),
                    "gadgetron-bundle-sdk".to_string(),
                ],
                "the Core-owned Bundle supervisor may depend only on the host and public SDK inside the platform"
            );
            continue;
        }

        if package_name == "gadgetron-bundle-migrations" {
            let platform_dependencies: Vec<_> = dependencies
                .iter()
                .filter(|dependency| dependency.starts_with("gadgetron-"))
                .cloned()
                .collect();
            assert_eq!(
                platform_dependencies,
                vec![
                    "gadgetron-bundle-host".to_string(),
                    "gadgetron-bundle-sdk".to_string(),
                ],
                "the Core-owned Bundle migrator may depend only on the host and public SDK inside the platform"
            );
            continue;
        }

        let is_domain_bundle = package_name.starts_with("gadgetron-bundle-");
        for dependency in dependencies {
            let is_domain_bundle_dependency = dependency.starts_with("gadgetron-bundle-")
                && dependency != "gadgetron-bundle-sdk"
                && dependency != "gadgetron-bundle-runtime"
                && dependency != "gadgetron-bundle-host"
                && dependency != "gadgetron-bundle-migrations"
                && dependency != "gadgetron-bundle-supervisor";
            let bundle_reaches_private_platform = is_domain_bundle
                && dependency.starts_with("gadgetron-")
                && dependency != "gadgetron-bundle-sdk"
                && dependency != "gadgetron-bundle-runtime";
            let core_reaches_domain_bundle = !is_domain_bundle && is_domain_bundle_dependency;
            if bundle_reaches_private_platform || core_reaches_domain_bundle {
                violations.insert((package_name.to_owned(), dependency));
            }
        }
    }

    let baseline_path = root.join("docs/releases/bundle-boundary-baseline.json");
    let baseline: BoundaryBaseline = serde_json::from_str(
        &fs::read_to_string(&baseline_path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", baseline_path.display())),
    )
    .unwrap_or_else(|error| panic!("cannot parse {}: {error}", baseline_path.display()));
    assert_eq!(
        baseline.schema_version, 1,
        "unknown boundary baseline schema"
    );

    let mut expected = BTreeSet::new();
    for violation in baseline.temporary_violations {
        assert!(
            !violation.reason.trim().is_empty(),
            "{} -> {} needs a reason",
            violation.from,
            violation.to
        );
        assert!(
            violation.removal_track.starts_with("E4-"),
            "{} -> {} needs an E4 removal track",
            violation.from,
            violation.to
        );
        assert!(
            expected.insert((violation.from, violation.to)),
            "duplicate dependency-boundary baseline entry"
        );
    }

    assert_eq!(
        violations, expected,
        "Core/Bundle dependency debt changed. New edges are forbidden; when an old edge is removed, remove its explicit baseline entry in the same change."
    );
}

#[test]
fn server_administrator_is_an_independently_versioned_public_contracts_only_package() {
    let root = workspace_root();
    let manifest = read_toml(&root.join("bundles/server-administrator/Cargo.toml"));
    let package = manifest["package"]
        .as_table()
        .expect("Server Administrator package table");
    assert_eq!(
        package.get("name").and_then(toml::Value::as_str),
        Some("gadgetron-bundle-server-administrator")
    );
    let version = package
        .get("version")
        .and_then(toml::Value::as_str)
        .expect("Server Administrator must own an explicit version");
    semver::Version::parse(version).expect("Server Administrator version must be semver");

    let platform_dependencies: Vec<_> = direct_dependencies(&manifest)
        .into_iter()
        .filter(|dependency| dependency.starts_with("gadgetron-"))
        .collect();
    assert_eq!(
        platform_dependencies,
        vec![
            "gadgetron-bundle-runtime".to_string(),
            "gadgetron-bundle-sdk".to_string(),
        ],
        "Server Administrator may use public Bundle contracts but no Core-private crate"
    );
}

#[test]
fn signed_server_package_replaces_the_retired_compatibility_sources() {
    let root = workspace_root();
    let workspace = read_toml(&root.join("Cargo.toml"));
    let members: BTreeSet<_> = workspace["workspace"]["members"]
        .as_array()
        .expect("workspace.members must be an array")
        .iter()
        .map(|member| member.as_str().expect("workspace member must be a path"))
        .collect();

    for retired in [
        "bundles/server-monitor",
        "bundles/log-analyzer",
        "bundles/server-administrator/legacy",
    ] {
        assert!(
            !members.contains(retired),
            "retired workspace member {retired}"
        );
        assert!(
            !root.join(retired).exists(),
            "retired compatibility source must stay removed: {retired}"
        );
    }

    let package = read_toml(&root.join("bundles/server-administrator/package.template.toml"));
    let capabilities = package["capabilities"]
        .as_table()
        .expect("Server Administrator capabilities table");
    let namespaces: BTreeSet<_> = capabilities["gadget_namespaces"]
        .as_array()
        .expect("Server Administrator Gadget namespaces")
        .iter()
        .map(|namespace| {
            namespace
                .as_str()
                .expect("Gadget namespace must be a string")
        })
        .collect();
    assert_eq!(namespaces, BTreeSet::from(["loganalysis", "server"]));

    let gadgets: BTreeSet<_> = capabilities["gadgets"]
        .as_array()
        .expect("Server Administrator Gadgets")
        .iter()
        .map(|gadget| {
            gadget["name"]
                .as_str()
                .expect("Server Administrator Gadget name")
        })
        .collect();
    for required in [
        "server.inventory-collect",
        "server.telemetry-collect",
        "server.topology-scan",
        "server.assets-list",
        "server.topology-graph",
        "loganalysis.scan",
        "server.gadgetini-telemetry-collect",
    ] {
        assert!(
            gadgets.contains(required),
            "missing replacement Gadget {required}"
        );
    }

    let workspaces: BTreeSet<_> = capabilities["workspaces"]
        .as_array()
        .expect("Server Administrator workspaces")
        .iter()
        .map(|workspace| {
            workspace["id"]
                .as_str()
                .expect("Server Administrator workspace id")
        })
        .collect();
    assert_eq!(
        workspaces,
        BTreeSet::from([
            "alerts",
            "cooling",
            "fleet",
            "fleet-map",
            "logs",
            "metrics",
            "raw-telemetry",
            "servers",
            "topology",
        ])
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root must exist")
}

fn read_toml(path: &Path) -> toml::Value {
    toml::from_str(
        &fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()))
}

fn direct_dependencies(manifest: &toml::Value) -> BTreeSet<String> {
    let mut dependencies = BTreeSet::new();
    collect_dependency_tables(manifest, &mut dependencies);
    dependencies
}

fn collect_dependency_tables(value: &toml::Value, dependencies: &mut BTreeSet<String>) {
    let Some(table) = value.as_table() else {
        return;
    };
    for (key, child) in table {
        if matches!(
            key.as_str(),
            "dependencies" | "dev-dependencies" | "build-dependencies"
        ) {
            if let Some(entries) = child.as_table() {
                for (alias, specification) in entries {
                    let package = specification
                        .as_table()
                        .and_then(|table| table.get("package"))
                        .and_then(toml::Value::as_str)
                        .unwrap_or(alias);
                    dependencies.insert(package.to_owned());
                }
            }
        } else {
            collect_dependency_tables(child, dependencies);
        }
    }
}
