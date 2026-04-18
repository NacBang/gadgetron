//! Error taxonomy for the `bundle` module.
//!
//! `BundleError` is the module-local failure type; it converts into
//! `GadgetronError::Bundle { kind }` at the crate boundary (see
//! `crate::error`). `HomeError` lives here (not in `home.rs`) so that
//! `BundleError` can `#[from]`-wrap it without introducing a circular
//! module dependency.
//!
//! Per D-12 / ADR-P2A-10-ADDENDUM-01 §4 D: Bundle crates never reach
//! into error internals — they surface `BundleError` through the core
//! enum and let the gateway render user-visible copy.

use thiserror::Error;

use crate::bundle::id::PlugIdError;

/// Module-local bundle error. Wrapped by `GadgetronError::Bundle { kind }`
/// at the crate public boundary.
#[derive(Error, Debug)]
pub enum BundleError {
    /// Manifest parse / shape failure (invalid TOML, missing required field,
    /// semver parse error, invalid Plug id in `plugs` list, etc.).
    #[error("bundle manifest error: {0}")]
    Manifest(String),

    /// Failure installing a Bundle at runtime (per-bundle install hook
    /// returned Err, registration path blew up, etc.).
    #[error("bundle install failed: {0}")]
    Install(String),

    /// Invalid PlugId / GadgetName bubbled up from the identifier layer.
    #[error("plug id: {0}")]
    PlugId(#[from] PlugIdError),

    /// Failure resolving `GadgetronBundlesHome` (ADDENDUM-01 §7). Fail-closed
    /// at startup — the daemon refuses to start with no writable bundles
    /// home rather than silently writing to host root.
    #[error("home resolution: {0}")]
    Home(#[from] HomeError),
}

/// `GadgetronBundlesHome` resolver failure modes. See ADDENDUM-01 §7 for
/// the priority chain and fail-closed rationale.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum HomeError {
    /// `$HOME` is not set and no explicit override was configured. Operator
    /// must set `GADGETRON_BUNDLES_HOME` (env) or `[bundles] workdir_root`
    /// (gadgetron.toml) to proceed.
    #[error("$HOME is not set and no fallback configured")]
    NoHome,

    /// `$HOME` resolved to `/`. Writing the daemon's bundles home under the
    /// filesystem root is a classic container misconfiguration (non-root
    /// user with no persistent mount). Fail-closed per ADDENDUM-01 §7.
    #[error("$HOME='/' is refused — set GADGETRON_BUNDLES_HOME or [bundles] workdir_root")]
    RootHomeRefused,

    /// Resolved path exists but the probe write failed. Carries the path and
    /// OS error text so operators can triage permission / FS-RO issues.
    #[error("resolved path '{path}' is not writable: {reason}")]
    NotWritable { path: String, reason: String },

    /// Resolved path did not exist and `create_dir_all` failed. Separate
    /// variant from `NotWritable` because the remediation is different
    /// (create parent vs chmod / fix mount).
    #[error("resolved path '{path}' does not exist and could not be created: {reason}")]
    CreateFailed { path: String, reason: String },
}
