use std::{
    borrow::Cow,
    error::Error,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use ed25519_dalek::{Signer, SigningKey};
use gadgetron_bundle_migrations::{BundleMigrationError, BundleMigrationManager};
use semver::Version;
use sha2::{Digest, Sha256};
use sqlx::{
    migrate::{Migrate, Migration, MigrationType, Migrator},
    postgres::{PgConnectOptions, PgPoolOptions},
    Connection, Executor, PgConnection, PgPool,
};
use uuid::Uuid;

static CORE_MIGRATOR: Migrator = sqlx::migrate!("../gadgetron-xaas/migrations");

const LEGACY_VERSIONS: [i64; 7] = [
    20260421000001,
    20260423000001,
    20260423000002,
    20260423000003,
    20260503000003,
    20260611000001,
    20260616000001,
];
const SUPPORTED_0_5_57_CORE_HEAD: i64 = 20260710000005;
const SUPPORTED_0_5_57_CORE_SOURCE_SHA256: &str =
    "2c748e97a4ba17b11acef2c2f0cc67bc47fa389ce7f138325ce12683a50d3637";

struct DatabaseGuard {
    maintenance: PgConnectOptions,
    names: Vec<String>,
}

impl DatabaseGuard {
    async fn new(base_url: &str) -> Result<Self, Box<dyn Error>> {
        let maintenance = PgConnectOptions::from_str(base_url)?;
        Ok(Self {
            maintenance,
            names: Vec::new(),
        })
    }

    async fn create(&mut self, label: &str) -> Result<PgPool, Box<dyn Error>> {
        self.create_named(label).await.map(|(_, pool)| pool)
    }

    async fn create_named(&mut self, label: &str) -> Result<(String, PgPool), Box<dyn Error>> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let name = format!(
            "gadgetron_bundle_test_{label}_{}_{nonce:x}",
            std::process::id()
        );
        assert!(name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'));
        let mut connection = PgConnection::connect_with(&self.maintenance).await?;
        connection
            .execute(format!("CREATE DATABASE \"{name}\"").as_str())
            .await?;
        self.names.push(name.clone());
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(self.maintenance.clone().database(&name))
            .await?;
        Ok((name, pool))
    }
}

impl Drop for DatabaseGuard {
    fn drop(&mut self) {
        let maintenance = self.maintenance.clone();
        let names = std::mem::take(&mut self.names);
        let _ = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                let Ok(mut connection) = PgConnection::connect_with(&maintenance).await else {
                    return;
                };
                for name in names {
                    let _ = sqlx::query(
                        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                         WHERE datname = $1 AND pid <> pg_backend_pid()",
                    )
                    .bind(&name)
                    .execute(&mut connection)
                    .await;
                    let _ = connection
                        .execute(format!("DROP DATABASE IF EXISTS \"{name}\"").as_str())
                        .await;
                }
            });
        })
        .join();
    }
}

fn stage_signed_server_administrator(root: &Path) -> Result<String, Box<dyn Error>> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = workspace.join("bundles/server-administrator");
    let package_root = root.join("server-administrator");
    let migration_root = package_root.join("migrations");
    fs::create_dir_all(&migration_root)?;
    for entry in fs::read_dir(source.join("migrations"))? {
        let entry = entry?;
        fs::copy(entry.path(), migration_root.join(entry.file_name()))?;
    }

    let catalog = fs::read_to_string(source.join("catalog.template.toml"))?;
    let package = fs::read_to_string(source.join("package.template.toml"))?.replace(
        "@ENTRY_SHA256@",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    fs::write(package_root.join("bundle.toml"), &catalog)?;
    fs::write(package_root.join("package.toml"), &package)?;

    let signing_key = SigningKey::from_bytes(&[11_u8; 32]);
    fs::write(
        package_root.join("catalog.sig"),
        hex::encode(signing_key.sign(catalog.as_bytes()).to_bytes()),
    )?;
    fs::write(
        package_root.join("package.sig"),
        hex::encode(signing_key.sign(package.as_bytes()).to_bytes()),
    )?;
    Ok(hex::encode(signing_key.verifying_key().to_bytes()))
}

fn server_migration_count() -> Result<usize, Box<dyn Error>> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Ok(
        fs::read_dir(workspace.join("bundles/server-administrator/migrations"))?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|value| value == "sql"))
            .count(),
    )
}

fn manager(
    pool: PgPool,
    bundles_dir: PathBuf,
    public_key: String,
) -> Result<Arc<BundleMigrationManager>, Box<dyn Error>> {
    Ok(Arc::new(BundleMigrationManager::new(
        pool,
        &CORE_MIGRATOR,
        Some(bundles_dir),
        vec![public_key],
        Version::parse(env!("CARGO_PKG_VERSION"))?,
    )?))
}

async fn legacy_sqlx_count(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE version = ANY($1)")
        .bind(LEGACY_VERSIONS.as_slice())
        .fetch_one(pool)
        .await
}

async fn domain_tables_exist(pool: &PgPool) -> Result<bool, sqlx::Error> {
    for name in [
        "host_metrics",
        "log_findings",
        "host_stats_latest",
        "alert_state",
    ] {
        let relation: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(name)
            .fetch_one(pool)
            .await?;
        if relation.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn operational_tables_exist(pool: &PgPool) -> Result<bool, sqlx::Error> {
    for name in [
        "server_assets_latest",
        "server_operation_outcomes",
        "server_job_runs",
    ] {
        let relation: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(name)
            .fetch_one(pool)
            .await?;
        if relation.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn verify_supported_0_5_57_core_source() -> Result<(), Box<dyn Error>> {
    let migration_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../gadgetron-xaas/migrations");
    let mut paths: Vec<_> = fs::read_dir(migration_root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.split('_').next())
                .and_then(|value| value.parse::<i64>().ok())
                .is_some_and(|version| version <= SUPPORTED_0_5_57_CORE_HEAD)
        })
        .collect();
    paths.sort();
    let mut aggregate = Sha256::new();
    for path in &paths {
        aggregate.update(
            path.file_name()
                .expect("migration path has filename")
                .to_string_lossy()
                .as_bytes(),
        );
        aggregate.update([0]);
        aggregate.update(fs::read(path)?);
        aggregate.update([0]);
    }
    let actual = hex::encode(aggregate.finalize());
    if paths.len() != 29 || actual != SUPPORTED_0_5_57_CORE_SOURCE_SHA256 {
        return Err(format!(
            "0.5.57 Core migration fixture drifted: files={}, sha256={actual}",
            paths.len()
        )
        .into());
    }
    Ok(())
}

async fn install_supported_0_5_57_core(pool: &PgPool) -> Result<(), Box<dyn Error>> {
    let mut connection = pool.acquire().await?;
    connection.ensure_migrations_table().await?;
    for migration in CORE_MIGRATOR
        .iter()
        .filter(|migration| migration.version <= SUPPORTED_0_5_57_CORE_HEAD)
    {
        (*connection).apply(migration).await?;
    }
    Ok(())
}

fn docker_postgres_command(database: &str, sql: &str) -> Result<(), Box<dyn Error>> {
    let container = std::env::var("GADGETRON_POSTGRES_CONTAINER")
        .unwrap_or_else(|_| "gadgetron-pg".to_string());
    let user = std::env::var("GADGETRON_POSTGRES_USER").unwrap_or_else(|_| "gadgetron".to_string());
    let output = Command::new("docker")
        .args([
            "exec",
            &container,
            "psql",
            "--set=ON_ERROR_STOP=1",
            "--username",
            &user,
            "--dbname",
            database,
            "--command",
            sql,
        ])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "PostgreSQL restore command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

fn docker_pg_dump_restore(source: &str, target: &str) -> Result<(), Box<dyn Error>> {
    let container = std::env::var("GADGETRON_POSTGRES_CONTAINER")
        .unwrap_or_else(|_| "gadgetron-pg".to_string());
    let user = std::env::var("GADGETRON_POSTGRES_USER").unwrap_or_else(|_| "gadgetron".to_string());
    let dump = Command::new("docker")
        .args([
            "exec",
            &container,
            "pg_dump",
            "--username",
            &user,
            "--format=custom",
            "--no-owner",
            "--no-acl",
            source,
        ])
        .output()?;
    if !dump.status.success() {
        return Err(format!("pg_dump failed: {}", String::from_utf8_lossy(&dump.stderr)).into());
    }

    docker_postgres_command(
        target,
        "CREATE EXTENSION IF NOT EXISTS timescaledb; SELECT timescaledb_pre_restore();",
    )?;
    let mut restore = Command::new("docker")
        .args([
            "exec",
            "--interactive",
            &container,
            "pg_restore",
            "--username",
            &user,
            "--dbname",
            target,
            "--no-owner",
            "--no-acl",
            "--exit-on-error",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    restore
        .stdin
        .take()
        .ok_or("pg_restore stdin is unavailable")?
        .write_all(&dump.stdout)?;
    let output = restore.wait_with_output()?;
    if !output.status.success() {
        return Err(format!(
            "pg_restore failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    docker_postgres_command(target, "SELECT timescaledb_post_restore();")?;
    Ok(())
}

fn database_url_for(base_url: &str, database: &str) -> Result<String, Box<dyn Error>> {
    let query_at = base_url.find('?').unwrap_or(base_url.len());
    let base = &base_url[..query_at];
    let query = &base_url[query_at..];
    let scheme_at = base
        .find("://")
        .ok_or("release binary probe requires a PostgreSQL URL")?;
    let database_at = base[scheme_at + 3..]
        .rfind('/')
        .map(|offset| scheme_at + 3 + offset)
        .ok_or("release binary probe PostgreSQL URL has no database path")?;
    if !database
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("release binary probe database name is invalid".into());
    }
    Ok(format!("{}{database}{query}", &base[..=database_at]))
}

#[test]
fn release_probe_rewrites_only_the_database_path() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        database_url_for(
            "postgresql://gadgetron:secret@127.0.0.1:5432/gadgetron_dev?sslmode=disable",
            "gadgetron_restore_123",
        )?,
        "postgresql://gadgetron:secret@127.0.0.1:5432/gadgetron_restore_123?sslmode=disable"
    );
    assert!(database_url_for("host=127.0.0.1 dbname=gadgetron", "restore").is_err());
    assert!(database_url_for("postgresql:///gadgetron", "../escape").is_err());
    Ok(())
}

fn ready_on(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}")
            .parse()
            .expect("loopback address"),
        Duration::from_millis(200),
    ) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    if stream
        .write_all(b"GET /ready HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut response = [0_u8; 4096];
    let Ok(read) = stream.read(&mut response) else {
        return false;
    };
    response[..read].starts_with(b"HTTP/1.1 200") || response[..read].starts_with(b"HTTP/1.0 200")
}

fn probe_release_binary(
    binary: &Path,
    database_url: &str,
    bundles_dir: &Path,
    public_key: &str,
) -> Result<(), Box<dyn Error>> {
    if !binary.is_file() {
        return Err(format!("release binary is missing: {}", binary.display()).into());
    }
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let workspace = tempfile::tempdir()?;
    let state_dir = workspace.path().join("bundle-state");
    let wiki_dir = workspace.path().join("wiki");
    let vault_dir = workspace.path().join("vaults");
    fs::create_dir_all(&state_dir)?;
    fs::create_dir_all(&wiki_dir)?;
    fs::create_dir_all(&vault_dir)?;
    let config = workspace.path().join("gadgetron.toml");
    fs::write(
        &config,
        format!(
            "[server]\nbind = \"127.0.0.1:{port}\"\n\
             [web]\nbundles_dir = \"{}\"\nbundle_state_dir = \"{}\"\n\
             [web.bundle_signing]\npublic_keys_hex = [\"{public_key}\"]\nrequire_signature = true\n\
             [knowledge]\nwiki_path = \"{}\"\nvault_path = \"{}\"\nwiki_autocommit = false\n",
            bundles_dir.display(),
            state_dir.display(),
            wiki_dir.display(),
            vault_dir.display(),
        ),
    )?;
    let log_path = workspace.path().join("gadgetron.log");
    let log = fs::File::create(&log_path)?;
    let mut child = Command::new(binary)
        .args([
            "serve",
            "--config",
            config
                .to_str()
                .ok_or("release probe config path is not UTF-8")?,
            "--bind",
            &format!("127.0.0.1:{port}"),
        ])
        .env("GADGETRON_DATABASE_URL", database_url)
        .env("HOME", workspace.path())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if ready_on(port) {
                return Ok(());
            }
            if let Some(status) = child.try_wait()? {
                return Err(format!("release binary exited before /ready: {status}").into());
            }
            if Instant::now() >= deadline {
                return Err("release binary /ready probe timed out".into());
            }
            thread::sleep(Duration::from_millis(100));
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[derive(Debug, PartialEq, Eq)]
struct VaultCheckpoint {
    git_head: String,
    files: Vec<(String, String, u64)>,
    checksum: String,
}

fn run_git(repository: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn collect_vault_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, String, u64)>,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        if path.is_dir() {
            collect_vault_files(root, &path, files)?;
            continue;
        }
        let bytes = fs::read(&path)?;
        files.push((
            path.strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/"),
            hex::encode(Sha256::digest(&bytes)),
            u64::try_from(bytes.len())?,
        ));
    }
    Ok(())
}

fn vault_checkpoint(repository: &Path) -> Result<VaultCheckpoint, Box<dyn Error>> {
    let mut files = Vec::new();
    collect_vault_files(repository, repository, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut aggregate = Sha256::new();
    for (path, sha256, bytes) in &files {
        aggregate.update(path.as_bytes());
        aggregate.update([0]);
        aggregate.update(sha256.as_bytes());
        aggregate.update([0]);
        aggregate.update(bytes.to_le_bytes());
    }
    Ok(VaultCheckpoint {
        git_head: run_git(repository, &["rev-parse", "HEAD"])?,
        files,
        checksum: hex::encode(aggregate.finalize()),
    })
}

fn create_vault_checkpoint(
    root: &Path,
    tenant_id: Uuid,
    space_id: Uuid,
    note_path: &str,
    note_bytes: &[u8],
) -> Result<(PathBuf, String), Box<dyn Error>> {
    let repository = root
        .join("tenants")
        .join(tenant_id.to_string())
        .join("vault");
    let domain = repository
        .join("spaces")
        .join(space_id.to_string())
        .join("domains/server-administrator");
    fs::create_dir_all(domain.join("notes"))?;
    fs::create_dir_all(domain.join("sources"))?;
    fs::create_dir_all(domain.join("_attachments"))?;
    fs::create_dir_all(repository.join(".gadgetron"))?;
    fs::write(
        repository.join(".gadgetron/layout.json"),
        format!("{{\n  \"layout_version\": 1,\n  \"tenant_id\": \"{tenant_id}\"\n}}\n"),
    )?;
    fs::write(
        domain.join("_domain.json"),
        format!(
            "{{\n  \"layout_version\": 1,\n  \"tenant_id\": \"{tenant_id}\",\n  \"space_id\": \"{space_id}\",\n  \"home_bundle_id\": \"server-administrator\"\n}}\n"
        ),
    )?;
    fs::write(domain.join(note_path), note_bytes)?;

    run_git(&repository, &["init", "--quiet"])?;
    run_git(
        &repository,
        &["config", "user.name", "Gadgetron Release Gate"],
    )?;
    run_git(
        &repository,
        &["config", "user.email", "release-gate@gadgetron.invalid"],
    )?;
    run_git(&repository, &["add", "."])?;
    run_git(
        &repository,
        &["commit", "--quiet", "-m", "fixture: R4.2 backup checkpoint"],
    )?;
    Ok((repository, hex::encode(Sha256::digest(note_bytes))))
}

fn clone_vault_checkpoint(
    source: &Path,
    target_root: &Path,
    tenant_id: Uuid,
) -> Result<PathBuf, Box<dyn Error>> {
    let target = target_root
        .join("tenants")
        .join(tenant_id.to_string())
        .join("vault");
    fs::create_dir_all(target.parent().ok_or("Vault target has no parent")?)?;
    let output = Command::new("git")
        .args(["clone", "--quiet"])
        .arg(source)
        .arg(&target)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(target)
}

#[derive(Debug, PartialEq)]
struct LifecycleInvariant {
    tenant_name: String,
    user_count: i64,
    conversation: (String, Option<String>, String, Option<String>),
    native_session_count: i64,
    host_metric: f64,
    core_migration_head: i64,
    bundle_migrations: (i64, i64),
    knowledge_object: (String, Option<String>, i64),
}

async fn lifecycle_invariant(
    pool: &PgPool,
    tenant_id: Uuid,
    conversation_id: Uuid,
    object_id: Uuid,
) -> Result<LifecycleInvariant, Box<dyn Error>> {
    Ok(LifecycleInvariant {
        tenant_name: sqlx::query_scalar("SELECT name FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await?,
        user_count: sqlx::query_scalar("SELECT count(*) FROM users WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await?,
        conversation: sqlx::query_as(
            "SELECT title, agent_backend, agent_model, agent_effort \
             FROM conversations WHERE id = $1 AND tenant_id = $2",
        )
        .bind(conversation_id)
        .bind(tenant_id)
        .fetch_one(pool)
        .await?,
        native_session_count: sqlx::query_scalar(
            "SELECT count(*) FROM conversation_agent_sessions WHERE conversation_id = $1",
        )
        .bind(conversation_id)
        .fetch_one(pool)
        .await?,
        host_metric: sqlx::query_scalar(
            "SELECT value FROM host_metrics WHERE tenant_id = $1 AND metric = 'cpu.utilization'",
        )
        .bind(tenant_id)
        .fetch_one(pool)
        .await?,
        core_migration_head: sqlx::query_scalar(
            "SELECT max(version) FROM _sqlx_migrations WHERE success",
        )
        .fetch_one(pool)
        .await?,
        bundle_migrations: sqlx::query_as(
            "SELECT count(*), count(*) FILTER (WHERE adopted_from_sqlx) \
             FROM gadgetron_bundle_migrations WHERE bundle_id = 'server-administrator'",
        )
        .fetch_one(pool)
        .await?,
        knowledge_object: sqlx::query_as(
            "SELECT path, content_hash, revision FROM knowledge_objects \
             WHERE id = $1 AND tenant_id = $2",
        )
        .bind(object_id)
        .bind(tenant_id)
        .fetch_one(pool)
        .await?,
    })
}

async fn install_legacy_sqlx_rows(pool: &PgPool) -> Result<(), Box<dyn Error>> {
    let migration_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bundles/server-administrator/migrations");
    let mut connection = pool.acquire().await?;
    for version in LEGACY_VERSIONS {
        let path = fs::read_dir(&migration_root)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&version.to_string()))
            })
            .ok_or_else(|| format!("legacy fixture migration {version} is missing"))?;
        let description = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("legacy_bundle")
            .to_string();
        let migration = Migration::new(
            version,
            Cow::Owned(description),
            MigrationType::Simple,
            Cow::Owned(fs::read_to_string(path)?),
            false,
        );
        (*connection).apply(&migration).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "release gate: requires PostgreSQL with CREATEDB, pgvector and TimescaleDB"]
async fn core_bundle_and_legacy_adoption_matrix() -> Result<(), Box<dyn Error>> {
    let database_url = std::env::var("GADGETRON_DATABASE_URL")?;
    let mut databases = DatabaseGuard::new(&database_url).await?;
    let package_workspace = tempfile::tempdir()?;
    let bundles_dir = package_workspace.path().join("bundles");
    fs::create_dir_all(&bundles_dir)?;
    let public_key = stage_signed_server_administrator(&bundles_dir)?;
    let expected_bundle_migrations = server_migration_count()?;

    // Fresh Core never creates Server Administrator tables or legacy SQLx rows.
    let fresh = databases.create("fresh").await?;
    let fresh_manager = manager(fresh.clone(), bundles_dir.clone(), public_key.clone())?;
    let core_report = fresh_manager.migrate_core().await?;
    assert!(core_report.core_applied > 0);
    assert_eq!(legacy_sqlx_count(&fresh).await?, 0);
    assert!(!domain_tables_exist(&fresh).await?);
    assert!(!operational_tables_exist(&fresh).await?);

    // Explicit enable-time application owns all legacy rows plus the current
    // operational vertical in the Bundle ledger.
    let apply_report = fresh_manager
        .clone()
        .apply_bundle("server-administrator".into())
        .await?;
    assert_eq!(apply_report.newly_applied, expected_bundle_migrations);
    assert_eq!(legacy_sqlx_count(&fresh).await?, 0);
    assert!(domain_tables_exist(&fresh).await?);
    assert!(operational_tables_exist(&fresh).await?);
    let (fresh_rows, fresh_adopted): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE adopted_from_sqlx) \
         FROM gadgetron_bundle_migrations WHERE bundle_id = 'server-administrator'",
    )
    .fetch_one(&fresh)
    .await?;
    assert_eq!(
        (fresh_rows, fresh_adopted),
        (i64::try_from(expected_bundle_migrations)?, 0)
    );
    let repeat = fresh_manager
        .clone()
        .apply_bundle("server-administrator".into())
        .await?;
    assert_eq!(
        (repeat.already_applied, repeat.newly_applied),
        (expected_bundle_migrations, 0)
    );

    // A legacy SQLx database fails closed without its signed compatibility package.
    let legacy = databases.create("legacy").await?;
    CORE_MIGRATOR.run(&legacy).await?;
    install_legacy_sqlx_rows(&legacy).await?;
    assert_eq!(legacy_sqlx_count(&legacy).await?, 7);
    assert!(domain_tables_exist(&legacy).await?);
    assert!(!operational_tables_exist(&legacy).await?);
    let empty_packages = tempfile::tempdir()?;
    let missing_manager = manager(
        legacy.clone(),
        empty_packages.path().to_path_buf(),
        public_key.clone(),
    )?;
    assert!(matches!(
        missing_manager.migrate_core().await,
        Err(BundleMigrationError::MissingLegacyAdopter { .. })
    ));

    // A correctly signed manifest with modified SQL bytes is also rejected.
    fs::write(
        bundles_dir
            .join("server-administrator")
            .join("migrations/20260423000001_log_findings.sql"),
        "SELECT 'tampered';\n",
    )?;
    let tampered_manager = manager(legacy.clone(), bundles_dir.clone(), public_key.clone())?;
    assert!(matches!(
        tampered_manager.migrate_core().await,
        Err(BundleMigrationError::MissingLegacyAdopter { .. })
    ));

    // Restoring the exact signed bytes adopts without re-executing SQL.
    stage_signed_server_administrator(&bundles_dir)?;
    let adoption_manager = manager(legacy.clone(), bundles_dir, public_key)?;
    let adoption = adoption_manager.migrate_core().await?;
    assert_eq!(adoption.legacy_adopted, 7);
    assert!(!operational_tables_exist(&legacy).await?);
    let upgraded = adoption_manager
        .clone()
        .apply_bundle("server-administrator".into())
        .await?;
    assert_eq!(
        (upgraded.already_applied, upgraded.newly_applied),
        (
            LEGACY_VERSIONS.len(),
            expected_bundle_migrations - LEGACY_VERSIONS.len()
        )
    );
    assert!(operational_tables_exist(&legacy).await?);
    let (legacy_rows, legacy_adopted): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE adopted_from_sqlx) \
         FROM gadgetron_bundle_migrations WHERE bundle_id = 'server-administrator'",
    )
    .fetch_one(&legacy)
    .await?;
    assert_eq!(
        (legacy_rows, legacy_adopted),
        (
            i64::try_from(expected_bundle_migrations)?,
            i64::try_from(LEGACY_VERSIONS.len())?
        )
    );
    let idempotent = adoption_manager.migrate_core().await?;
    assert_eq!((idempotent.core_applied, idempotent.legacy_adopted), (0, 0));

    fresh.close().await;
    legacy.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "release gate: requires PostgreSQL with CREATEDB, pgvector, TimescaleDB and Docker pg_dump"]
async fn supported_0_5_57_upgrade_dump_restore_and_vault_checkpoint() -> Result<(), Box<dyn Error>>
{
    verify_supported_0_5_57_core_source()?;
    let database_url = std::env::var("GADGETRON_DATABASE_URL")?;
    let mut databases = DatabaseGuard::new(&database_url).await?;
    let (legacy_name, legacy) = databases.create_named("v057").await?;
    install_supported_0_5_57_core(&legacy).await?;
    install_legacy_sqlx_rows(&legacy).await?;

    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let conversation_id = Uuid::new_v4();
    let host_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'R4.2 upgrade tenant')")
        .bind(tenant_id)
        .execute(&legacy)
        .await?;
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1, $2, 'upgrade-admin@example.invalid', 'Upgrade Admin', 'admin', 'fixture')",
    )
    .bind(user_id)
    .bind(tenant_id)
    .execute(&legacy)
    .await?;
    sqlx::query(
        "INSERT INTO conversations \
         (id, tenant_id, user_id, title, agent_backend, agent_model, agent_effort, agent_model_source) \
         VALUES ($1, $2, $3, 'Preserved 0.5.57 conversation', 'codex_exec', \
                 'gpt-5.6-sol', 'auto', 'default')",
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(&legacy)
    .await?;
    sqlx::query(
        "INSERT INTO conversation_agent_sessions \
         (conversation_id, tenant_id, user_id, backend, backend_session_id) \
         VALUES ($1, $2, $3, 'codex_exec', 'v057-native-session')",
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(&legacy)
    .await?;
    sqlx::query(
        "INSERT INTO host_metrics (tenant_id, host_id, ts, metric, value, unit, labels) \
         VALUES ($1, $2, '2026-07-14T00:00:00Z', 'cpu.utilization', 42.5, 'percent', \
                 '{\"source\":\"fixture\"}'::jsonb)",
    )
    .bind(tenant_id)
    .bind(host_id)
    .execute(&legacy)
    .await?;
    sqlx::query(
        "INSERT INTO host_stats_latest (host_id, tenant_id, stats, fetched_at) \
         VALUES ($1, $2, '{\"cpu\":42.5}'::jsonb, '2026-07-14T00:00:00Z')",
    )
    .bind(host_id)
    .bind(tenant_id)
    .execute(&legacy)
    .await?;

    let package_workspace = tempfile::tempdir()?;
    let bundles_dir = package_workspace.path().join("bundles");
    fs::create_dir_all(&bundles_dir)?;
    let public_key = stage_signed_server_administrator(&bundles_dir)?;
    let expected_bundle_migrations = server_migration_count()?;
    let release_binary = std::env::var_os("GADGETRON_RELEASE_BINARY").map(PathBuf::from);
    if let Some(binary) = release_binary.as_deref() {
        probe_release_binary(
            binary,
            &database_url_for(&database_url, &legacy_name)?,
            &bundles_dir,
            &public_key,
        )?;
    }
    let upgrade_manager = manager(legacy.clone(), bundles_dir.clone(), public_key.clone())?;
    let core_upgrade = upgrade_manager.migrate_core().await?;
    if release_binary.is_some() {
        assert_eq!(
            (core_upgrade.core_applied, core_upgrade.legacy_adopted),
            (0, 0)
        );
    } else {
        assert!(core_upgrade.core_applied > 0);
        assert_eq!(core_upgrade.legacy_adopted, LEGACY_VERSIONS.len());
    }
    let bundle_upgrade = upgrade_manager
        .clone()
        .apply_bundle("server-administrator".into())
        .await?;
    assert_eq!(
        bundle_upgrade.newly_applied,
        expected_bundle_migrations - LEGACY_VERSIONS.len()
    );

    let space_id = Uuid::new_v4();
    let vault_id = Uuid::new_v4();
    let object_id = Uuid::new_v4();
    let note_id = Uuid::new_v4();
    let note_path = format!("notes/{note_id}.md");
    let note_bytes =
        b"---\ntitle: Restored lifecycle note\n---\n\n[[R4.2]] preserves this knowledge.\n";
    let vault_source = tempfile::tempdir()?;
    let vault_restore = tempfile::tempdir()?;
    let (repository, note_content_hash) = create_vault_checkpoint(
        vault_source.path(),
        tenant_id,
        space_id,
        &note_path,
        note_bytes,
    )?;
    sqlx::query(
        "INSERT INTO knowledge_spaces (id, tenant_id, kind, title, owner_user_id) \
         VALUES ($1, $2, 'personal', 'Upgrade knowledge', $3)",
    )
    .bind(space_id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(&legacy)
    .await?;
    sqlx::query(
        "INSERT INTO knowledge_vaults \
         (id, tenant_id, space_id, home_bundle_id, knowledge_schema_id, schema_version) \
         VALUES ($1, $2, $3, 'server-administrator', 'server.operations', 1)",
    )
    .bind(vault_id)
    .bind(tenant_id)
    .bind(space_id)
    .execute(&legacy)
    .await?;
    sqlx::query(
        "INSERT INTO knowledge_objects \
         (id, tenant_id, vault_id, canonical_kind, path, content_hash, created_by) \
         VALUES ($1, $2, $3, 'lesson', $4, $5, $6)",
    )
    .bind(object_id)
    .bind(tenant_id)
    .bind(vault_id)
    .bind(&note_path)
    .bind(&note_content_hash)
    .bind(user_id)
    .execute(&legacy)
    .await?;

    let before_vault = vault_checkpoint(&repository)?;
    let restored_repository = clone_vault_checkpoint(&repository, vault_restore.path(), tenant_id)?;
    let after_vault = vault_checkpoint(&restored_repository)?;
    assert_eq!(before_vault, after_vault);

    let expected = lifecycle_invariant(&legacy, tenant_id, conversation_id, object_id).await?;
    assert_eq!(expected.tenant_name, "R4.2 upgrade tenant");
    assert_eq!(expected.user_count, 1);
    assert_eq!(expected.conversation.1.as_deref(), Some("codex_exec"));
    assert_eq!(expected.conversation.3.as_deref(), Some("auto"));
    assert_eq!(expected.native_session_count, 1);
    assert_eq!(expected.host_metric, 42.5);
    assert_eq!(
        expected.core_migration_head,
        CORE_MIGRATOR
            .iter()
            .map(|migration| migration.version)
            .max()
            .expect("current Core migration head")
    );
    assert_eq!(
        expected.bundle_migrations,
        (
            i64::try_from(expected_bundle_migrations)?,
            i64::try_from(LEGACY_VERSIONS.len())?
        )
    );
    assert_eq!(expected.knowledge_object.0, note_path);
    assert_eq!(
        expected.knowledge_object.1.as_deref(),
        Some(note_content_hash.as_str())
    );

    drop(upgrade_manager);
    legacy.close().await;
    let (restore_name, restored) = databases.create_named("restore").await?;
    docker_pg_dump_restore(&legacy_name, &restore_name)?;
    let actual = lifecycle_invariant(&restored, tenant_id, conversation_id, object_id).await?;
    assert_eq!(actual, expected);
    if let Some(binary) = release_binary.as_deref() {
        probe_release_binary(
            binary,
            &database_url_for(&database_url, &restore_name)?,
            &bundles_dir,
            &public_key,
        )?;
    }
    let restored_note = fs::read(
        restored_repository
            .join("spaces")
            .join(space_id.to_string())
            .join("domains/server-administrator")
            .join(&actual.knowledge_object.0),
    )?;
    assert_eq!(restored_note, note_bytes);
    assert_eq!(
        actual.knowledge_object.1.as_deref(),
        Some(hex::encode(Sha256::digest(&restored_note)).as_str())
    );
    restored.close().await;
    Ok(())
}
