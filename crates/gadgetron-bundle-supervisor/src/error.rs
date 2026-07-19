use std::{path::PathBuf, process::ExitStatus};

use gadgetron_bundle_sdk::HealthStatus;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BundleSupervisorError {
    #[error("external Bundle sandbox execution is supported only on Linux")]
    UnsupportedPlatform,

    #[error("Bundle supervisor I/O failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("Bundle supervisor JSON failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Bundle supervisor specification encoding failed: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("Bundle package contract failed validation: {0}")]
    Package(#[from] gadgetron_bundle_sdk::BundleSdkError),

    #[error("Bundle host protocol failed: {0}")]
    Host(#[from] gadgetron_bundle_host::BundleHostError),

    #[error("Bundle broker channel failed: {0}")]
    BrokerChannel(String),

    #[error("Bundle broker channel closed before runtime shutdown")]
    BrokerChannelClosed,

    #[error("Linux Bundle sandbox requires the fixed helper at {0}")]
    HelperUnavailable(PathBuf),

    #[error("Linux Bundle sandbox requires /usr/bin/unshare")]
    UnshareUnavailable,

    #[error("runtime kind/transport is not supported by the Linux v1 supervisor")]
    UnsupportedRuntime,

    #[error("network egress allowlists are not enforceable by the Linux v1 supervisor yet")]
    UnsupportedEgress,

    #[error("runtime entry {0:?} is not a regular executable inside the package root")]
    InvalidEntry(PathBuf),

    #[error("runtime entry digest mismatch: expected {expected}, got {actual}")]
    EntryDigestMismatch { expected: String, actual: String },

    #[error("Bundle runtime health was {status:?}: {message}")]
    HealthNotReady {
        status: HealthStatus,
        message: String,
    },

    #[error("Bundle sandbox process exited before becoming ready: {status}; stderr: {stderr}")]
    ChildExited { status: ExitStatus, stderr: String },

    #[error("Bundle sandbox {phase} probe failed: {error}; stderr: {stderr}")]
    ProbeFailed {
        phase: &'static str,
        error: String,
        stderr: String,
    },

    #[error("Bundle sandbox helper rejected its launch specification: {0}")]
    InvalidHelperSpec(String),

    #[error("Bundle sandbox syscall {operation} failed: {source}")]
    Isolation {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("Bundle sandbox runtime did not exit after shutdown")]
    ShutdownTimeout,
}

pub type Result<T> = std::result::Result<T, BundleSupervisorError>;
