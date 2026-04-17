//! Credential pattern detection for wiki writes.
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §4.8`.
//!
//! # M5 — 3 BLOCK + 4 AUDIT
//!
//! - **BLOCK** patterns are refused at write time with
//!   `WikiErrorKind::CredentialBlocked { pattern }`. Near-zero false positives;
//!   represent unambiguous high-severity credentials.
//! - **AUDIT** patterns emit a `wiki_write_secret_suspected` tracing warning
//!   but do NOT block. Higher false-positive rate; blocking would frustrate
//!   legitimate Penny use (e.g. pasting commands into wiki notes).
//!
//! Once AUDIT-only patterns are written, the content is permanent in git
//! history (see `00-overview.md §10 SEC-7 Disclosure 1`). The operator is
//! on their own to run `git filter-repo` if they leak a credential.

use once_cell::sync::Lazy;
use regex::Regex;

/// A single pattern match location in content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretPatternMatch {
    pub pattern_name: &'static str,
    pub position: usize,
}

/// Patterns that BLOCK writes. Any match → `CredentialBlocked` refusal.
static BLOCK_PATTERNS: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        (
            "pem_private_key",
            Regex::new(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----")
                .expect("valid pem regex"),
        ),
        (
            "aws_access_key_id",
            Regex::new(r"AKIA[0-9A-Z]{16}").expect("valid AKIA regex"),
        ),
        (
            "gcp_service_account",
            Regex::new(r#""private_key_id"\s*:\s*"[a-f0-9]{40}""#).expect("valid gcp regex"),
        ),
    ]
});

/// Patterns that log audit warnings but do NOT block. See module docs.
static AUDIT_PATTERNS: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        (
            "anthropic_api_key",
            Regex::new(r"sk-ant-[a-zA-Z0-9_\-]{40,}").expect("valid anthropic regex"),
        ),
        (
            "gadgetron_api_key",
            Regex::new(r"gad_(live|test)_[a-f0-9]{32}").expect("valid gadgetron regex"),
        ),
        (
            "bearer_token",
            Regex::new(r"(?i)bearer\s+[A-Za-z0-9._\-]{32,}").expect("valid bearer regex"),
        ),
        (
            "generic_secret",
            Regex::new(r"(?i)(api[_-]?key|secret|token)\s*[:=]\s*[A-Za-z0-9+/]{20,}")
                .expect("valid generic regex"),
        ),
    ]
});

/// Scan `content` for BLOCK patterns. Returns all matches (empty Vec = clean).
///
/// Callers MUST refuse the write with `WikiErrorKind::CredentialBlocked`
/// on any non-empty return. Only the first match's `pattern_name` needs to
/// surface to the user; the full list is kept for audit diagnostics.
pub fn check_block_patterns(content: &str) -> Vec<SecretPatternMatch> {
    let mut matches = Vec::new();
    for (name, re) in BLOCK_PATTERNS.iter() {
        for m in re.find_iter(content) {
            matches.push(SecretPatternMatch {
                pattern_name: name,
                position: m.start(),
            });
        }
    }
    matches
}

/// Scan `content` for AUDIT patterns. Returns all matches (empty Vec = clean).
///
/// Callers emit `tracing::warn!` per match but do NOT refuse the write.
/// The `pattern_name` in the warning lets operators grep audit logs later.
pub fn check_audit_patterns(content: &str) -> Vec<SecretPatternMatch> {
    let mut matches = Vec::new();
    for (name, re) in AUDIT_PATTERNS.iter() {
        for m in re.find_iter(content) {
            matches.push(SecretPatternMatch {
                pattern_name: name,
                position: m.start(),
            });
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- BLOCK patterns ----

    #[test]
    fn blocks_pem_rsa_private_key() {
        let content = "-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n-----END";
        let m = check_block_patterns(content);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].pattern_name, "pem_private_key");
    }

    #[test]
    fn blocks_pem_ec_private_key() {
        let content = "-----BEGIN EC PRIVATE KEY-----\nabc\n-----END";
        let m = check_block_patterns(content);
        assert_eq!(m[0].pattern_name, "pem_private_key");
    }

    #[test]
    fn blocks_pem_openssh_private_key() {
        let content = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END";
        let m = check_block_patterns(content);
        assert_eq!(m[0].pattern_name, "pem_private_key");
    }

    #[test]
    fn blocks_generic_pem_private_key() {
        let content = "-----BEGIN PRIVATE KEY-----\nabc\n-----END";
        let m = check_block_patterns(content);
        assert_eq!(m[0].pattern_name, "pem_private_key");
    }

    #[test]
    fn blocks_aws_access_key_id() {
        // 20-char AKIA key per AWS spec: AKIA + 16 uppercase alphanumerics.
        let content = "my key is AKIAIOSFODNN7EXAMPLE in production";
        let m = check_block_patterns(content);
        assert_eq!(m[0].pattern_name, "aws_access_key_id");
    }

    #[test]
    fn blocks_gcp_service_account_private_key_id() {
        let content = r#"{"type":"service_account","private_key_id":"0123456789abcdef0123456789abcdef01234567"}"#;
        let m = check_block_patterns(content);
        assert_eq!(m[0].pattern_name, "gcp_service_account");
    }

    #[test]
    fn clean_content_has_no_block_matches() {
        let content = "# My notes\n\nJust some wiki content. No secrets here.";
        assert!(check_block_patterns(content).is_empty());
    }

    #[test]
    fn near_miss_aws_key_does_not_match() {
        // AKIAaaaa... (lowercase) shouldn't match.
        let content = "AKIAaaaabbbbccccdddd";
        assert!(check_block_patterns(content).is_empty());
    }

    #[test]
    fn near_miss_pem_header_does_not_match() {
        // Wrong marker text shouldn't match.
        let content = "-----BEGIN RSA PUBLIC KEY-----";
        assert!(check_block_patterns(content).is_empty());
    }

    // ---- AUDIT patterns ----

    #[test]
    fn audits_anthropic_api_key() {
        let content = "token=sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJ";
        let m = check_audit_patterns(content);
        assert!(m.iter().any(|x| x.pattern_name == "anthropic_api_key"));
    }

    #[test]
    fn audits_gadgetron_api_key_live() {
        let content = "export KEY=gad_live_0123456789abcdef0123456789abcdef";
        let m = check_audit_patterns(content);
        assert!(m.iter().any(|x| x.pattern_name == "gadgetron_api_key"));
    }

    #[test]
    fn audits_gadgetron_api_key_test() {
        let content = "test: gad_test_0123456789abcdef0123456789abcdef";
        let m = check_audit_patterns(content);
        assert!(m.iter().any(|x| x.pattern_name == "gadgetron_api_key"));
    }

    #[test]
    fn audits_bearer_token_case_insensitive() {
        let content = "Authorization: Bearer aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789";
        let m = check_audit_patterns(content);
        assert!(m.iter().any(|x| x.pattern_name == "bearer_token"));
    }

    #[test]
    fn audits_generic_api_key_assignment() {
        let content = "api_key = abcdefghij0123456789XYZ";
        let m = check_audit_patterns(content);
        assert!(m.iter().any(|x| x.pattern_name == "generic_secret"));
    }

    #[test]
    fn audit_patterns_do_not_reach_block_list() {
        // A clean Anthropic key should match AUDIT but NOT BLOCK.
        let content = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJ";
        assert!(check_block_patterns(content).is_empty());
        assert!(!check_audit_patterns(content).is_empty());
    }

    #[test]
    fn position_field_reflects_match_offset() {
        // PEM header at offset 10 in a prefixed buffer.
        let content = "prefix....-----BEGIN RSA PRIVATE KEY-----\n";
        let m = check_block_patterns(content);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].position, 10);
    }

    #[test]
    fn multiple_matches_are_all_reported() {
        let content = "AKIAIOSFODNN7EXAMPLE and AKIAABCDEFGHIJKLMNOP together";
        let m = check_block_patterns(content);
        assert_eq!(m.len(), 2);
        assert!(m.iter().all(|x| x.pattern_name == "aws_access_key_id"));
    }
}
