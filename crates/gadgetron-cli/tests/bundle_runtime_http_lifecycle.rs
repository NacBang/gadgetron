#![cfg(target_os = "linux")]

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

const API_KEY: &str = "gad_test_abcdefghijklmnop1234567890123456";
const PACKAGE_ASSET: &[u8] = b"signed fixture asset\n";
const RUNTIME: &str = r#"#!/usr/bin/python3
import json, os, pathlib, sys
digest = os.environ["GADGETRON_BUNDLE_MANIFEST_SHA256"]
identity = {"id": "http-lifecycle", "version": "1.0.0"}
counter = pathlib.Path("/data/boot-count")
try: boots = int(counter.read_text()) + 1
except Exception: boots = 1
counter.write_text(str(boots))
handshaken = False
for line in sys.stdin:
    request = json.loads(line)
    method = request["payload"]["method"]
    params = request["payload"].get("params", {})
    stop = False
    if method == "handshake" and params.get("package_manifest_sha256") == digest:
        handshaken = True
        payload = {"result":"handshake","data":{"package_manifest_sha256":digest,"selected_protocol":1}}
    elif not handshaken:
        payload = {"result":"error","data":{"code":"handshake-required","message":"handshake required","retryable":False}}
    elif method == "health":
        payload = {"result":"health","data":{"status":"healthy"}}
    elif method == "invoke_gadget":
        context = dict(params.get("context", {}))
        context.pop("broker_lease", None)
        payload = {"result":"gadget_result","data":{
            "output":{"input":params.get("input"),"context":context},
            "evidence":[],"candidates":[],"outcomes":[]}}
    elif method == "shutdown":
        payload = {"result":"acknowledgement","data":{"message":"stopping"}}
        stop = True
    else:
        payload = {"result":"error","data":{"code":"unsupported-request","message":"unsupported","retryable":False}}
    response = {"protocol_version":1,"message_id":request["message_id"],"bundle":identity,"payload":payload}
    sys.stdout.write(json.dumps(response,separators=(",",":"))+"\n")
    sys.stdout.flush()
    if stop: break
"#;

struct ChildGuard(Child);

impl ChildGuard {
    fn terminate(&mut self) {
        if self.0.try_wait().unwrap().is_some() {
            return;
        }
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct ForwardingMcpClient {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _guard: ChildGuard,
}

impl ForwardingMcpClient {
    fn spawn(config_path: &Path, base_url: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_gadgetron"))
            .args(["gadget", "serve", "--config", config_path.to_str().unwrap()])
            .env("GADGETRON_GATEWAY_CALLBACK_URL", base_url)
            .env("GADGETRON_GATEWAY_CALLBACK_KEY", API_KEY)
            .env_remove("GADGETRON_DATABASE_URL")
            .env_remove("GADGETRON_CONFIG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            stdin,
            stdout,
            _guard: ChildGuard(child),
        }
    }

    fn request(&mut self, id: u64, method: &str, params: serde_json::Value) -> serde_json::Value {
        writeln!(
            self.stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            })
        )
        .unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        assert_ne!(self.stdout.read_line(&mut line).unwrap(), 0);
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], id);
        response
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
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

fn spawn_gateway(config_path: &Path, port: u16, log: &fs::File) -> ChildGuard {
    ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_gadgetron"))
            .args([
                "serve",
                "--no-db",
                "--config",
                config_path.to_str().unwrap(),
                "--bind",
                &format!("127.0.0.1:{port}"),
            ])
            .env_remove("GADGETRON_DATABASE_URL")
            .env_remove("GADGETRON_CONFIG")
            .env_remove("GADGETRON_BIND")
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log.try_clone().unwrap()))
            .spawn()
            .unwrap(),
    )
}

fn sign(key: &SigningKey, source: &str) -> String {
    hex::encode(key.sign(source.as_bytes()).to_bytes())
}

fn package_sources(id: &str, runtime: &[u8]) -> (String, String) {
    let catalog =
        format!("[bundle]\nid = \"{id}\"\nversion = \"1.0.0\"\nallow_direct_actions = true\n");
    let digest = hex::encode(Sha256::digest(runtime));
    let asset_digest = hex::encode(Sha256::digest(PACKAGE_ASSET));
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

[capabilities]
gadget_namespaces = ["http"]

[[capabilities.gadgets]]
name = "http.echo"
description = "Echo an authenticated HTTP lifecycle invocation"
tier = "read"
input_schema = {{ type = "object" }}
output_schema = {{ type = "object" }}

[capabilities.gadgets.effect]
risk = "low"
idempotent = true
reversible = true
requires_evidence = false

[[capabilities.workspaces]]
id = "echo"
label = "HTTP Echo"
renderer = "table"
data_capability = "http.echo"
action_gadgets = ["http.echo"]

[[capabilities.seed_assets]]
id = "fixture-asset"
path = "assets/fixture.txt"
media_type = "text/plain"
sha256 = "{asset_digest}"
"#
    );
    (catalog, package)
}

async fn wait_for_health(client: &reqwest::Client, base: &str, log: &Path) {
    for _ in 0..100 {
        if client
            .get(format!("{base}/health"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "test gateway did not become healthy:\n{}",
        fs::read_to_string(log).unwrap_or_default()
    );
}

async fn wait_for_bundle_state(
    client: &reqwest::Client,
    runtime_url: &str,
    expected_state: &str,
    log: &Path,
) -> serde_json::Value {
    for _ in 0..100 {
        if let Ok(response) = client.get(runtime_url).bearer_auth(API_KEY).send().await {
            if let Ok(status) = response.json::<serde_json::Value>().await {
                if status["state"] == expected_state {
                    return status;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "Bundle did not reach {expected_state:?}:\n{}",
        fs::read_to_string(log).unwrap_or_default()
    );
}

async fn post_json(
    client: &reqwest::Client,
    url: String,
    body: serde_json::Value,
) -> reqwest::Response {
    client
        .post(url)
        .bearer_auth(API_KEY)
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn http_restart_and_lifecycle_are_atomic_and_preserve_state() {
    let temp = tempfile::tempdir().unwrap();
    let packages = temp.path().join("packages");
    let state = temp.path().join("state");
    let key = SigningKey::from_bytes(&[41_u8; 32]);
    let port = free_port();
    let config_path = temp.path().join("gadgetron.toml");
    fs::write(
        &config_path,
        format!(
            r#"[server]
bind = "127.0.0.1:{port}"

[web]
enabled = true
bundles_dir = "{}"
bundle_state_dir = "{}"

[web.bundle_signing]
require_signature = true
public_keys_hex = ["{}"]
"#,
            packages.display(),
            state.display(),
            hex::encode(key.verifying_key().as_bytes()),
        ),
    )
    .unwrap();
    let log_path = temp.path().join("gateway.log");
    let log = fs::File::create(&log_path).unwrap();
    let mut child = spawn_gateway(&config_path, port, &log);
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    wait_for_health(&client, &base, &log_path).await;
    let runtime_base = format!("{base}/api/v1/web/workbench/admin/bundles");

    let (catalog, package) = package_sources("http-lifecycle", RUNTIME.as_bytes());
    let catalog_signature = sign(&key, &catalog);
    let package_signature = sign(&key, &package);
    let install = post_json(
        &client,
        runtime_base.clone(),
        serde_json::json!({
            "bundle_toml": catalog,
            "signature_hex": catalog_signature,
            "package_toml": package,
            "package_signature_hex": package_signature,
            "runtime_artifact_base64": STANDARD.encode(RUNTIME.as_bytes()),
            "package_assets_base64": {
                "assets/fixture.txt": STANDARD.encode(PACKAGE_ASSET),
            },
        }),
    )
    .await;
    assert_eq!(
        install.status(),
        reqwest::StatusCode::OK,
        "{}",
        install.text().await.unwrap()
    );

    let status: serde_json::Value = client
        .get(format!("{runtime_base}/http-lifecycle/runtime"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["state"], "installed_not_enabled");
    let installed_views: serde_json::Value = client
        .get(format!("{base}/api/v1/web/workbench/views"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!installed_views["views"]
        .as_array()
        .unwrap()
        .iter()
        .any(|view| view["id"] == "http-lifecycle.echo"));

    if !user_namespace_available() {
        eprintln!("skipped runtime lifecycle: host blocks user namespaces");
        return;
    }

    let enabled = post_json(
        &client,
        format!("{runtime_base}/http-lifecycle/enable"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        enabled.status(),
        reqwest::StatusCode::OK,
        "{}",
        enabled.text().await.unwrap()
    );
    assert_eq!(
        fs::read_to_string(state.join("http-lifecycle/boot-count")).unwrap(),
        "1"
    );

    let tools: serde_json::Value = client
        .get(format!("{base}/v1/tools"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tools["count"], 1);
    assert_eq!(tools["tools"][0]["name"], "http.echo");
    let invoked = post_json(
        &client,
        format!("{base}/v1/tools/http.echo/invoke"),
        serde_json::json!({"probe": "live"}),
    )
    .await;
    let invoked_status = invoked.status();
    let invoked_body = invoked.text().await.unwrap();
    assert_eq!(invoked_status, reqwest::StatusCode::OK, "{invoked_body}");
    let invoked: serde_json::Value = serde_json::from_str(&invoked_body).unwrap();
    let text = invoked["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("live"));
    assert!(text.contains("tenant_id"));
    assert!(text.contains("request_id"));
    assert!(!text.contains("broker_lease"));

    let views: serde_json::Value = client
        .get(format!("{base}/api/v1/web/workbench/views"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let bundle_view = views["views"]
        .as_array()
        .unwrap()
        .iter()
        .find(|view| view["id"] == "http-lifecycle.echo")
        .unwrap();
    assert_eq!(bundle_view["renderer"], "table");
    assert_eq!(bundle_view["owner_bundle"], "http-lifecycle");
    let view_data: serde_json::Value = client
        .get(format!(
            "{base}/api/v1/web/workbench/views/http-lifecycle.echo/data"
        ))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(view_data["view_id"], "http-lifecycle.echo");
    assert!(view_data["payload"].to_string().contains("tenant_id"));

    let actions: serde_json::Value = client
        .get(format!("{base}/api/v1/web/workbench/actions"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let action_id = "http-lifecycle.echo.action.http.echo";
    assert!(actions["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["id"] == action_id));
    let action = post_json(
        &client,
        format!("{base}/api/v1/web/workbench/actions/{action_id}"),
        serde_json::json!({"args": {"probe": "workspace-action"}}),
    )
    .await;
    let action_status = action.status();
    let action_body = action.text().await.unwrap();
    assert_eq!(action_status, reqwest::StatusCode::OK, "{action_body}");
    let action: serde_json::Value = serde_json::from_str(&action_body).unwrap();
    assert_eq!(action["result"]["status"], "ok");
    assert!(action["result"]["payload"]
        .to_string()
        .contains("workspace-action"));

    child.terminate();
    child = spawn_gateway(&config_path, port, &log);
    wait_for_health(&client, &base, &log_path).await;
    let restored = wait_for_bundle_state(
        &client,
        &format!("{runtime_base}/http-lifecycle/runtime"),
        "enabled",
        &log_path,
    )
    .await;
    assert_eq!(restored["health"], "healthy");
    assert_eq!(
        fs::read_to_string(state.join("http-lifecycle/boot-count")).unwrap(),
        "2"
    );
    let restored_tools: serde_json::Value = client
        .get(format!("{base}/v1/tools"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(restored_tools["count"], 1);
    assert_eq!(restored_tools["tools"][0]["name"], "http.echo");

    // Claude Code and Codex Exec both consume this same per-turn MCP child.
    // With no `[knowledge]` provider configured, the enabled Bundle must still
    // appear and execute solely through the authenticated parent callback.
    let mut mcp = ForwardingMcpClient::spawn(&config_path, &base);
    let initialized = mcp.request(1, "initialize", serde_json::json!({}));
    assert_eq!(
        initialized["result"]["serverInfo"]["name"],
        "gadgetron-knowledge"
    );
    let listed = mcp.request(2, "tools/list", serde_json::json!({}));
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(listed["result"]["tools"][0]["name"], "http.echo");
    let called = mcp.request(
        3,
        "tools/call",
        serde_json::json!({
            "name": "http.echo",
            "arguments": {"probe": "mcp-child"},
        }),
    );
    assert_eq!(called["result"]["isError"], false);
    let child_text = called["result"]["content"][0]["text"].as_str().unwrap();
    assert!(child_text.contains("mcp-child"));
    assert!(child_text.contains("tenant_id"));

    let disabled = post_json(
        &client,
        format!("{runtime_base}/http-lifecycle/disable"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(disabled.status(), reqwest::StatusCode::OK);
    let tools: serde_json::Value = client
        .get(format!("{base}/v1/tools"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tools["count"], 0);
    let views: serde_json::Value = client
        .get(format!("{base}/api/v1/web/workbench/views"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!views["views"]
        .as_array()
        .unwrap()
        .iter()
        .any(|view| view["id"] == "http-lifecycle.echo"));
    let stale_child_call = mcp.request(
        4,
        "tools/call",
        serde_json::json!({
            "name": "http.echo",
            "arguments": {"probe": "after-disable"},
        }),
    );
    assert_eq!(stale_child_call["result"]["isError"], true);
    assert!(stale_child_call["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("unknown tool"));

    let installed_asset = packages.join("http-lifecycle/assets/fixture.txt");
    fs::remove_file(&installed_asset).unwrap();
    fs::write(&installed_asset, b"tampered fixture asset\n").unwrap();
    let tampered_enable = post_json(
        &client,
        format!("{runtime_base}/http-lifecycle/enable"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(tampered_enable.status(), reqwest::StatusCode::BAD_REQUEST);
    let status: serde_json::Value = client
        .get(format!("{runtime_base}/http-lifecycle/runtime"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["state"], "disabled");
    fs::write(&installed_asset, PACKAGE_ASSET).unwrap();

    let reenabled = post_json(
        &client,
        format!("{runtime_base}/http-lifecycle/enable"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        reenabled.status(),
        reqwest::StatusCode::OK,
        "{}",
        reenabled.text().await.unwrap()
    );
    assert_eq!(
        fs::read_to_string(state.join("http-lifecycle/boot-count")).unwrap(),
        "3"
    );

    let installed_package = packages.join("http-lifecycle/package.toml");
    let package_backup = fs::read(&installed_package).unwrap();
    fs::remove_file(&installed_package).unwrap();
    let rejected_uninstall = client
        .delete(format!("{runtime_base}/http-lifecycle"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap();
    assert!(!rejected_uninstall.status().is_success());
    assert!(packages.join("http-lifecycle").is_dir());
    let tools_while_rejected: serde_json::Value = client
        .get(format!("{base}/v1/tools"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tools_while_rejected["count"], 1);
    fs::write(&installed_package, package_backup).unwrap();

    let uninstall: serde_json::Value = client
        .delete(format!("{runtime_base}/http-lifecycle"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(uninstall["uninstalled"], true);
    assert_eq!(uninstall["runtime_disabled"], true);
    assert_eq!(uninstall["state_preserved"], true);
    assert!(!packages.join("http-lifecycle").exists());
    assert_eq!(
        fs::read_to_string(state.join("http-lifecycle/boot-count")).unwrap(),
        "3"
    );
    let tools: serde_json::Value = client
        .get(format!("{base}/v1/tools"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tools["count"], 0);

    let catalog_only =
        "[bundle]\nid = \"catalog-only\"\nversion = \"1.0.0\"\nallow_direct_actions = false\n";
    let catalog_install = post_json(
        &client,
        runtime_base.clone(),
        serde_json::json!({
            "bundle_toml": catalog_only,
            "signature_hex": sign(&key, catalog_only),
        }),
    )
    .await;
    assert_eq!(catalog_install.status(), reqwest::StatusCode::OK);
    let catalog_uninstall: serde_json::Value = client
        .delete(format!("{runtime_base}/catalog-only"))
        .bearer_auth(API_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog_uninstall["uninstalled"], true);
    assert_eq!(catalog_uninstall["runtime_disabled"], false);
    assert!(!packages.join("catalog-only").exists());

    let (bad_catalog, bad_package) = package_sources("tampered-runtime", b"expected");
    let bad_catalog_signature = sign(&key, &bad_catalog);
    let bad_package_signature = sign(&key, &bad_package);
    let bad_install = post_json(
        &client,
        runtime_base,
        serde_json::json!({
            "bundle_toml": bad_catalog,
            "signature_hex": bad_catalog_signature,
            "package_toml": bad_package,
            "package_signature_hex": bad_package_signature,
            "runtime_artifact_base64": STANDARD.encode(b"tampered"),
            "package_assets_base64": {
                "assets/fixture.txt": STANDARD.encode(PACKAGE_ASSET),
            },
        }),
    )
    .await;
    assert_eq!(bad_install.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(!packages.join("tampered-runtime").exists());

    child.terminate();
}
