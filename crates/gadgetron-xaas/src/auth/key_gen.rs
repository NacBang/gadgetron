use rand::RngCore;
use sha2::{Digest, Sha256};

/// Generate a new Gadgetron API key for the given `kind` (e.g. `"live"` or `"test"`).
///
/// Returns `(raw_key, key_hash)` where:
/// - `raw_key`  — the plaintext key shown to the user once: `gad_{kind}_{32_hex_chars}`.
///   **Never log or store this value.**
/// - `key_hash` — the SHA-256 hex digest of `raw_key` (64 hex chars). Store only this.
///
/// # Security note
/// The raw key is generated from `OsRng`, which is cryptographically secure on all
/// supported platforms (Linux: `getrandom`, macOS: `SecRandomCopyBytes`, Windows: `BCryptGenRandom`).
/// The hash is a plain SHA-256 (not `argon2`) because key lookups must be fast (<1 ms)
/// and the 128-bit random suffix already provides sufficient entropy against brute-force.
///
/// # Example
/// ```rust
/// let (raw, hash) = gadgetron_xaas::auth::key_gen::generate_api_key("live");
/// assert!(raw.starts_with("gad_live_"));
/// assert_eq!(hash.len(), 64);
/// ```
pub fn generate_api_key(kind: &str) -> (String, String) {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let suffix = hex::encode(bytes); // 32 lowercase hex chars
    let raw = format!("gad_{kind}_{suffix}");
    let hash = hex::encode(Sha256::digest(raw.as_bytes()));
    (raw, hash)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// S7-1-T1: generated key has the correct `gad_{kind}_` prefix.
    #[test]
    fn generate_key_has_correct_prefix() {
        let (raw, _hash) = generate_api_key("live");
        assert!(
            raw.starts_with("gad_live_"),
            "raw key must start with 'gad_live_', got: {raw}"
        );
    }

    /// S7-1-T2: suffix is exactly 32 lowercase hex characters (16 random bytes).
    ///
    /// The key format is `gad_{kind}_{suffix}`. After stripping the `gad_live_`
    /// prefix (9 chars) the remainder must be exactly 32 hex chars.
    #[test]
    fn generate_key_suffix_is_32_hex() {
        let (raw, _hash) = generate_api_key("live");
        let prefix = "gad_live_";
        assert!(
            raw.starts_with(prefix),
            "raw key must start with '{prefix}', got: {raw}"
        );
        let suffix = &raw[prefix.len()..];
        assert_eq!(
            suffix.len(),
            32,
            "suffix must be 32 hex chars, got {} chars: '{suffix}'",
            suffix.len()
        );
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "suffix must contain only hex digits [0-9a-f], got: '{suffix}'"
        );
    }

    /// S7-1-T3: hash is exactly 64 lowercase hex characters (SHA-256 output).
    #[test]
    fn generate_key_hash_is_64_hex() {
        let (_raw, hash) = generate_api_key("live");
        assert_eq!(
            hash.len(),
            64,
            "SHA-256 hex digest must be 64 chars, got {} chars",
            hash.len()
        );
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash must contain only hex digits [0-9a-f], got: '{hash}'"
        );
    }

    /// S7-1-T4: two independently generated keys are always distinct.
    ///
    /// OsRng provides 128 bits of entropy per key — collision probability is
    /// astronomically low. This test would catch a broken RNG (e.g. constant seed).
    #[test]
    fn two_keys_are_different() {
        let (raw_a, hash_a) = generate_api_key("live");
        let (raw_b, hash_b) = generate_api_key("live");
        assert_ne!(raw_a, raw_b, "two raw keys must not be equal");
        assert_ne!(hash_a, hash_b, "two key hashes must not be equal");
    }

    /// Bonus: key passes `ApiKey::parse` validation rules (prefix + 16+ char suffix).
    ///
    /// This ensures `generate_api_key` output is always compatible with the
    /// validator in `crates/gadgetron-xaas/src/auth/key.rs`.
    #[test]
    fn generated_key_passes_api_key_parse() {
        use crate::auth::key::ApiKey;
        let (raw, _hash) = generate_api_key("live");
        let parsed = ApiKey::parse(&raw).expect("generated key must pass ApiKey::parse");
        assert_eq!(parsed.prefix, "gad_live");
        assert_eq!(parsed.hash.len(), 64);
    }
}
