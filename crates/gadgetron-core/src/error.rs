use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use uuid::Uuid;

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatabaseErrorKind {
    RowNotFound,
    PoolTimeout,
    ConnectionFailed,
    QueryFailed,
    MigrationFailed,
    Constraint,
    Other,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeErrorKind {
    InvalidMigProfile,
    NvmlInitFailed,
    ProcessSpawnFailed,
    VramAllocationFailed,
    PortAllocationFailed,
    ProcessKillFailed,
}

impl fmt::Display for NodeErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMigProfile => write!(f, "invalid_mig_profile"),
            Self::NvmlInitFailed => write!(f, "nvml_init_failed"),
            Self::ProcessSpawnFailed => write!(f, "process_spawn_failed"),
            Self::VramAllocationFailed => write!(f, "vram_allocation_failed"),
            Self::PortAllocationFailed => write!(f, "port_allocation_failed"),
            Self::ProcessKillFailed => write!(f, "process_kill_failed"),
        }
    }
}

#[non_exhaustive]
#[derive(Error, Debug)]
pub enum GadgetronError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Routing error: {0}")]
    Routing(String),

    #[error("Stream interrupted: {reason}")]
    StreamInterrupted { reason: String },

    #[error("Quota exceeded for tenant {tenant_id}")]
    QuotaExceeded { tenant_id: Uuid },

    #[error("Tenant not found")]
    TenantNotFound,

    #[error("Forbidden: insufficient scope")]
    Forbidden,

    #[error("Billing error: {0}")]
    Billing(String),

    #[error("Download failed: {0}")]
    DownloadFailed(String),

    #[error("Hot-swap failed: {0}")]
    HotSwapFailed(String),

    #[error("Database error ({kind:?}): {message}")]
    Database {
        kind: DatabaseErrorKind,
        message: String,
    },

    #[error("Node error ({kind}): {message}")]
    Node {
        kind: NodeErrorKind,
        message: String,
    },
}

impl GadgetronError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Config(_) => "config_error",
            Self::Provider(_) => "provider_error",
            Self::Routing(_) => "routing_failure",
            Self::StreamInterrupted { .. } => "stream_interrupted",
            Self::QuotaExceeded { .. } => "quota_exceeded",
            Self::TenantNotFound => "tenant_not_found",
            Self::Forbidden => "forbidden",
            Self::Billing(_) => "billing_error",
            Self::DownloadFailed(_) => "download_failed",
            Self::HotSwapFailed(_) => "hotswap_failed",
            Self::Database { kind, .. } => match kind {
                DatabaseErrorKind::PoolTimeout => "db_pool_timeout",
                DatabaseErrorKind::RowNotFound => "db_row_not_found",
                DatabaseErrorKind::ConnectionFailed => "db_connection_failed",
                DatabaseErrorKind::MigrationFailed => "db_migration_failed",
                DatabaseErrorKind::Constraint => "db_constraint",
                DatabaseErrorKind::QueryFailed => "db_query_failed",
                DatabaseErrorKind::Other => "db_error",
            },
            Self::Node { kind, .. } => match kind {
                NodeErrorKind::InvalidMigProfile => "node_invalid_mig_profile",
                _ => "node_error",
            },
        }
    }

    pub fn error_message(&self) -> &'static str {
        match self {
            Self::Config(_) => "Configuration is invalid. Check your gadgetron.toml and environment variables.",
            Self::Provider(_) => "The upstream LLM provider returned an error. Check provider status and API key validity.",
            Self::Routing(_) => "No suitable provider found for this request. Verify model availability and routing configuration. Run GET /v1/models to check available models.",
            Self::StreamInterrupted { .. } => "The response stream was interrupted. This may indicate a provider timeout or network issue.",
            Self::QuotaExceeded { .. } => "Your API usage quota has been exceeded. Update quota_configs table to increase limits, or see docs/manual/troubleshooting.md.",
            Self::TenantNotFound => "Invalid API key. Verify your API key is correct and has not been revoked.",
            Self::Forbidden => "Your API key does not have permission for this operation. Check your key's assigned scopes.",
            Self::Billing(_) => "A billing calculation error occurred. Check server logs for billing details. File an issue at github.com/NacBang/gadgetron if this persists.",
            Self::DownloadFailed(_) => "Model download failed. Check network connectivity and model repository access.",
            Self::HotSwapFailed(_) => "Model hot-swap failed. The previous model version remains active.",
            Self::Database { .. } => "A database error occurred. Check PostgreSQL connectivity and disk space.",
            Self::Node { .. } => "A node-level error occurred. Check GPU availability and NVML driver status.",
        }
    }

    pub fn error_type(&self) -> &'static str {
        match self {
            Self::Config(_) => "invalid_request_error",
            Self::Provider(_) => "api_error",
            Self::Routing(_) => "server_error",
            Self::StreamInterrupted { .. } => "api_error",
            Self::QuotaExceeded { .. } => "quota_error",
            Self::TenantNotFound => "authentication_error",
            Self::Forbidden => "permission_error",
            Self::Billing(_) => "api_error",
            Self::DownloadFailed(_) => "api_error",
            Self::HotSwapFailed(_) => "api_error",
            Self::Database { .. } => "server_error",
            Self::Node { .. } => "server_error",
        }
    }

    pub fn http_status_code(&self) -> u16 {
        match self {
            Self::Config(_) => 400,
            Self::Provider(_) => 502,
            Self::Routing(_) => 503,
            Self::StreamInterrupted { .. } => 502,
            Self::QuotaExceeded { .. } => 429,
            Self::TenantNotFound => 401,
            Self::Forbidden => 403,
            Self::Billing(_) => 500,
            Self::DownloadFailed(_) => 500,
            Self::HotSwapFailed(_) => 500,
            Self::Database { kind, .. } => match kind {
                DatabaseErrorKind::RowNotFound => 404,
                DatabaseErrorKind::PoolTimeout | DatabaseErrorKind::ConnectionFailed => 503,
                DatabaseErrorKind::Constraint => 409,
                _ => 500,
            },
            Self::Node { kind, .. } => match kind {
                NodeErrorKind::InvalidMigProfile => 400,
                _ => 500,
            },
        }
    }
}

pub type Result<T> = std::result::Result<T, GadgetronError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_returns_stable_machine_string() {
        assert_eq!(
            GadgetronError::Config("bad".into()).error_code(),
            "config_error"
        );
        assert_eq!(
            GadgetronError::Provider("down".into()).error_code(),
            "provider_error"
        );
        assert_eq!(
            GadgetronError::Routing("none".into()).error_code(),
            "routing_failure"
        );
        assert_eq!(
            GadgetronError::StreamInterrupted {
                reason: "timeout".into()
            }
            .error_code(),
            "stream_interrupted"
        );
        assert_eq!(
            GadgetronError::QuotaExceeded {
                tenant_id: Uuid::nil()
            }
            .error_code(),
            "quota_exceeded"
        );
        assert_eq!(
            GadgetronError::TenantNotFound.error_code(),
            "tenant_not_found"
        );
        assert_eq!(GadgetronError::Forbidden.error_code(), "forbidden");
        assert_eq!(
            GadgetronError::Billing("err".into()).error_code(),
            "billing_error"
        );
        assert_eq!(
            GadgetronError::DownloadFailed("err".into()).error_code(),
            "download_failed"
        );
        assert_eq!(
            GadgetronError::HotSwapFailed("err".into()).error_code(),
            "hotswap_failed"
        );
        assert_eq!(
            GadgetronError::Database {
                kind: DatabaseErrorKind::PoolTimeout,
                message: "".into()
            }
            .error_code(),
            "db_pool_timeout"
        );
        assert_eq!(
            GadgetronError::Node {
                kind: NodeErrorKind::InvalidMigProfile,
                message: "".into()
            }
            .error_code(),
            "node_invalid_mig_profile"
        );
    }

    #[test]
    fn error_message_is_human_readable_not_same_as_code() {
        let err = GadgetronError::QuotaExceeded {
            tenant_id: Uuid::nil(),
        };
        let msg = err.error_message();
        let code = err.error_code();
        assert_ne!(msg, code);
        assert!(msg.contains("quota"));
        assert!(msg.len() > 20);
    }

    #[test]
    fn error_type_follows_openai_taxonomy() {
        assert_eq!(
            GadgetronError::TenantNotFound.error_type(),
            "authentication_error"
        );
        assert_eq!(GadgetronError::Forbidden.error_type(), "permission_error");
        assert_eq!(
            GadgetronError::QuotaExceeded {
                tenant_id: Uuid::nil()
            }
            .error_type(),
            "quota_error"
        );
        assert_eq!(
            GadgetronError::Database {
                kind: DatabaseErrorKind::Other,
                message: "".into()
            }
            .error_type(),
            "server_error"
        );
        assert_eq!(
            GadgetronError::Config("".into()).error_type(),
            "invalid_request_error"
        );
        // Routing returns 503, so its error_type must be server_error, not invalid_request_error.
        assert_eq!(
            GadgetronError::Routing("".into()).error_type(),
            "server_error"
        );
    }

    #[test]
    fn http_status_codes_match_spec() {
        assert_eq!(GadgetronError::Config("".into()).http_status_code(), 400);
        assert_eq!(GadgetronError::Provider("".into()).http_status_code(), 502);
        assert_eq!(GadgetronError::Routing("".into()).http_status_code(), 503);
        assert_eq!(
            GadgetronError::QuotaExceeded {
                tenant_id: Uuid::nil()
            }
            .http_status_code(),
            429
        );
        assert_eq!(GadgetronError::TenantNotFound.http_status_code(), 401);
        assert_eq!(GadgetronError::Forbidden.http_status_code(), 403);
        assert_eq!(
            GadgetronError::Database {
                kind: DatabaseErrorKind::PoolTimeout,
                message: "".into()
            }
            .http_status_code(),
            503
        );
        assert_eq!(
            GadgetronError::Database {
                kind: DatabaseErrorKind::RowNotFound,
                message: "".into()
            }
            .http_status_code(),
            404
        );
        assert_eq!(
            GadgetronError::Database {
                kind: DatabaseErrorKind::Constraint,
                message: "".into()
            }
            .http_status_code(),
            409
        );
        assert_eq!(
            GadgetronError::Node {
                kind: NodeErrorKind::InvalidMigProfile,
                message: "".into()
            }
            .http_status_code(),
            400
        );
        assert_eq!(
            GadgetronError::Node {
                kind: NodeErrorKind::NvmlInitFailed,
                message: "".into()
            }
            .http_status_code(),
            500
        );
    }

    #[test]
    fn database_error_kind_is_non_exhaustive() {
        let kind = DatabaseErrorKind::Other;
        assert_eq!(format!("{kind:?}"), "Other");
    }

    #[test]
    fn display_includes_context() {
        let err = GadgetronError::Database {
            kind: DatabaseErrorKind::PoolTimeout,
            message: "connection timed out".into(),
        };
        let display = format!("{err}");
        assert!(display.contains("PoolTimeout"));
        assert!(display.contains("connection timed out"));
    }

    #[test]
    fn node_error_kind_process_kill_failed_display() {
        let kind = NodeErrorKind::ProcessKillFailed;
        assert_eq!(format!("{kind}"), "process_kill_failed");
        // Confirm it round-trips through the GadgetronError wrapper.
        let err = GadgetronError::Node {
            kind: NodeErrorKind::ProcessKillFailed,
            message: "SIGKILL timed out".into(),
        };
        let display = format!("{err}");
        assert!(
            display.contains("process_kill_failed"),
            "display: {display}"
        );
        assert!(display.contains("SIGKILL timed out"), "display: {display}");
        assert_eq!(err.error_code(), "node_error");
        assert_eq!(err.http_status_code(), 500);
    }

    #[test]
    fn all_twelve_variants_exist() {
        let variants: Vec<GadgetronError> = vec![
            GadgetronError::Config("".into()),
            GadgetronError::Provider("".into()),
            GadgetronError::Routing("".into()),
            GadgetronError::StreamInterrupted { reason: "".into() },
            GadgetronError::QuotaExceeded {
                tenant_id: Uuid::nil(),
            },
            GadgetronError::TenantNotFound,
            GadgetronError::Forbidden,
            GadgetronError::Billing("".into()),
            GadgetronError::DownloadFailed("".into()),
            GadgetronError::HotSwapFailed("".into()),
            GadgetronError::Database {
                kind: DatabaseErrorKind::Other,
                message: "".into(),
            },
            GadgetronError::Node {
                kind: NodeErrorKind::InvalidMigProfile,
                message: "".into(),
            },
        ];
        assert_eq!(variants.len(), 12);
    }
}
