use std::{fmt, str::FromStr};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::{BundleSdkError, Result};

const MAX_ID_LEN: usize = 64;
const MAX_GADGET_NAME_LEN: usize = 128;
const MAX_CAPABILITY_ID_LEN: usize = 160;
const MAX_RELATIVE_PATH_LEN: usize = 256;

macro_rules! string_newtype {
    ($name:ident, $kind:literal, $validator:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                $validator(&value).map_err(|reason| BundleSdkError::InvalidIdentifier {
                    kind: $kind,
                    value: value.clone(),
                    reason,
                })?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = BundleSdkError;

            fn from_str(value: &str) -> Result<Self> {
                Self::new(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(de::Error::custom)
            }
        }
    };
}

string_newtype!(BundleId, "Bundle id", validate_kebab_id);
string_newtype!(LocalId, "local id", validate_kebab_id);
string_newtype!(GadgetName, "Gadget name", validate_gadget_name);
string_newtype!(CapabilityId, "capability id", validate_capability_id);
string_newtype!(
    RelativePath,
    "relative package path",
    validate_relative_path
);

impl BundleId {
    /// Accept the historical catalog id alphabet without weakening the public
    /// package contract. This method is for migration adapters only.
    pub fn parse_legacy(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_legacy_bundle_id(&value).map_err(|reason| BundleSdkError::InvalidIdentifier {
            kind: "legacy Bundle id",
            value: value.clone(),
            reason,
        })?;
        Ok(Self(value))
    }
}

impl GadgetName {
    pub fn namespace(&self) -> &str {
        self.0.split('.').next().unwrap_or_default()
    }
}

fn validate_kebab_id(value: &str) -> std::result::Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_ID_LEN {
        return Err("must contain 1-64 ASCII characters");
    }
    if !value.is_ascii() {
        return Err("must be ASCII lowercase kebab-case");
    }
    if value.starts_with('-') || value.ends_with('-') || value.contains("--") {
        return Err("must not start/end with '-' or contain consecutive '-'");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("allowed characters are [a-z0-9-]");
    }
    Ok(())
}

fn validate_legacy_bundle_id(value: &str) -> std::result::Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_ID_LEN {
        return Err("must contain 1-64 ASCII characters");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("allowed characters are [a-zA-Z0-9_-]");
    }
    Ok(())
}

fn validate_gadget_name(value: &str) -> std::result::Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_GADGET_NAME_LEN {
        return Err("must contain 1-128 ASCII characters");
    }
    let mut parts = value.split('.');
    let Some(namespace) = parts.next() else {
        return Err("must contain a namespace and action separated by '.'");
    };
    let Some(action) = parts.next() else {
        return Err("must contain a namespace and action separated by '.'");
    };
    if validate_kebab_id(namespace).is_err() || validate_kebab_id(action).is_err() {
        return Err("each dotted segment must be lowercase kebab-case");
    }
    for part in parts {
        if validate_kebab_id(part).is_err() {
            return Err("each dotted segment must be lowercase kebab-case");
        }
    }
    Ok(())
}

fn validate_capability_id(value: &str) -> std::result::Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_CAPABILITY_ID_LEN {
        return Err("must contain 1-160 ASCII characters");
    }
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() < 2 || parts.iter().any(|part| validate_kebab_id(part).is_err()) {
        return Err("must contain at least two lowercase kebab-case segments separated by '.'");
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> std::result::Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_RELATIVE_PATH_LEN {
        return Err("must contain 1-256 characters");
    }
    if !value.is_ascii() || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err("must be printable ASCII");
    }
    if value.starts_with('/') || value.contains('\\') {
        return Err("must be a portable relative path");
    }
    if value
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("must not contain empty, '.' or '..' path components");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_and_legacy_bundle_ids_have_separate_entry_points() {
        assert!(BundleId::new("server-administrator").is_ok());
        assert!(BundleId::new("Server_Admin").is_err());
        assert!(BundleId::parse_legacy("Server_Admin").is_ok());
        assert!(BundleId::parse_legacy("../escape").is_err());
    }

    #[test]
    fn gadget_names_are_namespaced_and_paths_cannot_escape() {
        let name = GadgetName::new("server.inventory.refresh").unwrap();
        assert_eq!(name.namespace(), "server");
        assert!(GadgetName::new("refresh").is_err());
        assert!(RelativePath::new("schema/domain.json").is_ok());
        assert!(RelativePath::new("../secret").is_err());
        assert!(RelativePath::new("/etc/passwd").is_err());
        assert!(CapabilityId::new("gadgetron.intelligence.restaurant-context").is_ok());
        assert!(CapabilityId::new("restaurant-context").is_err());
        assert!(CapabilityId::new("Gadgetron.Intelligence").is_err());
    }
}
