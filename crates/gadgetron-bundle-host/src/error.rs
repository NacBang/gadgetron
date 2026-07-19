use std::{path::PathBuf, time::Duration};

use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BundleHostError {
    #[error("Bundle package contract failed validation: {0}")]
    Package(#[from] gadgetron_bundle_sdk::BundleSdkError),

    #[error("installed Bundle package is invalid: {0}")]
    InstalledPackage(String),

    #[error("installed Bundle {artifact} signature is invalid: {message}")]
    Signature {
        artifact: &'static str,
        message: String,
    },

    #[error("installed Bundle asset {path:?} is unavailable: {source}")]
    AssetIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("installed Bundle asset {path:?} escapes the package or is not a regular file")]
    InvalidAssetPath { path: PathBuf },

    #[error("installed Bundle asset {path:?} digest mismatch: expected {expected}, got {actual}")]
    AssetDigestMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("Bundle host I/O failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("Bundle host returned invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Bundle host frame is {actual} bytes; maximum is {maximum}")]
    FrameTooLarge { actual: usize, maximum: usize },

    #[error("Bundle host closed its channel before sending a response")]
    EndOfStream,

    #[error("Bundle host response was not newline terminated")]
    UnterminatedFrame,

    #[error("Bundle host request timed out after {0:?}")]
    Timeout(Duration),

    #[error("Bundle host response message id mismatch: expected {expected:?}, got {actual:?}")]
    MessageIdMismatch { expected: String, actual: String },

    #[error("Bundle host returned {actual} while {expected} was required")]
    UnexpectedResponse {
        expected: &'static str,
        actual: &'static str,
    },

    #[error("Bundle runtime returned {code}: {message}")]
    Remote {
        code: String,
        message: String,
        retryable: bool,
        details: Option<serde_json::Value>,
    },

    #[error("Bundle handshake digest mismatch: expected {expected}, got {actual}")]
    ManifestDigestMismatch { expected: String, actual: String },

    #[error("Bundle host session requires a successful handshake first")]
    HandshakeRequired,
}

pub type Result<T> = std::result::Result<T, BundleHostError>;
