//! Path validation for the static asset handler.
//!
//! Defense in depth against path traversal + Unicode bypass vectors.

use percent_encoding::percent_decode_str;
use std::path::Component;

/// Reject unsafe path inputs. Decodes once (percent-decoding), then re-checks.
/// Returns `Err(())` on any suspicious input; returns `Ok(decoded)` on pass.
///
/// Rejection categories:
/// 1. Null bytes + backslashes (pre-decode — sign of bypass attempt)
/// 2. Percent-decoding failure (invalid UTF-8 after decode)
/// 3. A `..` path SEGMENT or leading `/` after decode. Segment
///    equality, not substring: turbopack content hashes can end with a
///    dot, producing legitimate chunk names like `10swcmy6r70o..js`
///    whose `..` never crosses a directory. A substring check used to
///    400 that chunk and knock the whole UI into Next's global-error
///    page (ISSUE 57).
/// 4. ASCII-only allowlist `[A-Za-z0-9._\-/]` — rejects fullwidth unicode dots (U+FF0E),
///    any Unicode lookalike bypass, and all non-ASCII input. The embedded asset tree
///    (Next.js static export) only emits ASCII filenames, so this is tight without
///    being over-restrictive.
/// 5. Hidden files (any segment beginning with `.`)
/// 6. Any component that is not `Component::Normal` (Parent / Root / Prefix / CurDir)
///
/// The `Err(())` shape is deliberate — the caller maps failure to a fixed
/// HTTP 400 response with no detail, so there is no structured
/// error to carry. The `#[allow]` below documents the clippy override.
#[allow(clippy::result_unit_err)]
pub fn validate_and_decode(raw: &str) -> Result<String, ()> {
    if raw.contains('\0') || raw.contains('\\') {
        return Err(());
    }
    let decoded = percent_decode_str(raw).decode_utf8().map_err(|_| ())?;
    let decoded = decoded.into_owned();

    if decoded.starts_with('/') || decoded.split('/').any(|seg| seg == "..") {
        return Err(());
    }

    // ASCII-only allowlist. Rejects fullwidth dots and all non-ASCII input.
    // Parentheses `(` `)` permitted for Next.js route-group segments (`app/(shell)/…`).
    // `~` permitted for turbopack-generated chunk filenames (RFC 3986 unreserved).
    if !decoded.bytes().all(|b| {
        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b'(' | b')' | b'~')
    }) {
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
            "_next/static/chunks/08~pvhfxkn~e1.js",
            "_next/static/chunks/16rjnlch9~l9_.css",
            // Turbopack hashes can end with a dot — the `..` here is
            // inside ONE segment and never crosses a directory.
            "_next/static/chunks/10swcmy6r70o..js",
            "_next/static/chunks/a.b..c.js",
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
            "foo/../bar",
            "..",
            "chunks/..",
        ] {
            assert!(validate_and_decode(bad).is_err(), "should reject: {bad:?}");
        }
    }
}
