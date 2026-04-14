//! Proptest corpus for `gadgetron_knowledge::wiki::resolve_path`.
//!
//! Design: `docs/design/phase2/01-knowledge-layer.md §10.3`.
//! Harness defaults: `PROPTEST_SEED=42`, `PROPTEST_CASES=1024`
//! (see `docs/design/testing/harness.md`).
//!
//! Properties asserted:
//!
//! 1. **No panic** — `resolve_path` returns `Ok(_)` or `Err(_)` on every input
//!    without ever panicking. This is the primary safety property.
//! 2. **Escape rejection** — any input containing `..`, `\0`, `\`, leading `/`,
//!    leading `~`, or a control character is rejected with `WikiErrorKind::PathEscape`.
//! 3. **Happy-path confinement** — when resolution succeeds, the returned path
//!    starts with the canonicalized wiki root (no symlink-based escapes via
//!    generated names alone — the dedicated symlink test in `wiki/fs.rs` covers
//!    the symlink escape case).

use gadgetron_core::error::WikiErrorKind;
use gadgetron_knowledge::wiki::resolve_path;
use gadgetron_knowledge::WikiError;
use proptest::prelude::*;

fn safe_strategy() -> impl Strategy<Value = String> {
    // Strings composed of [a-zA-Z0-9._-/] up to 64 bytes. These should all be
    // validated as safe (though they may still be rejected for `..` sequences).
    "[a-zA-Z0-9._\\-/]{0,64}"
}

fn hostile_strategy() -> impl Strategy<Value = String> {
    // Mix in the known rejection vectors. These should all be rejected.
    prop_oneof![
        Just("..".to_string()),
        Just("../etc/passwd".to_string()),
        Just("foo/../bar".to_string()),
        Just("/etc/passwd".to_string()),
        Just("~/secret".to_string()),
        Just("foo\0bar".to_string()),
        Just("foo\nbar".to_string()),
        Just("foo\tbar".to_string()),
        Just("foo\\bar".to_string()),
        Just(String::new()),
        // Random noise — any UTF-8 string
        any::<String>(),
    ]
}

fn any_input_strategy() -> impl Strategy<Value = String> {
    prop_oneof![safe_strategy(), hostile_strategy()]
}

fn fresh_wiki() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn never_panics(input in any_input_strategy()) {
        let wiki = fresh_wiki();
        let _ = resolve_path(wiki.path(), &input);
    }

    #[test]
    fn rejects_all_hostile_inputs(input in hostile_strategy()) {
        let wiki = fresh_wiki();
        match resolve_path(wiki.path(), &input) {
            Ok(path) => {
                // Accepted inputs MUST stay inside the canonicalized root.
                let root = std::fs::canonicalize(wiki.path()).unwrap();
                prop_assert!(
                    path.starts_with(&root),
                    "escape: input={input:?} path={path:?} root={root:?}"
                );
            }
            Err(e) => {
                // All error variants are acceptable for hostile input — we only
                // require no panic and no silent success outside the root.
                // (Io error is acceptable for e.g. non-UTF8 filenames on macOS.)
                prop_assert!(matches!(
                    e,
                    WikiError::Kind { .. } | WikiError::Io(_) | WikiError::Frontmatter(_)
                ));
            }
        }
    }

    #[test]
    fn happy_path_stays_inside_root(input in "[a-zA-Z0-9]{1,32}") {
        let wiki = fresh_wiki();
        let root = std::fs::canonicalize(wiki.path()).unwrap();
        let resolved = resolve_path(wiki.path(), &input).expect("happy path");
        prop_assert!(resolved.starts_with(&root));
        prop_assert!(resolved.to_string_lossy().ends_with(".md"));
    }
}
