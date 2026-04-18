//! `bundle.toml` parser helpers.
//!
//! Thin wrappers over `toml::from_str` / `std::fs::read_to_string` that
//! translate errors into `BundleError::Manifest`. Permissive by default —
//! unknown top-level fields are accepted for forward compatibility with
//! manifest-v2 shipped in a v1 daemon.
//!
//! `RuntimeMetaV1` (W2 security MUST-LAND per D-20260418-07) will land a
//! strict deny-unknown-fields deserializer for the `external_runtime_meta`
//! JSONB column path — that is a different consumer and lives outside
//! this module.

use std::path::Path;

use crate::bundle::errors::BundleError;
use crate::bundle::manifest::BundleManifest;

/// Parse a TOML string into `BundleManifest`. Surfaces all underlying
/// `toml::de::Error` text verbatim inside `BundleError::Manifest` so the
/// operator sees the line number / span.
pub fn parse_bundle_toml(contents: &str) -> Result<BundleManifest, BundleError> {
    toml::from_str::<BundleManifest>(contents)
        .map_err(|e| BundleError::Manifest(format!("parse bundle.toml: {e}")))
}

/// Read and parse a `bundle.toml` file. I/O and parse errors both map to
/// `BundleError::Manifest` — downstream consumers (W2+ Bundle registry)
/// treat "manifest unreadable" and "manifest malformed" as the same
/// operator-triage class.
pub fn load_bundle_toml(path: &Path) -> Result<BundleManifest, BundleError> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| BundleError::Manifest(format!("read {}: {e}", path.display())))?;
    parse_bundle_toml(&contents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    #[test]
    fn toml_parser_roundtrips_manifest() {
        let src = r#"
            name = "ai-infra"
            version = "0.3.1"
            license = "MIT"
            homepage = "https://github.com/gadgetron/ai-infra"
            plugs = ["openai-llm", "anthropic-llm"]

            [[gadgets]]
            name = "gpu.list"
            tier = "read"

            [requires_plugs]
            "model-load" = ["anthropic-llm"]

            [runtime]
            kind = "in_core"
        "#;
        let m = parse_bundle_toml(src).expect("valid manifest parses");
        assert_eq!(m.name, "ai-infra");
        assert_eq!(m.version, Version::new(0, 3, 1));
        assert_eq!(m.license.as_deref(), Some("MIT"));
        assert_eq!(m.plugs.len(), 2);
        assert_eq!(m.gadgets.len(), 1);
    }

    #[test]
    fn toml_parser_rejects_invalid_toml_syntax() {
        let src = "not = valid = toml";
        let err = parse_bundle_toml(src).unwrap_err();
        assert!(
            matches!(err, BundleError::Manifest(_)),
            "expected Manifest variant, got {err:?}"
        );
        assert!(err.to_string().contains("parse bundle.toml"));
    }

    #[test]
    fn toml_parser_rejects_unknown_top_level_field_gracefully() {
        // Forward-compat note in §module docstring: unknown top-level fields
        // must be accepted (permissive parsing) so a manifest-v2 written by
        // a newer bundle keeps loading in a v1 daemon. This test asserts
        // the permissive contract.
        let src = r#"
            name = "future-bundle"
            version = "1.0.0"
            manifest_version = 2
            future_field = "some value"

            [future_block]
            some_key = 42
        "#;
        let m = parse_bundle_toml(src).expect("unknown fields must not reject");
        assert_eq!(m.name, "future-bundle");
        assert_eq!(m.manifest_version, 2);
    }

    #[test]
    fn toml_parser_rejects_invalid_plug_id() {
        // PlugId validation fires at deserialize — manifest with malformed
        // kebab identifier fails the parse, not a later registration step.
        let src = r#"
            name = "ai-infra"
            version = "0.3.1"
            plugs = ["Uppercase-Plug"]
        "#;
        let err = parse_bundle_toml(src).unwrap_err();
        assert!(
            matches!(err, BundleError::Manifest(_)),
            "expected Manifest variant, got {err:?}"
        );
        assert!(
            err.to_string().contains("Uppercase-Plug") || err.to_string().contains("kebab-case"),
            "err must mention the invalid id: {err}"
        );
    }

    #[test]
    fn toml_loader_surfaces_io_errors() {
        let missing = std::path::PathBuf::from("/nonexistent/path/to/bundle.toml");
        let err = load_bundle_toml(&missing).unwrap_err();
        assert!(matches!(err, BundleError::Manifest(_)));
        assert!(err.to_string().contains("read"));
    }
}
