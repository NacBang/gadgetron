//! Agent backend session persistence.
//!
//! The in-memory `SessionStore` keeps per-conversation mutexes and hot
//! session ids. This trait is the optional durable layer used by DB-backed
//! deployments so Claude Code, Codex, and future compatible backends can all
//! resume through the same Gadgetron conversation id after a server restart.

use async_trait::async_trait;
use gadgetron_core::agent::{AgentBackend, ConversationAgentProfile};

#[async_trait]
pub trait AgentBackendSessionPersistence: Send + Sync {
    /// Load the immutable-backend/per-chat model profile. The default keeps
    /// non-DB test adapters source-compatible and falls back to global config.
    async fn load_conversation_agent_profile(
        &self,
        _conversation_id: &str,
    ) -> Option<ConversationAgentProfile> {
        None
    }

    async fn load_backend_session_id(
        &self,
        conversation_id: &str,
        backend: AgentBackend,
    ) -> Option<String>;

    async fn save_backend_session_id(
        &self,
        conversation_id: &str,
        backend: AgentBackend,
        backend_session_id: &str,
    );
}
