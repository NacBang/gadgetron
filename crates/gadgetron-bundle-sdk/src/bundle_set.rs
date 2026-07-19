use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{
    resolve_bundle_dependencies, BundleDependencyCandidate, BundleDependencyPlan, BundleId,
    BundleSdkError, LocalId, Result, BUNDLE_SET_MANIFEST_VERSION,
};

const MAX_SET_PACKAGES: usize = 64;
const MAX_SET_MANIFEST_BYTES: usize = 1_048_576;
const MAX_PUBLISHER: usize = 256;
const MAX_SETTINGS_PER_PACKAGE: usize = 128;
const MAX_SETTING_TEXT: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleSetManifest {
    pub set_manifest_version: u32,
    pub set: BundleSetIdentity,
    pub packages: Vec<BundleSetPackageRef>,
}

impl BundleSetManifest {
    pub fn parse_toml(source: &str) -> Result<Self> {
        if source.len() > MAX_SET_MANIFEST_BYTES {
            return Err(BundleSdkError::manifest(
                "bundle_set",
                format!("exceeds {MAX_SET_MANIFEST_BYTES} bytes"),
            ));
        }
        let raw: toml::Value = toml::from_str(source)?;
        let found = raw
            .get("set_manifest_version")
            .and_then(toml::Value::as_integer)
            .ok_or_else(|| {
                BundleSdkError::manifest(
                    "set_manifest_version",
                    "a positive integer Bundle Set manifest version is required",
                )
            })?;
        if found != i64::from(BUNDLE_SET_MANIFEST_VERSION) {
            return Err(BundleSdkError::manifest(
                "set_manifest_version",
                format!(
                    "unsupported Bundle Set manifest version {found}; expected {BUNDLE_SET_MANIFEST_VERSION}"
                ),
            ));
        }
        let manifest: Self = toml::from_str(source)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        if self.set_manifest_version != BUNDLE_SET_MANIFEST_VERSION {
            return Err(BundleSdkError::manifest(
                "set_manifest_version",
                format!(
                    "unsupported Bundle Set manifest version {}; expected {BUNDLE_SET_MANIFEST_VERSION}",
                    self.set_manifest_version
                ),
            ));
        }
        bounded_nonempty("set.publisher", &self.set.publisher, MAX_PUBLISHER)?;
        if self.packages.is_empty() || self.packages.len() > MAX_SET_PACKAGES {
            return Err(BundleSdkError::manifest(
                "packages",
                format!("must contain 1-{MAX_SET_PACKAGES} independent package references"),
            ));
        }
        let mut ids = BTreeSet::new();
        for (index, package) in self.packages.iter().enumerate() {
            if !ids.insert(package.bundle_id.as_str()) {
                return Err(BundleSdkError::manifest(
                    format!("packages[{index}].bundle_id"),
                    "duplicates a Bundle already present in this Set",
                ));
            }
            validate_sha256(
                &format!("packages[{index}].package_manifest_sha256"),
                &package.package_manifest_sha256,
            )?;
            if package.settings.len() > MAX_SETTINGS_PER_PACKAGE {
                return Err(BundleSdkError::manifest(
                    format!("packages[{index}].settings"),
                    format!("at most {MAX_SETTINGS_PER_PACKAGE} scalar settings may be declared"),
                ));
            }
            for (setting, value) in &package.settings {
                if is_secret_like_field(setting.as_str()) {
                    return Err(BundleSdkError::manifest(
                        format!("packages[{index}].settings.{setting}"),
                        "secret-like values are forbidden; use Core-owned secret references after install",
                    ));
                }
                if let BundleSetSettingValue::String(value) = value {
                    bounded_nonempty(
                        &format!("packages[{index}].settings.{setting}"),
                        value,
                        MAX_SETTING_TEXT,
                    )?;
                }
                if matches!(value, BundleSetSettingValue::Float(number) if !number.is_finite()) {
                    return Err(BundleSdkError::manifest(
                        format!("packages[{index}].settings.{setting}"),
                        "floating-point settings must be finite",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleSetIdentity {
    pub id: BundleId,
    pub version: Version,
    pub publisher: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleSetPackageRef {
    pub bundle_id: BundleId,
    pub version: Version,
    pub package_manifest_sha256: String,
    #[serde(default = "default_enable")]
    pub enable: bool,
    #[serde(default)]
    pub settings: BTreeMap<LocalId, BundleSetSettingValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum BundleSetSettingValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleSetPlan {
    pub set_id: BundleId,
    pub set_version: Version,
    pub package_ids: Vec<BundleId>,
    pub enable_order: Vec<BundleId>,
    pub dependency_plan: BundleDependencyPlan,
}

impl BundleSetPlan {
    pub fn is_blocked(&self) -> bool {
        self.dependency_plan.is_blocked()
    }
}

pub fn resolve_bundle_set(
    manifest: &BundleSetManifest,
    candidates: &[BundleDependencyCandidate],
) -> Result<BundleSetPlan> {
    manifest.validate()?;
    let candidates_by_id: BTreeMap<_, _> = candidates
        .iter()
        .map(|candidate| (candidate.bundle_id.clone(), candidate))
        .collect();
    if candidates_by_id.len() != candidates.len() {
        return Err(BundleSdkError::manifest(
            "bundle_set.candidates",
            "contains duplicate Bundle candidates",
        ));
    }

    let mut desired_enabled: BTreeSet<BundleId> = candidates
        .iter()
        .filter(|candidate| candidate.state.is_enabled())
        .map(|candidate| candidate.bundle_id.clone())
        .collect();
    let mut selected_for_enable = BTreeSet::new();
    for (index, package) in manifest.packages.iter().enumerate() {
        let candidate = candidates_by_id.get(&package.bundle_id).ok_or_else(|| {
            BundleSdkError::manifest(
                format!("packages[{index}].bundle_id"),
                format!(
                    "Bundle {:?} has no inspected package candidate",
                    package.bundle_id.as_str()
                ),
            )
        })?;
        if candidate.bundle_version != package.version {
            return Err(BundleSdkError::manifest(
                format!("packages[{index}].version"),
                format!(
                    "expected {}, inspected {}",
                    package.version, candidate.bundle_version
                ),
            ));
        }
        if candidate.package_manifest_sha256 != package.package_manifest_sha256 {
            return Err(BundleSdkError::manifest(
                format!("packages[{index}].package_manifest_sha256"),
                "does not match the exact inspected package manifest bytes",
            ));
        }
        if package.enable {
            desired_enabled.insert(package.bundle_id.clone());
            selected_for_enable.insert(package.bundle_id.clone());
        }
    }

    let dependency_plan = resolve_bundle_dependencies(candidates, &desired_enabled)?;
    let enable_order = dependency_plan
        .enable_order
        .iter()
        .filter(|bundle_id| selected_for_enable.contains(*bundle_id))
        .cloned()
        .collect();
    Ok(BundleSetPlan {
        set_id: manifest.set.id.clone(),
        set_version: manifest.set.version.clone(),
        package_ids: manifest
            .packages
            .iter()
            .map(|package| package.bundle_id.clone())
            .collect(),
        enable_order,
        dependency_plan,
    })
}

fn default_enable() -> bool {
    true
}

fn bounded_nonempty(field: &str, value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(BundleSdkError::manifest(
            field,
            format!("must contain 1-{max} characters and no control characters"),
        ))
    } else {
        Ok(())
    }
}

fn validate_sha256(field: &str, value: &str) -> Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(BundleSdkError::manifest(
            field,
            "must be a 64-character lowercase hexadecimal SHA-256 digest",
        ))
    }
}

fn is_secret_like_field(name: &str) -> bool {
    let normalized = name.replace('-', "_");
    ["password", "secret", "token", "private_key", "credential"]
        .iter()
        .any(|sensitive| normalized.contains(sensitive))
}

#[cfg(test)]
mod tests {
    use semver::VersionReq;

    use super::*;
    use crate::{
        BundleCandidateState, BundleDependencies, BundleDependencyDeclaration, CapabilityId,
        ProvidedCapability,
    };

    fn candidate(
        id: &str,
        state: BundleCandidateState,
        provides: &[(&str, &str)],
        optional: &[(&str, &str)],
    ) -> BundleDependencyCandidate {
        BundleDependencyCandidate {
            bundle_id: BundleId::new(id).unwrap(),
            bundle_version: Version::new(1, 0, 0),
            package_manifest_sha256: match id {
                "travel-planner" => "a".repeat(64),
                "restaurant-research" => "b".repeat(64),
                _ => "c".repeat(64),
            },
            state,
            provides: provides
                .iter()
                .map(|(id, version)| ProvidedCapability {
                    id: CapabilityId::new(*id).unwrap(),
                    version: Version::parse(version).unwrap(),
                    description: format!("{id} provider"),
                })
                .collect(),
            dependencies: BundleDependencies {
                optional: optional
                    .iter()
                    .map(|(capability, feature)| BundleDependencyDeclaration {
                        capability: CapabilityId::new(*capability).unwrap(),
                        version: VersionReq::parse("^1.0").unwrap(),
                        feature: LocalId::new(*feature).unwrap(),
                        reason: format!("{feature} dependency"),
                        provider_bundle: None,
                        provider_version: None,
                    })
                    .collect(),
                ..BundleDependencies::default()
            },
        }
    }

    fn valid_set() -> String {
        format!(
            r#"
set_manifest_version = 1

[set]
id = "travel-research"
version = "1.0.0"
publisher = "gadgetron.project"

[[packages]]
bundle_id = "restaurant-research"
version = "1.0.0"
package_manifest_sha256 = "{}"

[[packages]]
bundle_id = "travel-planner"
version = "1.0.0"
package_manifest_sha256 = "{}"
settings = {{ home-region = "seoul", daily-budget = 100 }}
"#,
            "b".repeat(64),
            "a".repeat(64)
        )
    }

    #[test]
    fn set_pins_independent_packages_and_uses_dependency_plan_order() {
        let set = BundleSetManifest::parse_toml(&valid_set()).unwrap();
        let capability = "gadgetron.intelligence.restaurant-context";
        let candidates = vec![
            candidate(
                "travel-planner",
                BundleCandidateState::Installed,
                &[],
                &[(capability, "restaurant-assisted-planning")],
            ),
            candidate(
                "restaurant-research",
                BundleCandidateState::Installed,
                &[(capability, "1.0.0")],
                &[],
            ),
        ];
        let plan = resolve_bundle_set(&set, &candidates).unwrap();
        assert!(!plan.is_blocked());
        assert_eq!(
            plan.enable_order
                .iter()
                .map(BundleId::as_str)
                .collect::<Vec<_>>(),
            ["restaurant-research", "travel-planner"]
        );
    }

    #[test]
    fn set_rejects_digest_drift_duplicate_packages_and_secret_settings() {
        let set = BundleSetManifest::parse_toml(&valid_set()).unwrap();
        let candidates = vec![
            candidate("travel-planner", BundleCandidateState::Installed, &[], &[]),
            candidate(
                "restaurant-research",
                BundleCandidateState::Installed,
                &[],
                &[],
            ),
        ];
        let mut drifted = candidates.clone();
        drifted[0].package_manifest_sha256 = "d".repeat(64);
        assert!(resolve_bundle_set(&set, &drifted)
            .unwrap_err()
            .to_string()
            .contains("exact inspected"));

        let duplicate = valid_set().replace(
            "bundle_id = \"restaurant-research\"",
            "bundle_id = \"travel-planner\"",
        );
        assert!(BundleSetManifest::parse_toml(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("duplicates"));

        let secret = valid_set().replace("home-region = \"seoul\"", "api-token = \"not-allowed\"");
        assert!(BundleSetManifest::parse_toml(&secret)
            .unwrap_err()
            .to_string()
            .contains("secret-like"));

        let non_finite = valid_set().replace("daily-budget = 100", "daily-budget = nan");
        assert!(BundleSetManifest::parse_toml(&non_finite)
            .unwrap_err()
            .to_string()
            .contains("must be finite"));

        assert!(
            BundleSetManifest::parse_toml(&"#".repeat(MAX_SET_MANIFEST_BYTES + 1))
                .unwrap_err()
                .to_string()
                .contains("exceeds")
        );
    }
}
