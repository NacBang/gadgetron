//! `bundle.toml` manifest schema (ADR-P2A-10-ADDENDUM-01 §3).
//!
//! Every Bundle ships a `BundleManifest` either compile-time (Rust-native
//! bundles, via `Bundle::manifest()` in W2) or as a `bundle.toml` file
//! alongside its entry-point (external bundles, pip / npm / docker /
//! binary-URL). W1 lands only the serde shape and its tests; the
//! `Bundle` trait that consumes the manifest lands in W2.
//!
//! ## Forward compatibility
//!
//! Top-level fields are **permissive** — unknown keys do not reject the
//! manifest, so a v2 manifest in a v1 daemon keeps loading with the v1
//! subset. `manifest_version` is the operator's escape hatch; future
//! behaviour changes gate on it.
//!
//! `RuntimeKind` is `#[non_exhaustive]` so adding `Wasm` / `Container` in
//! a later release does not break third-party consumers that `match` on it.

use std::collections::HashMap;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::bundle::id::{GadgetName, PlugId};

/// Top-level `bundle.toml` shape. Field names match the ADDENDUM-01 §3
/// schema; adding a new optional field is a non-breaking change, adding
/// a required field requires a `manifest_version` bump.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    /// Kebab-case Bundle name. Globally unique per Gadgetron deployment.
    pub name: String,

    /// Semver version of the Bundle.
    pub version: Version,

    /// Manifest schema version. Defaults to 1 so a minimal manifest
    /// (`name = ...`, `version = ...`) round-trips.
    #[serde(default = "default_manifest_version")]
    pub manifest_version: u32,

    /// SPDX license identifier (informational).
    #[serde(default)]
    pub license: Option<String>,

    /// Upstream homepage / catalog URL (informational).
    #[serde(default)]
    pub homepage: Option<String>,

    /// Plugs this Bundle provides. Empty → pure-Gadget Bundle.
    ///
    /// Deserialization validates each entry via `PlugId::try_from(String)`,
    /// so a malformed kebab identifier fails at manifest parse time rather
    /// than at registration time.
    #[serde(default)]
    pub plugs: Vec<PlugId>,

    /// Gadgets this Bundle provides. Empty → pure-Plug Bundle.
    #[serde(default)]
    pub gadgets: Vec<GadgetManifestEntry>,

    /// Per-Gadget cascade dependency map (ADDENDUM-01 §3). If any listed
    /// Plug is missing at `Bundle::install` time, the corresponding Gadget
    /// is not registered and a `WARN` is emitted.
    #[serde(default)]
    pub requires_plugs: HashMap<GadgetName, Vec<PlugId>>,

    /// Runtime metadata for external Bundles. Omitted for in-core bundles
    /// (default `InCore`).
    #[serde(default)]
    pub runtime: Option<RuntimeManifest>,
}

fn default_manifest_version() -> u32 {
    1
}

/// One Gadget entry within a Bundle manifest. `tier` / `description` are
/// operator-facing metadata; the runtime contract is per-Gadget Rust trait.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GadgetManifestEntry {
    /// Dotted namespace.action name (e.g. `"gpu.list"`). Not validated as
    /// kebab because dots are required — full validation happens at
    /// `GadgetRegistry::freeze()` in P2B W2+.
    pub name: String,

    /// One of `"read"`, `"write"`, `"destructive"` (maps to `GadgetTier`).
    /// Optional in the manifest because the Rust `GadgetProvider`
    /// implementation owns the canonical tier; manifest declaration is
    /// documentation / audit bait.
    #[serde(default)]
    pub tier: Option<String>,

    /// Free-form doc string (informational).
    #[serde(default)]
    pub description: Option<String>,
}

/// Runtime metadata for an external Bundle. Populated only when
/// `runtime.kind != InCore`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeManifest {
    /// Runtime kind. Maps to `RuntimeKind` enum.
    #[serde(rename = "kind")]
    pub runtime_kind: RuntimeKind,

    /// Entry-point name (subprocess binary, HTTP URL, container image,
    /// wasm module identifier). Shape depends on `runtime_kind` — doc 12
    /// owns the wire-level interpretation.
    #[serde(default)]
    pub entry: Option<String>,

    /// Transport protocol (e.g. `"mcp-stdio"`, `"mcp-http"`).
    #[serde(default)]
    pub transport: Option<String>,

    /// Resource ceilings (ADDENDUM-01 §5 floor 6). Spawn fails closed if
    /// the Bundle is external and neither manifest nor operator config
    /// declares limits — enforced by the W2+ `Bundle::install` hook.
    #[serde(default)]
    pub limits: Option<RuntimeLimits>,

    /// Network egress allowlist (ADDENDUM-01 §5 floor 7). Default-deny.
    #[serde(default)]
    pub egress: Option<RuntimeEgress>,
}

/// External-runtime resource ceilings. All three are required when a
/// `RuntimeLimits` table is present — the daemon refuses to start a Bundle
/// whose runtime is external and whose limits are under-specified (§5 floor 6).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RuntimeLimits {
    /// `RLIMIT_AS` / cgroup v2 `memory.max` (MB).
    pub memory_mb: u64,

    /// `RLIMIT_NOFILE` open-file cap.
    pub open_files: u32,

    /// `RLIMIT_CPU` seconds.
    pub cpu_seconds: u32,
}

/// External-runtime egress allowlist. Empty / absent = default-deny
/// (ADDENDUM-01 §5 floor 7). The daemon injects the allowlist into
/// nftables (Linux) or the egress-proxy env (macOS dev) at spawn time.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeEgress {
    /// `host:port` allowed destinations. Exact match (no glob) — wildcard
    /// semantics are a doc-12 evolution, not a manifest-v1 feature.
    #[serde(default)]
    pub allow: Vec<String>,
}

/// Runtime dispatch kinds. Maps `bundle.toml` `[bundle.runtime] kind = ...`
/// to the in-process enum the W2+ dispatcher reads. `#[non_exhaustive]` so
/// future kinds (e.g. `Wasm`, `Container`) do not break downstream
/// consumers that `match` on this.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    /// Gadget code lives inside the `gadgetron` binary itself.
    InCore,
    /// Rust-native Bundle compiled into `gadgetron` (module boundary).
    InBundle,
    /// External process spawned per-call or long-lived (graphify, Whisper,
    /// PaddleOCR). Full §5 seven-floor enforcement applies.
    Subprocess,
    /// External HTTP MCP server (remote / SaaS). Loopback token + bearer.
    Http,
    /// OCI container image. cgroup v2 + network namespace by default.
    Container,
    /// Wasm module in `wasmtime` sandbox. Stateless tools only.
    Wasm,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_manifest_parses_minimal_toml() {
        // Smallest possible valid manifest — just name + version.
        // All other fields fall back to their defaults.
        let src = r#"
            name = "ai-infra"
            version = "0.3.1"
        "#;
        let m: BundleManifest = toml::from_str(src).expect("minimal manifest parses");
        assert_eq!(m.name, "ai-infra");
        assert_eq!(m.version, Version::new(0, 3, 1));
        assert_eq!(m.manifest_version, 1, "default manifest_version is 1");
        assert!(m.plugs.is_empty());
        assert!(m.gadgets.is_empty());
        assert!(m.requires_plugs.is_empty());
        assert!(m.runtime.is_none());
        assert!(m.license.is_none());
        assert!(m.homepage.is_none());
    }

    #[test]
    fn bundle_manifest_parses_requires_plugs() {
        // ADDENDUM-01 §3 per-Gadget cascade map. The cascade key uses the
        // kebab-safe form of the gadget name (`"model-load"`) because
        // `GadgetName`'s validator rejects dots. The dotted-action form
        // (`"model.load"`) stays in `GadgetManifestEntry::name` where
        // kebab validation does not apply. See the §Forward compatibility
        // note in the module docstring.
        let src = r#"
            name = "ai-infra"
            version = "0.3.1"
            plugs = ["openai-llm", "anthropic-llm", "vllm"]

            [[gadgets]]
            name = "model.load"
            tier = "destructive"

            [[gadgets]]
            name = "gpu.list"
            tier = "read"

            [requires_plugs]
            "model-load" = ["anthropic-llm"]
            "gpu-list"   = []
        "#;
        let m: BundleManifest = toml::from_str(src).expect("requires_plugs manifest parses");
        assert_eq!(m.plugs.len(), 3);
        assert_eq!(m.gadgets.len(), 2);

        let model_load_key = GadgetName::new("model-load").unwrap();
        let deps = m
            .requires_plugs
            .get(&model_load_key)
            .expect("cascade map contains model-load");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].as_str(), "anthropic-llm");

        let gpu_list_key = GadgetName::new("gpu-list").unwrap();
        let deps = m
            .requires_plugs
            .get(&gpu_list_key)
            .expect("cascade map contains gpu-list");
        assert!(deps.is_empty());
    }

    #[test]
    fn bundle_manifest_parses_runtime_limits() {
        let src = r#"
            name = "graphify"
            version = "0.4.21"

            [runtime]
            kind      = "subprocess"
            entry     = "graphify-mcp"
            transport = "mcp-stdio"

            [runtime.limits]
            memory_mb   = 2048
            open_files  = 256
            cpu_seconds = 300

            [runtime.egress]
            allow = ["api.anthropic.com:443", "api.openai.com:443"]
        "#;
        let m: BundleManifest = toml::from_str(src).expect("runtime manifest parses");
        let rt = m.runtime.expect("runtime block present");
        assert_eq!(rt.runtime_kind, RuntimeKind::Subprocess);
        assert_eq!(rt.entry.as_deref(), Some("graphify-mcp"));
        assert_eq!(rt.transport.as_deref(), Some("mcp-stdio"));
        let limits = rt.limits.expect("limits present");
        assert_eq!(limits.memory_mb, 2048);
        assert_eq!(limits.open_files, 256);
        assert_eq!(limits.cpu_seconds, 300);
        let egress = rt.egress.expect("egress present");
        assert_eq!(egress.allow.len(), 2);
        assert!(egress.allow.iter().any(|s| s == "api.anthropic.com:443"));
    }

    #[test]
    fn bundle_manifest_rejects_invalid_plug_id_in_plugs() {
        // `PlugId` validator fires at deserialize time — an uppercase plug
        // id in the manifest fails the parse outright.
        let src = r#"
            name = "ai-infra"
            version = "0.3.1"
            plugs = ["Anthropic-LLM"]
        "#;
        let err = toml::from_str::<BundleManifest>(src).unwrap_err();
        let msg = err.to_string();
        // Error message must name the invalid identifier (came from PlugIdError::InvalidChars).
        assert!(
            msg.contains("Anthropic-LLM") || msg.contains("kebab-case"),
            "err must name invalid plug id, got: {msg}"
        );
    }

    #[test]
    fn bundle_manifest_defaults_manifest_version_to_1() {
        let src = r#"
            name = "minimal"
            version = "1.0.0"
        "#;
        let m: BundleManifest = toml::from_str(src).unwrap();
        assert_eq!(m.manifest_version, 1);

        // And an explicit override is respected.
        let src = r#"
            name = "forward"
            version = "1.0.0"
            manifest_version = 2
        "#;
        let m: BundleManifest = toml::from_str(src).unwrap();
        assert_eq!(m.manifest_version, 2);
    }

    #[test]
    fn runtime_kind_snake_case_roundtrip() {
        // RuntimeKind uses `#[serde(rename_all = "snake_case")]` — confirm
        // the TOML shape matches the ADR.
        let src = r#"
            name = "wasm-bundle"
            version = "0.1.0"

            [runtime]
            kind = "wasm"
        "#;
        let m: BundleManifest = toml::from_str(src).unwrap();
        assert_eq!(m.runtime.unwrap().runtime_kind, RuntimeKind::Wasm);

        let src = r#"
            name = "x"
            version = "0.1.0"

            [runtime]
            kind = "in_core"
        "#;
        let m: BundleManifest = toml::from_str(src).unwrap();
        assert_eq!(m.runtime.unwrap().runtime_kind, RuntimeKind::InCore);
    }
}
