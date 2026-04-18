//! Validated kebab-case identifiers for Plug and Gadget names.
//!
//! Introduced by ADR-P2A-10-ADDENDUM-01 §4.A (chief-architect four-mandatory-
//! change item A). `PlugId` and `GadgetName` wrap `Arc<str>` so that per-call
//! clones (common in registration paths) are ref-count bumps rather than heap
//! allocations. Kebab-case validation is enforced once at construction time so
//! that every downstream consumer can rely on the invariant.
//!
//! Validation rules (shared by both newtypes):
//!
//! - non-empty
//! - length ≤ 128 chars
//! - characters in `[a-z0-9-]`
//! - does not start or end with `-`
//! - no consecutive `--`
//!
//! Stringly-typed `&str` crosses the boundary only at the config-parser
//! surface (see `BundleContext::is_plug_enabled_by_name` in W2). Inside the
//! Rust type system the validated newtype is the only shape.
//!
//! See also `docs/architecture/glossary.md` §Runtime-time vocabulary.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Kebab-case-validated identifier for a Plug (core-facing trait impl).
///
/// `Arc<str>` internal — per-call clone is a ref-count bump, free.
/// Serialized as a plain string via the `String` bridge, so `PlugId` round-
/// trips through TOML / JSON transparently.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PlugId(Arc<str>);

/// Kebab-case-validated identifier for a Gadget (Penny-facing MCP tool).
///
/// Separate newtype from `PlugId` for misuse resistance: the compiler catches
/// accidental swaps (`requires_plugs: HashMap<GadgetName, Vec<PlugId>>` cannot
/// be populated with `PlugId` keys). Underlying validation is identical.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GadgetName(Arc<str>);

/// Validation failure modes for `PlugId` / `GadgetName`.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum PlugIdError {
    #[error("plug id must be non-empty")]
    Empty,
    #[error("plug id '{0}' exceeds 128 char limit")]
    TooLong(String),
    #[error("plug id '{0}' must be kebab-case (lowercase ASCII, digits, hyphens)")]
    InvalidChars(String),
    #[error("plug id '{0}' must not start or end with hyphen")]
    EdgeHyphen(String),
    #[error("plug id '{0}' must not contain consecutive hyphens")]
    ConsecutiveHyphen(String),
}

/// Length cap. 128 chars comfortably fits any realistic plug / gadget name
/// (`anthropic-llm`, `vram-lru-sched`, `scheduler.stats` → 6–15 chars typical)
/// while still being short enough to protect downstream `tool_audit_events`
/// TEXT columns from adversarial growth.
const MAX_LEN: usize = 128;

/// Shared validator for both `PlugId` and `GadgetName`. Returning the input
/// back to the caller allows both constructors to keep the original string
/// around if they want to build the owned `Arc<str>` from it directly.
fn validate_kebab(s: &str) -> Result<(), PlugIdError> {
    if s.is_empty() {
        return Err(PlugIdError::Empty);
    }
    if s.len() > MAX_LEN {
        return Err(PlugIdError::TooLong(s.to_string()));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == '-')
    {
        return Err(PlugIdError::InvalidChars(s.to_string()));
    }
    if s.starts_with('-') || s.ends_with('-') {
        return Err(PlugIdError::EdgeHyphen(s.to_string()));
    }
    if s.contains("--") {
        return Err(PlugIdError::ConsecutiveHyphen(s.to_string()));
    }
    Ok(())
}

impl PlugId {
    /// Fallible constructor. Use this at the config-parse boundary.
    pub fn new(s: impl Into<String>) -> Result<Self, PlugIdError> {
        let s = s.into();
        validate_kebab(&s)?;
        Ok(Self(Arc::from(s)))
    }

    /// Borrow as `&str` — zero-cost view, safe for audit / log emission.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl GadgetName {
    /// Fallible constructor. Use this at the config-parse boundary.
    pub fn new(s: impl Into<String>) -> Result<Self, PlugIdError> {
        let s = s.into();
        validate_kebab(&s)?;
        Ok(Self(Arc::from(s)))
    }

    /// Borrow as `&str` — zero-cost view.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PlugId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for GadgetName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<PlugId> for String {
    fn from(id: PlugId) -> Self {
        id.0.as_ref().to_owned()
    }
}

impl From<GadgetName> for String {
    fn from(id: GadgetName) -> Self {
        id.0.as_ref().to_owned()
    }
}

impl TryFrom<String> for PlugId {
    type Error = PlugIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<String> for GadgetName {
    type Error = PlugIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for PlugId {
    type Error = PlugIdError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s.to_string())
    }
}

impl TryFrom<&str> for GadgetName {
    type Error = PlugIdError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- PlugId ----

    #[test]
    fn plug_id_rejects_empty() {
        assert_eq!(PlugId::new(""), Err(PlugIdError::Empty));
    }

    #[test]
    fn plug_id_rejects_over_128_chars() {
        let long = "a".repeat(129);
        let err = PlugId::new(long.clone()).unwrap_err();
        assert_eq!(err, PlugIdError::TooLong(long));
    }

    #[test]
    fn plug_id_rejects_uppercase() {
        // ADDENDUM-01 §Consequences mandatory test — kebab-case is lowercase ASCII only.
        let err = PlugId::new("Anthropic-LLM").unwrap_err();
        assert_eq!(err, PlugIdError::InvalidChars("Anthropic-LLM".into()));
    }

    #[test]
    fn plug_id_rejects_underscore() {
        let err = PlugId::new("anthropic_llm").unwrap_err();
        assert_eq!(err, PlugIdError::InvalidChars("anthropic_llm".into()));
    }

    #[test]
    fn plug_id_rejects_leading_hyphen() {
        let err = PlugId::new("-anthropic-llm").unwrap_err();
        assert_eq!(err, PlugIdError::EdgeHyphen("-anthropic-llm".into()));
    }

    #[test]
    fn plug_id_rejects_trailing_hyphen() {
        let err = PlugId::new("anthropic-llm-").unwrap_err();
        assert_eq!(err, PlugIdError::EdgeHyphen("anthropic-llm-".into()));
    }

    #[test]
    fn plug_id_rejects_double_hyphen() {
        let err = PlugId::new("anthropic--llm").unwrap_err();
        assert_eq!(err, PlugIdError::ConsecutiveHyphen("anthropic--llm".into()));
    }

    #[test]
    fn plug_id_accepts_valid_kebab() {
        let id = PlugId::new("anthropic-llm").expect("valid kebab");
        assert_eq!(id.as_str(), "anthropic-llm");

        // Digits and internal numerics are fine.
        let id = PlugId::new("vllm-7b").expect("digits ok");
        assert_eq!(id.as_str(), "vllm-7b");

        // Single character is allowed.
        let id = PlugId::new("a").expect("single char ok");
        assert_eq!(id.as_str(), "a");

        // Hitting the length cap exactly is allowed.
        let max_id = "a".repeat(MAX_LEN);
        let id = PlugId::new(max_id.clone()).expect("at limit ok");
        assert_eq!(id.as_str(), max_id);
    }

    #[test]
    fn plug_id_serde_roundtrips_via_string() {
        // PlugId serializes as a plain TOML string.
        let id = PlugId::new("anthropic-llm").unwrap();
        let tv = toml::Value::try_from(&id).unwrap();
        assert_eq!(tv, toml::Value::String("anthropic-llm".into()));

        // And round-trips back through deserialize.
        let roundtrip: PlugId = tv.try_into().unwrap();
        assert_eq!(roundtrip, id);

        // Invalid input fails at deserialize time.
        let bad = toml::Value::String("Bad-Plug".into());
        let res: Result<PlugId, _> = bad.try_into();
        assert!(res.is_err(), "deserialize must reject invalid kebab");
    }

    // ---- GadgetName — same validator, spot-checked ----

    #[test]
    fn gadget_name_validation_mirrors_plug_id() {
        assert!(GadgetName::new("gpu-list").is_ok());
        // Reuse the same validator.
        assert_eq!(GadgetName::new(""), Err(PlugIdError::Empty));
        assert_eq!(
            GadgetName::new("Has-Caps").unwrap_err(),
            PlugIdError::InvalidChars("Has-Caps".into())
        );
    }

    #[test]
    fn gadget_name_and_plug_id_are_distinct_types() {
        // Compile-time guarantee — the two newtypes do not unify. This test
        // exists as a living-doc anchor; the real guarantee is the type
        // checker.
        let _p: PlugId = PlugId::new("anthropic-llm").unwrap();
        let _g: GadgetName = GadgetName::new("gpu-list").unwrap();
        // `_p = _g;` would not compile — kept as a comment so future
        // refactors that accidentally collapse the types fail CI in the
        // build step.
    }

    #[test]
    fn plug_id_clone_is_cheap() {
        // Arc<str> clones bump the refcount. No heap alloc on the clone path.
        let id = PlugId::new("anthropic-llm").unwrap();
        let c1 = id.clone();
        let c2 = id.clone();
        // Best-effort — the three handles must point at the same allocation.
        assert!(Arc::ptr_eq(&id.0, &c1.0));
        assert!(Arc::ptr_eq(&id.0, &c2.0));
    }
}
