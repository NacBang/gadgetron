//! gadgetron-penny — agent adapter: Claude Code subprocess lifecycle,
//! Gadget registry dispatch, OpenAI-compatible LlmProvider impl.
//!
//! # Phase 2A scope (Path 1 — approval flow deferred to P2B)
//!
//! Per ADR-P2A-06, the interactive approval flow (ApprovalRegistry,
//! `POST /v1/approvals/{id}`, SSE `gadgetron.approval_required` event,
//! `<ApprovalCard>` frontend) is **deferred to Phase 2B**. P2A ships with
//! `GadgetMode::Auto` / `GadgetMode::Never` only; operators express Gadget
//! policy statically via `[agent.gadgets.*]` config.
//!
//! # Modules (P2A)
//! - `gadget_registry` — `GadgetRegistry` dispatch table, builder/freeze pattern
//! - `provider` — `PennyProvider: LlmProvider` router registration
//! - `session` — `ClaudeCodeSession` subprocess lifecycle (chief-arch B3)
//! - `stream`   — stream-json → `ChatChunk` translator
//! - `spawn`    — Command builder with `kill_on_drop(true)`
//! - `gadget_config` — tempfile M1 (atomic 0600, unix-only)
//! - `redact`   — `redact_stderr` (M2)
//! - `config`   — no runtime config here; `AgentConfig` lives in `gadgetron-core`
//! - `error`    — Local `PennyError` mapped into `GadgetronError::Penny`
//!
//! # Modules deferred to P2B
//! - `approval` — `ApprovalRegistry`, `PendingApproval`, cross-process bridge
//!
//! See `docs/design/phase2/04-gadget-registry.md` + `02-penny-agent.md` v4
//! and `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md`.

// Test code uses the `let mut cfg = X::default(); cfg.field = ...;` pattern
// extensively. See the matching cfg_attr in gadgetron-core/src/lib.rs.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

pub mod gadget_config;
pub mod gadget_registry;
pub mod gadget_server;
pub mod home;
pub mod provider;
pub mod redact;
pub mod session;
pub mod session_store;
pub mod spawn;
pub mod stream;

pub use gadget_config::{build_config_json, write_config_file};
pub use gadget_registry::{GadgetRegistry, GadgetRegistryBuilder};
pub use gadget_server::serve_stdio;
pub use home::{prepare_penny_home, HomeError, PennyHome};
pub use provider::{register_with_router, PennyProvider};
pub use redact::redact_stderr;
pub use session::ClaudeCodeSession;
pub use session_store::{
    parse_conversation_id, ConversationId, SessionEntry, SessionStore, DEFAULT_OWNER_ID,
};
pub use spawn::{build_claude_command, format_allowed_tools, SpawnError};
pub use stream::{event_to_chat_chunks, parse_event, MessageDelta, StreamJsonEvent};
