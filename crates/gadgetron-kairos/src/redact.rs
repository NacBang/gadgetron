//! Stderr redaction for Claude Code subprocess output (M2).
//!
//! Spec: `docs/design/phase2/02-kairos-agent.md §8`.
//!
//! # What this is for
//!
//! Claude Code subprocess stderr is captured by `ClaudeCodeSession` and
//! surfaced in `KairosErrorKind::AgentError.stderr_redacted`. That field
//! eventually reaches the audit log and — in debug logging paths — the
//! gadgetron operator's tracing subscriber. Both are places a leaked
//! credential string would be catastrophic.
//!
//! `redact_stderr` scrubs known-shape secrets from the captured string
//! **before** it crosses the ClaudeCodeSession boundary. The patterns
//! are tightly scoped — we REMOVED the `oauth_state` catch-all from the
//! design draft because it was destroying legitimate diagnostic content
//! (git SHAs, absolute paths, Rust backtrace symbols).
//!
//! # Guarantees
//!
//! - **Idempotent** — `redact_stderr(redact_stderr(x)) == redact_stderr(x)`
//!   for any input `x`.
//! - **Bounded regex** — every pattern uses bounded quantifiers (e.g.
//!   `{32,512}`) to avoid catastrophic backtracking on adversarial input.
//!   `redact_stderr_completes_fast_on_adversarial_input` locks this in.
//! - **No catch-all** — inputs without a known-shape match return
//!   byte-identical to the input.
//! - **Owned return** — always returns a fresh `String`; never borrows.
//!
//! # Known limitation
//!
//! Base64-encoded secrets without a recognizable prefix (no `sk-ant-`,
//! `gad_live_`, etc.) are NOT caught by any pattern. This is accepted
//! for the P2A single-user threat model — the operator is the only
//! audience for stderr output, and base64 blobs without context are
//! typically indistinguishable from legitimate binary data.

use once_cell::sync::Lazy;
use regex::Regex;

/// Redaction regex list. Each entry is `(pattern_name, compiled_regex)`.
/// Matches are replaced with `[REDACTED:<pattern_name>]`.
///
/// All patterns use bounded repetition ({lower,upper}) to avoid
/// catastrophic backtracking on adversarial input (SEC-B4).
static REDACTION_PATTERNS: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        (
            "anthropic_key",
            Regex::new(r"sk-ant-[a-zA-Z0-9_\-]{40,512}").expect("valid anthropic regex"),
        ),
        (
            "gadgetron_key",
            Regex::new(r"gad_(live|test)_[a-f0-9]{32}").expect("valid gadgetron regex"),
        ),
        (
            "bearer_token",
            Regex::new(r"(?i)bearer\s+[A-Za-z0-9._\-]{32,512}").expect("valid bearer regex"),
        ),
        (
            "generic_secret",
            Regex::new(
                r"(?i)(api[_-]?key|secret|token)\s*[:=]\s*[A-Za-z0-9+/]{20,512}",
            )
            .expect("valid generic regex"),
        ),
        (
            "aws_access_key",
            Regex::new(r"AKIA[0-9A-Z]{16}").expect("valid AKIA regex"),
        ),
        (
            "pem_header",
            Regex::new(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----")
                .expect("valid pem regex"),
        ),
    ]
});

/// Replace substrings matching any known secret pattern with
/// `[REDACTED:<name>]`. Preserves non-matching content byte-for-byte.
///
/// See module docs for guarantees.
pub fn redact_stderr(raw: &str) -> String {
    let mut result = raw.to_string();
    for (name, re) in REDACTION_PATTERNS.iter() {
        let marker = format!("[REDACTED:{name}]");
        result = re.replace_all(&result, marker.as_str()).into_owned();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_anthropic_key() {
        let input = "error: token sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefgh was rejected";
        let out = redact_stderr(input);
        assert!(out.contains("[REDACTED:anthropic_key]"));
        assert!(!out.contains("sk-ant-api03"));
    }

    #[test]
    fn redacts_gadgetron_live_key() {
        let input = "KEY=gad_live_0123456789abcdef0123456789abcdef";
        let out = redact_stderr(input);
        assert!(out.contains("[REDACTED:gadgetron_key]"));
        assert!(!out.contains("gad_live_0"));
    }

    #[test]
    fn redacts_gadgetron_test_key() {
        let input = "KEY=gad_test_0123456789abcdef0123456789abcdef";
        let out = redact_stderr(input);
        assert!(out.contains("[REDACTED:gadgetron_key]"));
    }

    #[test]
    fn redacts_bearer_token_case_insensitive() {
        let input = "Authorization: Bearer aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789_-.";
        let out = redact_stderr(input);
        assert!(out.contains("[REDACTED:bearer_token]"));
    }

    #[test]
    fn redacts_generic_secret_assignment() {
        let input = "api_key = ABCDEFGHIJKLMNOPQRSTUVWXYZ0123";
        let out = redact_stderr(input);
        assert!(out.contains("[REDACTED:generic_secret]"));
    }

    #[test]
    fn redacts_aws_access_key() {
        let input = "using key AKIAIOSFODNN7EXAMPLE in production";
        let out = redact_stderr(input);
        assert!(out.contains("[REDACTED:aws_access_key]"));
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn redacts_pem_private_key_header() {
        let input = "stderr: -----BEGIN RSA PRIVATE KEY-----\nbody\n-----END";
        let out = redact_stderr(input);
        assert!(out.contains("[REDACTED:pem_header]"));
    }

    #[test]
    fn is_idempotent() {
        let input = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJ and AKIAIOSFODNN7EXAMPLE";
        let once = redact_stderr(input);
        let twice = redact_stderr(&once);
        assert_eq!(once, twice);
    }

    // ---- preservation of legitimate diagnostic content ----

    #[test]
    fn preserves_clean_error_message() {
        let input = "error: file not found: /usr/local/bin/claude";
        assert_eq!(redact_stderr(input), input);
    }

    #[test]
    fn preserves_long_path_in_clean_text() {
        // SEC-B2 regression: removal of the `oauth_state` catch-all must
        // not destroy long diagnostic paths.
        let input =
            "/home/user/.claude/session/abc123def456ghi789jkl012mno345pqr678stu/config.json";
        assert_eq!(redact_stderr(input), input);
    }

    #[test]
    fn preserves_git_commit_sha() {
        let input = "commit 0123456789abcdef0123456789abcdef01234567 fixes bug";
        assert_eq!(redact_stderr(input), input);
    }

    #[test]
    fn preserves_rust_backtrace() {
        let input = "  3: std::panicking::rust_panic_with_hook\n             at rustc/abc1234/library/std/src/panicking.rs:646:17";
        assert_eq!(redact_stderr(input), input);
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(redact_stderr(""), "");
    }

    #[test]
    fn redacts_multiple_secrets_in_one_pass() {
        let input = "key1: sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJ and AKIAIOSFODNN7EXAMPLE";
        let out = redact_stderr(input);
        assert!(out.contains("[REDACTED:anthropic_key]"));
        assert!(out.contains("[REDACTED:aws_access_key]"));
    }

    #[test]
    fn adversarial_long_input_completes_quickly() {
        // SEC-B4: bounded quantifiers must prevent catastrophic backtracking.
        // 50k chars with a "token =" prefix should redact in bounded time.
        // Debug build with 6 regex patterns × 50KB runs in roughly 200-400ms
        // on a modest VM; the real signal is "does NOT explode to seconds/
        // minutes", so the threshold is 2 seconds — well below catastrophic
        // backtracking and still flagging any pathological regression.
        let input = format!("token = {}", "A".repeat(50_000));
        let start = std::time::Instant::now();
        let _ = redact_stderr(&input);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "redact_stderr took > 2s on 50KB adversarial input ({}ms) — \
             possible catastrophic backtracking regression",
            start.elapsed().as_millis()
        );
    }
}
