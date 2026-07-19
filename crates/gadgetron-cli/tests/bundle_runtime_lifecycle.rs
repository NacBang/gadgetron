#![cfg(target_os = "linux")]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
};

use ed25519_dalek::{Signer, SigningKey};
use gadgetron_bundle_sdk::InvocationContext;
use gadgetron_bundle_supervisor::LinuxSandboxSupervisor;
use gadgetron_core::{
    agent::tools::{GadgetCatalog, GadgetDispatchContext, GadgetDispatcher},
    config::BundleSigningConfig,
};
use gadgetron_gateway::web::bundle_runtime::{BundleRuntimeManager, BundleRuntimeState};
use sha2::{Digest, Sha256};

const RUNTIME: &str = r#"#!/usr/bin/python3
import json, os, pathlib, sys

digest = os.environ["GADGETRON_BUNDLE_MANIFEST_SHA256"]
identity = {"id": "lifecycle-fixture", "version": "1.0.0"}
counter = pathlib.Path("/data/boot-count")
try:
    boots = int(counter.read_text()) + 1
except Exception:
    boots = 1
counter.write_text(str(boots))
handshaken = False

for line in sys.stdin:
    request = json.loads(line)
    method = request["payload"]["method"]
    params = request["payload"].get("params", {})
    stop = False
    if method == "handshake" and params.get("package_manifest_sha256") == digest:
        handshaken = True
        payload = {"result": "handshake", "data": {
            "package_manifest_sha256": digest, "selected_protocol": 1}}
    elif not handshaken:
        payload = {"result": "error", "data": {
            "code": "handshake-required", "message": "handshake required",
            "retryable": False}}
    elif method == "health":
        payload = {"result": "health", "data": {"status": "healthy"}}
    elif method == "invoke_gadget" and (params.get("input") or {}).get("crash"):
        sys.exit(23)
    elif method == "invoke_gadget" and (params.get("input") or {}).get("fail"):
        payload = {"result": "error", "data": {
            "code": "fixture-failed", "message": "fixture request rejected",
            "retryable": False}}
    elif method == "invoke_gadget":
        context = dict(params.get("context") or {})
        broker_lease_present = "broker_lease" in context
        context.pop("broker_lease", None)
        payload = {"result": "gadget_result", "data": {
            "output": {"input": params.get("input"), "context": context,
                       "broker_lease_present": broker_lease_present},
            "evidence": [], "candidates": [], "outcomes": []}}
    elif method == "start_job":
        payload = {"result": "job_accepted", "data": {"job_id": "fixture-job-1"}}
    elif method == "poll_job":
        payload = {"result": "job_status", "data": {
            "job_id": params.get("job_id"), "status": "succeeded"}}
    elif method == "cancel_job":
        payload = {"result": "job_status", "data": {
            "job_id": params.get("job_id"), "status": "cancelled"}}
    elif method == "shutdown":
        payload = {"result": "acknowledgement", "data": {"message": "stopping"}}
        stop = True
    else:
        payload = {"result": "error", "data": {
            "code": "unsupported-request", "message": "unsupported",
            "retryable": False}}
    response = {"protocol_version": 1, "message_id": request["message_id"],
                "bundle": identity, "payload": payload}
    sys.stdout.write(json.dumps(response, separators=(",", ":")) + "\n")
    sys.stdout.flush()
    if stop:
        break
"#;

fn sign(sk: &SigningKey, source: &str) -> String {
    hex::encode(sk.sign(source.as_bytes()).to_bytes())
}

fn user_namespace_available() -> bool {
    Command::new("/usr/bin/unshare")
        .args(["--user", "--map-root-user", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn stage_signed_package(root: &Path, sk: &SigningKey) {
    stage_signed_runtime_package(root, sk, "lifecycle-fixture", RUNTIME);
}

fn stage_signed_runtime_package(root: &Path, sk: &SigningKey, id: &str, runtime: &str) {
    let package_root = root.join("packages").join(id);
    let runtime_path = package_root.join("bin/runtime");
    fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
    fs::write(&runtime_path, runtime).unwrap();
    fs::set_permissions(&runtime_path, fs::Permissions::from_mode(0o500)).unwrap();
    let digest = hex::encode(Sha256::digest(runtime.as_bytes()));
    let catalog = format!(
        r#"[bundle]
id = "{id}"
version = "1.0.0"
allow_direct_actions = false
"#
    );
    let package = format!(
        r#"manifest_version = 1

[bundle]
id = "{id}"
version = "1.0.0"
publisher = "gadgetron.tests"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/runtime"
entry_sha256 = "{digest}"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 30

[[permissions]]
id = "telemetry-read"
kind = "database"
description = "Fixture permission grant lifecycle"
resources = ["postgres:table:fixture_telemetry"]

[capabilities]
gadget_namespaces = ["fixture"]

[[capabilities.gadgets]]
name = "fixture.echo"
description = "Echo a bounded invocation for lifecycle verification"
tier = "read"
input_schema = {{ type = "object" }}
output_schema = {{ type = "object" }}

[capabilities.gadgets.effect]
risk = "low"
idempotent = true
reversible = true
requires_evidence = false

[[capabilities.jobs]]
id = "fixture-job"
role = "operator"
triggers = ["on_demand"]
gadget_allowlist = ["fixture.echo"]
"#
    );
    fs::write(package_root.join("bundle.toml"), &catalog).unwrap();
    fs::write(package_root.join("package.toml"), &package).unwrap();
    fs::write(package_root.join("catalog.sig"), sign(sk, &catalog)).unwrap();
    fs::write(package_root.join("package.sig"), sign(sk, &package)).unwrap();
}

fn stage_signed_migration_package(root: &Path, sk: &SigningKey) {
    let package_root = root.join("packages/migration-fixture");
    let runtime_path = package_root.join("bin/runtime");
    let migration_path = package_root.join("migrations/1_fixture.sql");
    fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
    fs::create_dir_all(migration_path.parent().unwrap()).unwrap();
    fs::write(&runtime_path, RUNTIME).unwrap();
    fs::set_permissions(&runtime_path, fs::Permissions::from_mode(0o500)).unwrap();
    let migration = b"SELECT 1;\n";
    fs::write(&migration_path, migration).unwrap();
    let runtime_digest = hex::encode(Sha256::digest(RUNTIME.as_bytes()));
    let migration_digest = hex::encode(Sha256::digest(migration));
    let catalog = r#"[bundle]
id = "migration-fixture"
version = "1.0.0"
allow_direct_actions = false
"#;
    let package = format!(
        r#"manifest_version = 1

[bundle]
id = "migration-fixture"
version = "1.0.0"
publisher = "gadgetron.tests"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/runtime"
entry_sha256 = "{runtime_digest}"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 30

[[capabilities.migrations]]
id = "fixture-schema"
revision = 1
kind = "schema"
path = "migrations/1_fixture.sql"
sha256 = "{migration_digest}"
"#
    );
    fs::write(package_root.join("bundle.toml"), catalog).unwrap();
    fs::write(package_root.join("package.toml"), &package).unwrap();
    fs::write(package_root.join("catalog.sig"), sign(sk, catalog)).unwrap();
    fs::write(package_root.join("package.sig"), sign(sk, &package)).unwrap();
}

fn stage_signed_set_package(
    root: &Path,
    sk: &SigningKey,
    id: &str,
    degraded: bool,
    provides: Option<&str>,
    requires: Option<&str>,
    settings: bool,
) -> String {
    let package_root = root.join("packages").join(id);
    let runtime_path = package_root.join("bin/runtime");
    fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
    let mut runtime = RUNTIME.replace("lifecycle-fixture", id);
    if degraded {
        runtime = runtime.replace(
            r#"{"result": "health", "data": {"status": "healthy"}}"#,
            r#"{"result": "health", "data": {"status": "degraded"}}"#,
        );
    }
    fs::write(&runtime_path, &runtime).unwrap();
    fs::set_permissions(&runtime_path, fs::Permissions::from_mode(0o500)).unwrap();
    let runtime_digest = hex::encode(Sha256::digest(runtime.as_bytes()));
    let catalog = format!(
        r#"[bundle]
id = "{id}"
version = "1.0.0"
allow_direct_actions = false
"#
    );
    let dependency = requires.map_or_else(String::new, |capability| {
        format!(
            r#"
[[dependencies.requires]]
capability = "{capability}"
version = "^1.0"
feature = "required-provider"
reason = "Bundle Set lifecycle fixture"
"#,
        )
    });
    let provided = provides.map_or_else(String::new, |capability| {
        format!(
            r#"
[[capabilities.provides]]
id = "{capability}"
version = "1.0.0"
description = "Bundle Set lifecycle fixture capability"
"#,
        )
    });
    let settings_schema = if settings {
        r#"
[capabilities.settings_schema]
type = "object"
additionalProperties = false
required = ["region"]

[capabilities.settings_schema.properties.region]
type = "string"
title = "Region"
"#
    } else {
        ""
    };
    let package = format!(
        r#"manifest_version = 3

[bundle]
id = "{id}"
version = "1.0.0"
class = "operational"
publisher = "gadgetron.tests"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.8.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1
{dependency}
[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/runtime"
entry_sha256 = "{runtime_digest}"

[runtime.limits]
memory_mb = 128
open_files = 32
cpu_seconds = 10

[capabilities]
gadget_namespaces = []
{provided}{settings_schema}"#,
    );
    fs::write(package_root.join("bundle.toml"), &catalog).unwrap();
    fs::write(package_root.join("package.toml"), &package).unwrap();
    fs::write(package_root.join("catalog.sig"), sign(sk, &catalog)).unwrap();
    fs::write(package_root.join("package.sig"), sign(sk, &package)).unwrap();
    hex::encode(Sha256::digest(package.as_bytes()))
}

fn stage_signed_dependency_package(
    root: &Path,
    sk: &SigningKey,
    id: &str,
    gadget_name: &str,
    provides: Option<&str>,
    dependency: Option<(&str, &str)>,
) {
    let package_root = root.join("packages").join(id);
    let runtime_path = package_root.join("bin/runtime");
    fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
    let runtime = RUNTIME.replace("lifecycle-fixture", id);
    fs::write(&runtime_path, &runtime).unwrap();
    fs::set_permissions(&runtime_path, fs::Permissions::from_mode(0o500)).unwrap();
    let runtime_digest = hex::encode(Sha256::digest(runtime.as_bytes()));
    let catalog = format!(
        r#"[bundle]
id = "{id}"
version = "1.0.0"
allow_direct_actions = false
"#
    );
    let dependency = dependency.map_or_else(String::new, |(relation, capability)| {
        format!(
            r#"
[[dependencies.{relation}]]
capability = "{capability}"
version = "^1.0"
feature = "runtime-resilience"
reason = "Runtime dependency failure fixture"
"#,
        )
    });
    let provided = provides.map_or_else(String::new, |capability| {
        format!(
            r#"
[[capabilities.provides]]
id = "{capability}"
version = "1.0.0"
description = "Runtime dependency failure fixture capability"
"#,
        )
    });
    let package = format!(
        r#"manifest_version = 3

[bundle]
id = "{id}"
version = "1.0.0"
class = "operational"
publisher = "gadgetron.tests"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.8.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1
{dependency}
[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/runtime"
entry_sha256 = "{runtime_digest}"

[runtime.limits]
memory_mb = 128
open_files = 32
cpu_seconds = 10

[capabilities]
gadget_namespaces = ["resilience"]
{provided}
[[capabilities.gadgets]]
name = "{gadget_name}"
description = "Runtime dependency failure probe"
tier = "read"
input_schema = {{ type = "object" }}
output_schema = {{ type = "object" }}

[capabilities.gadgets.effect]
risk = "low"
idempotent = true
reversible = true
requires_evidence = false
"#,
    );
    fs::write(package_root.join("bundle.toml"), &catalog).unwrap();
    fs::write(package_root.join("package.toml"), &package).unwrap();
    fs::write(package_root.join("catalog.sig"), sign(sk, &catalog)).unwrap();
    fs::write(package_root.join("package.sig"), sign(sk, &package)).unwrap();
}

fn bundle_set_source(packages: &[(&str, &str, Option<&str>)]) -> String {
    let mut source = String::from(
        r#"set_manifest_version = 1

[set]
id = "lifecycle-set"
version = "1.0.0"
publisher = "gadgetron.tests"
"#,
    );
    for (bundle_id, digest, region) in packages {
        source.push_str(&format!(
            r#"
[[packages]]
bundle_id = "{bundle_id}"
version = "1.0.0"
package_manifest_sha256 = "{digest}"
"#,
        ));
        if let Some(region) = region {
            source.push_str(&format!("settings = {{ region = \"{region}\" }}\n"));
        }
    }
    source
}

#[tokio::test]
async fn signed_bundle_enable_disable_reenable_preserves_state_and_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let signing_key = SigningKey::from_bytes(&[29_u8; 32]);
    stage_signed_package(temp.path(), &signing_key);
    let config = BundleSigningConfig {
        public_keys_hex: vec![hex::encode(signing_key.verifying_key().as_bytes())],
        require_signature: true,
    };
    let supervisor =
        LinuxSandboxSupervisor::from_trusted_helper_path(env!("CARGO_BIN_EXE_gadgetron")).unwrap();
    let manager = Arc::new(
        BundleRuntimeManager::new(
            supervisor,
            temp.path().join("packages"),
            temp.path().join("state"),
            config,
        )
        .unwrap(),
    );

    assert!(manager.all_schemas().is_empty());

    let package_source =
        fs::read_to_string(temp.path().join("packages/lifecycle-fixture/package.toml")).unwrap();
    let package_digest = hex::encode(Sha256::digest(package_source.as_bytes()));
    assert!(manager
        .grant_permissions(
            "lifecycle-fixture",
            &"0".repeat(64),
            vec!["telemetry-read".into()],
        )
        .await
        .is_err());
    let grant = manager
        .grant_permissions(
            "lifecycle-fixture",
            &package_digest,
            vec!["telemetry-read".into()],
        )
        .await
        .unwrap();
    assert_eq!(grant.package_manifest_sha256, package_digest);
    assert!(temp
        .path()
        .join("state/.core-grants/lifecycle-fixture.json")
        .is_file());

    if !user_namespace_available() {
        eprintln!("skipped runtime lifecycle: host blocks user namespaces");
        return;
    }

    let enabled = manager.enable("lifecycle-fixture").await.unwrap();
    assert_eq!(enabled.state, BundleRuntimeState::Enabled);
    assert_eq!(
        manager
            .all_schemas()
            .into_iter()
            .map(|schema| schema.name)
            .collect::<Vec<_>>(),
        vec!["fixture.echo"]
    );
    assert!(manager
        .dispatch_gadget("fixture.echo", serde_json::json!({}))
        .await
        .is_err());
    let echo = manager
        .dispatch_gadget_with_context(
            GadgetDispatchContext::new("tenant-live", "actor-live", "request-live")
                .with_scopes(["management".to_string()]),
            "fixture.echo",
            serde_json::json!({"probe": true}),
        )
        .await
        .unwrap();
    let echoed = echo.content[0]["text"].as_str().unwrap();
    assert!(echoed.contains("tenant-live"));
    assert!(echoed.contains("request-live"));
    assert!(echoed.contains(r#""broker_lease_present":true"#));
    let request_error = manager
        .dispatch_gadget_with_context(
            GadgetDispatchContext::new("tenant-live", "actor-live", "request-error")
                .with_scopes(["management".to_string()]),
            "fixture.echo",
            serde_json::json!({"fail": true}),
        )
        .await
        .unwrap_err();
    assert!(request_error.to_string().contains("fixture-failed"));
    assert_eq!(
        manager.status("lifecycle-fixture").await.unwrap().state,
        BundleRuntimeState::Enabled,
        "a valid correlated HostError is invocation-local, not a runtime failure"
    );
    assert_eq!(manager.all_schemas().len(), 1);
    assert_eq!(
        fs::read_to_string(temp.path().join("state/lifecycle-fixture/boot-count")).unwrap(),
        "1"
    );

    assert!(manager
        .revoke_permissions("lifecycle-fixture")
        .await
        .unwrap());
    assert_eq!(
        manager.status("lifecycle-fixture").await.unwrap().state,
        BundleRuntimeState::Disabled
    );
    assert!(!temp
        .path()
        .join("state/.core-grants/lifecycle-fixture.json")
        .exists());
    assert!(manager.all_schemas().is_empty());
    assert!(temp
        .path()
        .join("state/lifecycle-fixture/boot-count")
        .is_file());

    let reenabled = manager.enable("lifecycle-fixture").await.unwrap();
    assert_eq!(reenabled.state, BundleRuntimeState::Enabled);
    assert_eq!(
        fs::read_to_string(temp.path().join("state/lifecycle-fixture/boot-count")).unwrap(),
        "2"
    );
    manager.disable("lifecycle-fixture").await.unwrap();
    assert!(manager.all_schemas().is_empty());

    fs::remove_file(temp.path().join("packages/lifecycle-fixture/package.sig")).unwrap();
    assert!(manager.enable("lifecycle-fixture").await.is_err());
    assert_eq!(
        manager.status("lifecycle-fixture").await.unwrap().state,
        BundleRuntimeState::Disabled
    );

    stage_signed_migration_package(temp.path(), &signing_key);
    let error = manager.enable("migration-fixture").await.unwrap_err();
    assert!(format!("{error:?}").contains("no transactional Bundle migration manager"));
    assert_eq!(
        manager.status("migration-fixture").await.unwrap().state,
        BundleRuntimeState::Failed
    );

    let slow_runtime = RUNTIME
        .replace("lifecycle-fixture", "slow-fixture")
        .replace(
            "import json, os, pathlib, sys",
            "import json, os, pathlib, sys, time\ntime.sleep(0.5)",
        );
    stage_signed_runtime_package(temp.path(), &signing_key, "slow-fixture", &slow_runtime);
    let enable_manager = manager.clone();
    let enabling = tokio::spawn(async move { enable_manager.enable("slow-fixture").await });
    for _ in 0..100 {
        if manager.status("slow-fixture").await.unwrap().state == BundleRuntimeState::Probing {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        manager.status("slow-fixture").await.unwrap().state,
        BundleRuntimeState::Probing
    );
    assert_eq!(
        manager.disable("slow-fixture").await.unwrap().state,
        BundleRuntimeState::Disabled
    );
    let error = enabling.await.unwrap().unwrap_err();
    assert!(format!("{error:?}").contains("superseded"));
    assert_eq!(
        manager.status("slow-fixture").await.unwrap().state,
        BundleRuntimeState::Disabled
    );

    stage_signed_runtime_package(temp.path(), &signing_key, "collision-fixture", RUNTIME);
    let collision_manager = BundleRuntimeManager::new_with_migrations_and_reserved(
        LinuxSandboxSupervisor::from_trusted_helper_path(env!("CARGO_BIN_EXE_gadgetron")).unwrap(),
        temp.path().join("packages"),
        temp.path().join("collision-state"),
        BundleSigningConfig {
            public_keys_hex: vec![hex::encode(signing_key.verifying_key().as_bytes())],
            require_signature: true,
        },
        None,
        ["fixture.echo".to_string()],
    )
    .unwrap();
    let collision = collision_manager
        .enable("collision-fixture")
        .await
        .unwrap_err();
    assert!(format!("{collision:?}").contains("collides with a Core capability"));
    assert!(collision_manager.all_schemas().is_empty());
}

#[tokio::test]
async fn runtime_failure_safe_stops_required_dependents_and_preserves_optional_ones() {
    if !user_namespace_available() {
        eprintln!("skipped dependency failure lifecycle: host blocks user namespaces");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let signing_key = SigningKey::from_bytes(&[43_u8; 32]);
    let capability = "gadgetron.fixture.resilience-provider";
    stage_signed_dependency_package(
        temp.path(),
        &signing_key,
        "resilience-provider",
        "resilience.provider",
        Some(capability),
        None,
    );
    stage_signed_dependency_package(
        temp.path(),
        &signing_key,
        "resilience-required",
        "resilience.required",
        None,
        Some(("requires", capability)),
    );
    stage_signed_dependency_package(
        temp.path(),
        &signing_key,
        "resilience-optional",
        "resilience.optional",
        None,
        Some(("optional", capability)),
    );
    let manager = BundleRuntimeManager::new(
        LinuxSandboxSupervisor::from_trusted_helper_path(env!("CARGO_BIN_EXE_gadgetron")).unwrap(),
        temp.path().join("packages"),
        temp.path().join("state"),
        BundleSigningConfig {
            public_keys_hex: vec![hex::encode(signing_key.verifying_key().as_bytes())],
            require_signature: true,
        },
    )
    .unwrap();

    for bundle_id in [
        "resilience-provider",
        "resilience-required",
        "resilience-optional",
    ] {
        manager.enable(bundle_id).await.unwrap();
    }
    let failure = manager
        .dispatch_gadget_with_context(
            GadgetDispatchContext::new("tenant-live", "actor-live", "request-crash")
                .with_scopes(["management".to_string()]),
            "resilience.provider",
            serde_json::json!({"crash": true}),
        )
        .await
        .unwrap_err();
    assert!(failure.to_string().contains("runtime failed"));
    assert_eq!(
        manager.status("resilience-provider").await.unwrap().state,
        BundleRuntimeState::Failed
    );
    let required = manager.status("resilience-required").await.unwrap();
    assert_eq!(required.state, BundleRuntimeState::Failed);
    assert!(required
        .detail
        .as_deref()
        .unwrap()
        .contains("required dependency became unavailable"));
    assert_eq!(
        manager.status("resilience-optional").await.unwrap().state,
        BundleRuntimeState::Enabled
    );
    assert_eq!(
        manager
            .all_schemas()
            .into_iter()
            .map(|schema| schema.name)
            .collect::<Vec<_>>(),
        ["resilience.optional"]
    );

    assert!(manager.enable("resilience-required").await.is_err());
    manager.enable("resilience-provider").await.unwrap();
    manager.enable("resilience-required").await.unwrap();
    assert_eq!(
        fs::read_to_string(temp.path().join("state/resilience-provider/boot-count")).unwrap(),
        "2"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("state/resilience-required/boot-count")).unwrap(),
        "2"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("state/resilience-optional/boot-count")).unwrap(),
        "1"
    );

    drop(manager);
    let manager = BundleRuntimeManager::new(
        LinuxSandboxSupervisor::from_trusted_helper_path(env!("CARGO_BIN_EXE_gadgetron")).unwrap(),
        temp.path().join("packages"),
        temp.path().join("state"),
        BundleSigningConfig {
            public_keys_hex: vec![hex::encode(signing_key.verifying_key().as_bytes())],
            require_signature: true,
        },
    )
    .unwrap();
    manager.restore_runtime_intents().await;
    for bundle_id in [
        "resilience-provider",
        "resilience-required",
        "resilience-optional",
    ] {
        assert_eq!(
            manager.status(bundle_id).await.unwrap().state,
            BundleRuntimeState::Enabled
        );
    }
    assert_eq!(
        fs::read_to_string(temp.path().join("state/resilience-provider/boot-count")).unwrap(),
        "3"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("state/resilience-required/boot-count")).unwrap(),
        "3"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("state/resilience-optional/boot-count")).unwrap(),
        "2"
    );

    drop(manager);
    fs::write(
        temp.path()
            .join("state/resilience-provider/.gadgetron-runtime-intent.json"),
        format!(
            "{{\"format_version\":1,\"package_manifest_sha256\":\"{}\",\"updated_at_ms\":0}}",
            "0".repeat(64)
        ),
    )
    .unwrap();
    let manager = BundleRuntimeManager::new(
        LinuxSandboxSupervisor::from_trusted_helper_path(env!("CARGO_BIN_EXE_gadgetron")).unwrap(),
        temp.path().join("packages"),
        temp.path().join("state"),
        BundleSigningConfig {
            public_keys_hex: vec![hex::encode(signing_key.verifying_key().as_bytes())],
            require_signature: true,
        },
    )
    .unwrap();
    manager.restore_runtime_intents().await;
    assert_eq!(
        manager.status("resilience-provider").await.unwrap().state,
        BundleRuntimeState::InstalledNotEnabled
    );
    let required = manager.status("resilience-required").await.unwrap();
    assert_eq!(required.state, BundleRuntimeState::Failed);
    assert!(required
        .detail
        .as_deref()
        .unwrap()
        .contains("required dependency is unavailable"));
    assert_eq!(
        manager.status("resilience-optional").await.unwrap().state,
        BundleRuntimeState::Enabled
    );
    assert_eq!(
        manager
            .all_schemas()
            .into_iter()
            .map(|schema| schema.name)
            .collect::<Vec<_>>(),
        ["resilience.optional"]
    );

    manager.enable("resilience-provider").await.unwrap();
    manager.enable("resilience-required").await.unwrap();
    manager.disable("resilience-required").await.unwrap();
    manager.disable("resilience-optional").await.unwrap();
    manager.disable("resilience-provider").await.unwrap();
}

#[tokio::test]
async fn duplicate_job_id_safe_stops_runtime_and_releases_tracking() {
    if !user_namespace_available() {
        eprintln!("skipped duplicate job lifecycle: host blocks user namespaces");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let signing_key = SigningKey::from_bytes(&[47_u8; 32]);
    stage_signed_package(temp.path(), &signing_key);
    let manager = BundleRuntimeManager::new(
        LinuxSandboxSupervisor::from_trusted_helper_path(env!("CARGO_BIN_EXE_gadgetron")).unwrap(),
        temp.path().join("packages"),
        temp.path().join("state"),
        BundleSigningConfig {
            public_keys_hex: vec![hex::encode(signing_key.verifying_key().as_bytes())],
            require_signature: true,
        },
    )
    .unwrap();
    let package_source =
        fs::read_to_string(temp.path().join("packages/lifecycle-fixture/package.toml")).unwrap();
    let package_digest = hex::encode(Sha256::digest(package_source.as_bytes()));
    manager
        .grant_permissions(
            "lifecycle-fixture",
            &package_digest,
            vec!["telemetry-read".into()],
        )
        .await
        .unwrap();
    manager.enable("lifecycle-fixture").await.unwrap();

    let context = |request: &str| {
        InvocationContext::new("tenant-live", "actor-live", request)
            .with_scopes(["management".to_string()])
    };
    let first = manager
        .start_job(
            "lifecycle-fixture",
            "fixture-job",
            serde_json::json!({"target_id": "target-one"}),
            context("request-job-one"),
        )
        .await
        .unwrap();
    let duplicate = manager
        .start_job(
            "lifecycle-fixture",
            "fixture-job",
            serde_json::json!({"target_id": "target-two"}),
            context("request-job-two"),
        )
        .await
        .unwrap_err();
    assert!(format!("{duplicate:?}").contains("duplicate active job id"));
    let failed = manager.status("lifecycle-fixture").await.unwrap();
    assert_eq!(failed.state, BundleRuntimeState::Failed);
    assert!(failed
        .detail
        .as_deref()
        .unwrap()
        .contains("duplicate active job id"));
    assert!(manager.all_schemas().is_empty());

    manager.enable("lifecycle-fixture").await.unwrap();
    let recovered = manager
        .start_job(
            "lifecycle-fixture",
            "fixture-job",
            serde_json::json!({"target_id": "target-one"}),
            context("request-job-recovered"),
        )
        .await
        .unwrap();
    assert_eq!(recovered.job_id, first.job_id);
    manager
        .cancel_job(
            "lifecycle-fixture",
            &recovered.job_id,
            Some("fixture complete".into()),
        )
        .await
        .unwrap();
    manager.disable("lifecycle-fixture").await.unwrap();
}

#[tokio::test]
async fn bundle_set_applies_settings_and_activates_in_required_order() {
    if !user_namespace_available() {
        eprintln!("skipped Bundle Set apply: host blocks user namespaces");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let signing_key = SigningKey::from_bytes(&[31_u8; 32]);
    let root_digest = stage_signed_set_package(
        temp.path(),
        &signing_key,
        "set-root",
        false,
        Some("gadgetron.fixture.root"),
        None,
        false,
    );
    let provider_digest = stage_signed_set_package(
        temp.path(),
        &signing_key,
        "set-provider",
        false,
        Some("gadgetron.fixture.provider"),
        Some("gadgetron.fixture.root"),
        true,
    );
    let consumer_digest = stage_signed_set_package(
        temp.path(),
        &signing_key,
        "set-consumer",
        false,
        None,
        Some("gadgetron.fixture.provider"),
        false,
    );
    let manager = BundleRuntimeManager::new(
        LinuxSandboxSupervisor::from_trusted_helper_path(env!("CARGO_BIN_EXE_gadgetron")).unwrap(),
        temp.path().join("packages"),
        temp.path().join("state"),
        BundleSigningConfig {
            public_keys_hex: vec![hex::encode(signing_key.verifying_key().as_bytes())],
            require_signature: true,
        },
    )
    .unwrap();
    let set = bundle_set_source(&[
        ("set-consumer", &consumer_digest, None),
        ("set-provider", &provider_digest, Some("ap-northeast-2")),
        ("set-root", &root_digest, None),
    ]);

    let outcome = manager.apply_bundle_set(&set).await.unwrap();
    assert_eq!(
        outcome.state,
        gadgetron_gateway::web::bundle_runtime::BundleSetApplyState::Applied
    );
    assert_eq!(
        outcome.enabled_bundle_ids,
        ["set-root", "set-provider", "set-consumer"]
    );
    assert_eq!(outcome.settings_updated_bundle_ids, ["set-provider"]);
    assert_eq!(
        manager.settings("set-provider").unwrap().values["region"],
        "ap-northeast-2"
    );
    for bundle_id in ["set-root", "set-provider", "set-consumer"] {
        assert_eq!(
            manager.status(bundle_id).await.unwrap().state,
            BundleRuntimeState::Enabled
        );
    }

    manager.disable("set-consumer").await.unwrap();
    manager.disable("set-provider").await.unwrap();
    manager.disable("set-root").await.unwrap();
}

#[tokio::test]
async fn bundle_set_failure_rolls_back_only_new_activation_and_settings() {
    if !user_namespace_available() {
        eprintln!("skipped Bundle Set rollback: host blocks user namespaces");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let signing_key = SigningKey::from_bytes(&[37_u8; 32]);
    let root_digest = stage_signed_set_package(
        temp.path(),
        &signing_key,
        "set-existing",
        false,
        Some("gadgetron.fixture.root"),
        None,
        false,
    );
    let provider_digest = stage_signed_set_package(
        temp.path(),
        &signing_key,
        "set-provider",
        false,
        Some("gadgetron.fixture.provider"),
        Some("gadgetron.fixture.root"),
        true,
    );
    let consumer_digest = stage_signed_set_package(
        temp.path(),
        &signing_key,
        "set-failing",
        true,
        None,
        Some("gadgetron.fixture.provider"),
        false,
    );
    let manager = BundleRuntimeManager::new(
        LinuxSandboxSupervisor::from_trusted_helper_path(env!("CARGO_BIN_EXE_gadgetron")).unwrap(),
        temp.path().join("packages"),
        temp.path().join("state"),
        BundleSigningConfig {
            public_keys_hex: vec![hex::encode(signing_key.verifying_key().as_bytes())],
            require_signature: true,
        },
    )
    .unwrap();
    manager.enable("set-existing").await.unwrap();
    let set = bundle_set_source(&[
        ("set-failing", &consumer_digest, None),
        ("set-provider", &provider_digest, Some("ap-northeast-2")),
        ("set-existing", &root_digest, None),
    ]);

    let outcome = manager.apply_bundle_set(&set).await.unwrap();
    assert_eq!(
        outcome.state,
        gadgetron_gateway::web::bundle_runtime::BundleSetApplyState::RolledBack
    );
    assert_eq!(outcome.previously_enabled_bundle_ids, ["set-existing"]);
    assert_eq!(outcome.enabled_bundle_ids, ["set-provider"]);
    assert_eq!(outcome.rolled_back_bundle_ids, ["set-provider"]);
    assert_eq!(outcome.settings_restored_bundle_ids, ["set-provider"]);
    assert!(outcome
        .failure
        .as_deref()
        .unwrap()
        .contains("activation failed"));
    assert!(outcome.rollback_failures.is_empty());
    assert_eq!(
        manager.status("set-existing").await.unwrap().state,
        BundleRuntimeState::Enabled
    );
    assert_eq!(
        manager.status("set-provider").await.unwrap().state,
        BundleRuntimeState::Disabled
    );
    assert_eq!(manager.settings("set-provider").unwrap().revision, None);
    assert_eq!(
        manager.settings("set-provider").unwrap().values,
        serde_json::json!({})
    );
    assert!(temp.path().join("state/set-provider/boot-count").is_file());

    manager.disable("set-existing").await.unwrap();
}
