// Re-export the existing FakeLlmProvider and FailingProvider from providers.rs
// under the new canonical module path expected by E2E tests.
pub use crate::providers::{FailMode, FailingProvider, FakeLlmProvider};
