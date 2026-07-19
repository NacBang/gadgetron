#![cfg(target_os = "linux")]

use std::{
    fs,
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

use gadgetron_bundle_host::{BrokerCaller, BundleBroker, ValidatedPackageContract};
use gadgetron_bundle_sdk::{
    BrokerError, BrokerProbeResult, BrokerRequest, BrokerResponse, GadgetInvocation, GadgetName,
    InvocationContext, LocalId,
};
use gadgetron_bundle_supervisor::{BundleSupervisorError, LinuxSandboxSupervisor};
use semver::Version;
use sha2::{Digest, Sha256};

fn helper_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gadgetron-bundle-sandbox-init"))
}

fn fixture_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gadgetron-bundle-sandbox-fixture"))
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

fn stage_fixture(root: &Path) -> (PathBuf, String) {
    let package_root = root.join("package");
    let entry = package_root.join("bin/runtime");
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    fs::copy(fixture_binary(), &entry).unwrap();
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o500)).unwrap();
    let digest = hex::encode(Sha256::digest(fs::read(&entry).unwrap()));
    (package_root, digest)
}

fn package_contract(
    digest: &str,
    args: &[String],
    egress: Option<&str>,
) -> ValidatedPackageContract {
    let args = args
        .iter()
        .map(|arg| format!("\"{}\"", arg.replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let egress = egress
        .map(|destination| format!("\n[runtime.egress]\nallow = [\"{destination}\"]\n"))
        .unwrap_or_default();
    let source = format!(
        r#"
manifest_version = 1

[bundle]
id = "sandbox-fixture"
version = "{version}"
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
args = [{args}]

[runtime.limits]
memory_mb = 512
open_files = 64
cpu_seconds = 30
{egress}

[[permissions]]
id = "broker-probe"
kind = "database"
description = "Probe the sandbox broker transport"
resources = ["postgres:table:sandbox_broker"]

[capabilities]
gadget_namespaces = ["fixture"]

[[capabilities.gadgets]]
name = "fixture.probe"
description = "Probe the Linux isolation floor"
tier = "read"
input_schema = {{ type = "object" }}
output_schema = {{ type = "object" }}

[capabilities.gadgets.effect]
risk = "low"
idempotent = true
reversible = true
requires_evidence = false
"#,
        version = env!("CARGO_PKG_VERSION"),
    );
    ValidatedPackageContract::parse(&source, &Version::new(1, 0, 0)).unwrap()
}

#[derive(Debug)]
struct AllowSandboxProbe;

#[async_trait::async_trait]
impl BundleBroker for AllowSandboxProbe {
    async fn handle(&self, caller: &BrokerCaller, request: BrokerRequest) -> BrokerResponse {
        assert_eq!(caller.identity().id.as_str(), "sandbox-fixture");
        match request {
            BrokerRequest::Probe(request)
                if request.permission_id.as_str() == "broker-probe"
                    && request.resource.as_str() == "postgres:table:sandbox_broker" =>
            {
                BrokerResponse::Probe(BrokerProbeResult::ready(
                    request.permission_id,
                    request.resource,
                ))
            }
            _ => BrokerResponse::Error(BrokerError::new(
                LocalId::new("not-granted").unwrap(),
                "fixture broker request was not granted",
                false,
            )),
        }
    }
}

#[tokio::test]
async fn namespace_sandbox_enforces_filesystem_network_env_caps_and_limits() {
    if !user_namespace_available() {
        eprintln!("skipped: host does not permit an unprivileged user namespace");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (package_root, digest) = stage_fixture(temp.path());
    let state_root = temp.path().join("state");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host_probe = listener.local_addr().unwrap().to_string();
    let contract = package_contract(&digest, &[host_probe], None);
    let supervisor = LinuxSandboxSupervisor::from_trusted_helper_path(helper_binary()).unwrap();

    let mut runtime = supervisor
        .launch_and_probe_with_broker(
            &contract,
            &package_root,
            &state_root,
            Arc::new(AllowSandboxProbe),
        )
        .await
        .unwrap();
    let result = runtime
        .invoke_gadget(GadgetInvocation::new(
            GadgetName::new("fixture.probe").unwrap(),
            serde_json::json!({}),
            InvocationContext::new("tenant-1", "actor-1", "request-1"),
        ))
        .await
        .unwrap();
    for field in [
        "host_home_hidden",
        "parent_database_url_hidden",
        "runtime_path_minimal",
        "package_write_blocked",
        "host_usr_write_blocked",
        "state_write_succeeded",
        "host_network_blocked",
        "mount_blocked",
        "capabilities_empty",
        "pid_namespace_init",
        "broker_fd_is_fixed",
        "broker_channel_available",
        "broker_probe_ready",
    ] {
        assert_eq!(
            result.output[field],
            serde_json::Value::Bool(true),
            "isolation predicate {field} failed: {}",
            result.output
        );
    }
    assert_eq!(result.output["address_space_limit"], 512 * 1024 * 1024_u64);
    assert_eq!(result.output["open_file_limit"], 64_u64);
    assert_eq!(result.output["cpu_limit"], 30_u64);
    assert_eq!(
        fs::read(state_root.join("probe")).unwrap(),
        b"sandbox-state"
    );
    runtime.shutdown("test complete").await.unwrap();
}

#[tokio::test]
async fn entry_digest_egress_and_degraded_health_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let (package_root, digest) = stage_fixture(temp.path());
    let state_root = temp.path().join("state");
    let supervisor = LinuxSandboxSupervisor::from_trusted_helper_path(helper_binary()).unwrap();

    let wrong_digest = package_contract(&"0".repeat(64), &[], None);
    assert!(matches!(
        supervisor
            .launch_and_probe(&wrong_digest, &package_root, &state_root)
            .await,
        Err(BundleSupervisorError::EntryDigestMismatch { .. })
    ));

    let egress = package_contract(&digest, &[], Some("example.invalid:443"));
    assert!(matches!(
        supervisor
            .launch_and_probe(&egress, &package_root, &state_root)
            .await,
        Err(BundleSupervisorError::UnsupportedEgress)
    ));

    if !user_namespace_available() {
        eprintln!("skipped degraded-health probe: host blocks user namespaces");
        return;
    }

    let degraded = package_contract(&digest, &["unused".into(), "degraded".into()], None);
    assert!(matches!(
        supervisor
            .launch_and_probe(&degraded, &package_root, &state_root)
            .await,
        Err(BundleSupervisorError::HealthNotReady {
            status: gadgetron_bundle_sdk::HealthStatus::Degraded,
            ..
        })
    ));
}
