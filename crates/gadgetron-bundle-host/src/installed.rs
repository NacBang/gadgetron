use std::{
    fs,
    path::{Path, PathBuf},
};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use gadgetron_bundle_sdk::{DomainOntology, RelativePath, RuntimeKind};
use semver::Version;
use sha2::{Digest, Sha256};

use crate::{BundleHostError, Result, ValidatedPackageContract};

/// A filesystem package whose catalog and SDK contract were independently
/// verified against the same Core-owned publisher trust set.
#[derive(Debug, Clone)]
pub struct SignedInstalledPackage {
    root: PathBuf,
    catalog_source: String,
    package_source: String,
    contract: ValidatedPackageContract,
}

impl SignedInstalledPackage {
    pub fn load(
        package_root: impl AsRef<Path>,
        expected_id: &str,
        core_version: &Version,
        public_keys_hex: &[String],
    ) -> Result<Self> {
        let requested_root = package_root.as_ref();
        let root_metadata =
            fs::symlink_metadata(requested_root).map_err(|source| BundleHostError::AssetIo {
                path: requested_root.to_path_buf(),
                source,
            })?;
        if !root_metadata.file_type().is_dir() {
            return Err(BundleHostError::InstalledPackage(format!(
                "package root {requested_root:?} is not a regular directory"
            )));
        }
        let root = requested_root
            .canonicalize()
            .map_err(|source| BundleHostError::AssetIo {
                path: requested_root.to_path_buf(),
                source,
            })?;
        if !root.is_dir() {
            return Err(BundleHostError::InstalledPackage(format!(
                "package root {root:?} is not a directory"
            )));
        }

        let catalog_source = read_regular_text(&root, "bundle.toml")?;
        let package_source = read_regular_text(&root, "package.toml")?;
        let catalog_signature = read_signature(&root, "catalog.sig", "catalog")?;
        let package_signature = read_signature(&root, "package.sig", "package")?;
        verify_required_detached_signature(
            public_keys_hex,
            "catalog",
            catalog_source.as_bytes(),
            &catalog_signature,
        )?;
        verify_required_detached_signature(
            public_keys_hex,
            "package",
            package_source.as_bytes(),
            &package_signature,
        )?;

        let catalog: toml::Value = toml::from_str(&catalog_source).map_err(|error| {
            BundleHostError::InstalledPackage(format!("catalog TOML parse failed: {error}"))
        })?;
        let catalog_bundle = catalog
            .get("bundle")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| {
                BundleHostError::InstalledPackage("catalog has no [bundle] identity".into())
            })?;
        let catalog_id = catalog_bundle
            .get("id")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| {
                BundleHostError::InstalledPackage("catalog bundle id is missing".into())
            })?;
        let catalog_version = catalog_bundle
            .get("version")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| {
                BundleHostError::InstalledPackage("catalog bundle version is missing".into())
            })?;
        if catalog_id != expected_id {
            return Err(BundleHostError::InstalledPackage(format!(
                "directory id {expected_id:?} does not match catalog id {catalog_id:?}"
            )));
        }

        let contract = ValidatedPackageContract::parse(&package_source, core_version)?;
        let manifest = contract.manifest();
        if manifest.bundle.id.as_str() != catalog_id {
            return Err(BundleHostError::InstalledPackage(format!(
                "package id {:?} does not match catalog id {catalog_id:?}",
                manifest.bundle.id.as_str()
            )));
        }
        if manifest.bundle.version.to_string() != catalog_version {
            return Err(BundleHostError::InstalledPackage(format!(
                "package version {:?} does not match catalog version {catalog_version:?}",
                manifest.bundle.version.to_string()
            )));
        }

        Ok(Self {
            root,
            catalog_source,
            package_source,
            contract,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn catalog_source(&self) -> &str {
        &self.catalog_source
    }

    pub fn package_source(&self) -> &str {
        &self.package_source
    }

    pub fn contract(&self) -> &ValidatedPackageContract {
        &self.contract
    }

    pub fn verify_all_hashed_assets(&self) -> Result<()> {
        let manifest = self.contract.manifest();
        if matches!(
            manifest.runtime.kind,
            RuntimeKind::Subprocess | RuntimeKind::Wasm
        ) {
            let relative = RelativePath::new(manifest.runtime.entry.clone())?;
            let expected = manifest.runtime.entry_sha256.as_deref().ok_or_else(|| {
                BundleHostError::InstalledPackage("filesystem runtime has no entry_sha256".into())
            })?;
            self.verified_asset_bytes(&relative, expected)?;
        }
        for schema in &manifest.capabilities.domain_schemas {
            let bytes = self.verified_asset_bytes(&schema.schema_path, &schema.sha256)?;
            DomainOntology::parse_json(&bytes, schema.version)?;
        }
        for asset in &manifest.capabilities.seed_assets {
            self.verified_asset_bytes(&asset.path, &asset.sha256)?;
        }
        for migration in &manifest.capabilities.migrations {
            self.verified_asset_bytes(&migration.path, &migration.sha256)?;
        }
        Ok(())
    }

    pub fn verified_asset_bytes(
        &self,
        relative: &RelativePath,
        expected_sha256: &str,
    ) -> Result<Vec<u8>> {
        let requested = self.root.join(relative.as_str());
        let metadata =
            fs::symlink_metadata(&requested).map_err(|source| BundleHostError::AssetIo {
                path: requested.clone(),
                source,
            })?;
        if !metadata.file_type().is_file() {
            return Err(BundleHostError::InvalidAssetPath { path: requested });
        }
        let canonical = requested
            .canonicalize()
            .map_err(|source| BundleHostError::AssetIo {
                path: requested.clone(),
                source,
            })?;
        if !canonical.starts_with(&self.root) {
            return Err(BundleHostError::InvalidAssetPath { path: requested });
        }
        let bytes = fs::read(&canonical).map_err(|source| BundleHostError::AssetIo {
            path: canonical.clone(),
            source,
        })?;
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != expected_sha256 {
            return Err(BundleHostError::AssetDigestMismatch {
                path: canonical,
                expected: expected_sha256.to_string(),
                actual,
            });
        }
        Ok(bytes)
    }
}

pub fn verify_required_detached_signature(
    public_keys_hex: &[String],
    artifact: &'static str,
    message: &[u8],
    signature_hex: &str,
) -> Result<()> {
    if public_keys_hex.is_empty() {
        return Err(signature_error(
            artifact,
            "no publisher trust anchors are configured",
        ));
    }
    let bytes = hex::decode(signature_hex)
        .map_err(|error| signature_error(artifact, format!("signature is not hex: {error}")))?;
    let raw: [u8; 64] = bytes.as_slice().try_into().map_err(|_| {
        signature_error(
            artifact,
            format!("signature must contain 64 bytes, got {}", bytes.len()),
        )
    })?;
    let signature = Signature::from_bytes(&raw);

    for (index, encoded) in public_keys_hex.iter().enumerate() {
        let bytes = hex::decode(encoded).map_err(|error| {
            signature_error(
                artifact,
                format!("trust anchor {index} is not hex: {error}"),
            )
        })?;
        let raw: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            signature_error(
                artifact,
                format!("trust anchor {index} must contain 32 bytes"),
            )
        })?;
        let key = VerifyingKey::from_bytes(&raw).map_err(|error| {
            signature_error(
                artifact,
                format!("trust anchor {index} is not an Ed25519 key: {error}"),
            )
        })?;
        if key.verify(message, &signature).is_ok() {
            return Ok(());
        }
    }
    Err(signature_error(
        artifact,
        "signature did not verify against any configured trust anchor",
    ))
}

fn read_regular_text(root: &Path, name: &str) -> Result<String> {
    let requested = root.join(name);
    let metadata = fs::symlink_metadata(&requested).map_err(|source| BundleHostError::AssetIo {
        path: requested.clone(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(BundleHostError::InvalidAssetPath { path: requested });
    }
    let canonical = requested
        .canonicalize()
        .map_err(|source| BundleHostError::AssetIo {
            path: requested.clone(),
            source,
        })?;
    if !canonical.starts_with(root) {
        return Err(BundleHostError::InvalidAssetPath { path: requested });
    }
    fs::read_to_string(&canonical).map_err(|source| BundleHostError::AssetIo {
        path: canonical,
        source,
    })
}

fn read_signature(root: &Path, name: &str, artifact: &'static str) -> Result<String> {
    let signature = read_regular_text(root, name)?;
    let signature = signature.trim();
    if signature.is_empty() {
        return Err(signature_error(artifact, "signature file is empty"));
    }
    Ok(signature.to_string())
}

fn signature_error(artifact: &'static str, message: impl Into<String>) -> BundleHostError {
    BundleHostError::Signature {
        artifact,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use tempfile::TempDir;

    struct Fixture {
        _temp: TempDir,
        root: PathBuf,
        public_key: String,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("signed-example");
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("migrations")).unwrap();
            let migration = b"CREATE TABLE signed_example(id BIGINT PRIMARY KEY);\n";
            fs::write(root.join("migrations/0001.sql"), migration).unwrap();
            let migration_sha256 = hex::encode(Sha256::digest(migration));
            let catalog = "[bundle]\nid = \"signed-example\"\nversion = \"1.0.0\"\n";
            let package = format!(
                r#"manifest_version = 1

[bundle]
id = "signed-example"
version = "1.0.0"
publisher = "example.publisher"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "container"
transport = "json_rpc_stdio"
entry = "unused"

[runtime.limits]
memory_mb = 64
open_files = 32
cpu_seconds = 10

[[capabilities.migrations]]
id = "initial"
revision = 1
kind = "schema"
path = "migrations/0001.sql"
sha256 = "{migration_sha256}"
"#
            );
            fs::write(root.join("bundle.toml"), catalog).unwrap();
            fs::write(root.join("package.toml"), &package).unwrap();

            let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
            fs::write(
                root.join("catalog.sig"),
                hex::encode(signing_key.sign(catalog.as_bytes()).to_bytes()),
            )
            .unwrap();
            fs::write(
                root.join("package.sig"),
                hex::encode(signing_key.sign(package.as_bytes()).to_bytes()),
            )
            .unwrap();

            Self {
                _temp: temp,
                root,
                public_key: hex::encode(signing_key.verifying_key().to_bytes()),
            }
        }

        fn load(&self) -> Result<SignedInstalledPackage> {
            SignedInstalledPackage::load(
                &self.root,
                "signed-example",
                &Version::new(1, 0, 0),
                std::slice::from_ref(&self.public_key),
            )
        }

        fn add_domain_schema(&self, bytes: &[u8]) {
            fs::create_dir(self.root.join("schema")).unwrap();
            fs::write(self.root.join("schema/domain.json"), bytes).unwrap();
            let digest = hex::encode(Sha256::digest(bytes));
            let mut package = fs::read_to_string(self.root.join("package.toml")).unwrap();
            package.push_str(&format!(
                r#"
[[capabilities.domain_schemas]]
id = "fixture-domain"
version = 1
schema_path = "schema/domain.json"
sha256 = "{digest}"
"#
            ));
            fs::write(self.root.join("package.toml"), &package).unwrap();
            let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
            fs::write(
                self.root.join("package.sig"),
                hex::encode(signing_key.sign(package.as_bytes()).to_bytes()),
            )
            .unwrap();
        }
    }

    #[test]
    fn signed_package_loads_and_verifies_every_declared_asset() {
        let fixture = Fixture::new();
        fixture.add_domain_schema(
            br#"{"properties":{"entities":{"const":["Place"]},"relations":{"const":["applies_to"]}}}"#,
        );
        let package = fixture.load().unwrap();
        package.verify_all_hashed_assets().unwrap();
        assert_eq!(
            package.contract().manifest().bundle.id.as_str(),
            "signed-example"
        );
    }

    #[test]
    fn signed_manifest_does_not_authorize_tampered_asset_bytes() {
        let fixture = Fixture::new();
        fs::write(
            fixture.root.join("migrations/0001.sql"),
            "SELECT 'tampered';\n",
        )
        .unwrap();
        let package = fixture.load().unwrap();
        assert!(matches!(
            package.verify_all_hashed_assets(),
            Err(BundleHostError::AssetDigestMismatch { .. })
        ));
    }

    #[test]
    fn digest_correct_domain_schema_must_also_satisfy_the_ontology_contract() {
        let fixture = Fixture::new();
        fixture.add_domain_schema(
            br#"{"format_version":2,"types":[{"id":"Place","label":"Place","family":"entity"}],"relations":[]}"#,
        );
        let package = fixture.load().unwrap();
        let error = package
            .verify_all_hashed_assets()
            .expect_err("unsupported ontology format must fail before enable");
        assert!(format!("{error:?}").contains("domain_schema.format_version"));
    }

    #[cfg(unix)]
    #[test]
    fn signed_metadata_symlink_is_rejected_even_when_bytes_match() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let package_source = fs::read_to_string(fixture.root.join("package.toml")).unwrap();
        let outside = fixture._temp.path().join("outside-package.toml");
        fs::write(&outside, package_source).unwrap();
        fs::remove_file(fixture.root.join("package.toml")).unwrap();
        symlink(outside, fixture.root.join("package.toml")).unwrap();

        assert!(matches!(
            fixture.load(),
            Err(BundleHostError::InvalidAssetPath { .. })
        ));
    }
}
