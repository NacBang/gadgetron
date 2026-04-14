//! gadgetron-kairos — agent adapter: Claude Code subprocess lifecycle,
//! MCP tool registry dispatch, OpenAI LlmProvider impl.
//!
//! # Phase 2A scope (Path 1 — approval flow deferred to P2B)
//!
//! Per ADR-P2A-06, the interactive approval flow (ApprovalRegistry,
//! `POST /v1/approvals/{id}`, SSE `gadgetron.approval_required` event,
//! `<ApprovalCard>` frontend) is **deferred to Phase 2B**. P2A ships with
//! `ToolMode::Auto` / `ToolMode::Never` only; operators express tool policy
//! statically via `[agent.tools.*]` config.
//!
//! # Modules (P2A)
//! - `registry` — `McpToolRegistry` dispatch table, builder/freeze pattern
//! - `provider` — `KairosProvider: LlmProvider` router registration
//! - `session` — `ClaudeCodeSession` subprocess lifecycle (chief-arch B3)
//! - `stream`   — stream-json → `ChatChunk` translator
//! - `spawn`    — Command builder with `kill_on_drop(true)`
//! - `mcp_config` — tempfile M1 (atomic 0600, unix-only)
//! - `redact`   — `redact_stderr` (M2)
//! - `config`   — no runtime config here; `AgentConfig` lives in `gadgetron-core`
//! - `error`    — Local `KairosError` mapped into `GadgetronError::Kairos`
//!
//! # Modules deferred to P2B
//! - `approval` — `ApprovalRegistry`, `PendingApproval`, cross-process bridge
//!
//! See `docs/design/phase2/04-mcp-tool-registry.md` v2 + `02-kairos-agent.md` v4.

pub mod mcp_config;
pub mod mcp_server;
pub mod provider;
pub mod redact;
pub mod registry;
pub mod session;
pub mod spawn;
pub mod stream;

pub use mcp_config::{build_config_json, write_config_file};
pub use mcp_server::serve_stdio;
pub use provider::{register_with_router, KairosProvider};
pub use redact::redact_stderr;
pub use registry::{McpToolRegistry, McpToolRegistryBuilder};
pub use session::ClaudeCodeSession;
pub use spawn::{build_claude_command, format_allowed_tools, SpawnError};
pub use stream::{event_to_chat_chunks, parse_event, MessageDelta, StreamJsonEvent};
