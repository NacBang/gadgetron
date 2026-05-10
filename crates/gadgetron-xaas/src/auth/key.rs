use gadgetron_core::error::GadgetronError;
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct ApiKey {
    pub prefix: String,
    pub hash: String,
}

impl ApiKey {
    pub fn parse(raw: &str) -> Result<Self, GadgetronError> {
        if !raw.starts_with("gad_") {
            return Err(GadgetronError::TenantNotFound);
        }

        let parts: Vec<&str> = raw.splitn(3, '_').collect();
        if parts.len() < 3 || parts[2].len() < 16 {
            return Err(GadgetronError::TenantNotFound);
        }

        let prefix = format!("{}_{}", parts[0], parts[1]);
        let hash = hex::encode(Sha256::digest(raw.as_bytes()));

        Ok(Self { prefix, hash })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_live_key() {
        let key = ApiKey::parse("gad_live_abcdefghijklmnop1234567890123456").unwrap();
        assert_eq!(key.prefix, "gad_live");
        assert_eq!(key.hash.len(), 64);
    }

    #[test]
    fn parse_valid_test_key() {
        let key = ApiKey::parse("gad_test_abcdefghijklmnop1234567890123456").unwrap();
        assert_eq!(key.prefix, "gad_test");
    }

    #[test]
    fn parse_rejects_no_prefix() {
        assert!(ApiKey::parse("sk-some-other-key").is_err());
    }

    #[test]
    fn parse_rejects_too_short() {
        assert!(ApiKey::parse("gad_live_short").is_err());
    }

    #[test]
    fn same_input_produces_same_hash() {
        let raw = "gad_live_abcdefghijklmnop1234567890123456";
        let a = ApiKey::parse(raw).unwrap();
        let b = ApiKey::parse(raw).unwrap();
        assert_eq!(a.hash, b.hash);
    }

    #[test]
    fn different_keys_produce_different_hash() {
        let a = ApiKey::parse("gad_live_abcdefghijklmnop1234567890123456").unwrap();
        let b = ApiKey::parse("gad_live_zyxwvutsrqponmlk0987654321098765").unwrap();
        assert_ne!(a.hash, b.hash);
    }
}
