use gadgetron_bundle_sdk::{
    BundlePackageManifest, BundleRuntimeIdentity, BUNDLE_HOST_PROTOCOL_VERSION,
};
use semver::Version;
use sha2::{Digest, Sha256};

use crate::Result;

/// Parsed package authority pinned to the exact signed `package.toml` bytes.
#[derive(Debug, Clone)]
pub struct ValidatedPackageContract {
    manifest: BundlePackageManifest,
    manifest_sha256: String,
}

impl ValidatedPackageContract {
    pub fn parse(source: &str, core_version: &Version) -> Result<Self> {
        let manifest = BundlePackageManifest::parse_toml(source)?;
        manifest.validate_for_core(core_version, BUNDLE_HOST_PROTOCOL_VERSION)?;
        let manifest_sha256 = hex::encode(Sha256::digest(source.as_bytes()));
        Ok(Self {
            manifest,
            manifest_sha256,
        })
    }

    pub fn manifest(&self) -> &BundlePackageManifest {
        &self.manifest
    }

    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub fn runtime_identity(&self) -> BundleRuntimeIdentity {
        BundleRuntimeIdentity::new(
            self.manifest.bundle.id.clone(),
            self.manifest.bundle.version.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE: &str = r#"
manifest_version = 1

[bundle]
id = "example-research"
version = "1.0.0"
publisher = "example.publisher"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/example-research"
entry_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 30
"#;

    #[test]
    fn package_contract_pins_identity_and_exact_digest() {
        let contract = ValidatedPackageContract::parse(PACKAGE, &Version::new(1, 0, 0)).unwrap();
        assert_eq!(contract.runtime_identity().id.as_str(), "example-research");
        assert_eq!(contract.manifest_sha256().len(), 64);

        let changed = ValidatedPackageContract::parse(
            &PACKAGE.replace("cpu_seconds = 30", "cpu_seconds = 31"),
            &Version::new(1, 0, 0),
        )
        .unwrap();
        assert_ne!(contract.manifest_sha256(), changed.manifest_sha256());
    }
}
