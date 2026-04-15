use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use uuid::Uuid;

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatabaseErrorKind {
    RowNotFound,
    PoolTimeout,
    ConnectionFailed,
    QueryFailed,
    MigrationFailed,
    Constraint,
    Other,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeErrorKind {
    InvalidMigProfile,
    NvmlInitFailed,
    ProcessSpawnFailed,
    VramAllocationFailed,
    PortAllocationFailed,
    ProcessKillFailed,
}

impl fmt::Display for NodeErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMigProfile => write!(f, "invalid_mig_profile"),
            Self::NvmlInitFailed => write!(f, "nvml_init_failed"),
            Self::ProcessSpawnFailed => write!(f, "process_spawn_failed"),
            Self::VramAllocationFailed => write!(f, "vram_allocation_failed"),
            Self::PortAllocationFailed => write!(f, "port_allocation_failed"),
            Self::ProcessKillFailed => write!(f, "process_kill_failed"),
        }
    }
}

/// Kairos agent subsystem error kinds (Phase 2).
///
/// Nested variant pattern matching `DatabaseErrorKind` and `NodeErrorKind`
/// — avoids a flat explosion of `GadgetronError` variants.
///
/// Corresponds to `GadgetronError::Kairos { kind, message }`. HTTP dispatch
/// is handled centrally; see `GadgetronError::http_status_code`.
///
/// # Subprocess kinds (P2A, 02-kairos-agent.md v4)
/// - `NotInstalled`, `SpawnFailed`, `AgentError`, `Timeout`
///
/// # MCP tool dispatch kinds (P2A, 04-mcp-tool-registry.md v2 §10.1)
/// - `ToolUnknown`, `ToolDenied`, `ToolRateLimited`, `ToolApprovalTimeout`,
///   `ToolInvalidArgs`, `ToolExecution`
/// - Populated by `impl From<McpError> for GadgetronError` at the dispatch
///   boundary. `ToolApprovalTimeout` is reserved for P2B (no approval flow
///   in P2A per ADR-P2A-06) but the variant ships in P2A for forward
///   compatibility of the enum surface.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KairosErrorKind {
    /// Claude Code binary not found on PATH (`which` failed). HTTP 503.
    NotInstalled,
    /// Claude Code subprocess spawn failed for reasons other than binary
    /// absence (permissions, resource limits, etc.). HTTP 503.
    SpawnFailed { reason: String },
    /// Claude Code subprocess exited with non-zero status mid-stream.
    /// `stderr_redacted` is ALREADY redacted via `gadgetron_kairos::redact_stderr`
    /// per 00-overview §8 M2. HTTP 500.
    AgentError {
        exit_code: i32,
        stderr_redacted: String,
    },
    /// Subprocess wallclock exceeded `kairos.request_timeout_secs`. HTTP 504.
    Timeout { seconds: u64 },

    // ---- MCP tool dispatch kinds (04 v2 §10.1) ----
    /// Agent called a tool name that is not registered with the MCP tool
    /// registry. Indicates version mismatch between Claude Code's cached
    /// tool manifest and the live registry. HTTP 500.
    ToolUnknown { name: String },
    /// Tool call denied by policy (never-mode subcategory, feature gate
    /// disabled, reserved namespace violation). The `reason` is a fixed
    /// operator-facing string drawn from the MCP server; does NOT contain
    /// subprocess stderr content. HTTP 403.
    ToolDenied { reason: String },
    /// Tool rate-limit exceeded. Only used by Destructive-tier tools in P2B+.
    /// P2A never emits this variant (T3 `enabled = false` is forced by V5).
    /// HTTP 429.
    ToolRateLimited {
        tool: String,
        remaining: u32,
        limit: u32,
    },
    /// Approval flow timed out waiting for user decision. Reserved for P2B
    /// (no approval flow in P2A). HTTP 504.
    ToolApprovalTimeout { secs: u64 },
    /// Arguments passed to the tool did not match its input schema. HTTP 400.
    ToolInvalidArgs { reason: String },
    /// Tool execution failed at the provider level (wiki write I/O failure,
    /// SearXNG HTTP error, etc.). HTTP 500.
    ToolExecution { reason: String },

    // ---- Native session kinds (ADR-P2A-06 addendum item 7 / §5.2.9) ----
    /// Client requested `conversation_id` but the `SessionStore` has no
    /// entry under `session_mode = NativeOnly`. Client must start a new
    /// conversation with a fresh id. HTTP 404, `kairos_session_not_found`.
    SessionNotFound { conversation_id: String },
    /// Two concurrent requests for the same `conversation_id`; the second
    /// request timed out waiting for the per-session mutex. Client should
    /// retry after the first request completes. HTTP 429,
    /// `kairos_session_concurrent`.
    SessionConcurrent { conversation_id: String },
    /// Claude Code reported that the session UUID is unrecognized (for
    /// example after a manual jsonl delete), or the jsonl file is
    /// corrupted. The store entry is removed; the next request with the
    /// same `conversation_id` falls through the first-turn branch and
    /// creates a fresh session. HTTP 500, `kairos_session_corrupted`.
    SessionCorrupted {
        conversation_id: String,
        reason: String,
    },
}

impl fmt::Display for KairosErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => write!(f, "not_installed"),
            Self::SpawnFailed { .. } => write!(f, "spawn_failed"),
            Self::AgentError { .. } => write!(f, "agent_error"),
            Self::Timeout { .. } => write!(f, "timeout"),
            Self::ToolUnknown { .. } => write!(f, "tool_unknown"),
            Self::ToolDenied { .. } => write!(f, "tool_denied"),
            Self::ToolRateLimited { .. } => write!(f, "tool_rate_limited"),
            Self::ToolApprovalTimeout { .. } => write!(f, "tool_approval_timeout"),
            Self::ToolInvalidArgs { .. } => write!(f, "tool_invalid_args"),
            Self::ToolExecution { .. } => write!(f, "tool_execution"),
            Self::SessionNotFound { .. } => write!(f, "session_not_found"),
            Self::SessionConcurrent { .. } => write!(f, "session_concurrent"),
            Self::SessionCorrupted { .. } => write!(f, "session_corrupted"),
        }
    }
}

/// Wiki knowledge layer error kinds (Phase 2).
///
/// Paired with `GadgetronError::Wiki { kind, message }`. See
/// `docs/design/phase2/01-knowledge-layer.md` §8 for the canonical
/// specification of each kind and its HTTP mapping.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WikiErrorKind {
    /// Path traversal attempt rejected by `wiki::fs::resolve_path`. HTTP 400.
    PathEscape { input: String },
    /// Content exceeds `wiki_max_page_bytes`. HTTP 413.
    /// All 3 fields are load-bearing for the user-visible error message.
    PageTooLarge {
        path: String,
        bytes: usize,
        limit: usize,
    },
    /// Content matched a BLOCK credential pattern (PEM / AKIA / GCP). HTTP 422.
    /// See 01-knowledge-layer.md §4.8 for the pattern list.
    CredentialBlocked { path: String, pattern: String },
    /// Git repo in inconsistent state (locked index / detached HEAD /
    /// missing objects / etc.). HTTP 503.
    GitCorruption { path: String, reason: String },
    /// Merge conflict during auto-commit. HTTP 409.
    Conflict { path: String },
}

impl fmt::Display for WikiErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PathEscape { .. } => write!(f, "path_escape"),
            Self::PageTooLarge { .. } => write!(f, "page_too_large"),
            Self::CredentialBlocked { .. } => write!(f, "credential_blocked"),
            Self::GitCorruption { .. } => write!(f, "git_corruption"),
            Self::Conflict { .. } => write!(f, "conflict"),
        }
    }
}

#[non_exhaustive]
#[derive(Error, Debug)]
pub enum GadgetronError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Routing error: {0}")]
    Routing(String),

    #[error("Stream interrupted: {reason}")]
    StreamInterrupted { reason: String },

    #[error("Quota exceeded for tenant {tenant_id}")]
    QuotaExceeded { tenant_id: Uuid },

    #[error("Tenant not found")]
    TenantNotFound,

    #[error("Forbidden: insufficient scope")]
    Forbidden,

    #[error("Billing error: {0}")]
    Billing(String),

    #[error("Download failed: {0}")]
    DownloadFailed(String),

    #[error("Hot-swap failed: {0}")]
    HotSwapFailed(String),

    #[error("Database error ({kind:?}): {message}")]
    Database {
        kind: DatabaseErrorKind,
        message: String,
    },

    #[error("Node error ({kind}): {message}")]
    Node {
        kind: NodeErrorKind,
        message: String,
    },

    /// Kairos agent subsystem error (Phase 2). Subprocess spawn/run failures,
    /// timeouts. Never contains raw subprocess stderr — only post-redaction
    /// content per M2. Added by `docs/design/phase2/02-kairos-agent.md` §9.
    #[error("Kairos error ({kind}): {message}")]
    Kairos {
        kind: KairosErrorKind,
        message: String,
    },

    /// Wiki knowledge layer error (Phase 2). Path traversal, oversize pages,
    /// blocked credentials, git corruption. Added by
    /// `docs/design/phase2/01-knowledge-layer.md` §8.
    #[error("Wiki error ({kind}): {message}")]
    Wiki {
        kind: WikiErrorKind,
        message: String,
    },
}

impl GadgetronError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Config(_) => "config_error",
            Self::Provider(_) => "provider_error",
            Self::Routing(_) => "routing_failure",
            Self::StreamInterrupted { .. } => "stream_interrupted",
            Self::QuotaExceeded { .. } => "quota_exceeded",
            Self::TenantNotFound => "invalid_api_key",
            Self::Forbidden => "forbidden",
            Self::Billing(_) => "billing_error",
            Self::DownloadFailed(_) => "download_failed",
            Self::HotSwapFailed(_) => "hotswap_failed",
            Self::Database { kind, .. } => match kind {
                DatabaseErrorKind::PoolTimeout => "db_pool_timeout",
                DatabaseErrorKind::RowNotFound => "db_row_not_found",
                DatabaseErrorKind::ConnectionFailed => "db_connection_failed",
                DatabaseErrorKind::MigrationFailed => "db_migration_failed",
                DatabaseErrorKind::Constraint => "db_constraint",
                DatabaseErrorKind::QueryFailed => "db_query_failed",
                DatabaseErrorKind::Other => "db_error",
            },
            Self::Node { kind, .. } => match kind {
                NodeErrorKind::InvalidMigProfile => "node_invalid_mig_profile",
                _ => "node_error",
            },
            Self::Kairos { kind, .. } => match kind {
                KairosErrorKind::NotInstalled => "kairos_not_installed",
                KairosErrorKind::SpawnFailed { .. } => "kairos_spawn_failed",
                KairosErrorKind::AgentError { .. } => "kairos_agent_error",
                KairosErrorKind::Timeout { .. } => "kairos_timeout",
                KairosErrorKind::ToolUnknown { .. } => "kairos_tool_unknown",
                KairosErrorKind::ToolDenied { .. } => "kairos_tool_denied",
                KairosErrorKind::ToolRateLimited { .. } => "kairos_tool_rate_limited",
                KairosErrorKind::ToolApprovalTimeout { .. } => "kairos_tool_approval_timeout",
                KairosErrorKind::ToolInvalidArgs { .. } => "kairos_tool_invalid_args",
                KairosErrorKind::ToolExecution { .. } => "kairos_tool_execution",
                KairosErrorKind::SessionNotFound { .. } => "kairos_session_not_found",
                KairosErrorKind::SessionConcurrent { .. } => "kairos_session_concurrent",
                KairosErrorKind::SessionCorrupted { .. } => "kairos_session_corrupted",
            },
            Self::Wiki { kind, .. } => match kind {
                WikiErrorKind::PathEscape { .. } => "wiki_invalid_path",
                WikiErrorKind::PageTooLarge { .. } => "wiki_page_too_large",
                WikiErrorKind::CredentialBlocked { .. } => "wiki_credential_blocked",
                WikiErrorKind::GitCorruption { .. } => "wiki_git_corrupted",
                WikiErrorKind::Conflict { .. } => "wiki_conflict",
            },
        }
    }

    /// Returns a user-visible error message.
    ///
    /// Return type changed from `&'static str` to `String` in Phase 2 to
    /// support runtime interpolation of values (Kairos timeout seconds,
    /// Wiki page size limits, etc.). Existing variants `.to_string()` their
    /// static literals — zero semantic change for callers.
    pub fn error_message(&self) -> String {
        match self {
            Self::Config(_) => "Configuration is invalid. Check your gadgetron.toml and environment variables.".to_string(),
            Self::Provider(_) => "The upstream LLM provider returned an error. Check provider status and API key validity.".to_string(),
            Self::Routing(_) => "No suitable provider found for this request. Verify model availability and routing configuration. Run GET /v1/models to check available models.".to_string(),
            Self::StreamInterrupted { .. } => "The response stream was interrupted. This may indicate a provider timeout or network issue.".to_string(),
            Self::QuotaExceeded { .. } => "Your API usage quota has been exceeded. Update quota_configs table to increase limits, or see docs/manual/troubleshooting.md.".to_string(),
            Self::TenantNotFound => "Invalid API key. Verify your API key is correct and has not been revoked.".to_string(),
            Self::Forbidden => "Your API key does not have permission for this operation. Check your key's assigned scopes.".to_string(),
            Self::Billing(_) => "A billing calculation error occurred. Check server logs for billing details. File an issue at github.com/NacBang/gadgetron if this persists.".to_string(),
            Self::DownloadFailed(_) => "Model download failed. Check network connectivity and model repository access.".to_string(),
            Self::HotSwapFailed(_) => "Model hot-swap failed. The previous model version remains active.".to_string(),
            Self::Database { .. } => "A database error occurred. Check PostgreSQL connectivity and disk space.".to_string(),
            Self::Node { .. } => "A node-level error occurred. Check GPU availability and NVML driver status.".to_string(),
            // Kairos variants — NEVER includes the `stderr_redacted` field content.
            // Enforced by test `kairos_agent_error_message_does_not_contain_stderr`.
            //
            // Tool-dispatch variants (04 v2 §10.1) are safe to interpolate
            // because their content comes from the MCP server, not subprocess
            // stderr. Provider authors are instructed to keep reason strings
            // operator-readable and non-sensitive.
            Self::Kairos { kind, .. } => match kind {
                KairosErrorKind::NotInstalled =>
                    "The Kairos assistant is not available. The Claude Code CLI (`claude`) was not found on the server. Contact your administrator to install Claude Code and run `claude login`.".to_string(),
                KairosErrorKind::SpawnFailed { .. } =>
                    "The Kairos assistant is not available. The server could not start the Claude Code process. Run `gadgetron serve` with `RUST_LOG=gadgetron_kairos=debug` for spawn diagnostics, or check `journalctl -u gadgetron` for spawn errors.".to_string(),
                KairosErrorKind::AgentError { .. } =>
                    "The Kairos assistant encountered an error and stopped. The assistant process exited unexpectedly. Try again; if the problem persists, contact your administrator.".to_string(),
                KairosErrorKind::Timeout { seconds } =>
                    format!("The Kairos assistant did not respond in time (limit: {seconds}s). Your request may have been too complex. Try a shorter or simpler request."),
                KairosErrorKind::ToolUnknown { name } =>
                    format!("The agent requested tool {name:?}, which is not registered on this server. This usually means a version mismatch between the agent's cached tool manifest and the live MCP registry. Restart `gadgetron serve` to refresh the manifest."),
                KairosErrorKind::ToolDenied { reason } =>
                    format!("A tool call was denied by policy: {reason}. Check your `[agent.tools.*]` configuration in `gadgetron.toml`."),
                KairosErrorKind::ToolRateLimited { tool, remaining, limit } =>
                    format!("Tool {tool:?} is rate-limited ({remaining}/{limit} calls remaining this hour). Wait and retry, or increase `[agent.tools.destructive].max_per_hour` in `gadgetron.toml`."),
                KairosErrorKind::ToolApprovalTimeout { secs } =>
                    format!("A tool call required user approval but none arrived within {secs} seconds. (Approval flow is not functional in Phase 2A — this error indicates a misconfiguration or a forward-compat P2B path.)"),
                KairosErrorKind::ToolInvalidArgs { reason } =>
                    format!("The agent passed invalid arguments to a tool: {reason}. This is an agent-side bug; try rephrasing your request."),
                KairosErrorKind::ToolExecution { reason } =>
                    format!("A tool failed to execute: {reason}. Check server logs for details."),
                KairosErrorKind::SessionNotFound { conversation_id } =>
                    format!("Conversation {conversation_id:?} is not known to this server. The conversation may have expired or been evicted from the session store. Start a new conversation without a conversation_id, or with a fresh id."),
                KairosErrorKind::SessionConcurrent { conversation_id } =>
                    format!("Conversation {conversation_id:?} is already serving another request. Wait for the current turn to finish, then retry."),
                KairosErrorKind::SessionCorrupted { conversation_id, .. } =>
                    format!("Conversation {conversation_id:?} session state is unreadable. The session has been discarded; retry with the same conversation_id to start a fresh session."),
            },
            // Wiki variants — always safe to surface to clients
            // (path/bytes/limit are user-provided values, not secrets).
            Self::Wiki { kind, .. } => match kind {
                WikiErrorKind::PathEscape { .. } =>
                    "The requested wiki page path is invalid. Page paths must not contain `..`, absolute paths, or special characters.".to_string(),
                WikiErrorKind::PageTooLarge { bytes, limit, .. } =>
                    format!("Page too large: {bytes} bytes exceeds the {limit}-byte limit. Split the content into multiple smaller pages."),
                WikiErrorKind::CredentialBlocked { pattern, .. } =>
                    format!("Credential detected in content (pattern: {pattern}). Wiki writes must not contain unambiguous secrets. Remove the credential and retry."),
                WikiErrorKind::GitCorruption { .. } =>
                    "The wiki git repository is in an inconsistent state. Run `git status` in the wiki directory and resolve manually.".to_string(),
                WikiErrorKind::Conflict { path } =>
                    format!("A wiki page could not be saved because it was modified by another process (path: {path}). Resolve the git conflict in the wiki directory, then retry."),
            },
        }
    }

    pub fn error_type(&self) -> &'static str {
        match self {
            Self::Config(_) => "invalid_request_error",
            Self::Provider(_) => "api_error",
            Self::Routing(_) => "server_error",
            Self::StreamInterrupted { .. } => "api_error",
            Self::QuotaExceeded { .. } => "quota_error",
            Self::TenantNotFound => "authentication_error",
            Self::Forbidden => "permission_error",
            Self::Billing(_) => "api_error",
            Self::DownloadFailed(_) => "api_error",
            Self::HotSwapFailed(_) => "api_error",
            Self::Database { .. } => "server_error",
            Self::Node { .. } => "server_error",
            Self::Kairos { kind, .. } => match kind {
                // Tool-dispatch variants get OpenAI-taxonomy-aligned types so
                // SDK clients can `match` on the shape (invalid_request_error
                // vs permission_error vs quota_error).
                KairosErrorKind::ToolDenied { .. } => "permission_error",
                KairosErrorKind::ToolInvalidArgs { .. } => "invalid_request_error",
                KairosErrorKind::ToolRateLimited { .. } => "quota_error",
                KairosErrorKind::ToolApprovalTimeout { .. } => "server_error",
                _ => "server_error",
            },
            Self::Wiki { kind, .. } => match kind {
                WikiErrorKind::PathEscape { .. } => "invalid_request_error",
                WikiErrorKind::PageTooLarge { .. } => "invalid_request_error",
                WikiErrorKind::CredentialBlocked { .. } => "invalid_request_error",
                _ => "server_error",
            },
        }
    }

    pub fn http_status_code(&self) -> u16 {
        match self {
            Self::Config(_) => 400,
            Self::Provider(_) => 502,
            Self::Routing(_) => 503,
            Self::StreamInterrupted { .. } => 502,
            Self::QuotaExceeded { .. } => 429,
            Self::TenantNotFound => 401,
            Self::Forbidden => 403,
            Self::Billing(_) => 500,
            Self::DownloadFailed(_) => 500,
            Self::HotSwapFailed(_) => 500,
            Self::Database { kind, .. } => match kind {
                DatabaseErrorKind::RowNotFound => 404,
                DatabaseErrorKind::PoolTimeout | DatabaseErrorKind::ConnectionFailed => 503,
                DatabaseErrorKind::Constraint => 409,
                _ => 500,
            },
            Self::Node { kind, .. } => match kind {
                NodeErrorKind::InvalidMigProfile => 400,
                _ => 500,
            },
            Self::Kairos { kind, .. } => match kind {
                KairosErrorKind::NotInstalled | KairosErrorKind::SpawnFailed { .. } => 503,
                KairosErrorKind::AgentError { .. } => 500,
                KairosErrorKind::Timeout { .. } => 504,
                KairosErrorKind::ToolUnknown { .. } => 500,
                KairosErrorKind::ToolDenied { .. } => 403,
                KairosErrorKind::ToolRateLimited { .. } => 429,
                KairosErrorKind::ToolApprovalTimeout { .. } => 504,
                KairosErrorKind::ToolInvalidArgs { .. } => 400,
                KairosErrorKind::ToolExecution { .. } => 500,
                KairosErrorKind::SessionNotFound { .. } => 404,
                KairosErrorKind::SessionConcurrent { .. } => 429,
                KairosErrorKind::SessionCorrupted { .. } => 500,
            },
            Self::Wiki { kind, .. } => match kind {
                WikiErrorKind::PathEscape { .. } => 400,
                WikiErrorKind::PageTooLarge { .. } => 413,
                WikiErrorKind::CredentialBlocked { .. } => 422,
                WikiErrorKind::GitCorruption { .. } => 503,
                WikiErrorKind::Conflict { .. } => 409,
            },
        }
    }
}

pub type Result<T> = std::result::Result<T, GadgetronError>;

// ---------------------------------------------------------------------------
// McpError → GadgetronError conversion (04-mcp-tool-registry.md v2 §10.1)
// ---------------------------------------------------------------------------

/// Maps an `McpError` from the MCP dispatch boundary into a
/// `GadgetronError::Kairos` variant so the gateway can render HTTP + SSE
/// responses through the single user-facing error path per D-13.
///
/// Called at the `KairosProvider::chat_stream` seam when a tool call
/// returns `Err`. The `message` field is a generic one-line summary; the
/// `kind` holds the structured payload that `error_message()`,
/// `http_status_code()`, and `error_code()` consume.
impl From<crate::agent::tools::McpError> for GadgetronError {
    fn from(err: crate::agent::tools::McpError) -> Self {
        use crate::agent::tools::McpError;
        let kind = match err {
            McpError::UnknownTool(name) => KairosErrorKind::ToolUnknown { name },
            McpError::Denied { reason } => KairosErrorKind::ToolDenied { reason },
            McpError::RateLimited {
                tool,
                remaining,
                limit,
            } => KairosErrorKind::ToolRateLimited {
                tool,
                remaining,
                limit,
            },
            McpError::ApprovalTimeout { secs } => KairosErrorKind::ToolApprovalTimeout { secs },
            McpError::InvalidArgs(reason) => KairosErrorKind::ToolInvalidArgs { reason },
            McpError::Execution(reason) => KairosErrorKind::ToolExecution { reason },
        };
        // Generic summary — the kind carries the structured detail.
        let message = format!("tool dispatch error: {kind}");
        GadgetronError::Kairos { kind, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_returns_stable_machine_string() {
        assert_eq!(
            GadgetronError::Config("bad".into()).error_code(),
            "config_error"
        );
        assert_eq!(
            GadgetronError::Provider("down".into()).error_code(),
            "provider_error"
        );
        assert_eq!(
            GadgetronError::Routing("none".into()).error_code(),
            "routing_failure"
        );
        assert_eq!(
            GadgetronError::StreamInterrupted {
                reason: "timeout".into()
            }
            .error_code(),
            "stream_interrupted"
        );
        assert_eq!(
            GadgetronError::QuotaExceeded {
                tenant_id: Uuid::nil()
            }
            .error_code(),
            "quota_exceeded"
        );
        assert_eq!(
            GadgetronError::TenantNotFound.error_code(),
            "invalid_api_key",
            "OpenAI SDK clients match on 'invalid_api_key' as the canonical 401 code",
        );
        assert_eq!(GadgetronError::Forbidden.error_code(), "forbidden");
        assert_eq!(
            GadgetronError::Billing("err".into()).error_code(),
            "billing_error"
        );
        assert_eq!(
            GadgetronError::DownloadFailed("err".into()).error_code(),
            "download_failed"
        );
        assert_eq!(
            GadgetronError::HotSwapFailed("err".into()).error_code(),
            "hotswap_failed"
        );
        assert_eq!(
            GadgetronError::Database {
                kind: DatabaseErrorKind::PoolTimeout,
                message: "".into()
            }
            .error_code(),
            "db_pool_timeout"
        );
        assert_eq!(
            GadgetronError::Node {
                kind: NodeErrorKind::InvalidMigProfile,
                message: "".into()
            }
            .error_code(),
            "node_invalid_mig_profile"
        );
        // Phase 2: Kairos kinds
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::NotInstalled,
                message: "".into(),
            }
            .error_code(),
            "kairos_not_installed"
        );
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::SpawnFailed { reason: "x".into() },
                message: "".into(),
            }
            .error_code(),
            "kairos_spawn_failed"
        );
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::AgentError {
                    exit_code: 42,
                    stderr_redacted: "[REDACTED:anthropic_key]".into()
                },
                message: "".into(),
            }
            .error_code(),
            "kairos_agent_error"
        );
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::Timeout { seconds: 300 },
                message: "".into(),
            }
            .error_code(),
            "kairos_timeout"
        );
        // Phase 2: Wiki kinds
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::PathEscape {
                    input: "../etc/passwd".into()
                },
                message: "".into(),
            }
            .error_code(),
            "wiki_invalid_path"
        );
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::PageTooLarge {
                    path: "notes".into(),
                    bytes: 2_000_000,
                    limit: 1_048_576
                },
                message: "".into(),
            }
            .error_code(),
            "wiki_page_too_large"
        );
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::CredentialBlocked {
                    path: "notes".into(),
                    pattern: "pem_private_key".into()
                },
                message: "".into(),
            }
            .error_code(),
            "wiki_credential_blocked"
        );
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::GitCorruption {
                    path: "".into(),
                    reason: "locked index".into()
                },
                message: "".into(),
            }
            .error_code(),
            "wiki_git_corrupted"
        );
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::Conflict {
                    path: "notes".into()
                },
                message: "".into(),
            }
            .error_code(),
            "wiki_conflict"
        );
    }

    #[test]
    fn error_message_is_human_readable_not_same_as_code() {
        let err = GadgetronError::QuotaExceeded {
            tenant_id: Uuid::nil(),
        };
        let msg = err.error_message();
        let code = err.error_code();
        assert_ne!(msg, code);
        assert!(msg.contains("quota"));
        assert!(msg.len() > 20);
    }

    #[test]
    fn error_type_follows_openai_taxonomy() {
        assert_eq!(
            GadgetronError::TenantNotFound.error_type(),
            "authentication_error"
        );
        assert_eq!(GadgetronError::Forbidden.error_type(), "permission_error");
        assert_eq!(
            GadgetronError::QuotaExceeded {
                tenant_id: Uuid::nil()
            }
            .error_type(),
            "quota_error"
        );
        assert_eq!(
            GadgetronError::Database {
                kind: DatabaseErrorKind::Other,
                message: "".into()
            }
            .error_type(),
            "server_error"
        );
        assert_eq!(
            GadgetronError::Config("".into()).error_type(),
            "invalid_request_error"
        );
        // Routing returns 503, so its error_type must be server_error, not invalid_request_error.
        assert_eq!(
            GadgetronError::Routing("".into()).error_type(),
            "server_error"
        );
        // Phase 2: Kairos subprocess kinds = server_error
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::NotInstalled,
                message: "".into(),
            }
            .error_type(),
            "server_error"
        );
        // Phase 2: Kairos tool-dispatch kinds (04 v2 §10.1)
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::ToolDenied {
                    reason: "never".into()
                },
                message: "".into(),
            }
            .error_type(),
            "permission_error"
        );
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::ToolInvalidArgs {
                    reason: "missing path".into()
                },
                message: "".into(),
            }
            .error_type(),
            "invalid_request_error"
        );
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::ToolRateLimited {
                    tool: "x".into(),
                    remaining: 0,
                    limit: 3
                },
                message: "".into(),
            }
            .error_type(),
            "quota_error"
        );
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::ToolUnknown { name: "x".into() },
                message: "".into(),
            }
            .error_type(),
            "server_error"
        );
        // Phase 2: Wiki PathEscape / PageTooLarge / CredentialBlocked = invalid_request_error
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::PathEscape { input: "".into() },
                message: "".into(),
            }
            .error_type(),
            "invalid_request_error"
        );
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::PageTooLarge {
                    path: "".into(),
                    bytes: 0,
                    limit: 0
                },
                message: "".into(),
            }
            .error_type(),
            "invalid_request_error"
        );
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::CredentialBlocked {
                    path: "".into(),
                    pattern: "".into()
                },
                message: "".into(),
            }
            .error_type(),
            "invalid_request_error"
        );
        // Wiki GitCorruption / Conflict = server_error
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::GitCorruption {
                    path: "".into(),
                    reason: "".into()
                },
                message: "".into(),
            }
            .error_type(),
            "server_error"
        );
    }

    #[test]
    fn http_status_codes_match_spec() {
        assert_eq!(GadgetronError::Config("".into()).http_status_code(), 400);
        assert_eq!(GadgetronError::Provider("".into()).http_status_code(), 502);
        assert_eq!(GadgetronError::Routing("".into()).http_status_code(), 503);
        assert_eq!(
            GadgetronError::QuotaExceeded {
                tenant_id: Uuid::nil()
            }
            .http_status_code(),
            429
        );
        assert_eq!(GadgetronError::TenantNotFound.http_status_code(), 401);
        assert_eq!(GadgetronError::Forbidden.http_status_code(), 403);
        assert_eq!(
            GadgetronError::Database {
                kind: DatabaseErrorKind::PoolTimeout,
                message: "".into()
            }
            .http_status_code(),
            503
        );
        assert_eq!(
            GadgetronError::Database {
                kind: DatabaseErrorKind::RowNotFound,
                message: "".into()
            }
            .http_status_code(),
            404
        );
        assert_eq!(
            GadgetronError::Database {
                kind: DatabaseErrorKind::Constraint,
                message: "".into()
            }
            .http_status_code(),
            409
        );
        assert_eq!(
            GadgetronError::Node {
                kind: NodeErrorKind::InvalidMigProfile,
                message: "".into()
            }
            .http_status_code(),
            400
        );
        assert_eq!(
            GadgetronError::Node {
                kind: NodeErrorKind::NvmlInitFailed,
                message: "".into()
            }
            .http_status_code(),
            500
        );
        // Phase 2: Kairos HTTP codes
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::NotInstalled,
                message: "".into(),
            }
            .http_status_code(),
            503
        );
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::SpawnFailed { reason: "".into() },
                message: "".into(),
            }
            .http_status_code(),
            503
        );
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::AgentError {
                    exit_code: 1,
                    stderr_redacted: "".into()
                },
                message: "".into(),
            }
            .http_status_code(),
            500
        );
        assert_eq!(
            GadgetronError::Kairos {
                kind: KairosErrorKind::Timeout { seconds: 300 },
                message: "".into(),
            }
            .http_status_code(),
            504
        );
        // Phase 2: Wiki HTTP codes
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::PathEscape { input: "".into() },
                message: "".into(),
            }
            .http_status_code(),
            400
        );
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::PageTooLarge {
                    path: "".into(),
                    bytes: 0,
                    limit: 0
                },
                message: "".into(),
            }
            .http_status_code(),
            413
        );
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::CredentialBlocked {
                    path: "".into(),
                    pattern: "".into()
                },
                message: "".into(),
            }
            .http_status_code(),
            422
        );
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::GitCorruption {
                    path: "".into(),
                    reason: "".into()
                },
                message: "".into(),
            }
            .http_status_code(),
            503
        );
        assert_eq!(
            GadgetronError::Wiki {
                kind: WikiErrorKind::Conflict { path: "".into() },
                message: "".into(),
            }
            .http_status_code(),
            409
        );
    }

    #[test]
    fn database_error_kind_is_non_exhaustive() {
        let kind = DatabaseErrorKind::Other;
        assert_eq!(format!("{kind:?}"), "Other");
    }

    #[test]
    fn display_includes_context() {
        let err = GadgetronError::Database {
            kind: DatabaseErrorKind::PoolTimeout,
            message: "connection timed out".into(),
        };
        let display = format!("{err}");
        assert!(display.contains("PoolTimeout"));
        assert!(display.contains("connection timed out"));
    }

    #[test]
    fn node_error_kind_process_kill_failed_display() {
        let kind = NodeErrorKind::ProcessKillFailed;
        assert_eq!(format!("{kind}"), "process_kill_failed");
        // Confirm it round-trips through the GadgetronError wrapper.
        let err = GadgetronError::Node {
            kind: NodeErrorKind::ProcessKillFailed,
            message: "SIGKILL timed out".into(),
        };
        let display = format!("{err}");
        assert!(
            display.contains("process_kill_failed"),
            "display: {display}"
        );
        assert!(display.contains("SIGKILL timed out"), "display: {display}");
        assert_eq!(err.error_code(), "node_error");
        assert_eq!(err.http_status_code(), 500);
    }

    #[test]
    fn all_fourteen_variants_exist() {
        let variants: Vec<GadgetronError> = vec![
            GadgetronError::Config("".into()),
            GadgetronError::Provider("".into()),
            GadgetronError::Routing("".into()),
            GadgetronError::StreamInterrupted { reason: "".into() },
            GadgetronError::QuotaExceeded {
                tenant_id: Uuid::nil(),
            },
            GadgetronError::TenantNotFound,
            GadgetronError::Forbidden,
            GadgetronError::Billing("".into()),
            GadgetronError::DownloadFailed("".into()),
            GadgetronError::HotSwapFailed("".into()),
            GadgetronError::Database {
                kind: DatabaseErrorKind::Other,
                message: "".into(),
            },
            GadgetronError::Node {
                kind: NodeErrorKind::InvalidMigProfile,
                message: "".into(),
            },
            // Phase 2 additions
            GadgetronError::Kairos {
                kind: KairosErrorKind::NotInstalled,
                message: "".into(),
            },
            GadgetronError::Wiki {
                kind: WikiErrorKind::Conflict {
                    path: "notes".into(),
                },
                message: "".into(),
            },
        ];
        assert_eq!(variants.len(), 14);
    }

    /// M2 enforcement — 02-kairos-agent.md §9: the user-visible error message
    /// must NEVER contain the `stderr_redacted` field content, regardless of
    /// what that field holds. Generic string only.
    #[test]
    fn kairos_agent_error_message_does_not_contain_stderr() {
        let err = GadgetronError::Kairos {
            kind: KairosErrorKind::AgentError {
                exit_code: 42,
                stderr_redacted: "sensitive-token-leaked-here-abc123def456".into(),
            },
            message: "should not matter".into(),
        };
        let msg = err.error_message();
        assert!(
            !msg.contains("sensitive-token-leaked-here"),
            "user-visible message must NOT echo stderr_redacted content, got: {msg}"
        );
        assert!(
            !msg.contains("abc123def456"),
            "user-visible message must NOT echo stderr_redacted content, got: {msg}"
        );
        // And the message must still be meaningful
        assert!(msg.contains("Kairos") || msg.contains("assistant"));
    }

    /// Kairos Timeout message must interpolate the configured timeout seconds
    /// so the user knows what the limit actually was.
    #[test]
    fn kairos_timeout_message_interpolates_seconds() {
        let err = GadgetronError::Kairos {
            kind: KairosErrorKind::Timeout { seconds: 300 },
            message: "".into(),
        };
        assert!(err.error_message().contains("300"));
    }

    /// Wiki PageTooLarge message must include both `bytes` and `limit` so
    /// the caller knows exactly how much over they were — dx Round 1 A3.
    #[test]
    fn wiki_page_too_large_message_includes_bytes_and_limit() {
        let err = GadgetronError::Wiki {
            kind: WikiErrorKind::PageTooLarge {
                path: "huge".into(),
                bytes: 2_000_000,
                limit: 1_048_576,
            },
            message: "".into(),
        };
        let msg = err.error_message();
        assert!(msg.contains("2000000") || msg.contains("2_000_000"));
        assert!(msg.contains("1048576") || msg.contains("1_048_576"));
    }

    /// Wiki CredentialBlocked message must include the pattern name so the
    /// user knows WHY it was blocked.
    #[test]
    fn wiki_credential_blocked_message_includes_pattern() {
        let err = GadgetronError::Wiki {
            kind: WikiErrorKind::CredentialBlocked {
                path: "leaked".into(),
                pattern: "pem_private_key".into(),
            },
            message: "".into(),
        };
        let msg = err.error_message();
        assert!(msg.contains("pem_private_key"));
    }

    /// `KairosErrorKind::Display` produces the expected snake_case token.
    #[test]
    fn kairos_error_kind_display() {
        assert_eq!(
            format!("{}", KairosErrorKind::NotInstalled),
            "not_installed"
        );
        assert_eq!(
            format!(
                "{}",
                KairosErrorKind::SpawnFailed {
                    reason: "ignored".into()
                }
            ),
            "spawn_failed"
        );
        assert_eq!(
            format!(
                "{}",
                KairosErrorKind::AgentError {
                    exit_code: 1,
                    stderr_redacted: "ignored".into()
                }
            ),
            "agent_error"
        );
        assert_eq!(
            format!("{}", KairosErrorKind::Timeout { seconds: 300 }),
            "timeout"
        );
    }

    // ---- MCP tool dispatch conversions (04 v2 §10.1) ----

    #[test]
    fn from_mcp_unknown_tool_maps_to_tool_unknown_kind() {
        let err: GadgetronError =
            crate::agent::tools::McpError::UnknownTool("wiki.ghost".into()).into();
        assert_eq!(err.error_code(), "kairos_tool_unknown");
        assert_eq!(err.http_status_code(), 500);
        let msg = err.error_message();
        assert!(msg.contains("wiki.ghost"), "msg: {msg}");
    }

    #[test]
    fn from_mcp_denied_preserves_reason() {
        let err: GadgetronError = crate::agent::tools::McpError::Denied {
            reason: "tool disabled by policy (never)".into(),
        }
        .into();
        assert_eq!(err.error_code(), "kairos_tool_denied");
        assert_eq!(err.http_status_code(), 403);
        assert!(err.error_message().contains("disabled by policy"));
    }

    #[test]
    fn from_mcp_rate_limited_includes_tool_and_counts() {
        let err: GadgetronError = crate::agent::tools::McpError::RateLimited {
            tool: "infra.deploy_model".into(),
            remaining: 0,
            limit: 3,
        }
        .into();
        assert_eq!(err.error_code(), "kairos_tool_rate_limited");
        assert_eq!(err.http_status_code(), 429);
        let msg = err.error_message();
        assert!(msg.contains("infra.deploy_model"));
        assert!(msg.contains("3"));
    }

    #[test]
    fn from_mcp_approval_timeout_maps_to_504() {
        let err: GadgetronError =
            crate::agent::tools::McpError::ApprovalTimeout { secs: 60 }.into();
        assert_eq!(err.error_code(), "kairos_tool_approval_timeout");
        assert_eq!(err.http_status_code(), 504);
        assert!(err.error_message().contains("60"));
    }

    #[test]
    fn from_mcp_invalid_args_maps_to_400() {
        let err: GadgetronError =
            crate::agent::tools::McpError::InvalidArgs("missing field 'path'".into()).into();
        assert_eq!(err.error_code(), "kairos_tool_invalid_args");
        assert_eq!(err.http_status_code(), 400);
        assert!(err.error_message().contains("missing field"));
    }

    #[test]
    fn from_mcp_execution_maps_to_500() {
        let err: GadgetronError =
            crate::agent::tools::McpError::Execution("SearXNG returned 502".into()).into();
        assert_eq!(err.error_code(), "kairos_tool_execution");
        assert_eq!(err.http_status_code(), 500);
        assert!(err.error_message().contains("SearXNG"));
    }

    #[test]
    fn kairos_tool_variants_display_tokens() {
        assert_eq!(
            format!("{}", KairosErrorKind::ToolUnknown { name: "x".into() }),
            "tool_unknown"
        );
        assert_eq!(
            format!("{}", KairosErrorKind::ToolDenied { reason: "x".into() }),
            "tool_denied"
        );
        assert_eq!(
            format!(
                "{}",
                KairosErrorKind::ToolRateLimited {
                    tool: "x".into(),
                    remaining: 0,
                    limit: 3
                }
            ),
            "tool_rate_limited"
        );
        assert_eq!(
            format!("{}", KairosErrorKind::ToolApprovalTimeout { secs: 60 }),
            "tool_approval_timeout"
        );
        assert_eq!(
            format!(
                "{}",
                KairosErrorKind::ToolInvalidArgs { reason: "x".into() }
            ),
            "tool_invalid_args"
        );
        assert_eq!(
            format!("{}", KairosErrorKind::ToolExecution { reason: "x".into() }),
            "tool_execution"
        );
    }

    /// `WikiErrorKind::Display` produces the expected snake_case token.
    #[test]
    fn wiki_error_kind_display() {
        assert_eq!(
            format!(
                "{}",
                WikiErrorKind::PathEscape {
                    input: "ignored".into()
                }
            ),
            "path_escape"
        );
        assert_eq!(
            format!(
                "{}",
                WikiErrorKind::PageTooLarge {
                    path: "".into(),
                    bytes: 0,
                    limit: 0
                }
            ),
            "page_too_large"
        );
        assert_eq!(
            format!(
                "{}",
                WikiErrorKind::CredentialBlocked {
                    path: "".into(),
                    pattern: "".into()
                }
            ),
            "credential_blocked"
        );
        assert_eq!(
            format!(
                "{}",
                WikiErrorKind::GitCorruption {
                    path: "".into(),
                    reason: "".into()
                }
            ),
            "git_corruption"
        );
        assert_eq!(
            format!("{}", WikiErrorKind::Conflict { path: "".into() }),
            "conflict"
        );
    }
}
