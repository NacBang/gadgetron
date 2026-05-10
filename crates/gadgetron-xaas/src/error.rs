use gadgetron_core::error::{DatabaseErrorKind, GadgetronError};

pub(crate) fn sqlx_to_gadgetron(e: sqlx::Error) -> GadgetronError {
    let kind = match &e {
        sqlx::Error::RowNotFound => DatabaseErrorKind::RowNotFound,
        sqlx::Error::PoolTimedOut => DatabaseErrorKind::PoolTimeout,
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => DatabaseErrorKind::ConnectionFailed,
        sqlx::Error::Database(_) => DatabaseErrorKind::Constraint,
        sqlx::Error::Migrate(_) => DatabaseErrorKind::MigrationFailed,
        _ => DatabaseErrorKind::Other,
    };
    GadgetronError::Database {
        kind,
        message: e.to_string(),
    }
}
