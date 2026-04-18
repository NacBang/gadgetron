//! `GadgetronBundlesHome` path resolver (ADR-P2A-10-ADDENDUM-01 §7).
//!
//! `resolve_bundles_home` walks a four-tier priority chain and returns the
//! first writable path. Fails closed (`HomeError`) if no tier resolves —
//! operators must provide an explicit root rather than silently writing to
//! host `/`.
//!
//! Priority chain (highest first):
//!
//! 1. `[bundles] workdir_root` in `gadgetron.toml` — passed as
//!    `config_override`. Recommended for container / K8s deployments.
//! 2. `GADGETRON_BUNDLES_HOME` env var — CI / Helm can inject without a
//!    config rebuild.
//! 3. `GADGETRON_DATA_DIR/.gadgetron` — data-dir convention.
//! 4. `$HOME/.gadgetron` — legacy, refused when `$HOME == "/"`.
//!
//! Tier-resolution logging is emitted by the internal helper
//! `resolve_bundles_home_traced` so that SRE debugging "why does staging
//! write under `/data/...` but prod writes under `~/.gadgetron`" is a single
//! log grep (`tracing::info!(target = "gadgetron_config", tier = …)`).

use std::path::{Path, PathBuf};

use crate::bundle::errors::HomeError;

/// Stable identifiers for the four resolver tiers. Emitted as the `tier`
/// field on the startup `tracing::info!` event so downstream log consumers
/// can filter.
const TIER_CONFIG_OVERRIDE: &str = "config_override";
const TIER_ENV_BUNDLES_HOME: &str = "env_GADGETRON_BUNDLES_HOME";
const TIER_ENV_DATA_DIR: &str = "env_GADGETRON_DATA_DIR";
const TIER_HOME_DIR: &str = "home_dir";

/// Resolve the `GadgetronBundlesHome` directory per ADDENDUM-01 §7.
///
/// `config_override` is the `[bundles] workdir_root` field if set; `None`
/// falls through to the env / home tiers.
///
/// Emits a `tracing::info!(target: "gadgetron_config", tier = …,
/// resolved_path = …, "bundles_home resolved")` exactly once on success.
/// Returns `HomeError` (never panics) when every tier fails.
pub fn resolve_bundles_home(config_override: Option<&str>) -> Result<PathBuf, HomeError> {
    let (tier, path) = resolve_bundles_home_raw(config_override)?;
    tracing::info!(
        target: "gadgetron_config",
        tier = tier,
        resolved_path = %path.display(),
        "bundles_home resolved"
    );
    Ok(path)
}

/// The tier-aware inner form — exists so callers / tests can observe which
/// tier fired without parsing structured log output. Not part of the public
/// surface; kept `pub(crate)` so integration-style module tests can use it.
pub(crate) fn resolve_bundles_home_raw(
    config_override: Option<&str>,
) -> Result<(&'static str, PathBuf), HomeError> {
    // Tier 1: explicit config override.
    if let Some(p) = config_override {
        if !p.is_empty() {
            let resolved = validate_writable(PathBuf::from(p))?;
            return Ok((TIER_CONFIG_OVERRIDE, resolved));
        }
    }

    // Tier 2: GADGETRON_BUNDLES_HOME.
    if let Ok(p) = std::env::var("GADGETRON_BUNDLES_HOME") {
        if !p.is_empty() {
            let resolved = validate_writable(PathBuf::from(p))?;
            return Ok((TIER_ENV_BUNDLES_HOME, resolved));
        }
    }

    // Tier 3: GADGETRON_DATA_DIR/.gadgetron.
    if let Ok(p) = std::env::var("GADGETRON_DATA_DIR") {
        if !p.is_empty() {
            let resolved = validate_writable(PathBuf::from(p).join(".gadgetron"))?;
            return Ok((TIER_ENV_DATA_DIR, resolved));
        }
    }

    // Tier 4: $HOME/.gadgetron — refuse "/".
    //
    // We deliberately read `HOME` directly from the environment rather than
    // pulling `dirs` as a dependency (ADDENDUM-01 §7 explicitly refuses "/";
    // `dirs::home_dir()` on some platforms silently picks alternative roots
    // which would defeat the fail-closed guarantee).
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(HomeError::NoHome)?;
    if home.as_os_str() == "/" {
        return Err(HomeError::RootHomeRefused);
    }
    let resolved = validate_writable(home.join(".gadgetron"))?;
    Ok((TIER_HOME_DIR, resolved))
}

/// Create the directory if missing and verify writability via a probe file
/// (`.gadgetron_probe`). Returns the path on success — callers do not need
/// to re-canonicalize.
fn validate_writable(path: PathBuf) -> Result<PathBuf, HomeError> {
    std::fs::create_dir_all(&path).map_err(|e| HomeError::CreateFailed {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;
    let probe = path.join(".gadgetron_probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(path)
        }
        Err(e) => Err(HomeError::NotWritable {
            path: path.display().to_string(),
            reason: e.to_string(),
        }),
    }
}

/// Compose a tenant-scoped workdir path under a resolved bundles-home root.
///
/// Layout: `<base>/tenants/<tenant_id>/bundles/<bundle_name>/workdir/`.
///
/// Pure path composition — does not touch the filesystem. Callers
/// (xaas `WorkdirPurgeJob`, external Gadget runtime spawn in W3+) are
/// responsible for creation, quota check, and canonicalize verification.
pub fn tenant_workdir(base: &Path, tenant_id: &str, bundle_name: &str) -> PathBuf {
    base.join("tenants")
        .join(tenant_id)
        .join("bundles")
        .join(bundle_name)
        .join("workdir")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// `std::env::*_var` is process-global and Rust runs tests in parallel
    /// threads. Serializing env-touching tests through this mutex is the
    /// cheapest way to avoid cross-test interference without adding the
    /// `temp_env` crate to the workspace.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// RAII guard that clears env vars on drop so a panicking test cannot
    /// leak state into later tests.
    struct EnvGuard<'a> {
        vars: Vec<&'a str>,
    }

    impl<'a> EnvGuard<'a> {
        fn new(vars: Vec<&'a str>) -> Self {
            for v in &vars {
                std::env::remove_var(v);
            }
            Self { vars }
        }
    }

    impl Drop for EnvGuard<'_> {
        fn drop(&mut self) {
            for v in &self.vars {
                std::env::remove_var(v);
            }
        }
    }

    #[test]
    fn bundles_home_tier1_config_override_wins() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::new(vec!["GADGETRON_BUNDLES_HOME", "GADGETRON_DATA_DIR", "HOME"]);

        let tmp = TempDir::new().unwrap();
        // Set lower-tier env vars pointing elsewhere; tier 1 must still win.
        std::env::set_var("GADGETRON_BUNDLES_HOME", "/nonexistent/env-home");
        std::env::set_var("HOME", "/tmp");

        let (tier, path) = resolve_bundles_home_raw(Some(tmp.path().to_str().unwrap())).unwrap();
        assert_eq!(tier, TIER_CONFIG_OVERRIDE);
        assert_eq!(path, tmp.path());
        // Probe file cleaned up.
        assert!(!tmp.path().join(".gadgetron_probe").exists());
    }

    #[test]
    fn bundles_home_tier2_env_var_when_config_absent() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::new(vec!["GADGETRON_BUNDLES_HOME", "GADGETRON_DATA_DIR", "HOME"]);

        let tmp = TempDir::new().unwrap();
        std::env::set_var("GADGETRON_BUNDLES_HOME", tmp.path());
        // Tier 3/4 env set — but tier 2 must fire first.
        std::env::set_var("GADGETRON_DATA_DIR", "/nonexistent/data-dir");
        std::env::set_var("HOME", "/tmp");

        let (tier, path) = resolve_bundles_home_raw(None).unwrap();
        assert_eq!(tier, TIER_ENV_BUNDLES_HOME);
        assert_eq!(path, tmp.path());
    }

    #[test]
    fn bundles_home_tier3_data_dir_fallback() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::new(vec!["GADGETRON_BUNDLES_HOME", "GADGETRON_DATA_DIR", "HOME"]);

        let tmp = TempDir::new().unwrap();
        std::env::set_var("GADGETRON_DATA_DIR", tmp.path());
        std::env::set_var("HOME", "/tmp"); // fallback, but tier 3 wins first

        let (tier, path) = resolve_bundles_home_raw(None).unwrap();
        assert_eq!(tier, TIER_ENV_DATA_DIR);
        assert_eq!(path, tmp.path().join(".gadgetron"));
        assert!(path.is_dir(), "tier 3 must create the .gadgetron subdir");
    }

    #[test]
    fn bundles_home_tier4_home_dir_fallback() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::new(vec!["GADGETRON_BUNDLES_HOME", "GADGETRON_DATA_DIR", "HOME"]);

        let tmp = TempDir::new().unwrap();
        std::env::set_var("HOME", tmp.path());

        let (tier, path) = resolve_bundles_home_raw(None).unwrap();
        assert_eq!(tier, TIER_HOME_DIR);
        assert_eq!(path, tmp.path().join(".gadgetron"));
    }

    #[test]
    fn bundles_home_resolver_fail_closed_on_root_home() {
        // ADDENDUM-01 §Consequences mandatory test — no silent fallback to /.
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::new(vec!["GADGETRON_BUNDLES_HOME", "GADGETRON_DATA_DIR", "HOME"]);

        std::env::set_var("HOME", "/");

        let err = resolve_bundles_home_raw(None).unwrap_err();
        assert_eq!(err, HomeError::RootHomeRefused);
    }

    #[test]
    fn bundles_home_no_home_when_all_tiers_empty() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::new(vec!["GADGETRON_BUNDLES_HOME", "GADGETRON_DATA_DIR", "HOME"]);

        // No config override, no env, no HOME → NoHome.
        let err = resolve_bundles_home_raw(None).unwrap_err();
        assert_eq!(err, HomeError::NoHome);
    }

    #[test]
    fn tenant_workdir_path_composition() {
        // Pure path composition — does not touch the FS.
        let base = PathBuf::from("/data/gadgetron/bundles");
        let wd = tenant_workdir(&base, "tenant-a", "ai-infra");
        assert_eq!(
            wd,
            PathBuf::from("/data/gadgetron/bundles/tenants/tenant-a/bundles/ai-infra/workdir")
        );
    }

    #[test]
    fn bundles_home_public_entry_emits_info_and_succeeds() {
        // Sanity-check the public `resolve_bundles_home` path (which calls
        // the `_raw` form and emits tracing::info!). No subscriber is
        // installed in the unit test — the call must simply succeed.
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::new(vec!["GADGETRON_BUNDLES_HOME", "GADGETRON_DATA_DIR", "HOME"]);
        let tmp = TempDir::new().unwrap();
        let path = resolve_bundles_home(Some(tmp.path().to_str().unwrap())).unwrap();
        assert_eq!(path, tmp.path());
    }
}
