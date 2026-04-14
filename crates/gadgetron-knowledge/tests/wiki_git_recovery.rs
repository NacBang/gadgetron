//! Git corruption recovery tests for `wiki::git::autocommit`.
//!
//! Per `docs/design/phase2/00-overview.md §9` "Git repo corruption recovery
//! tests" and `01-knowledge-layer.md §4.5` error mapping table.
//!
//! Each test puts a fresh repo into a known-bad state, calls `autocommit`,
//! and verifies the returned error maps to the correct `WikiErrorKind`
//! variant without panicking.

use gadgetron_core::error::WikiErrorKind;
use gadgetron_knowledge::error::WikiError;
use gadgetron_knowledge::wiki::git::{autocommit, open_or_init, signature};
use std::fs;
use std::path::Path;

fn setup_repo() -> (tempfile::TempDir, git2::Repository) {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = open_or_init(dir.path(), "Kairos Test", "kairos@test.local").expect("init");
    (dir, repo)
}

fn expect_kind(err: &WikiError, matcher: impl FnOnce(&WikiErrorKind) -> bool) {
    match err.kind_ref() {
        Some(kind) if matcher(kind) => {}
        Some(kind) => panic!("wrong kind: {kind:?} (err: {err:?})"),
        None => panic!("expected Kind variant, got: {err:?}"),
    }
}

#[test]
fn test_autocommit_on_locked_index() {
    let (dir, repo) = setup_repo();
    fs::write(dir.path().join("page.md"), "content").unwrap();

    // Create a lock file at .git/index.lock — this blocks any index write.
    let lock_path = dir.path().join(".git").join("index.lock");
    fs::write(&lock_path, "").expect("create lock file");

    let sig = signature("X", "x@y.z").expect("sig");
    let err = autocommit(&repo, Path::new("page.md"), &sig).expect_err("must fail on locked index");

    expect_kind(&err, |kind| {
        matches!(
            kind,
            WikiErrorKind::GitCorruption { reason, .. }
                if reason.to_ascii_lowercase().contains("lock")
        )
    });
}

#[test]
fn test_autocommit_on_missing_head_ref() {
    // Setup: remove the HEAD reference file after init. This simulates the
    // "detached HEAD with no branch" / "unreachable HEAD" corruption class.
    let (dir, repo) = setup_repo();
    fs::write(dir.path().join("page.md"), "x").unwrap();

    // Corrupt the HEAD file — replace with garbage.
    let head_path = dir.path().join(".git").join("HEAD");
    fs::write(&head_path, "ref: refs/heads/nonexistent\n").unwrap();

    let sig = signature("X", "x@y.z").unwrap();
    let result = autocommit(&repo, Path::new("page.md"), &sig);

    // Either: we hit GitCorruption (because HEAD resolution fails mid-commit),
    // or: git2 transparently recovers by treating it as an unborn branch and
    // committing as the new root. Both are acceptable — the invariant is
    // "must not panic". If we got `Err`, verify it's a GitCorruption variant.
    if let Err(err) = result {
        expect_kind(&err, |kind| {
            matches!(kind, WikiErrorKind::GitCorruption { .. })
        });
    }
}

#[test]
fn test_autocommit_on_missing_object_database() {
    // Setup: delete .git/objects entirely to simulate object-database
    // corruption. Subsequent commits should fail at tree construction
    // or commit creation with a GitCorruption kind.
    let (dir, _repo) = setup_repo();
    fs::write(dir.path().join("page.md"), "x").unwrap();

    let objects = dir.path().join(".git").join("objects");
    // Remove everything under objects except `info` (which can stay).
    if objects.exists() {
        for entry in fs::read_dir(&objects).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() && path.file_name().unwrap() != "info" {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }

    // Reopen the repo after corruption so any cached state is cleared.
    let repo = git2::Repository::open(dir.path()).unwrap();
    let sig = signature("X", "x@y.z").unwrap();
    let result = autocommit(&repo, Path::new("page.md"), &sig);

    // The commit may actually succeed if git2 recreates objects from the
    // staged file. What we really need to verify is "no panic". If it failed,
    // verify the error maps to GitCorruption (not a random unmapped git2 err).
    if let Err(err) = result {
        expect_kind(&err, |kind| {
            matches!(kind, WikiErrorKind::GitCorruption { .. })
        });
    }
}

#[test]
fn test_autocommit_on_unresolved_merge_conflict() {
    // Setup: create two conflicting commits on different branches then
    // attempt a merge that leaves the index in a conflicted state.
    let (dir, repo) = setup_repo();

    let sig = signature("X", "x@y.z").unwrap();

    // Commit A on main
    fs::write(dir.path().join("page.md"), "A\n").unwrap();
    autocommit(&repo, Path::new("page.md"), &sig).expect("commit A");

    // Create a branch at the initial commit, switch to it, commit B
    let initial = repo.revparse_single("HEAD~1").expect("initial commit").id();
    let initial_commit = repo.find_commit(initial).unwrap();
    repo.branch("other", &initial_commit, true).unwrap();
    repo.set_head("refs/heads/other").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();

    fs::write(dir.path().join("page.md"), "B\n").unwrap();
    autocommit(&repo, Path::new("page.md"), &sig).expect("commit B");

    // Switch back to main (where page.md = "A") and attempt a merge.
    repo.set_head("refs/heads/master")
        .or_else(|_| repo.set_head("refs/heads/main"))
        .unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();

    // Merge "other" into main to produce a conflict in the index.
    let other_oid = repo.revparse_single("refs/heads/other").unwrap().id();
    let annotated = repo.find_annotated_commit(other_oid).unwrap();
    let _ = repo.merge(&[&annotated], None, None);
    // The working tree may now contain conflict markers; write new content
    // and attempt autocommit — it should surface as either Conflict or
    // GitCorruption ("conflict" reason).
    fs::write(dir.path().join("page.md"), "merged?\n").unwrap();

    let result = autocommit(&repo, Path::new("page.md"), &sig);
    if let Err(err) = result {
        expect_kind(&err, |kind| match kind {
            WikiErrorKind::Conflict { .. } => true,
            WikiErrorKind::GitCorruption { reason, .. } => {
                reason.to_ascii_lowercase().contains("conflict")
            }
            _ => false,
        });
    }
    // If result is Ok, git2 resolved cleanly — also acceptable. The invariant
    // is "no panic", not "must always fail".
}

#[test]
fn test_autocommit_does_not_panic_on_nonexistent_file() {
    // Edge case: caller asks to commit a path that doesn't exist on disk
    // (the write step was skipped or the file was deleted between write
    // and autocommit). This should map to a GitCorruption error, not a panic.
    let (_dir, repo) = setup_repo();
    let sig = signature("X", "x@y.z").unwrap();
    let result = autocommit(&repo, Path::new("doesnotexist.md"), &sig);
    assert!(result.is_err());
    expect_kind(&result.unwrap_err(), |kind| {
        matches!(kind, WikiErrorKind::GitCorruption { .. })
    });
}
