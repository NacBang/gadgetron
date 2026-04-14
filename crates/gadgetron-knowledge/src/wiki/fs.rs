//! Path resolution for wiki pages. Enforces M3 (path traversal) per
//! `docs/design/phase2/00-overview.md §8` and `01-knowledge-layer.md §4.2`.
//!
//! **CRITICAL**: This function is pure — no filesystem mutation. The `Wiki::write`
//! caller is responsible for creating parent directories AFTER validation passes.
//!
//! NOTE on URL-encoding: `%2e%2e` is NOT decoded by Linux/macOS filesystems — the
//! kernel treats it as a literal filename token, not `..`. The pre-canonicalize
//! rejection list catches `..` as an actual path component; `%2e%2e` is allowed
//! through as a weird-but-harmless filename. The `gadgetron-web::path` validator
//! (SEC-W-B4) handles URL-level percent-decoding; that is a separate boundary.

use crate::error::WikiError;
use std::path::{Component, Path, PathBuf};

const MAX_INPUT_BYTES: usize = 256;

/// Resolves a user-supplied page name to a canonical absolute path inside `root`.
///
/// Returns a canonicalized path ready to pass to `open()` in read mode, or — for
/// not-yet-existing write targets — a computed path whose ancestor chain has been
/// canonicalized. **Does NOT create directories.**
///
/// # Pre-canonicalize rejections (R1–R6)
///
/// 1. Empty or > 256 bytes
/// 2. Null bytes or control characters (`< 0x20` per `char::is_control`)
/// 3. Backslash (Windows UNC attempt)
/// 4. Absolute path (`Path::is_absolute`) or starts with `~`
/// 5. Any `Component::ParentDir` (`..`)
/// 6. Any `Component` that is not `Normal` or `CurDir`
///
/// # Canonicalization
///
/// After R1–R6 pass:
/// 1. Append `.md` if missing
/// 2. Build `candidate = root.join(normalized)`
/// 3. If `candidate` exists → `canonicalize(candidate)` (follows symlinks)
/// 4. Else → canonicalize the deepest existing ancestor, then append the
///    missing tail (write-mode)
/// 5. Assert `canonical.starts_with(canonicalize(root))`
///
/// Any failure returns `WikiError::path_escape(user_input)` or an I/O error.
///
/// # Determinism
///
/// On the happy path this function is deterministic for a given `(root, user_input)`
/// pair across identical filesystem states. Symlink topology changes between calls
/// will produce different results — that is the point of canonicalization.
pub fn resolve_path(root: &Path, user_input: &str) -> Result<PathBuf, WikiError> {
    // R1
    if user_input.is_empty() || user_input.len() > MAX_INPUT_BYTES {
        return Err(WikiError::path_escape(user_input));
    }
    // R2
    if user_input.contains('\0') || user_input.chars().any(|c| c.is_control()) {
        return Err(WikiError::path_escape(user_input));
    }
    // R3
    if user_input.contains('\\') {
        return Err(WikiError::path_escape(user_input));
    }
    // R4
    if Path::new(user_input).is_absolute() || user_input.starts_with('~') {
        return Err(WikiError::path_escape(user_input));
    }
    // R5 + R6
    for component in Path::new(user_input).components() {
        match component {
            Component::ParentDir => return Err(WikiError::path_escape(user_input)),
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(WikiError::path_escape(user_input)),
        }
    }

    // Normalize: append .md suffix if missing. Wiki pages are always markdown.
    let normalized = if user_input.ends_with(".md") {
        user_input.to_string()
    } else {
        format!("{user_input}.md")
    };

    let candidate = root.join(&normalized);
    let canonical_root = std::fs::canonicalize(root)?;

    let canonical = if candidate.exists() {
        std::fs::canonicalize(&candidate)?
    } else {
        canonicalize_with_missing_tail(&candidate)?
    };

    if !canonical.starts_with(&canonical_root) {
        return Err(WikiError::path_escape(user_input));
    }

    Ok(canonical)
}

/// Walks up `path` until an existing ancestor is found, canonicalizes it, then
/// rejoins the non-existent tail. No filesystem mutation. Used for write paths.
///
/// If `path` is absolute but does not exist and has no existing ancestor (e.g.
/// `/nonexistent/foo.md`), returns `Err(WikiError::path_escape(""))` — a
/// pathological case that should not occur for paths rooted in a valid wiki.
fn canonicalize_with_missing_tail(path: &Path) -> Result<PathBuf, WikiError> {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        let parent = existing
            .parent()
            .ok_or_else(|| WikiError::path_escape(""))?
            .to_path_buf();
        let name = existing
            .file_name()
            .ok_or_else(|| WikiError::path_escape(""))?
            .to_os_string();
        tail.push(name);
        existing = parent;
    }
    let mut canonical = std::fs::canonicalize(&existing)?;
    for segment in tail.into_iter().rev() {
        canonical.push(segment);
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_wiki() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn accepts_simple_name() {
        let dir = tmp_wiki();
        let resolved = resolve_path(dir.path(), "foo").expect("ok");
        assert!(resolved.ends_with("foo.md"));
        assert!(resolved.starts_with(std::fs::canonicalize(dir.path()).unwrap()));
    }

    #[test]
    fn accepts_nested_name() {
        let dir = tmp_wiki();
        let resolved = resolve_path(dir.path(), "sub/page").expect("ok");
        assert!(resolved.ends_with("sub/page.md"));
    }

    #[test]
    fn accepts_explicit_md_suffix() {
        let dir = tmp_wiki();
        let resolved = resolve_path(dir.path(), "foo.md").expect("ok");
        assert!(resolved.ends_with("foo.md"));
    }

    #[test]
    fn rejects_empty() {
        let dir = tmp_wiki();
        assert!(resolve_path(dir.path(), "").is_err());
    }

    #[test]
    fn rejects_oversize() {
        let dir = tmp_wiki();
        let big = "a".repeat(257);
        assert!(resolve_path(dir.path(), &big).is_err());
    }

    #[test]
    fn rejects_null_byte() {
        let dir = tmp_wiki();
        assert!(resolve_path(dir.path(), "foo\0bar").is_err());
    }

    #[test]
    fn rejects_control_char() {
        let dir = tmp_wiki();
        assert!(resolve_path(dir.path(), "foo\nbar").is_err());
        assert!(resolve_path(dir.path(), "foo\tbar").is_err());
    }

    #[test]
    fn rejects_backslash() {
        let dir = tmp_wiki();
        assert!(resolve_path(dir.path(), "foo\\bar").is_err());
    }

    #[test]
    fn rejects_absolute_path() {
        let dir = tmp_wiki();
        assert!(resolve_path(dir.path(), "/etc/passwd").is_err());
    }

    #[test]
    fn rejects_tilde_prefix() {
        let dir = tmp_wiki();
        assert!(resolve_path(dir.path(), "~/secret").is_err());
    }

    #[test]
    fn rejects_parent_component() {
        let dir = tmp_wiki();
        assert!(resolve_path(dir.path(), "..").is_err());
        assert!(resolve_path(dir.path(), "../foo").is_err());
        assert!(resolve_path(dir.path(), "foo/../bar").is_err());
    }

    #[test]
    fn percent_encoded_dotdot_is_not_decoded_by_fs_but_stays_inside_root() {
        // URL-level decoding is the gadgetron-web boundary (SEC-W-B4).
        // Here `%2e%2e` is a literal filename token. It passes R1..R6 and lands
        // inside root as `<root>/%2e%2e.md`.
        let dir = tmp_wiki();
        let resolved = resolve_path(dir.path(), "%2e%2e").expect("literal token ok");
        assert!(resolved.starts_with(std::fs::canonicalize(dir.path()).unwrap()));
        assert!(resolved.ends_with("%2e%2e.md"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let dir = tmp_wiki();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("victim.md");
        std::fs::write(&outside_file, "secret").unwrap();

        // Create a symlink INSIDE the wiki that points to the outside file.
        let link = dir.path().join("escape.md");
        symlink(&outside_file, &link).unwrap();

        // resolve_path follows the symlink during canonicalize and detects the escape.
        let err = resolve_path(dir.path(), "escape").expect_err("must escape");
        assert!(
            matches!(
                err.kind_ref(),
                Some(gadgetron_core::error::WikiErrorKind::PathEscape { .. })
            ),
            "got {err:?}"
        );
    }
}
