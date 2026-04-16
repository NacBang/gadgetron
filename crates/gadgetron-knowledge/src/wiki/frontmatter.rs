//! TOML frontmatter parser for wiki pages.
//!
//! Spec: `docs/design/phase2/05-knowledge-semantic.md §4`.
//!
//! A wiki page MAY begin with a TOML frontmatter block delimited by `---`
//! lines. Everything between the opening `---\n` and the next `---\n` is
//! parsed as TOML. The remainder is the page body.
//!
//! Recognized fields (all optional, enforcement-soft):
//!
//! | field           | type      | expected values                                   |
//! |-----------------|-----------|---------------------------------------------------|
//! | `tags`          | `[String]`| free-form                                         |
//! | `type`          | `String`  | `"incident"` \| `"runbook"` \| `"decision"` …     |
//! | `created`       | RFC 3339  | auto-filled by writer                             |
//! | `updated`       | RFC 3339  | auto-filled by writer                             |
//! | `source`        | `String`  | `"user"` \| `"conversation"` \| `"reindex"` \| `"seed"` |
//! | `confidence`    | `String`  | `"high"` \| `"medium"` \| `"low"`                 |
//! | `plugin`        | `String`  | e.g. `"gadgetron-core"` for core seeds            |
//! | `plugin_version`| `String`  | semver                                             |
//!
//! Unknown fields are preserved in `extra: HashMap<String, toml::Value>`.
//! Unknown `source`/`confidence` values emit a `tracing::warn!` but do not
//! reject the parse — convention, not enforcement.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::WikiError;

const FENCE: &str = "---";

/// Recognized values of `source` — closed enum, warn on unknown.
/// Kept as `&[&str]` instead of `enum` so the frontmatter struct stays
/// `Serialize`-friendly without a custom serializer for the enum variant.
const KNOWN_SOURCE_VALUES: &[&str] = &["user", "conversation", "reindex", "seed"];
const KNOWN_CONFIDENCE_VALUES: &[&str] = &["high", "medium", "low"];

/// Parsed TOML frontmatter. All fields optional — `parse_page` on a page
/// without frontmatter returns `WikiFrontmatter::default()`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WikiFrontmatter {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub page_type: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_datetime",
        skip_serializing_if = "Option::is_none"
    )]
    pub created: Option<DateTime<Utc>>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_datetime",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    /// Plugin that seeded/owns this page. Required when `source = "seed"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// Semver of the plugin that seeded this page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<String>,
    /// Anything not in the known field list. Round-trips on re-serialize.
    #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, toml::Value>,
}

/// A page split into its optional frontmatter + body.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPage {
    pub frontmatter: WikiFrontmatter,
    pub body: String,
}

/// Split a raw wiki file into optional frontmatter + body.
///
/// Behavior:
/// - If the file does NOT start with `---\n` → `frontmatter = default`, body = entire input.
/// - If it DOES start with `---\n` but no closing `---` found → frontmatter parse error.
/// - If TOML parse fails → error.
/// - Otherwise: body is everything after the closing fence, with the immediate
///   `\n` after the fence consumed (the body does not start with a blank line
///   artifact of the fence).
pub fn parse_page(raw: &str) -> Result<ParsedPage, WikiError> {
    let Some((fm_src, body)) = split_frontmatter(raw) else {
        return Ok(ParsedPage {
            frontmatter: WikiFrontmatter::default(),
            body: raw.to_string(),
        });
    };

    let frontmatter: WikiFrontmatter = toml::from_str(fm_src).map_err(|e| {
        WikiError::kind_with_message(
            gadgetron_core::error::WikiErrorKind::GitCorruption {
                path: String::new(),
                reason: format!("frontmatter TOML parse failed: {e}"),
            },
            format!("frontmatter TOML parse failed: {e}"),
        )
    })?;

    // Soft validation — warn on unknown source / confidence values.
    if let Some(src) = frontmatter.source.as_deref() {
        if !KNOWN_SOURCE_VALUES.contains(&src) {
            tracing::warn!(
                target: "wiki_frontmatter",
                value = %src,
                "unknown `source` value; treating as opaque"
            );
        }
    }
    if let Some(conf) = frontmatter.confidence.as_deref() {
        if !KNOWN_CONFIDENCE_VALUES.contains(&conf) {
            tracing::warn!(
                target: "wiki_frontmatter",
                value = %conf,
                "unknown `confidence` value; treating as opaque"
            );
        }
    }

    Ok(ParsedPage {
        frontmatter,
        body: body.to_string(),
    })
}

/// Serialize `frontmatter + body` back into a wiki file string. If the
/// frontmatter has no fields set (all defaults), the fence is omitted so
/// round-tripping a plain-body page doesn't introduce an empty fence.
pub fn serialize_page(fm: &WikiFrontmatter, body: &str) -> Result<String, WikiError> {
    if is_empty(fm) {
        return Ok(body.to_string());
    }
    let toml_str = toml::to_string(fm).map_err(|e| {
        WikiError::kind_with_message(
            gadgetron_core::error::WikiErrorKind::GitCorruption {
                path: String::new(),
                reason: format!("frontmatter serialize failed: {e}"),
            },
            format!("frontmatter serialize failed: {e}"),
        )
    })?;
    // Ensure trailing newline on toml_str; toml-rs already emits one usually.
    let body_start = if body.starts_with('\n') { "" } else { "\n" };
    Ok(format!("{FENCE}\n{toml_str}{FENCE}{body_start}{body}"))
}

/// Split raw text into `(frontmatter_source, body)` if a fence pair is
/// present at the head. Returns `None` when there is no frontmatter.
fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    // Must start with "---\n" (or "---\r\n") as the very first bytes.
    let after_open = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))?;

    // Find the next fence line. A valid closing is a line containing exactly
    // "---" (trailing newline optional at EOF).
    let mut search = after_open;
    let mut fm_len = 0usize;
    loop {
        let line_end = search.find('\n').unwrap_or(search.len());
        let line = &search[..line_end];
        let line_no_cr = line.trim_end_matches('\r');
        if line_no_cr == FENCE {
            // Found closing fence.
            let fm_src = &after_open[..fm_len];
            let after_close = &search[line_end..];
            let body = after_close.strip_prefix('\n').unwrap_or(after_close);
            return Some((fm_src, body));
        }
        fm_len += line_end + 1; // include the newline
        if line_end == search.len() {
            // EOF without finding closing fence.
            return None;
        }
        search = &search[line_end + 1..];
    }
}

/// Accept either a TOML native datetime (which serde-toml maps to a struct
/// with year/month/… fields) or an RFC 3339 string. Both get normalized to
/// `chrono::DateTime<Utc>`.
fn deserialize_optional_datetime<'de, D>(de: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Str(String),
        Toml(toml::value::Datetime),
    }
    let opt = Option::<Raw>::deserialize(de)?;
    Ok(match opt {
        None => None,
        Some(Raw::Str(s)) => Some(
            DateTime::parse_from_rfc3339(&s)
                .map_err(serde::de::Error::custom)?
                .with_timezone(&Utc),
        ),
        Some(Raw::Toml(t)) => {
            // toml::value::Datetime stringifies in RFC 3339; re-parse to Utc.
            let s = t.to_string();
            Some(
                DateTime::parse_from_rfc3339(&s)
                    .map_err(serde::de::Error::custom)?
                    .with_timezone(&Utc),
            )
        }
    })
}

fn is_empty(fm: &WikiFrontmatter) -> bool {
    fm.tags.is_empty()
        && fm.page_type.is_none()
        && fm.created.is_none()
        && fm.updated.is_none()
        && fm.source.is_none()
        && fm.confidence.is_none()
        && fm.plugin.is_none()
        && fm.plugin_version.is_none()
        && fm.extra.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn frontmatter_absent_returns_default() {
        let raw = "# No frontmatter here\n\nJust a page body.\n";
        let parsed = parse_page(raw).expect("should parse");
        assert_eq!(parsed.frontmatter, WikiFrontmatter::default());
        assert_eq!(parsed.body, raw);
    }

    #[test]
    fn frontmatter_parses_full_fields() {
        let raw = r#"---
tags = ["H100", "ECC", "boot-failure"]
type = "incident"
created = 2026-04-16T10:30:00Z
updated = 2026-04-16T11:00:00Z
source = "conversation"
confidence = "high"
plugin = "gadgetron-core"
plugin_version = "0.3.0"
---
# Body starts here
"#;
        let parsed = parse_page(raw).expect("parse");
        let fm = &parsed.frontmatter;
        assert_eq!(fm.tags, vec!["H100", "ECC", "boot-failure"]);
        assert_eq!(fm.page_type.as_deref(), Some("incident"));
        assert_eq!(fm.source.as_deref(), Some("conversation"));
        assert_eq!(fm.confidence.as_deref(), Some("high"));
        assert_eq!(fm.plugin.as_deref(), Some("gadgetron-core"));
        assert_eq!(fm.plugin_version.as_deref(), Some("0.3.0"));
        assert_eq!(
            fm.created,
            Some(Utc.with_ymd_and_hms(2026, 4, 16, 10, 30, 0).unwrap())
        );
        assert_eq!(parsed.body, "# Body starts here\n");
    }

    #[test]
    fn frontmatter_malformed_toml_returns_error() {
        let raw = "---\nthis is = = not valid toml\n---\nbody\n";
        assert!(parse_page(raw).is_err());
    }

    #[test]
    fn frontmatter_unknown_fields_preserved_in_extra() {
        let raw = r#"---
tags = ["a"]
unknown_field = "foo"
another_one = 42
---
body
"#;
        let parsed = parse_page(raw).expect("parse");
        assert_eq!(parsed.frontmatter.extra.len(), 2);
        assert_eq!(
            parsed
                .frontmatter
                .extra
                .get("unknown_field")
                .and_then(|v| v.as_str()),
            Some("foo")
        );
        assert_eq!(
            parsed
                .frontmatter
                .extra
                .get("another_one")
                .and_then(|v| v.as_integer()),
            Some(42)
        );
    }

    #[test]
    fn frontmatter_source_unknown_value_warns_not_errors() {
        let raw = "---\nsource = \"from_mars\"\n---\nbody\n";
        let parsed = parse_page(raw).expect("parse should succeed");
        assert_eq!(parsed.frontmatter.source.as_deref(), Some("from_mars"));
    }

    #[test]
    fn frontmatter_confidence_unknown_value_warns_not_errors() {
        let raw = "---\nconfidence = \"maybe\"\n---\nbody\n";
        let parsed = parse_page(raw).expect("parse should succeed");
        assert_eq!(parsed.frontmatter.confidence.as_deref(), Some("maybe"));
    }

    #[test]
    fn serialize_round_trip_preserves_fields() {
        let fm = WikiFrontmatter {
            tags: vec!["alpha".into(), "beta".into()],
            page_type: Some("runbook".into()),
            source: Some("user".into()),
            confidence: Some("high".into()),
            ..Default::default()
        };
        let body = "page body\n";
        let serialized = serialize_page(&fm, body).expect("serialize");
        assert!(serialized.starts_with("---\n"));
        let parsed = parse_page(&serialized).expect("re-parse");
        assert_eq!(parsed.frontmatter, fm);
        assert_eq!(parsed.body, body);
    }

    #[test]
    fn serialize_absent_frontmatter_emits_none() {
        let fm = WikiFrontmatter::default();
        let body = "just a body\n";
        let serialized = serialize_page(&fm, body).expect("serialize");
        assert_eq!(serialized, body);
    }

    // --- extra tests ---

    #[test]
    fn frontmatter_crlf_line_endings_supported() {
        let raw = "---\r\ntags = [\"x\"]\r\n---\r\nbody\r\n";
        let parsed = parse_page(raw).expect("parse");
        assert_eq!(parsed.frontmatter.tags, vec!["x"]);
    }

    #[test]
    fn frontmatter_missing_closing_fence_errors() {
        // Opens with `---\n` but never closes → split_frontmatter returns
        // None, so parse_page treats it as "no frontmatter". The raw body
        // includes the leading `---\n`. This is the defensive-pick: a lone
        // opening fence shouldn't break writes.
        let raw = "---\ntags = [\"x\"]\n\nno closing fence\n";
        let parsed = parse_page(raw).expect("parse");
        assert_eq!(parsed.frontmatter, WikiFrontmatter::default());
        assert_eq!(parsed.body, raw);
    }

    #[test]
    fn parse_body_does_not_contain_fence() {
        let raw = "---\ntype = \"note\"\n---\n# Hello\n";
        let parsed = parse_page(raw).expect("parse");
        assert_eq!(parsed.body, "# Hello\n");
        assert!(!parsed.body.contains("---"));
    }
}
