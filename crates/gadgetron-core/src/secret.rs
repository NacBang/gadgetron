use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(inner: T) -> Self {
        Self(inner)
    }

    pub fn expose(&self) -> &T {
        &self.0
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl<T: PartialEq> PartialEq for Secret<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: Eq> Eq for Secret<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let s = Secret::new("sk-my-secret-key".to_string());
        let debug = format!("{s:?}");
        assert_eq!(debug, "[REDACTED]");
        assert!(!debug.contains("sk-my-secret"));
    }

    #[test]
    fn display_is_redacted() {
        let s = Secret::new("sk-my-secret-key".to_string());
        let display = format!("{s}");
        assert_eq!(display, "[REDACTED]");
    }

    #[test]
    fn expose_returns_inner_value() {
        let s = Secret::new("sk-my-secret-key".to_string());
        assert_eq!(s.expose(), "sk-my-secret-key");
    }

    #[test]
    fn into_inner_consumes() {
        let s = Secret::new(42u32);
        assert_eq!(s.into_inner(), 42);
    }

    #[test]
    fn clone_preserves_value() {
        let s = Secret::new("key".to_string());
        let cloned = s.clone();
        assert_eq!(cloned.expose(), "key");
    }

    #[test]
    fn serde_roundtrip() {
        let s = Secret::new("my-key".to_string());
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"my-key\"");
        let deserialized: Secret<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.expose(), "my-key");
    }

    #[test]
    fn equality_compares_inner() {
        let a = Secret::new("same");
        let b = Secret::new("same");
        let c = Secret::new("diff");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
