use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BundleMigrationError {
    #[error("Bundle migration configuration is invalid: {0}")]
    Config(String),

    #[error("signed Bundle package validation failed: {0}")]
    Package(#[from] gadgetron_bundle_host::BundleHostError),

    #[error("Bundle migration filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("Bundle migration database query failed: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("Core SQLx migration operation failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("Core SQLx migration {version} checksum mismatch")]
    CoreChecksumMismatch { version: i64 },

    #[error("applied SQLx migration {version} has no Core or adopted Bundle owner; {detail}")]
    MissingLegacyAdopter { version: i64, detail: String },

    #[error("legacy SQLx migration {version} has ambiguous Bundle owners: {owners:?}")]
    AmbiguousLegacyAdopter { version: i64, owners: Vec<String> },

    #[error("Bundle migration ownership conflict: {0}")]
    Ownership(String),

    #[error("Bundle {bundle_id:?} migration history changed: {detail}")]
    HistoryDrift { bundle_id: String, detail: String },

    #[error("Bundle {bundle_id:?} migration {migration_id:?} is not transactional")]
    NonTransactional {
        bundle_id: String,
        migration_id: String,
    },
}

pub type Result<T> = std::result::Result<T, BundleMigrationError>;
