use thiserror::Error;

/// Errors produced while parsing or validating public Bundle contracts.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BundleSdkError {
    #[error("package.toml could not be parsed: {0}")]
    ManifestToml(#[from] toml::de::Error),

    #[error(
        "unsupported package manifest version {found}; this SDK supports {minimum}..={maximum}"
    )]
    UnsupportedManifestVersion {
        found: u32,
        minimum: u32,
        maximum: u32,
    },

    #[error("invalid {kind} {value:?}: {reason}")]
    InvalidIdentifier {
        kind: &'static str,
        value: String,
        reason: &'static str,
    },

    #[error("invalid package manifest at {field}: {reason}")]
    InvalidManifest { field: String, reason: String },

    #[error("Bundle requires Gadgetron {required}, but Core is {current}")]
    IncompatibleCore { required: String, current: String },

    #[error("Bundle supports host protocol {minimum}..={maximum}, but Core provides {current}")]
    IncompatibleProtocol {
        minimum: u32,
        maximum: u32,
        current: u32,
    },

    #[error("invalid host protocol message at {field}: {reason}")]
    InvalidProtocol { field: String, reason: String },
}

pub type Result<T> = std::result::Result<T, BundleSdkError>;

impl BundleSdkError {
    pub(crate) fn manifest(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidManifest {
            field: field.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn protocol(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidProtocol {
            field: field.into(),
            reason: reason.into(),
        }
    }
}
