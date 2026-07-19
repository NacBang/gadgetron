use std::{path::Path, sync::Arc};

use gadgetron_bundle_host::{BundleBroker, ValidatedPackageContract};
use gadgetron_bundle_sdk::{Acknowledgement, GadgetInvocation, GadgetResult, HealthReport};

use crate::{BundleSupervisorError, Result};

pub const INTERNAL_HELPER_MARKER: &str = "__bundle-sandbox-init";

/// Fail-closed compatibility surface for non-Linux Core builds.
///
/// Package metadata remains usable, but no external Bundle runtime can become
/// enabled until that platform has an isolation backend meeting the v1 floor.
#[derive(Debug, Clone)]
pub struct LinuxSandboxSupervisor;

impl LinuxSandboxSupervisor {
    pub fn for_current_executable() -> Result<Self> {
        Err(BundleSupervisorError::UnsupportedPlatform)
    }

    #[doc(hidden)]
    pub fn from_trusted_helper_path(_path: impl AsRef<Path>) -> Result<Self> {
        Err(BundleSupervisorError::UnsupportedPlatform)
    }

    pub async fn launch_and_probe(
        &self,
        _package: &ValidatedPackageContract,
        _package_root: impl AsRef<Path>,
        _state_root: impl AsRef<Path>,
    ) -> Result<SandboxedBundle> {
        Err(BundleSupervisorError::UnsupportedPlatform)
    }

    pub async fn launch_and_probe_with_broker(
        &self,
        _package: &ValidatedPackageContract,
        _package_root: impl AsRef<Path>,
        _state_root: impl AsRef<Path>,
        _broker: Arc<dyn BundleBroker>,
    ) -> Result<SandboxedBundle> {
        Err(BundleSupervisorError::UnsupportedPlatform)
    }
}

pub struct SandboxedBundle;

impl SandboxedBundle {
    pub fn health(&self) -> &HealthReport {
        unreachable!("a non-Linux supervisor never returns a sandboxed Bundle")
    }

    pub async fn invoke_gadget(&mut self, _invocation: GadgetInvocation) -> Result<GadgetResult> {
        Err(BundleSupervisorError::UnsupportedPlatform)
    }

    pub async fn start_job(
        &mut self,
        _request: gadgetron_bundle_sdk::JobStartRequest,
    ) -> Result<gadgetron_bundle_sdk::JobAccepted> {
        Err(BundleSupervisorError::UnsupportedPlatform)
    }

    pub async fn poll_job(
        &mut self,
        _request: gadgetron_bundle_sdk::JobPollRequest,
    ) -> Result<gadgetron_bundle_sdk::JobStatusReport> {
        Err(BundleSupervisorError::UnsupportedPlatform)
    }

    pub async fn cancel_job(
        &mut self,
        _request: gadgetron_bundle_sdk::JobCancelRequest,
    ) -> Result<gadgetron_bundle_sdk::JobStatusReport> {
        Err(BundleSupervisorError::UnsupportedPlatform)
    }

    pub fn stderr_snapshot(&self) -> String {
        String::new()
    }

    pub async fn shutdown(&mut self, _reason: impl Into<String>) -> Result<Acknowledgement> {
        Err(BundleSupervisorError::UnsupportedPlatform)
    }
}

pub fn run_internal_helper(_encoded_spec: &str) -> Result<()> {
    Err(BundleSupervisorError::UnsupportedPlatform)
}
