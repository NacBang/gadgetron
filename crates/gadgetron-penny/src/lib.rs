//! gadgetron-penny — agent adapter: Claude Code subprocess lifecycle,
//! Gadget registry dispatch, OpenAI-compatible LlmProvider impl.
//!
//! # Modules
//! - `gadget_registry` — `GadgetRegistry` dispatch table, builder/freeze pattern
//! - `provider` — `PennyProvider: LlmProvider` router registration
//! - `session` — backend subprocess lifecycle
//! - `agent_backend` — backend turn planning + stream adapters
//! - `prompt`   — backend-neutral stdin prompt rendering
//! - `stream`   — stream-json → `ChatChunk` translator
//! - `spawn`    — Command builder with `kill_on_drop(true)`
//! - `gadget_config` — atomic tempfile (0600, unix-only)
//! - `redact`   — `redact_stderr`
//! - `config`   — no runtime config here; `AgentConfig` lives in `gadgetron-core`
//! - `error`    — Local `PennyError` mapped into `GadgetronError::Penny`

// Test code uses the `let mut cfg = X::default(); cfg.field = ...;` pattern
// extensively. See the matching cfg_attr in gadgetron-core/src/lib.rs.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

mod agent_backend;
pub mod backend_session;
pub mod citation;
pub mod gadget_config;
pub mod gadget_registry;
pub mod gadget_server;
pub mod home;
pub mod prompt;
pub mod provider;
pub mod redact;
mod responses_bridge;
pub mod session;
pub mod session_store;
pub mod spawn;
pub mod stream;
pub mod workbench_awareness;

pub use backend_session::AgentBackendSessionPersistence;
pub use gadget_config::{
    build_config_json, build_config_json_for_agent, build_config_json_for_agent_with_env,
    write_config_file, write_config_file_for_agent, GADGETRON_AGENT_GADGETS_JSON_ENV,
};
pub use gadget_registry::{ForwardConfig, GadgetRegistry, GadgetRegistryBuilder};
pub use gadget_server::serve_stdio;
pub use home::{prepare_penny_home, HomeError, PennyHome};
pub use provider::{register_with_router, register_with_router_and_brain_config, PennyProvider};
pub use redact::redact_stderr;
pub use session::{AgentSession, ClaudeCodeSession};
pub use session_store::{
    parse_conversation_id, ConversationId, SessionEntry, SessionStore, DEFAULT_OWNER_ID,
};
pub use spawn::{
    build_claude_command, build_codex_exec_command_with_env, build_codex_exec_command_with_mode,
    format_allowed_tools, CodexExecMode, SpawnError,
};
pub use stream::{event_to_chat_chunks, parse_event, MessageDelta, StreamJsonEvent};
