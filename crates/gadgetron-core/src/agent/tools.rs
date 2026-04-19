//! Gadget registry interface — the Bundle-Penny seam.
//!
//! See `docs/design/phase2/04-gadget-registry.md` §2 + §13.
//! Decision: D-20260414-04, ADR-P2A-05, ADR-P2A-10.
//!
//! The `GadgetProvider` trait is the stable seam between:
//! - `gadgetron-knowledge::gadget::KnowledgeGadgetProvider` (P2A — wiki + web_search)
//! - `bundles/ai-infra::gadget::InfraGadgetProvider` (P2B+ — nodes, GPUs, providers)
//! - `bundles/cicd::gadget::CiCdGadgetProvider` (P3 — build, deploy)
//! - `bundles/server::gadget::ServerGadgetProvider` (P3 — SSH primitives)
//!
//! Adding a new Gadget namespace = new `impl GadgetProvider for XxxProvider {}`
//! plus a `GadgetRegistry::register(...)` call at startup. The trait itself
//! is not expected to change across phases.
//!
//! Terminology (per `docs/architecture/glossary.md`):
//! - **Gadget** — MCP tool consumed by Penny. Defined by a `GadgetSchema`.
//! - **GadgetProvider** — Rust supplier of Gadgets, owned by a Bundle.
//! - **Bundle** — distribution unit that provides Plugs and/or GadgetProviders.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Registry-facing dispatch seam for callers outside the Penny session.
///
/// Penny's `GadgetRegistry` lives in `gadgetron-penny`, but the gateway
/// (workbench direct-action path, `/web` invocations) depends only on
/// `gadgetron-core`. This trait lets the gateway hold
/// `Arc<dyn GadgetDispatcher>` and dispatch Gadgets without taking a
/// dependency on the penny crate.
///
/// Concrete implementors:
/// - `gadgetron_penny::GadgetRegistry` — real dispatch with the L3
///   allowed-names gate (ADR-P2A-06 §"Tier + Mode").
/// - Test fakes — return canned results, skip the L3 gate.
///
/// # Audit invariant
///
/// Direct-action dispatch is definitionally session-less (workbench doc
/// §2.2.4 + D-20260411-*) so it does not route through Penny's
/// `GadgetAuditEventSink`. The parallel sink originally tracked as
/// `TODO(audit-direct-action)` shipped in ISSUE 3 / v0.2.6 (PR #188) as
/// `gadgetron_core::audit::ActionAuditSink` — every terminal path in
/// `InProcessWorkbenchActionService::invoke` populates
/// `WorkbenchActionResult.audit_event_id` with a fresh UUID and emits
/// one `ActionAuditEvent::DirectActionCompleted` event to the wired
/// sink (Postgres-backed via `gadgetron_xaas::audit::ActionAuditEventWriter`
/// when a pool is configured, Noop otherwise).
#[async_trait]
pub trait GadgetDispatcher: Send + Sync + 'static {
    /// Dispatch a Gadget call by namespaced name (e.g. `"wiki.search"`).
    ///
    /// Implementors MUST preserve:
    /// - `GadgetError::UnknownGadget(name)` when the name is unregistered.
    /// - The L3 allowed-names gate when wrapping an operator-config-aware
    ///   registry — otherwise `Ask` / `Never`-mode tools would be reachable
    ///   via the workbench path after being disabled for Penny.
    async fn dispatch_gadget(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<GadgetResult, GadgetError>;
}

/// Read-only discovery seam for callers that need to enumerate Gadget
/// schemas without taking a dependency on `gadgetron-penny`.
///
/// The MCP `/v1/tools` endpoint (ISSUE 7) lives in the gateway and
/// exposes the operator-allowed gadget set to external agents. The
/// gateway depends only on `gadgetron-core`, so this trait lets it hold
/// `Arc<dyn GadgetCatalog>` for schema discovery, paralleling the
/// existing `GadgetDispatcher` seam used for Gadget invocation.
///
/// Concrete implementors:
/// - `gadgetron_penny::GadgetRegistry` — returns the frozen L3-allowed
///   schema set built from operator config.
/// - Test fakes — return a fixed schema slice.
///
/// The returned schemas are already deduplicated by tool name inside
/// `GadgetRegistry::freeze`; callers can trust `name` as a unique key.
pub trait GadgetCatalog: Send + Sync + 'static {
    /// All Gadget schemas exposed by this catalog.
    fn all_schemas(&self) -> Vec<GadgetSchema>;
}

/// Stable Bundle-facing interface for Gadget suppliers.
///
/// Each provider bundles a set of related Gadgets under a namespace (category).
/// Providers are registered statically at startup via `GadgetRegistry::register`.
/// **The agent cannot register, deregister, or mutate providers** — this is a
/// hard scope boundary enforced by `GadgetRegistry` (ADR-P2A-05 §14).
#[async_trait]
pub trait GadgetProvider: Send + Sync + 'static {
    /// Namespace for this provider's Gadgets. Gadget names are `"{category}.{gadget}"`.
    ///
    /// Reserved categories:
    /// - `"knowledge"` — wiki, web search, (P2B) vectors
    /// - `"inference"` — (P2B) list models, call provider
    /// - `"infrastructure"` — (P2C) nodes, GPUs, providers, routing
    /// - `"scheduler"` — (P3) slurm, k8s jobs
    /// - `"cluster"` — (P3) kubectl, helm
    /// - `"custom"` — (P4+) user-defined extensions
    ///
    /// `"agent"` is PERMANENTLY RESERVED and cannot be used by any provider
    /// (ADR-P2A-05 §14 — scope boundary).
    fn category(&self) -> &'static str;

    /// Enumerate the Gadget schemas this provider exposes. Called once at startup.
    fn gadget_schemas(&self) -> Vec<GadgetSchema>;

    /// Dispatch a Gadget call.
    ///
    /// `name` is the full namespaced name (e.g. `"wiki.read"`, not `"read"`).
    /// The registry routes by full Gadget name via a HashMap lookup — it does
    /// NOT assume `name.starts_with(self.category())`. Gadget names and
    /// categories are independent identifiers; a `"knowledge"` category may
    /// host Gadgets named `"wiki.read"`, `"web.search"`, etc.
    async fn call(&self, name: &str, args: serde_json::Value) -> Result<GadgetResult, GadgetError>;

    /// Optional runtime availability check. A provider gated on a Cargo
    /// feature or runtime config returns `false` to be excluded from the
    /// registry at startup. Defaults to `true`.
    fn is_available(&self) -> bool {
        true
    }
}

/// Schema for a single Gadget exposed by a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GadgetSchema {
    /// Namespaced: `{category}.{gadget_name}`. Must match the `--allowed-tools`
    /// format used by Claude Code.
    pub name: String,
    /// Tier — determines the default permission mode.
    pub tier: GadgetTier,
    /// Human-readable description. Shown to the agent in the Gadget manifest
    /// AND on the approval card, so it must be end-user-friendly.
    pub description: String,
    /// JSON Schema (draft-07) for the `args` object.
    pub input_schema: serde_json::Value,
    /// Idempotency hint. `None` = no claim; `Some(true)` = safe to retry;
    /// `Some(false)` = MUST NOT be retried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent: Option<bool>,
}

/// Gadget risk tier. Declared by the Gadget author on `GadgetSchema`, consumed by
/// the permission model (`GadgetMode`) and the approval card renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GadgetTier {
    /// Observes state; no mutation.
    Read,
    /// Mutates state; reversible.
    Write,
    /// Mutates state; NOT reversible without significant operator effort.
    /// Approval mode is hardcoded `ask` — cannot be set to `auto` (cardinal rule).
    Destructive,
}

/// Gadget call outcome returned to Claude Code as a `tool_result` block
/// (MCP wire-level term; internally it is a Gadget result).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GadgetResult {
    /// Content returned to the agent. Rendered in the tool_result block.
    pub content: serde_json::Value,
    /// If true, Claude Code treats this as a Gadget failure.
    #[serde(default)]
    pub is_error: bool,
}

/// Structured Gadget execution errors. Mapped to `GadgetResult { is_error: true }`
/// at the MCP dispatch boundary; the agent sees a friendly message + the
/// `error_code` for machine parsing.
#[derive(Debug, thiserror::Error)]
pub enum GadgetError {
    #[error("gadget not found: {0}")]
    UnknownGadget(String),
    #[error("denied by policy: {reason}")]
    Denied { reason: String },
    #[error("rate limit exceeded for {gadget}: {remaining}/{limit} this hour")]
    RateLimited {
        gadget: String,
        remaining: u32,
        limit: u32,
    },
    #[error("approval timed out after {secs}s")]
    ApprovalTimeout { secs: u64 },
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("gadget execution failed: {0}")]
    Execution(String),
}

impl GadgetError {
    /// Stable machine-readable error code for agent tool_result content.
    ///
    /// **Wire-frozen strings (do NOT rename).** These codes are persisted
    /// in `audit_log.error_code` (migration `20260416000001_tool_audit_events.sql`)
    /// and consumed by downstream SIEM / BI queries. They name **MCP
    /// protocol-level states** (unknown tool, denied, rate-limited, invalid
    /// args, execution failed, approval timeout) and are therefore stable
    /// across the Driver→Plug→Gadget naming evolution. Per ADR-P2A-10
    /// §"Unchanged" (security-compliance-lead review, 2026-04-18), the
    /// Rust type name `GadgetError` may change but the string table below
    /// stays bit-identical to the v0.2 `McpError` codes.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::UnknownGadget(_) => "mcp_unknown_tool",
            Self::Denied { .. } => "mcp_denied_by_policy",
            Self::RateLimited { .. } => "mcp_rate_limited",
            Self::ApprovalTimeout { .. } => "mcp_approval_timeout",
            Self::InvalidArgs(_) => "mcp_invalid_args",
            Self::Execution(_) => "mcp_execution_failed",
        }
    }
}

// ---------------------------------------------------------------------------
// Reserved namespace enforcement (ADR-P2A-05 §14)
// ---------------------------------------------------------------------------

/// Tool names that are PERMANENTLY FORBIDDEN in any provider's schema.
/// The agent cannot modify its own environment — meta-operations are
/// operator-only (config file + restart).
pub const RESERVED_TOOL_NAMES: &[&str] = &[
    "agent.set_brain",
    "agent.list_brains",
    "agent.switch_model",
    "agent.read_config",
    "agent.write_config",
    "agent.grant_self_permission",
    "agent.register_tool",
    "agent.deregister_tool",
    // Defense in depth — unnamespaced variants too
    "set_brain",
    "list_brains",
    "switch_model",
    "read_config",
    "write_config",
];

/// The entire `agent.*` namespace is reserved and empty. Any provider
/// claiming `category() == "agent"` is rejected at registration time.
pub const RESERVED_CATEGORY: &str = "agent";

/// Validate that a Gadget schema does not violate the reserved-namespace
/// cardinal rule. Called by `GadgetRegistry::register` on every Gadget.
///
/// Three defense layers (ADR-P2A-05 §14, SEC-MCP-B3):
/// 1. Category cannot be `"agent"` — the entire category is reserved.
/// 2. Gadget name cannot start with `"agent."` — defense in depth against a
///    provider declaring a non-agent category but smuggling in an
///    `agent.set_brain`-style Gadget.
/// 3. Specific well-known meta-operation names are banned regardless of
///    namespace prefix (covers unnamespaced legacy names).
pub fn ensure_tool_name_allowed(name: &str, category: &str) -> Result<(), GadgetError> {
    if category == RESERVED_CATEGORY {
        return Err(GadgetError::Denied {
            reason: format!(
                "category 'agent' is permanently reserved and cannot host Gadgets \
                 (ADR-P2A-05 §14); provider {name:?} rejected"
            ),
        });
    }
    if name.starts_with("agent.") {
        return Err(GadgetError::Denied {
            reason: format!(
                "gadget {name:?} starts with the reserved 'agent.' prefix \
                 (ADR-P2A-05 §14) — the agent cannot modify its own environment"
            ),
        });
    }
    if RESERVED_TOOL_NAMES.contains(&name) {
        return Err(GadgetError::Denied {
            reason: format!(
                "gadget {name:?} is in the reserved meta-operation namespace \
                 (ADR-P2A-05 §14) — the agent cannot modify its own environment"
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_agent_namespace_is_rejected() {
        assert!(ensure_tool_name_allowed("set_brain", "agent").is_err());
        assert!(ensure_tool_name_allowed("anything", "agent").is_err());
    }

    #[test]
    fn reserved_tool_names_are_rejected_even_outside_agent_category() {
        assert!(ensure_tool_name_allowed("agent.set_brain", "custom").is_err());
        assert!(ensure_tool_name_allowed("set_brain", "custom").is_err());
        assert!(ensure_tool_name_allowed("read_config", "infrastructure").is_err());
    }

    #[test]
    fn any_agent_prefix_is_rejected_even_if_not_in_reserved_list() {
        // Defense in depth per SEC-MCP-B3 — `agent.*` prefix catches
        // future meta-operation names without requiring a list update.
        assert!(ensure_tool_name_allowed("agent.anything_else", "knowledge").is_err());
        assert!(ensure_tool_name_allowed("agent.foo", "custom").is_err());
        assert!(ensure_tool_name_allowed("agent.read_current_brain", "knowledge").is_err());
    }

    #[test]
    fn legitimate_tools_pass() {
        assert!(ensure_tool_name_allowed("wiki.read", "knowledge").is_ok());
        assert!(ensure_tool_name_allowed("infra.list_nodes", "infrastructure").is_ok());
        assert!(ensure_tool_name_allowed("scheduler.schedule_job", "scheduler").is_ok());
    }

    #[test]
    fn tier_round_trips_serde() {
        let t = GadgetTier::Destructive;
        let j = serde_json::to_string(&t).unwrap();
        assert_eq!(j, "\"destructive\"");
        let back: GadgetTier = serde_json::from_str(&j).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn gadget_error_codes_are_wire_frozen() {
        // Wire-frozen strings per ADR-P2A-10 security review (D-20260418-05):
        // these codes are persisted in audit_log and downstream consumers
        // (SIEM, BI) depend on them. Rust type rename McpError → GadgetError
        // did NOT change the string table. Any diff here is a BREAKING CHANGE
        // to audit consumers and requires a separate ADR.
        assert_eq!(
            GadgetError::UnknownGadget("x".into()).error_code(),
            "mcp_unknown_tool"
        );
        assert_eq!(
            GadgetError::Denied { reason: "x".into() }.error_code(),
            "mcp_denied_by_policy"
        );
        assert_eq!(
            GadgetError::RateLimited {
                gadget: "x".into(),
                remaining: 0,
                limit: 10,
            }
            .error_code(),
            "mcp_rate_limited"
        );
        assert_eq!(
            GadgetError::ApprovalTimeout { secs: 60 }.error_code(),
            "mcp_approval_timeout"
        );
        assert_eq!(
            GadgetError::InvalidArgs("bad".into()).error_code(),
            "mcp_invalid_args"
        );
        assert_eq!(
            GadgetError::Execution("boom".into()).error_code(),
            "mcp_execution_failed"
        );
    }
}
