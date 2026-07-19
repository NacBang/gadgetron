//! Core-owned migration gate for independently installed Bundles.
//!
//! Core keeps SQLx's global ledger for Core schema only. Bundle schema uses a
//! namespaced ledger and is applied transactionally immediately before runtime
//! enable. Historical domain revisions already present in `_sqlx_migrations`
//! are adopted only through an explicit, signed descriptor with an exact SQLx
//! checksum match.

mod error;

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    time::Instant,
};

use futures::TryStreamExt;
use gadgetron_bundle_host::SignedInstalledPackage;
use gadgetron_bundle_sdk::{BundleId, MigrationDescriptor, MigrationKind};
use semver::Version;
use sha2::{Digest, Sha384};
use sqlx::{
    migrate::{AppliedMigration, Migrate, Migration, Migrator},
    Connection, Executor, PgConnection, PgPool,
};

pub use error::{BundleMigrationError, Result};

const LEDGER_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS gadgetron_bundle_migrations (
    bundle_id TEXT NOT NULL,
    migration_id TEXT NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    first_applied_bundle_version TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('schema', 'index', 'data')),
    path TEXT NOT NULL,
    content_sha256 BYTEA NOT NULL CHECK (octet_length(content_sha256) = 32),
    legacy_sqlx_version BIGINT,
    legacy_sqlx_checksum BYTEA,
    adopted_from_sqlx BOOLEAN NOT NULL DEFAULT FALSE,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    execution_time_ns BIGINT NOT NULL DEFAULT 0 CHECK (execution_time_ns >= 0),
    PRIMARY KEY (bundle_id, migration_id),
    UNIQUE (bundle_id, revision),
    UNIQUE (legacy_sqlx_version),
    CHECK (
        (legacy_sqlx_version IS NULL AND legacy_sqlx_checksum IS NULL AND adopted_from_sqlx = FALSE)
        OR
        (legacy_sqlx_version IS NOT NULL AND legacy_sqlx_checksum IS NOT NULL)
    )
);
COMMENT ON TABLE gadgetron_bundle_migrations IS
    'Core-owned ownership ledger for transactional Bundle schema migrations and adopted legacy SQLx revisions';
"#;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreMigrationReport {
    pub core_applied: usize,
    pub legacy_adopted: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BundleMigrationReport {
    pub bundle_id: String,
    pub already_applied: usize,
    pub newly_applied: usize,
    pub legacy_adopted: usize,
}

#[derive(Clone)]
pub struct BundleMigrationManager {
    pool: PgPool,
    core_migrations: Arc<Vec<Migration>>,
    bundles_dir: Option<PathBuf>,
    public_keys_hex: Arc<Vec<String>>,
    core_version: Version,
}

impl BundleMigrationManager {
    pub fn new(
        pool: PgPool,
        core_migrator: &Migrator,
        bundles_dir: Option<impl Into<PathBuf>>,
        public_keys_hex: Vec<String>,
        core_version: Version,
    ) -> Result<Self> {
        let mut versions = BTreeSet::new();
        let mut core_migrations: Vec<_> = core_migrator.iter().cloned().collect();
        core_migrations.sort_by_key(|migration| migration.version);
        for migration in &core_migrations {
            if !versions.insert(migration.version) {
                return Err(BundleMigrationError::Config(format!(
                    "duplicate Core migration version {}",
                    migration.version
                )));
            }
        }
        Ok(Self {
            pool,
            core_migrations: Arc::new(core_migrations),
            bundles_dir: bundles_dir.map(Into::into),
            public_keys_hex: Arc::new(public_keys_hex),
            core_version,
        })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Validate/adopt every historical SQLx row, then apply Core-only schema.
    pub async fn migrate_core(&self) -> Result<CoreMigrationReport> {
        let mut connection = self.pool.acquire().await?;
        (*connection).lock().await?;
        let result = self.migrate_core_locked(&mut connection).await;
        let unlock = (*connection).unlock().await;
        match (result, unlock) {
            (Ok(report), Ok(())) => Ok(report),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }

    /// Apply one signed Bundle's pending migrations in a single transaction.
    /// This must complete before the runtime sandbox is launched.
    pub fn apply_bundle(
        self: Arc<Self>,
        bundle_id: String,
    ) -> Pin<Box<dyn Future<Output = Result<BundleMigrationReport>> + Send + 'static>> {
        Box::pin(async move {
            BundleId::new(bundle_id.clone()).map_err(|error| {
                BundleMigrationError::Config(format!("invalid Bundle id {bundle_id:?}: {error}"))
            })?;
            let package = self.load_package(&bundle_id)?;
            let resolved = resolve_migrations(&package)?;

            let mut connection = self.pool.acquire().await?;
            (*connection).lock().await?;
            let result = self
                .apply_bundle_locked(&mut connection, bundle_id, resolved)
                .await;
            let unlock = (*connection).unlock().await;
            match (result, unlock) {
                (Ok(report), Ok(())) => Ok(report),
                (Err(error), _) => Err(error),
                (Ok(_), Err(error)) => Err(error.into()),
            }
        })
    }

    async fn migrate_core_locked(
        &self,
        connection: &mut PgConnection,
    ) -> Result<CoreMigrationReport> {
        connection.ensure_migrations_table().await?;
        connection.execute(sqlx::raw_sql(LEDGER_DDL)).await?;
        if let Some(version) = connection.dirty_version().await? {
            return Err(BundleMigrationError::Config(format!(
                "Core SQLx ledger is dirty at migration {version}"
            )));
        }

        let applied = connection.list_applied_migrations().await?;
        let applied_by_version: BTreeMap<_, _> = applied
            .iter()
            .map(|migration| (migration.version, migration))
            .collect();
        let core_by_version: BTreeMap<_, _> = self
            .core_migrations
            .iter()
            .map(|migration| (migration.version, migration))
            .collect();
        for (version, core) in &core_by_version {
            if let Some(applied_migration) = applied_by_version.get(version) {
                if core.checksum.as_ref() != applied_migration.checksum.as_ref() {
                    return Err(BundleMigrationError::CoreChecksumMismatch { version: *version });
                }
            }
        }

        let mut ledger = load_ledger(connection).await?;
        validate_existing_adoptions(&ledger, &applied_by_version, &core_by_version)?;
        let missing: Vec<_> = applied
            .iter()
            .filter(|migration| !core_by_version.contains_key(&migration.version))
            .filter(|migration| {
                !ledger.iter().any(|row| {
                    row.adopted_from_sqlx && row.legacy_sqlx_version == Some(migration.version)
                })
            })
            .cloned()
            .collect();

        let mut adoptions = Vec::new();
        if !missing.is_empty() {
            let (candidates, package_errors) = self.scan_adoption_candidates();
            for applied_migration in &missing {
                let claimed: Vec<_> = candidates
                    .iter()
                    .filter(|candidate| {
                        candidate.legacy_sqlx_version == Some(applied_migration.version)
                    })
                    .collect();
                let matching: Vec<_> = claimed
                    .iter()
                    .copied()
                    .filter(|candidate| {
                        candidate.sqlx_checksum == applied_migration.checksum.as_ref()
                    })
                    .cloned()
                    .collect();
                match matching.as_slice() {
                    [candidate] => adoptions.push((candidate.clone(), applied_migration.clone())),
                    [] => {
                        let detail = if !claimed.is_empty() {
                            format!(
                                "signed adopter checksum mismatch from {:?}",
                                claimed
                                    .iter()
                                    .map(|candidate| candidate.bundle_id.as_str())
                                    .collect::<Vec<_>>()
                            )
                        } else if package_errors.is_empty() {
                            "stage the signed Bundle compatibility package before upgrade".into()
                        } else {
                            format!("installed package errors: {}", package_errors.join("; "))
                        };
                        return Err(BundleMigrationError::MissingLegacyAdopter {
                            version: applied_migration.version,
                            detail,
                        });
                    }
                    many => {
                        return Err(BundleMigrationError::AmbiguousLegacyAdopter {
                            version: applied_migration.version,
                            owners: many
                                .iter()
                                .map(|candidate| candidate.bundle_id.clone())
                                .collect(),
                        });
                    }
                }
            }
        }

        if !adoptions.is_empty() {
            let mut transaction = connection.begin().await?;
            for (candidate, applied_migration) in &adoptions {
                insert_ledger_row(
                    &mut transaction,
                    candidate,
                    true,
                    Some(applied_migration.checksum.to_vec()),
                    0,
                )
                .await?;
            }
            transaction.commit().await?;
            ledger = load_ledger(connection).await?;
            validate_existing_adoptions(&ledger, &applied_by_version, &core_by_version)?;
        }

        let mut core_applied = 0;
        for migration in self.core_migrations.iter().cloned() {
            if !applied_by_version.contains_key(&migration.version) {
                connection.apply(&migration).await?;
                core_applied += 1;
            }
        }
        Ok(CoreMigrationReport {
            core_applied,
            legacy_adopted: adoptions.len(),
        })
    }

    async fn apply_bundle_locked(
        &self,
        connection: &mut PgConnection,
        bundle_id: String,
        resolved: Vec<ResolvedMigration>,
    ) -> Result<BundleMigrationReport> {
        let core_report = self.migrate_core_locked(connection).await?;
        let existing: Vec<_> = load_ledger(connection)
            .await?
            .into_iter()
            .filter(|row| row.bundle_id == bundle_id)
            .collect();
        let by_id: BTreeMap<_, _> = resolved
            .iter()
            .map(|migration| (migration.migration_id.clone(), migration))
            .collect();
        for row in &existing {
            let Some(current) = by_id.get(&row.migration_id) else {
                return Err(BundleMigrationError::HistoryDrift {
                    bundle_id: bundle_id.clone(),
                    detail: format!(
                        "applied migration {:?} is missing from package history",
                        row.migration_id
                    ),
                });
            };
            validate_ledger_row(row, current)?;
        }

        let existing_ids: BTreeSet<_> = existing
            .iter()
            .map(|row| row.migration_id.clone())
            .collect();
        let pending: Vec<_> = resolved
            .iter()
            .filter(|migration| !existing_ids.contains(&migration.migration_id))
            .cloned()
            .collect();
        drop(by_id);
        drop(existing_ids);
        if pending.is_empty() {
            return Ok(BundleMigrationReport {
                bundle_id: bundle_id.clone(),
                already_applied: existing.len(),
                newly_applied: 0,
                legacy_adopted: core_report.legacy_adopted,
            });
        }

        let mut transaction = connection.begin().await?;
        for migration in &pending {
            if migration.sql.starts_with("-- no-transaction") {
                return Err(BundleMigrationError::NonTransactional {
                    bundle_id: bundle_id.clone(),
                    migration_id: migration.migration_id.clone(),
                });
            }
            let started = Instant::now();
            (&mut *transaction)
                .execute(sqlx::raw_sql(&migration.sql))
                .await?;
            let elapsed = started.elapsed().as_nanos().min(i64::MAX as u128) as i64;
            insert_ledger_row(&mut transaction, migration, false, None, elapsed).await?;
        }
        transaction.commit().await?;
        Ok(BundleMigrationReport {
            bundle_id,
            already_applied: existing.len(),
            newly_applied: pending.len(),
            legacy_adopted: core_report.legacy_adopted,
        })
    }

    fn load_package(&self, bundle_id: &str) -> Result<SignedInstalledPackage> {
        let root = self.bundles_dir.as_ref().ok_or_else(|| {
            BundleMigrationError::Config(
                "Bundle migrations require [web] bundles_dir to be configured".into(),
            )
        })?;
        Ok(SignedInstalledPackage::load(
            root.join(bundle_id),
            bundle_id,
            &self.core_version,
            &self.public_keys_hex,
        )?)
    }

    fn scan_adoption_candidates(&self) -> (Vec<ResolvedMigration>, Vec<String>) {
        let Some(root) = self.bundles_dir.as_ref() else {
            return (Vec::new(), Vec::new());
        };
        let Ok(entries) = std::fs::read_dir(root) else {
            return (
                Vec::new(),
                vec![format!("cannot read bundles directory {root:?}")],
            );
        };
        let mut candidates = Vec::new();
        let mut errors = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if id.starts_with('.') || !path.join("package.toml").is_file() {
                continue;
            }
            if BundleId::new(id.clone()).is_err() {
                continue;
            }
            match SignedInstalledPackage::load(
                &path,
                &id,
                &self.core_version,
                &self.public_keys_hex,
            ) {
                Ok(package) => match resolve_migrations(&package) {
                    Ok(migrations) => candidates.extend(
                        migrations
                            .into_iter()
                            .filter(|migration| migration.legacy_sqlx_version.is_some()),
                    ),
                    Err(error) => errors.push(format!("{id}: {error}")),
                },
                Err(error) => errors.push(format!("{id}: {error}")),
            }
        }
        (candidates, errors)
    }
}

#[derive(Debug, Clone)]
struct ResolvedMigration {
    bundle_id: String,
    bundle_version: String,
    migration_id: String,
    revision: i64,
    kind: String,
    path: String,
    content_sha256: Vec<u8>,
    sqlx_checksum: Vec<u8>,
    legacy_sqlx_version: Option<i64>,
    sql: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct LedgerRow {
    bundle_id: String,
    migration_id: String,
    revision: i64,
    kind: String,
    path: String,
    content_sha256: Vec<u8>,
    legacy_sqlx_version: Option<i64>,
    legacy_sqlx_checksum: Option<Vec<u8>>,
    adopted_from_sqlx: bool,
}

fn resolve_migrations(package: &SignedInstalledPackage) -> Result<Vec<ResolvedMigration>> {
    let manifest = package.contract().manifest();
    let mut migrations = Vec::with_capacity(manifest.capabilities.migrations.len());
    for descriptor in &manifest.capabilities.migrations {
        let bytes = package.verified_asset_bytes(&descriptor.path, &descriptor.sha256)?;
        let sql = String::from_utf8(bytes).map_err(|error| {
            BundleMigrationError::Config(format!(
                "Bundle {:?} migration {:?} is not UTF-8 SQL: {error}",
                manifest.bundle.id.as_str(),
                descriptor.id.as_str()
            ))
        })?;
        if sql.trim().is_empty() {
            return Err(BundleMigrationError::Config(format!(
                "Bundle {:?} migration {:?} is empty",
                manifest.bundle.id.as_str(),
                descriptor.id.as_str()
            )));
        }
        migrations.push(ResolvedMigration {
            bundle_id: manifest.bundle.id.to_string(),
            bundle_version: manifest.bundle.version.to_string(),
            migration_id: descriptor.id.to_string(),
            revision: descriptor.revision as i64,
            kind: migration_kind_name(descriptor)?,
            path: descriptor.path.to_string(),
            content_sha256: hex::decode(&descriptor.sha256).map_err(|error| {
                BundleMigrationError::Config(format!("invalid signed SHA-256: {error}"))
            })?,
            sqlx_checksum: Sha384::digest(sql.as_bytes()).to_vec(),
            legacy_sqlx_version: descriptor.legacy_sqlx_version,
            sql,
        });
    }
    migrations.sort_by_key(|migration| migration.revision);
    Ok(migrations)
}

fn migration_kind_name(descriptor: &MigrationDescriptor) -> Result<String> {
    let name = match descriptor.kind {
        MigrationKind::Schema => "schema",
        MigrationKind::Index => "index",
        MigrationKind::Data => "data",
        _ => {
            return Err(BundleMigrationError::Config(format!(
                "unsupported migration kind for {:?}",
                descriptor.id.as_str()
            )))
        }
    };
    Ok(name.to_string())
}

fn load_ledger(
    connection: &mut PgConnection,
) -> Pin<Box<dyn Future<Output = Result<Vec<LedgerRow>>> + Send + '_>> {
    Box::pin(async move {
        Ok(sqlx::query_as::<_, LedgerRow>(
            r#"SELECT bundle_id, migration_id, revision, kind, path,
                  content_sha256, legacy_sqlx_version,
                  legacy_sqlx_checksum, adopted_from_sqlx
           FROM gadgetron_bundle_migrations
           ORDER BY bundle_id, revision"#,
        )
        .fetch(connection)
        .try_collect()
        .await?)
    })
}

fn validate_existing_adoptions(
    ledger: &[LedgerRow],
    applied: &BTreeMap<i64, &AppliedMigration>,
    core: &BTreeMap<i64, &Migration>,
) -> Result<()> {
    for row in ledger.iter().filter(|row| row.adopted_from_sqlx) {
        let version = row.legacy_sqlx_version.ok_or_else(|| {
            BundleMigrationError::Ownership(format!(
                "adopted row {}:{} has no legacy SQLx version",
                row.bundle_id, row.migration_id
            ))
        })?;
        if core.contains_key(&version) {
            return Err(BundleMigrationError::Ownership(format!(
                "SQLx version {version} is owned by both Core and Bundle {}",
                row.bundle_id
            )));
        }
        let applied_migration = applied.get(&version).ok_or_else(|| {
            BundleMigrationError::Ownership(format!(
                "adopted Bundle migration {}:{} references absent SQLx version {version}",
                row.bundle_id, row.migration_id
            ))
        })?;
        if row.legacy_sqlx_checksum.as_deref() != Some(applied_migration.checksum.as_ref()) {
            return Err(BundleMigrationError::Ownership(format!(
                "adopted SQLx version {version} checksum drift for Bundle {}",
                row.bundle_id
            )));
        }
    }
    Ok(())
}

fn validate_ledger_row(row: &LedgerRow, current: &ResolvedMigration) -> Result<()> {
    let current_legacy_checksum = current
        .legacy_sqlx_version
        .map(|_| current.sqlx_checksum.as_slice());
    let matches = row.revision == current.revision
        && row.kind == current.kind
        && row.path == current.path
        && row.content_sha256 == current.content_sha256
        && row.legacy_sqlx_version == current.legacy_sqlx_version
        && row.legacy_sqlx_checksum.as_deref() == current_legacy_checksum;
    if matches {
        Ok(())
    } else {
        Err(BundleMigrationError::HistoryDrift {
            bundle_id: row.bundle_id.clone(),
            detail: format!(
                "migration {:?} no longer matches its applied revision/path/hash",
                row.migration_id
            ),
        })
    }
}

fn insert_ledger_row<'a>(
    executor: &'a mut PgConnection,
    migration: &'a ResolvedMigration,
    adopted_from_sqlx: bool,
    adopted_checksum: Option<Vec<u8>>,
    execution_time_ns: i64,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let legacy_checksum = if migration.legacy_sqlx_version.is_some() {
            Some(adopted_checksum.unwrap_or_else(|| migration.sqlx_checksum.clone()))
        } else {
            None
        };
        let query = sqlx::query(
            r#"INSERT INTO gadgetron_bundle_migrations
           (bundle_id, migration_id, revision, first_applied_bundle_version,
            kind, path, content_sha256, legacy_sqlx_version,
            legacy_sqlx_checksum, adopted_from_sqlx, execution_time_ns)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)"#,
        )
        .bind(&migration.bundle_id)
        .bind(&migration.migration_id)
        .bind(migration.revision)
        .bind(&migration.bundle_version)
        .bind(&migration.kind)
        .bind(&migration.path)
        .bind(&migration.content_sha256)
        .bind(migration.legacy_sqlx_version)
        .bind(legacy_checksum)
        .bind(adopted_from_sqlx)
        .bind(execution_time_ns);
        executor.execute(query).await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_bootstrap_is_namespaced_and_does_not_extend_sqlx() {
        assert!(LEDGER_DDL.contains("PRIMARY KEY (bundle_id, migration_id)"));
        assert!(LEDGER_DDL.contains("UNIQUE (bundle_id, revision)"));
        assert!(!LEDGER_DDL.contains("INSERT INTO _sqlx_migrations"));
    }
}
