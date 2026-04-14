//! Git backend for the wiki store.
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §4.5`.
//!
//! Uses `git2` (libgit2 bindings). Errors from git2 are mapped into the
//! gadgetron error taxonomy — specifically `WikiErrorKind::GitCorruption`
//! and `WikiErrorKind::Conflict` per the corruption recovery matrix.
//!
//! # Commit message policy (M5 + SEC-7)
//!
//! Auto-commit messages are deliberately abstract: they never include
//! the request id, user query text, assistant response text, or content
//! excerpts — that information would leak into git history, which is
//! user-readable and potentially syncable to remotes.
//!
//! Format: `"auto-commit: {relative_path} {iso8601_utc}"`.

use crate::error::WikiError;
use gadgetron_core::error::WikiErrorKind;
use std::path::Path;

/// Opens an existing git repo at `root`, or initializes a new one if absent.
///
/// On init, creates an empty initial commit so downstream `autocommit` calls
/// have a parent to chain against.
pub fn open_or_init(
    root: &Path,
    author_name: &str,
    author_email: &str,
) -> Result<git2::Repository, WikiError> {
    match git2::Repository::open(root) {
        Ok(repo) => Ok(repo),
        Err(e) if e.code() == git2::ErrorCode::NotFound => init_with_empty_commit(root, author_name, author_email),
        Err(e) => Err(map_git_error(e, root)),
    }
}

fn init_with_empty_commit(
    root: &Path,
    author_name: &str,
    author_email: &str,
) -> Result<git2::Repository, WikiError> {
    let repo = git2::Repository::init(root).map_err(|e| map_git_error(e, root))?;
    // Scope the borrows from `repo` so they drop before we move `repo`.
    {
        let sig = signature(author_name, author_email)?;
        let mut index = repo.index().map_err(|e| map_git_error(e, root))?;
        let tree_id = index.write_tree().map_err(|e| map_git_error(e, root))?;
        let tree = repo.find_tree(tree_id).map_err(|e| map_git_error(e, root))?;
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "initial commit",
            &tree,
            &[],
        )
        .map_err(|e| map_git_error(e, root))?;
    }
    Ok(repo)
}

/// Build a `git2::Signature` from the configured author identity.
///
/// Returns a fresh `Signature::now` on every call so the commit timestamp
/// is current. Invalid name/email produce `WikiErrorKind::GitCorruption`
/// with a reason string pointing operators at `git config user.name/email`.
pub fn signature(
    author_name: &str,
    author_email: &str,
) -> Result<git2::Signature<'static>, WikiError> {
    git2::Signature::now(author_name, author_email).map_err(|e| {
        WikiError::kind_with_message(
            WikiErrorKind::GitCorruption {
                path: String::new(),
                reason: format!("invalid git author identity: {e}"),
            },
            format!(
                "cannot construct git signature — check `[knowledge] wiki_git_author_name` \
                 and `wiki_git_author_email` in gadgetron.toml, or run \
                 `git config user.name` / `git config user.email` (reason: {e})"
            ),
        )
    })
}

/// Stages and commits a single file change.
///
/// `relative_path` must be relative to the repo root. It is NOT validated
/// here — callers are expected to have already passed through
/// `wiki::fs::resolve_path`.
///
/// Returns the new commit's `Oid`. Errors from git2 are mapped per
/// `map_git_error`:
///
/// - `ErrorClass::Index` + `ErrorCode::Locked` → `GitCorruption { reason: "index locked" }`
/// - Detached HEAD (resolved HEAD is not a reference) → `GitCorruption { reason: "detached HEAD" }`
/// - Missing tree/blob → `GitCorruption { reason: "missing objects" }`
/// - `ErrorClass::Merge` → `Conflict`
/// - Any other git2 error → `GitCorruption { reason: <raw git2 message> }`
pub fn autocommit(
    repo: &git2::Repository,
    relative_path: &Path,
    signature: &git2::Signature,
) -> Result<git2::Oid, WikiError> {
    let path_display = relative_path.to_string_lossy().into_owned();

    // Stage the file.
    let mut index = repo.index().map_err(|e| map_git_error_for_path(e, &path_display))?;
    index
        .add_path(relative_path)
        .map_err(|e| map_git_error_for_path(e, &path_display))?;
    index.write().map_err(|e| map_git_error_for_path(e, &path_display))?;

    // Build the tree from the staged index.
    let tree_id = index
        .write_tree()
        .map_err(|e| map_git_error_for_path(e, &path_display))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| map_git_error_for_path(e, &path_display))?;

    // Resolve HEAD for the parent commit.
    let parent_commit = match repo.head() {
        Ok(head_ref) => {
            let oid = head_ref.target().ok_or_else(|| {
                WikiError::kind_with_message(
                    WikiErrorKind::GitCorruption {
                        path: path_display.clone(),
                        reason: "HEAD reference has no target".into(),
                    },
                    format!(
                        "wiki git repo at {path_display:?} has a corrupted HEAD — \
                         run `git status` in the wiki directory and repair manually"
                    ),
                )
            })?;
            Some(
                repo.find_commit(oid)
                    .map_err(|e| map_git_error_for_path(e, &path_display))?,
            )
        }
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => None,
        Err(e) if e.code() == git2::ErrorCode::NotFound => None,
        Err(e) => return Err(map_git_error_for_path(e, &path_display)),
    };

    // Abstract commit message — no content excerpt, no user input, no request id.
    let timestamp = chrono_like_iso8601_utc(&signature.when());
    let message = format!("auto-commit: {path_display} {timestamp}");

    let parents: Vec<&git2::Commit> = parent_commit.as_ref().map(|c| vec![c]).unwrap_or_default();

    let oid = repo
        .commit(Some("HEAD"), signature, signature, &message, &tree, &parents)
        .map_err(|e| map_git_error_for_path(e, &path_display))?;

    Ok(oid)
}

/// Tiny ISO 8601 UTC formatter derived from a `git2::Time`. We avoid pulling
/// in `chrono` just for this one string — `git2::Time::seconds` gives us a
/// Unix timestamp and we format manually in `YYYY-MM-DDTHH:MM:SSZ`.
fn chrono_like_iso8601_utc(t: &git2::Time) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let secs = t.seconds();
    // git2::Time can be negative for dates before epoch; clamp at 0 to be safe.
    let secs = if secs < 0 { 0 } else { secs as u64 };
    let datetime = UNIX_EPOCH + Duration::from_secs(secs);
    // Split into Y/M/D H:M:S without a date crate.
    let total_secs = datetime
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (days, rem) = (total_secs / 86_400, total_secs % 86_400);
    let (hours, rem) = (rem / 3600, rem % 3600);
    let (minutes, seconds) = (rem / 60, rem % 60);
    // Convert `days` from 1970-01-01 to Y/M/D.
    let (year, month, day) = days_to_ymd(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert days-since-1970-01-01 to `(year, month, day)` using the civil
/// calendar algorithm from Howard Hinnant. Gregorian, no DST.
fn days_to_ymd(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Generic error-mapper from `git2::Error` → `WikiError`.
fn map_git_error(e: git2::Error, _root: &Path) -> WikiError {
    map_git_error_for_path(e, "")
}

/// Path-aware error-mapper used by `autocommit` so the resulting
/// `GitCorruption.path` carries the file that failed to commit.
fn map_git_error_for_path(e: git2::Error, path: &str) -> WikiError {
    use git2::ErrorClass;

    let class = e.class();
    let code = e.code();
    let raw = e.message().to_string();

    // Merge conflict → Conflict
    if matches!(class, ErrorClass::Merge | ErrorClass::Checkout)
        || raw.to_ascii_lowercase().contains("conflict")
    {
        return WikiError::kind_with_message(
            WikiErrorKind::Conflict {
                path: path.to_string(),
            },
            format!(
                "wiki page {path:?} has an unresolved merge conflict — \
                 resolve the git conflict in the wiki directory, then retry"
            ),
        );
    }

    // Index-class errors (locked index, corrupt index, etc.) → GitCorruption
    let reason = if matches!(class, ErrorClass::Index) {
        if code == git2::ErrorCode::Locked || raw.to_ascii_lowercase().contains("locked") {
            "index locked".to_string()
        } else {
            format!("index error: {raw}")
        }
    } else if raw.to_ascii_lowercase().contains("missing") {
        // Missing tree/blob objects
        "missing objects".to_string()
    } else if raw.to_ascii_lowercase().contains("detached") {
        "detached HEAD".to_string()
    } else if raw.to_ascii_lowercase().contains("not found")
        || code == git2::ErrorCode::NotFound
    {
        format!("git resource not found: {raw}")
    } else {
        raw.clone()
    };

    WikiError::kind_with_message(
        WikiErrorKind::GitCorruption {
            path: path.to_string(),
            reason: reason.clone(),
        },
        format!(
            "wiki git repo error ({reason}) — run `git status` in the wiki \
             directory and repair manually"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fresh_repo() -> (TempDir, git2::Repository) {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = open_or_init(dir.path(), "Kairos Test", "kairos@test.local").expect("init");
        (dir, repo)
    }

    #[test]
    fn open_or_init_creates_repo_with_initial_commit() {
        let (_dir, repo) = fresh_repo();
        // Initial commit should exist — HEAD resolves.
        let head = repo.head().expect("HEAD");
        let oid = head.target().expect("oid");
        let commit = repo.find_commit(oid).expect("commit");
        assert_eq!(commit.message().unwrap_or(""), "initial commit");
    }

    #[test]
    fn open_or_init_on_existing_repo_returns_it() {
        let dir = tempfile::tempdir().unwrap();
        // First open: init.
        let _repo1 = open_or_init(dir.path(), "A", "a@x.y").expect("first");
        // Second open: reuse.
        let repo2 = open_or_init(dir.path(), "A", "a@x.y").expect("second");
        assert!(repo2.head().is_ok());
    }

    #[test]
    fn autocommit_writes_and_creates_new_commit() {
        let (dir, repo) = fresh_repo();
        let file = dir.path().join("hello.md");
        fs::write(&file, "# Hello\n").unwrap();

        let sig = signature("Kairos", "kairos@test.local").expect("sig");
        let oid = autocommit(&repo, Path::new("hello.md"), &sig).expect("commit");

        // New commit is now HEAD.
        let head = repo.head().unwrap().target().unwrap();
        assert_eq!(head, oid);

        // Commit message must be abstract (path + timestamp only).
        let commit = repo.find_commit(oid).unwrap();
        let msg = commit.message().unwrap();
        assert!(msg.starts_with("auto-commit: hello.md "), "msg: {msg}");
        // No content excerpt.
        assert!(!msg.contains("Hello"));
    }

    #[test]
    fn autocommit_commits_multiple_files_in_sequence() {
        let (dir, repo) = fresh_repo();
        fs::write(dir.path().join("a.md"), "a").unwrap();
        fs::write(dir.path().join("b.md"), "b").unwrap();

        let sig = signature("X", "x@y.z").unwrap();
        autocommit(&repo, Path::new("a.md"), &sig).expect("a");
        autocommit(&repo, Path::new("b.md"), &sig).expect("b");

        // Two new commits on top of the initial one = 3 total.
        let mut walker = repo.revwalk().unwrap();
        walker.push_head().unwrap();
        let count = walker.count();
        assert_eq!(count, 3);
    }

    #[test]
    fn autocommit_abstract_message_never_contains_content() {
        // SEC-7: git history is user-readable. Commit messages must not
        // leak page content, user queries, or response text.
        let (dir, repo) = fresh_repo();
        let secret = "USER PRIVATE QUERY DO NOT LEAK";
        fs::write(dir.path().join("note.md"), secret).unwrap();

        let sig = signature("X", "x@y.z").unwrap();
        let oid = autocommit(&repo, Path::new("note.md"), &sig).unwrap();
        let msg = repo.find_commit(oid).unwrap().message().unwrap().to_string();

        assert!(!msg.contains("USER PRIVATE QUERY"));
        assert!(!msg.contains("DO NOT LEAK"));
    }

    #[test]
    fn days_to_ymd_matches_known_dates() {
        // 1970-01-01 = day 0
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        // 2000-01-01 = day 10957
        assert_eq!(days_to_ymd(10_957), (2000, 1, 1));
        // 2026-04-14 = day 20557 (14 leap days between 1972 and 2024 inclusive)
        assert_eq!(days_to_ymd(20_557), (2026, 4, 14));
    }

    #[test]
    fn chrono_like_iso8601_utc_produces_z_suffix() {
        // 2026-04-14T00:00:00Z in seconds = 20557 * 86400
        let t = git2::Time::new(20_557 * 86_400, 0);
        let s = chrono_like_iso8601_utc(&t);
        assert_eq!(s, "2026-04-14T00:00:00Z");
    }
}
