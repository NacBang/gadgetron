#![cfg(target_os = "linux")]

use std::{
    fs,
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

use ed25519_dalek::{Signer, SigningKey};
use gadgetron_testing::harness::pg::PgHarness;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const REVISION: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const STALE_REVISION: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const STORED_REVISION: &str = "3333333333333333333333333333333333333333333333333333333333333333";

struct ChildGuard(Child);

impl ChildGuard {
    fn terminate(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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

fn sign(key: &SigningKey, source: &str) -> String {
    hex::encode(key.sign(source.as_bytes()).to_bytes())
}

fn write_signed_package(
    packages: &Path,
    key: &SigningKey,
    id: &str,
    runtime: &str,
    package: &str,
    assets: &[(&str, &[u8])],
) -> String {
    let root = packages.join(id);
    let runtime_path = root.join("bin/runtime");
    fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
    fs::write(&runtime_path, runtime).unwrap();
    fs::set_permissions(&runtime_path, fs::Permissions::from_mode(0o500)).unwrap();
    for (path, bytes) in assets {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let catalog =
        format!("[bundle]\nid = \"{id}\"\nversion = \"1.0.0\"\nallow_direct_actions = false\n");
    fs::write(root.join("bundle.toml"), &catalog).unwrap();
    fs::write(root.join("package.toml"), package).unwrap();
    fs::write(root.join("catalog.sig"), sign(key, &catalog)).unwrap();
    fs::write(root.join("package.sig"), sign(key, package)).unwrap();
    hex::encode(Sha256::digest(package.as_bytes()))
}

fn target_runtime() -> String {
    format!(
        r#"#!/usr/bin/python3
import json, os, sys
digest = os.environ["GADGETRON_BUNDLE_MANIFEST_SHA256"]
identity = {{"id":"row-target","version":"1.0.0"}}
handshaken = False
for line in sys.stdin:
    request = json.loads(line)
    method = request["payload"]["method"]
    params = request["payload"].get("params", {{}})
    stop = False
    if method == "handshake" and params.get("package_manifest_sha256") == digest:
        handshaken = True
        payload = {{"result":"handshake","data":{{"package_manifest_sha256":digest,"selected_protocol":1}}}}
    elif not handshaken:
        payload = {{"result":"error","data":{{"code":"handshake-required","message":"handshake required","retryable":False}}}}
    elif method == "health":
        payload = {{"result":"health","data":{{"status":"healthy"}}}}
    elif method == "invoke_gadget" and params.get("gadget") == "target.rows":
        rows = [
          {{"incident_id":"good","revision":"{REVISION}","title":"Good base row"}},
          {{"incident_id":"duplicate","revision":"{REVISION}","title":"Duplicate provider row"}},
          {{"incident_id":"stale","revision":"{STALE_REVISION}","title":"Stale base row"}},
          {{"incident_id":"failed","revision":"{REVISION}","title":"Failed base row"}},
          {{"incident_id":"pending","revision":"{REVISION}","title":"Pending base row"}}
        ]
        payload = {{"result":"gadget_result","data":{{"output":{{"rows":rows,"count":len(rows)}},"evidence":[],"candidates":[],"outcomes":[]}}}}
    elif method == "shutdown":
        payload = {{"result":"acknowledgement","data":{{"message":"stopping"}}}}
        stop = True
    else:
        payload = {{"result":"error","data":{{"code":"unsupported","message":"unsupported","retryable":False}}}}
    response = {{"protocol_version":1,"message_id":request["message_id"],"bundle":identity,"payload":payload}}
    sys.stdout.write(json.dumps(response,separators=(",",":"))+"\n")
    sys.stdout.flush()
    if stop: break
"#
    )
}

fn provider_runtime() -> &'static str {
    r#"#!/usr/bin/python3
import json, os, sys
digest = os.environ["GADGETRON_BUNDLE_MANIFEST_SHA256"]
identity = {"id":"row-provider","version":"1.0.0"}
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
    elif method == "invoke_gadget" and params.get("gadget") == "provider.context-batch":
        context = dict(params.get("context") or {})
        context.pop("broker_lease", None)
        rows = []
        for subject in (params.get("input") or {}).get("subjects", []):
            item = {"id":subject["id"],"revision":subject["revision"],"status":"ready",
                    "data":{"tenant_id":context.get("tenant_id"),"actor_id":context.get("actor_id")}}
            rows.append(item)
            if subject["id"] == "duplicate": rows.append(item)
        rows.append({"id":"unknown","revision":"0" * 64,"status":"ready","data":{}})
        payload = {"result":"gadget_result","data":{"output":{"subjects":rows},"evidence":[],"candidates":[],"outcomes":[]}}
    elif method == "shutdown":
        payload = {"result":"acknowledgement","data":{"message":"stopping"}}
        stop = True
    else:
        payload = {"result":"error","data":{"code":"unsupported","message":"unsupported","retryable":False}}
    response = {"protocol_version":1,"message_id":request["message_id"],"bundle":identity,"payload":payload}
    sys.stdout.write(json.dumps(response,separators=(",",":"))+"\n")
    sys.stdout.flush()
    if stop: break
"#
}

fn target_package(runtime: &str) -> String {
    let digest = hex::encode(Sha256::digest(runtime.as_bytes()));
    format!(
        r#"manifest_version = 3

[bundle]
id = "row-target"
version = "1.0.0"
class = "operational"
publisher = "gadgetron.tests"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.8.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/runtime"
entry_sha256 = "{digest}"

[runtime.limits]
memory_mb = 128
open_files = 32
cpu_seconds = 10

[capabilities]
gadget_namespaces = ["target"]

[[capabilities.gadgets]]
name = "target.rows"
description = "Return actor-visible incident rows"
tier = "read"
input_schema = {{ type = "object" }}
output_schema = {{ type = "object", properties = {{ rows = {{ type = "array" }}, count = {{ type = "integer" }} }}, required = ["rows", "count"] }}

[capabilities.gadgets.effect]
risk = "low"
idempotent = true
reversible = true
requires_evidence = false

[[capabilities.workspaces]]
id = "incidents"
label = "Incidents"
renderer = "table"
data_capability = "target.rows"
required_scopes = ["openai_compat"]

[[capabilities.ui_contributions]]
id = "incidents-main"
kind = "workspace"
label = "Incidents"
placement = "main"
order_hint = 100
icon = "table"
required_scopes = ["openai_compat"]
empty_state = "No incidents"
error_state = "Incidents unavailable"
workspace = "incidents"
"#
    )
}

fn provider_package(runtime: &str, recipe: &[u8]) -> String {
    let runtime_digest = hex::encode(Sha256::digest(runtime.as_bytes()));
    let recipe_digest = hex::encode(Sha256::digest(recipe));
    format!(
        r#"manifest_version = 3

[bundle]
id = "row-provider"
version = "1.0.0"
class = "intelligence"
publisher = "gadgetron.tests"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.8.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

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
gadget_namespaces = ["provider"]

[[capabilities.gadgets]]
name = "provider.attach"
description = "Attach one revision-pinned result"
tier = "write"
input_schema = {{ type = "object" }}
output_schema = {{ type = "object" }}

[capabilities.gadgets.effect]
risk = "medium"
idempotent = true
reversible = false
requires_evidence = true

[[capabilities.gadgets]]
name = "provider.context-batch"
description = "Read ready enrichment payloads"
tier = "read"
input_schema = {{ type = "object", additionalProperties = false, properties = {{ subjects = {{ type = "array", maxItems = 200, items = {{ type = "object", additionalProperties = false, properties = {{ id = {{ type = "string" }}, revision = {{ type = "string" }} }}, required = ["id", "revision"] }} }} }}, required = ["subjects"] }}
output_schema = {{ type = "object", additionalProperties = false, properties = {{ subjects = {{ type = "array", maxItems = 200, items = {{ type = "object", additionalProperties = false, properties = {{ id = {{ type = "string" }}, revision = {{ type = "string" }}, status = {{ type = "string", enum = ["ready"] }}, data = {{ type = "object" }} }}, required = ["id", "revision", "status", "data"] }} }} }}, required = ["subjects"] }}

[capabilities.gadgets.effect]
risk = "low"
idempotent = true
reversible = true
requires_evidence = false

[[capabilities.jobs]]
id = "incident-enrichment"
role = "researcher"
triggers = ["event"]
goal = "Add bounded context to one revision-pinned incident"
event_kinds = ["incident-updated"]
gadget_allowlist = ["provider.context-batch"]

[[capabilities.agent_roles]]
id = "incident-researcher"
label = "Incident researcher"
description = "Adds bounded context without changing incident authority"
core_role = "researcher"
job = "incident-enrichment"
recipe_asset = "incident-recipe"
prompt_contract_revision = "incident-research-v1"

[[capabilities.seed_assets]]
id = "incident-recipe"
path = "recipes/incident.json"
media_type = "application/json"
sha256 = "{recipe_digest}"

[[capabilities.event_jobs]]
id = "incident-event"
event_kind = "incident-updated"
subject_owner_bundle = "row-target"
subject_kind = "incident"
agent_role = "incident-researcher"
input_schema = {{ type = "object", additionalProperties = false, properties = {{ incident_id = {{ type = "string" }} }}, required = ["incident_id"] }}
result_gadget = "provider.attach"

[[capabilities.row_enrichments]]
id = "incident-context"
target_bundle = "row-target"
target_workspace = "incidents"
target_data_capability = "target.rows"
subject_kind = "incident"
row_join_key_field = "incident_id"
row_revision_field = "revision"
read_gadget = "provider.context-batch"
event_job = "incident-event"
"#
    )
}

async fn insert_tenant_key(pool: &sqlx::PgPool, label: &str) -> (Uuid, String, Uuid) {
    let tenant_id = Uuid::new_v4();
    let key_id = Uuid::new_v4();
    let raw = format!("gad_live_{:032x}", Uuid::new_v4().as_u128());
    let key_hash = hex::encode(Sha256::digest(raw.as_bytes()));
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(tenant_id)
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) VALUES ($1,$2,$3,$4,'admin','test')",
    )
    .bind(Uuid::new_v4())
    .bind(tenant_id)
    .bind(format!("{label}@example.test"))
    .bind(label)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO api_keys (id, tenant_id, prefix, key_hash, kind, scopes) VALUES ($1,$2,'gad_live',$3,'live',ARRAY['OpenAiCompat','Management']::TEXT[])",
    )
    .bind(key_id)
    .bind(tenant_id)
    .bind(key_hash)
    .execute(pool)
    .await
    .unwrap();
    (tenant_id, raw, key_id)
}

async fn insert_event_goal(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    package_digest: &str,
    subject_id: &str,
    revision: &str,
    status: &str,
) {
    sqlx::query(
        r#"INSERT INTO autonomy_goals
           (tenant_id, goal_key, source_kind, status, context_state, goal,
            owner_bundle_id, recipe_id, package_manifest_sha256, target_kind,
            target_id, target_revision, target_label, interval_seconds,
            max_wall_seconds, max_attempts, event_kind, subject_bundle_id,
            subject_kind, event_payload, agent_role_id, result_gadget,
            agent_profile_snapshot)
           VALUES ($1,$2,'bundle_event',$3,'missing','Fixture enrichment',
                   'row-provider','incident-enrichment',$4,'incident',$5,$6,$5,
                   10,120,1,'incident-updated','row-target','incident','{}'::jsonb,
                   'incident-researcher','provider.attach','{"model":"fixture"}'::jsonb)"#,
    )
    .bind(tenant_id)
    .bind(format!("row-enrichment:{}", Uuid::new_v4()))
    .bind(status)
    .bind(package_digest)
    .bind(subject_id)
    .bind(revision)
    .execute(pool)
    .await
    .unwrap();
}

fn database_url(harness: &PgHarness) -> String {
    let admin = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".into());
    let base = admin
        .rsplit_once('/')
        .map_or(admin.as_str(), |(base, _)| base);
    format!("{base}/{}", harness.db_name)
}

fn spawn_gateway(config: &Path, port: u16, database_url: &str, log: &fs::File) -> ChildGuard {
    ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_gadgetron"))
            .args([
                "serve",
                "--config",
                config.to_str().unwrap(),
                "--bind",
                &format!("127.0.0.1:{port}"),
            ])
            .env("GADGETRON_DATABASE_URL", database_url)
            .env_remove("GADGETRON_CONFIG")
            .env_remove("GADGETRON_BIND")
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log.try_clone().unwrap()))
            .spawn()
            .unwrap(),
    )
}

async fn wait_for_health(client: &reqwest::Client, base: &str, log: &Path) {
    for _ in 0..150 {
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
        "gateway did not become healthy:\n{}",
        fs::read_to_string(log).unwrap_or_default()
    );
}

async fn post(client: &reqwest::Client, url: String, key: &str, log: &Path) {
    let response = client
        .post(url)
        .bearer_auth(key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "{body}\n{}",
        fs::read_to_string(log).unwrap_or_default()
    );
}

async fn view_rows(client: &reqwest::Client, base: &str, key: &str) -> Vec<serde_json::Value> {
    let response = client
        .get(format!(
            "{base}/api/v1/web/workbench/views/row-target.incidents/data"
        ))
        .bearer_auth(key)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "{body}");
    serde_json::from_str::<serde_json::Value>(&body).unwrap()["payload"]["rows"]
        .as_array()
        .unwrap()
        .clone()
}

fn row<'a>(rows: &'a [serde_json::Value], id: &str) -> &'a serde_json::Value {
    rows.iter().find(|row| row["incident_id"] == id).unwrap()
}

#[tokio::test]
async fn core_t2_http_projection_is_actor_scoped_batched_and_lifecycle_safe() {
    if !user_namespace_available() {
        eprintln!("skipping row-enrichment HTTP test: user namespaces unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let (tenant_a, key_a, actor_a) = insert_tenant_key(pool, "row-enrichment-a").await;
    let (_tenant_b, key_b, _actor_b) = insert_tenant_key(pool, "row-enrichment-b").await;
    let temp = tempfile::tempdir().unwrap();
    let packages = temp.path().join("packages");
    let state = temp.path().join("state");
    let signing = SigningKey::from_bytes(&[73_u8; 32]);
    let target_runtime = target_runtime();
    let target_package = target_package(&target_runtime);
    gadgetron_bundle_sdk::BundlePackageManifest::parse_toml(&target_package).unwrap();
    write_signed_package(
        &packages,
        &signing,
        "row-target",
        &target_runtime,
        &target_package,
        &[],
    );
    let recipe = br#"{"version":1,"steps":["read"]}"#;
    let provider_runtime = provider_runtime();
    let provider_package = provider_package(provider_runtime, recipe);
    gadgetron_bundle_sdk::BundlePackageManifest::parse_toml(&provider_package).unwrap();
    let provider_digest = write_signed_package(
        &packages,
        &signing,
        "row-provider",
        provider_runtime,
        &provider_package,
        &[("recipes/incident.json", recipe)],
    );
    for (id, revision, status) in [
        ("good", REVISION, "succeeded"),
        ("duplicate", REVISION, "succeeded"),
        ("stale", STORED_REVISION, "succeeded"),
        ("failed", REVISION, "failed_provider"),
    ] {
        insert_event_goal(pool, tenant_a, &provider_digest, id, revision, status).await;
    }

    let port = free_port();
    let config = temp.path().join("gadgetron.toml");
    fs::write(
        &config,
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
            hex::encode(signing.verifying_key().as_bytes()),
        ),
    )
    .unwrap();
    let log_path = temp.path().join("gateway.log");
    let log = fs::File::create(&log_path).unwrap();
    let mut child = spawn_gateway(&config, port, &database_url(&harness), &log);
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    wait_for_health(&client, &base, &log_path).await;
    let admin = format!("{base}/api/v1/web/workbench/admin/bundles");
    post(
        &client,
        format!("{admin}/row-target/enable"),
        &key_a,
        &log_path,
    )
    .await;

    let disabled = view_rows(&client, &base, &key_a).await;
    assert!(disabled.iter().all(|row| {
        row["enrichments"]["incident-context"]["status"] == "Unavailable(provider disabled)"
    }));

    post(
        &client,
        format!("{admin}/row-provider/enable"),
        &key_a,
        &log_path,
    )
    .await;
    let rows = view_rows(&client, &base, &key_a).await;
    assert_eq!(row(&rows, "good")["title"], "Good base row");
    let good = &row(&rows, "good")["enrichments"]["incident-context"];
    assert_eq!(good["status"], "Ready");
    assert_eq!(good["provider_bundle"], "row-provider");
    assert_eq!(good["subject_revision"], REVISION);
    assert_eq!(good["data"]["tenant_id"], tenant_a.to_string());
    assert_eq!(good["data"]["actor_id"], actor_a.to_string());
    assert_eq!(
        row(&rows, "duplicate")["enrichments"]["incident-context"]["status"],
        "Failed(read)"
    );
    assert_eq!(
        row(&rows, "stale")["enrichments"]["incident-context"]["status"],
        "Stale"
    );
    assert_eq!(
        row(&rows, "failed")["enrichments"]["incident-context"]["status"],
        "Failed(provider)"
    );
    assert_eq!(
        row(&rows, "pending")["enrichments"]["incident-context"]["status"],
        "Pending"
    );

    let other_tenant = view_rows(&client, &base, &key_b).await;
    assert!(other_tenant.iter().all(|row| {
        row["enrichments"]["incident-context"]["status"] == "Pending"
            && row["enrichments"]["incident-context"]["data"] == serde_json::json!({})
    }));

    post(
        &client,
        format!("{admin}/row-provider/disable"),
        &key_a,
        &log_path,
    )
    .await;
    let disabled_again = view_rows(&client, &base, &key_a).await;
    assert!(disabled_again.iter().all(|row| {
        row["enrichments"]["incident-context"]["status"] == "Unavailable(provider disabled)"
    }));
    let response = client
        .delete(format!("{admin}/row-provider"))
        .bearer_auth(&key_a)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "{body}");
    let uninstalled = view_rows(&client, &base, &key_a).await;
    assert!(uninstalled
        .iter()
        .all(|row| row.get("enrichments").is_none()));

    child.terminate();
    harness.cleanup().await;
}
