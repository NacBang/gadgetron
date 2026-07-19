use std::sync::Arc;

use gadgetron_bundle_host::{BrokerCaller, BundleBroker, ValidatedPackageContract};
use gadgetron_bundle_sdk::{
    BrokerRequest, BrokerResource, BrokerResponse, DatabaseDeleteRequest, DatabaseInsertRequest,
    DatabaseMutationEvent, DatabaseSelectRequest, DatabaseUpdateRequest, InvocationContext,
    LocalId,
};
use gadgetron_gateway::web::{
    bundle_broker::BundleBrokerRuntime,
    bundle_grants::{BundlePermissionGrant, GrantedBundlePermission},
};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::knowledge_spaces::{self as spaces, CreateProject, SpaceActor};
use semver::Version;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn database_broker_forces_the_core_lease_tenant() {
    let Ok(database_url) = std::env::var("GADGETRON_BROKER_TEST_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL Bundle broker test: dedicated DSN is unset");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&database_url)
        .await
        .expect("connect dedicated Bundle broker test database");
    let table = format!("bundle_broker_test_{}", Uuid::new_v4().simple());
    let view = format!("{table}_view");
    let unsafe_table = format!("{table}_unsafe");
    let create = format!(
        "CREATE TABLE \"{table}\" (tenant_id UUID NOT NULL, host_id UUID NOT NULL, label TEXT NOT NULL, labels JSONB NOT NULL DEFAULT '{{}}'::jsonb, PRIMARY KEY (tenant_id, host_id))"
    );
    sqlx::query(&create).execute(&pool).await.unwrap();
    let create_view = format!(
        "CREATE VIEW \"{view}\" AS SELECT tenant_id, host_id, label, labels FROM \"{table}\""
    );
    sqlx::query(&create_view).execute(&pool).await.unwrap();
    let create_unsafe =
        format!("CREATE TABLE \"{unsafe_table}\" (host_id UUID NOT NULL, label TEXT NOT NULL)");
    sqlx::query(&create_unsafe).execute(&pool).await.unwrap();

    let test_result = async {
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();
        let host_a = Uuid::new_v4();
        let host_b = Uuid::new_v4();
        let insert = format!(
            "INSERT INTO \"{table}\" (tenant_id, host_id, label) VALUES ($1, $2, $3), ($4, $5, $6)"
        );
        sqlx::query(&insert)
            .bind(tenant_a)
            .bind(host_a)
            .bind("tenant-a-host")
            .bind(tenant_b)
            .bind(host_b)
            .bind("tenant-b-host")
            .execute(&pool)
            .await
            .unwrap();

        let package = package_contract(&table, &view, &unsafe_table);
        let temp = tempfile::tempdir().unwrap();
        let runtime = BundleBrokerRuntime::open(temp.path(), Some(pool.clone())).unwrap();
        runtime
            .grants()
            .put(
                BundlePermissionGrant::new(
                    &package.runtime_identity().id,
                    package.manifest_sha256(),
                    package
                        .manifest()
                        .permissions
                        .iter()
                        .map(GrantedBundlePermission::from),
                )
                .unwrap(),
            )
            .unwrap();
        let broker = runtime.broker_for(&package);
        let caller = BrokerCaller::from_package(&package);

        let context_a = InvocationContext::new(tenant_a.to_string(), "manager-a", "request-a")
            .with_scopes(["Management".into()]);
        let lease_a = runtime
            .issue_lease(
                package.runtime_identity().id.to_string(),
                package.manifest_sha256().to_string(),
                &context_a,
            )
            .unwrap();
        let rows_a = select(
            Arc::clone(&broker),
            &caller,
            lease_a.token().clone(),
            &table,
        )
        .await;
        assert_eq!(rows_a.len(), 1);
        assert_eq!(rows_a[0]["label"], "tenant-a-host");
        assert_eq!(rows_a[0]["host_id"], host_a.to_string());

        let view_rows = select(Arc::clone(&broker), &caller, lease_a.token().clone(), &view).await;
        assert_eq!(view_rows.len(), 1);
        assert_eq!(view_rows[0]["label"], "tenant-a-host");

        let labels_request = DatabaseSelectRequest::new(
            lease_a.token().clone(),
            LocalId::new("telemetry-read").unwrap(),
            BrokerResource::database_table(&view).unwrap(),
            ["label".to_string()],
        )
        .with_filter("labels", serde_json::json!({}))
        .with_limit(10);
        let labels_response = broker
            .handle(&caller, BrokerRequest::DatabaseSelect(labels_request))
            .await;
        assert!(matches!(
            labels_response,
            BrokerResponse::DatabaseRows(ref rows)
                if rows.rows.len() == 1 && rows.rows[0]["label"] == "tenant-a-host"
        ));

        let view_insert = DatabaseInsertRequest::new(
            lease_a.token().clone(),
            LocalId::new("telemetry-write").unwrap(),
            BrokerResource::database_table(&view).unwrap(),
            std::collections::BTreeMap::from([
                ("host_id".into(), serde_json::json!(Uuid::new_v4())),
                ("label".into(), serde_json::json!("must-not-write-view")),
            ]),
        );
        let view_mutation = broker
            .handle(&caller, BrokerRequest::DatabaseInsert(view_insert))
            .await;
        assert!(matches!(
            view_mutation,
            BrokerResponse::Error(ref error) if error.code.as_str() == "resource-unavailable"
        ));

        let context_b = InvocationContext::new(tenant_b.to_string(), "manager-b", "request-b")
            .with_scopes(["Management".into()]);
        let lease_b = runtime
            .issue_lease(
                package.runtime_identity().id.to_string(),
                package.manifest_sha256().to_string(),
                &context_b,
            )
            .unwrap();
        let rows_b = select(
            Arc::clone(&broker),
            &caller,
            lease_b.token().clone(),
            &table,
        )
        .await;
        assert_eq!(rows_b.len(), 1);
        assert_eq!(rows_b[0]["label"], "tenant-b-host");
        assert_eq!(rows_b[0]["host_id"], host_b.to_string());

        let inserted_host = Uuid::new_v4();
        let insert = DatabaseInsertRequest::new(
            lease_a.token().clone(),
            LocalId::new("telemetry-write").unwrap(),
            BrokerResource::database_table(&table).unwrap(),
            std::collections::BTreeMap::from([
                ("host_id".into(), serde_json::json!(inserted_host)),
                ("label".into(), serde_json::json!("inserted")),
            ]),
        )
        .with_conflict_keys(["host_id".into()]);
        assert_eq!(
            mutation(&broker, &caller, BrokerRequest::DatabaseInsert(insert)).await,
            1
        );

        let upsert = DatabaseInsertRequest::new(
            lease_a.token().clone(),
            LocalId::new("telemetry-write").unwrap(),
            BrokerResource::database_table(&table).unwrap(),
            std::collections::BTreeMap::from([
                ("host_id".into(), serde_json::json!(inserted_host)),
                ("label".into(), serde_json::json!("upserted")),
            ]),
        )
        .with_conflict_keys(["host_id".into()]);
        assert_eq!(
            mutation(&broker, &caller, BrokerRequest::DatabaseInsert(upsert)).await,
            1
        );

        let update = DatabaseUpdateRequest::new(
            lease_a.token().clone(),
            LocalId::new("telemetry-write").unwrap(),
            BrokerResource::database_table(&table).unwrap(),
            std::collections::BTreeMap::from([("label".into(), serde_json::json!("updated"))]),
            std::collections::BTreeMap::from([(
                "host_id".into(),
                serde_json::json!(inserted_host),
            )]),
        );
        assert_eq!(
            mutation(&broker, &caller, BrokerRequest::DatabaseUpdate(update)).await,
            1
        );

        let cross_tenant = DatabaseUpdateRequest::new(
            lease_a.token().clone(),
            LocalId::new("telemetry-write").unwrap(),
            BrokerResource::database_table(&table).unwrap(),
            std::collections::BTreeMap::from([("label".into(), serde_json::json!("leaked"))]),
            std::collections::BTreeMap::from([("host_id".into(), serde_json::json!(host_b))]),
        );
        assert_eq!(
            mutation(
                &broker,
                &caller,
                BrokerRequest::DatabaseUpdate(cross_tenant)
            )
            .await,
            0
        );
        let rows_b_after = select(
            Arc::clone(&broker),
            &caller,
            lease_b.token().clone(),
            &table,
        )
        .await;
        assert_eq!(rows_b_after[0]["label"], "tenant-b-host");

        let delete = DatabaseDeleteRequest::new(
            lease_a.token().clone(),
            LocalId::new("telemetry-write").unwrap(),
            BrokerResource::database_table(&table).unwrap(),
            std::collections::BTreeMap::from([(
                "host_id".into(),
                serde_json::json!(inserted_host),
            )]),
        );
        assert_eq!(
            mutation(&broker, &caller, BrokerRequest::DatabaseDelete(delete)).await,
            1
        );

        let unsafe_response = broker
            .handle(
                &caller,
                BrokerRequest::DatabaseSelect(select_request(
                    lease_b.token().clone(),
                    &unsafe_table,
                )),
            )
            .await;
        assert!(matches!(
            unsafe_response,
            BrokerResponse::Error(ref error) if error.code.as_str() == "resource-unavailable"
        ));

        let revoked_token = lease_a.token().clone();
        drop(lease_a);
        let denied = broker
            .handle(
                &caller,
                BrokerRequest::DatabaseSelect(select_request(revoked_token, &table)),
            )
            .await;
        assert!(matches!(
            denied,
            BrokerResponse::Error(ref error) if error.code.as_str() == "lease-invalid"
        ));
    }
    .await;

    let drop_view = format!("DROP VIEW IF EXISTS \"{view}\"");
    sqlx::query(&drop_view).execute(&pool).await.unwrap();
    let drop_table = format!("DROP TABLE IF EXISTS \"{table}\"");
    sqlx::query(&drop_table).execute(&pool).await.unwrap();
    let drop_unsafe = format!("DROP TABLE IF EXISTS \"{unsafe_table}\"");
    sqlx::query(&drop_unsafe).execute(&pool).await.unwrap();
    test_result
}

#[tokio::test]
async fn knowledge_event_delete_and_snapshot_outbox_commit_atomically() {
    if !pg_available().await {
        eprintln!("skipping Knowledge event broker test: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let tenant_id = Uuid::new_v4();
    let actor_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'CORE-T3 broker test')")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO users (id, tenant_id, email, display_name, role, password_hash)
           VALUES ($1,$2,$3,'CORE-T3 Operator','admin','test')"#,
    )
    .bind(actor_id)
    .bind(tenant_id)
    .bind(format!("core-t3-{tenant_id}@example.test"))
    .execute(pool)
    .await
    .unwrap();
    let actor = SpaceActor {
        tenant_id,
        user_id: actor_id,
    };
    let project = spaces::create_project(
        pool,
        actor,
        CreateProject {
            slug: "incident-review".to_string(),
            title: "Incident Review".to_string(),
            goal: "Review closed incidents".to_string(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();

    let suffix = &Uuid::new_v4().simple().to_string()[..12];
    let signals = format!("ke_{suffix}_signals");
    let incidents = format!("ke_{suffix}_incidents");
    let snapshot = format!("ke_{suffix}_snapshot");
    let trigger_fn = format!("ke_{suffix}_close");
    sqlx::raw_sql(&format!(
        r#"CREATE TABLE "{incidents}" (
               tenant_id UUID NOT NULL,
               incident_id TEXT NOT NULL,
               revision TEXT NOT NULL,
               title TEXT NOT NULL,
               status TEXT NOT NULL
           );
           CREATE TABLE "{signals}" (
               tenant_id UUID NOT NULL,
               signal_id TEXT NOT NULL,
               incident_id TEXT NOT NULL
           );
           CREATE VIEW "{snapshot}" AS
             SELECT tenant_id, incident_id, revision, title,
                    jsonb_build_object('status', status) AS snapshot
               FROM "{incidents}" WHERE status = 'closed';
           CREATE FUNCTION "{trigger_fn}"() RETURNS trigger AS $$
           BEGIN
             IF NOT EXISTS (
               SELECT 1 FROM "{signals}"
                WHERE tenant_id = OLD.tenant_id AND incident_id = OLD.incident_id
             ) THEN
               UPDATE "{incidents}"
                  SET status = 'closed', revision = gen_random_uuid()::text
                WHERE tenant_id = OLD.tenant_id AND incident_id = OLD.incident_id;
             END IF;
             RETURN OLD;
           END;
           $$ LANGUAGE plpgsql;
           CREATE TRIGGER close_incident AFTER DELETE ON "{signals}"
             FOR EACH ROW EXECUTE FUNCTION "{trigger_fn}"()"#,
    ))
    .execute(pool)
    .await
    .unwrap();

    let package = knowledge_event_package_contract(&signals, &snapshot);
    let temp = tempfile::tempdir().unwrap();
    let runtime = BundleBrokerRuntime::open(temp.path(), Some(pool.clone())).unwrap();
    runtime
        .grants()
        .put(
            BundlePermissionGrant::new(
                &package.runtime_identity().id,
                package.manifest_sha256(),
                package
                    .manifest()
                    .permissions
                    .iter()
                    .map(GrantedBundlePermission::from),
            )
            .unwrap(),
        )
        .unwrap();
    let broker = runtime.broker_for(&package);
    let caller = BrokerCaller::from_package(&package);
    let context =
        InvocationContext::new(tenant_id.to_string(), actor_id.to_string(), "core-t3-close")
            .with_acting_space_id(project.space.id.to_string())
            .with_scopes(["Management".into()]);
    let lease = runtime
        .issue_lease(
            package.runtime_identity().id.to_string(),
            package.manifest_sha256().to_string(),
            &context,
        )
        .unwrap();

    sqlx::query(&format!(
        "INSERT INTO \"{incidents}\" VALUES ($1, 'incident-ok', 'open-1', 'Disk pressure', 'active')"
    ))
    .bind(tenant_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(&format!(
        "INSERT INTO \"{signals}\" VALUES ($1, 'signal-ok', 'incident-ok')"
    ))
    .bind(tenant_id)
    .execute(pool)
    .await
    .unwrap();
    let close = DatabaseDeleteRequest::new(
        lease.token().clone(),
        LocalId::new("operations-write").unwrap(),
        BrokerResource::database_table(&signals).unwrap(),
        std::collections::BTreeMap::from([("signal_id".into(), serde_json::json!("signal-ok"))]),
    )
    .with_event(DatabaseMutationEvent::post_mutation(
        LocalId::new("server-incident-closed").unwrap(),
        LocalId::new("server-incident").unwrap(),
        std::collections::BTreeMap::from([(
            "incident_id".into(),
            serde_json::json!("incident-ok"),
        )]),
    ));
    assert_eq!(
        mutation(&broker, &caller, BrokerRequest::DatabaseDelete(close)).await,
        1
    );
    let signal_count: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM \"{signals}\" WHERE tenant_id = $1 AND signal_id = 'signal-ok'"
    ))
    .bind(tenant_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(signal_count, 0);
    let (status, revision): (String, String) = sqlx::query_as(&format!(
        "SELECT status, revision FROM \"{incidents}\" WHERE tenant_id = $1 AND incident_id = 'incident-ok'"
    ))
    .bind(tenant_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(status, "closed");
    assert!(Uuid::parse_str(&revision).is_ok());
    let outbox: (String, String, serde_json::Value, String) = sqlx::query_as(
        r#"SELECT subject_id, subject_revision, snapshot, snapshot_hash
             FROM knowledge_event_outbox WHERE tenant_id = $1"#,
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(outbox.0, "incident-ok");
    assert_eq!(outbox.1, revision);
    assert_eq!(outbox.2["snapshot"]["status"], "closed");
    assert!(outbox.3.starts_with("sha256:"));

    sqlx::query(&format!(
        "INSERT INTO \"{incidents}\" VALUES ($1, 'incident-active', 'open-active', 'Shared incident', 'active')"
    ))
    .bind(tenant_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(&format!(
        "INSERT INTO \"{signals}\" VALUES \
         ($1, 'signal-active-a', 'incident-active'), \
         ($1, 'signal-active-b', 'incident-active')"
    ))
    .bind(tenant_id)
    .execute(pool)
    .await
    .unwrap();
    let remains_active = DatabaseDeleteRequest::new(
        lease.token().clone(),
        LocalId::new("operations-write").unwrap(),
        BrokerResource::database_table(&signals).unwrap(),
        std::collections::BTreeMap::from([(
            "signal_id".into(),
            serde_json::json!("signal-active-a"),
        )]),
    )
    .with_event(DatabaseMutationEvent::post_mutation(
        LocalId::new("server-incident-closed").unwrap(),
        LocalId::new("server-incident").unwrap(),
        std::collections::BTreeMap::from([(
            "incident_id".into(),
            serde_json::json!("incident-active"),
        )]),
    ));
    assert_eq!(
        mutation(
            &broker,
            &caller,
            BrokerRequest::DatabaseDelete(remains_active)
        )
        .await,
        1
    );
    let active_state: (i64, String) = sqlx::query_as(&format!(
        "SELECT \
           (SELECT COUNT(*) FROM \"{signals}\" WHERE tenant_id = $1 AND incident_id = 'incident-active'), \
           (SELECT status FROM \"{incidents}\" WHERE tenant_id = $1 AND incident_id = 'incident-active')"
    ))
    .bind(tenant_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(active_state, (1, "active".to_string()));
    let before_close_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_event_outbox WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(before_close_count, 1);

    sqlx::query(&format!(
        "INSERT INTO \"{incidents}\" VALUES \
           ($1, 'incident-ambiguous', 'open-a', 'Ambiguous A', 'active'), \
           ($1, 'incident-ambiguous', 'open-b', 'Ambiguous B', 'active')"
    ))
    .bind(tenant_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(&format!(
        "INSERT INTO \"{signals}\" VALUES ($1, 'signal-ambiguous', 'incident-ambiguous')"
    ))
    .bind(tenant_id)
    .execute(pool)
    .await
    .unwrap();
    let ambiguous = DatabaseDeleteRequest::new(
        lease.token().clone(),
        LocalId::new("operations-write").unwrap(),
        BrokerResource::database_table(&signals).unwrap(),
        std::collections::BTreeMap::from([(
            "signal_id".into(),
            serde_json::json!("signal-ambiguous"),
        )]),
    )
    .with_event(DatabaseMutationEvent::post_mutation(
        LocalId::new("server-incident-closed").unwrap(),
        LocalId::new("server-incident").unwrap(),
        std::collections::BTreeMap::from([(
            "incident_id".into(),
            serde_json::json!("incident-ambiguous"),
        )]),
    ));
    let response = broker
        .handle(&caller, BrokerRequest::DatabaseDelete(ambiguous))
        .await;
    assert!(matches!(
        response,
        BrokerResponse::Error(ref error)
            if error.code.as_str() == "knowledge-event-snapshot-ambiguous"
    ));
    let rollback: (i64, i64) = sqlx::query_as(&format!(
        "SELECT \
           (SELECT COUNT(*) FROM \"{signals}\" WHERE tenant_id = $1 AND signal_id = 'signal-ambiguous'), \
           (SELECT COUNT(*) FROM \"{incidents}\" WHERE tenant_id = $1 AND incident_id = 'incident-ambiguous' AND status = 'active')"
    ))
    .bind(tenant_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(rollback, (1, 2));
    let outbox_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_event_outbox WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(outbox_count, 1);

    drop(lease);
    drop(broker);
    drop(runtime);
    harness.cleanup().await;
}

async fn pg_available() -> bool {
    let database_url = std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
        .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .is_ok()
}

async fn select(
    broker: Arc<dyn BundleBroker>,
    caller: &BrokerCaller,
    lease: gadgetron_bundle_sdk::InvocationLeaseToken,
    table: &str,
) -> Vec<std::collections::BTreeMap<String, serde_json::Value>> {
    match broker
        .handle(
            caller,
            BrokerRequest::DatabaseSelect(select_request(lease, table)),
        )
        .await
    {
        BrokerResponse::DatabaseRows(rows) => rows.rows,
        response => panic!("expected database rows, got {response:?}"),
    }
}

async fn mutation(
    broker: &Arc<dyn BundleBroker>,
    caller: &BrokerCaller,
    request: BrokerRequest,
) -> u32 {
    match broker.handle(caller, request).await {
        BrokerResponse::DatabaseMutation(result) => result.affected_rows,
        response => panic!("expected database mutation, got {response:?}"),
    }
}

fn select_request(
    lease: gadgetron_bundle_sdk::InvocationLeaseToken,
    table: &str,
) -> DatabaseSelectRequest {
    DatabaseSelectRequest::new(
        lease,
        LocalId::new("telemetry-read").unwrap(),
        BrokerResource::database_table(table).unwrap(),
        ["host_id".to_string(), "label".to_string()],
    )
    .with_limit(10)
}

fn package_contract(table: &str, view: &str, unsafe_table: &str) -> ValidatedPackageContract {
    let source = format!(
        r#"
manifest_version = 1

[bundle]
id = "server-administrator"
version = "0.1.0"
publisher = "gadgetron.project"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/server-administrator"
entry_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 30

[[permissions]]
id = "telemetry-read"
kind = "database"
description = "Read current host telemetry"
resources = ["postgres:table:{table}", "postgres:table:{view}", "postgres:table:{unsafe_table}"]

[[permissions]]
id = "telemetry-write"
kind = "database"
description = "Write current host telemetry"
resources = ["postgres:table:{table}", "postgres:table:{view}"]
"#
    );
    ValidatedPackageContract::parse(&source, &Version::new(1, 0, 0)).unwrap()
}

fn knowledge_event_package_contract(
    mutation_table: &str,
    snapshot_view: &str,
) -> ValidatedPackageContract {
    let source = format!(
        r#"
manifest_version = 1

[bundle]
id = "server-administrator"
version = "0.1.0"
publisher = "gadgetron.project"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/server-administrator"
entry_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 30

[[permissions]]
id = "operations-write"
kind = "database"
description = "Close an incident signal"
resources = ["postgres:table:{mutation_table}"]

[[permissions]]
id = "incident-snapshot-read"
kind = "database"
description = "Read the exact post-close incident snapshot"
resources = ["postgres:table:{snapshot_view}"]

[[capabilities.knowledge_events]]
id = "incident-closed-knowledge"
event_kind = "server-incident-closed"
subject_kind = "server-incident"
snapshot_permission_id = "incident-snapshot-read"
snapshot_resource = "postgres:table:{snapshot_view}"
snapshot_fields = ["incident_id", "revision", "title", "snapshot"]
subject_id_field = "incident_id"
subject_revision_field = "revision"
title_field = "title"
researcher_bundle = "server-operations-intelligence"
researcher_role = "server-incident-researcher"
output_vault_bundle = "server-administrator"
knowledge_schema_id = "server.knowledge"
source_path_prefix = "incidents"
"#,
    );
    ValidatedPackageContract::parse(&source, &Version::new(1, 0, 0)).unwrap()
}
