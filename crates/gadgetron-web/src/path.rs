//! Path validation for the static asset handler.
//!
//! Defense in depth against path traversal + Unicode bypass vectors. See
//! `docs/design/phase2/03-gadgetron-web.md` §6 and SEC-W-B4 / SEC-W-B8 in the v1/v2
//! security reviews.

use percent_encoding::percent_decode_str;
use std::path::Component;

/// Reject unsafe path inputs. Decodes once (percent-decoding), then re-checks.
/// Returns `Err(())` on any suspicious input; returns `Ok(decoded)` on pass.
///
/// Rejection categories:
/// 1. Null bytes + backslashes (pre-decode — sign of bypass attempt)
/// 2. Percent-decoding failure (invalid UTF-8 after decode)
/// 3. Literal `..` or leading `/` after decode
/// 4. ASCII-only allowlist `[A-Za-z0-9._\-/]` — rejects fullwidth unicode dots (U+FF0E),
///    any Unicode lookalike bypass, and all non-ASCII input. The embedded asset tree
///    (Next.js static export) only emits ASCII filenames, so this is tight without
///    being over-restrictive. (SEC-W-B8)
/// 5. Hidden files (any segment beginning with `.`)
/// 6. Any component that is not `Component::Normal` (Parent / Root / Prefix / CurDir)
///
/// The `Err(())` shape is deliberate — the caller maps failure to a fixed
/// HTTP 400 response with no detail (SEC-W-B4), so there is no structured
/// error to carry. The `#[allow]` below documents the clippy override.
#[allow(clippy::result_unit_err)]
pub fn validate_and_decode(raw: &str) -> Result<String, ()> {
    if raw.contains('\0') || raw.contains('\\') {
        return Err(());
    }
    let decoded = percent_decode_str(raw).decode_utf8().map_err(|_| ())?;
    let decoded = decoded.into_owned();

    if decoded.starts_with('/') || decoded.contains("..") {
        return Err(());
    }

    // ASCII-only allowlist (SEC-W-B8). Rejects fullwidth dots and all non-ASCII input.
    if !decoded
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
    {
        return Err(());
    }

    if decoded.split('/').any(|seg| seg.starts_with('.')) {
        return Err(());
    }

    // Walk components; only `Normal` is allowed.
    let p = std::path::Path::new(&decoded);
    if !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(());
    }

    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_paths_accepted() {
        for ok in [
            "index.html",
            "settings/index.html",
            "_next/static/abc123.js",
            "_next/static/def456.css",
            "favicon.ico",
        ] {
            assert!(validate_and_decode(ok).is_ok(), "should accept: {ok}");
        }
    }

    #[test]
    fn traversal_variants_all_rejected() {
        for bad in [
            "../Cargo.toml",
            "%2e%2e/Cargo.toml",
            "%2E%2E/Cargo.toml",
            ".%2e/Cargo.toml",
            "/etc/passwd",
            "//etc/passwd",
            "..\\windows\\system32",
            "foo\0bar",
            ".env",
            "foo/.git/HEAD",
            "\u{FF0E}\u{FF0E}/etc/passwd",
        ] {
            assert!(validate_and_decode(bad).is_err(), "should reject: {bad:?}");
        }
    }
}
