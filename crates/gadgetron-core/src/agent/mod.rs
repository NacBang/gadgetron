//! Agent-Centric Control Plane types.
//!
//! Agent (Claude Code) permission model, brain model selection, MCP tool
//! registry plugin interface. See:
//!
//! - D-20260414-04 (decision log)
//! - ADR-P2A-05 (agent-centric control plane)
//! - docs/design/phase2/04-mcp-tool-registry.md
//!
//! Submodules:
//! - [`config`] — `AgentConfig`, `BrainConfig`, `ToolsConfig`, + validation
//! - [`tools`] — `McpToolProvider` trait, `ToolSchema`, `Tier`, `ToolResult`, `McpError`

pub mod config;
pub mod tools;

pub use config::{
    AgentConfig, BrainConfig, BrainMode, BrainShimConfig, DestructiveToolsConfig,
    ExtraConfirmation, ToolMode, ToolsConfig, WriteToolsConfig,
};
pub use tools::{McpError, McpToolProvider, Tier, ToolResult, ToolSchema};
