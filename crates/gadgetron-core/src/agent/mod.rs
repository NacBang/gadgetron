//! Agent-Centric Control Plane types.
//!
//! Agent (Claude Code) permission model, brain model selection, Gadget
//! registry provider interface.
//!
//! Submodules:
//! - [`config`] — `AgentConfig`, `BrainConfig`, `GadgetsConfig`, + validation
//! - [`tools`] — `GadgetProvider` trait, `GadgetSchema`, `GadgetTier`,
//!   `GadgetResult`, `GadgetError`

pub mod config;
pub mod shared_context;
pub mod tools;

pub use config::{
    classify_agent_task, AgentBackend, AgentBrainSettings, AgentBrainSettingsSource, AgentConfig,
    AgentEffort, AgentTaskComplexity, BrainConfig, BrainMode, BrainShimConfig, CodexApprovalPolicy,
    CodexAuthMode, CodexConfig, CodexSandboxMode, ConversationAgentProfile,
    DestructiveGadgetsConfig, ExtraConfirmation, GadgetMode, GadgetsConfig, ModelSource,
    UpdateAgentBrainSettingsRequest, WriteGadgetsConfig, AUTO_MODEL_ID,
};
pub use tools::{
    DynamicGadgetSurface, GadgetDispatchContext, GadgetDispatcher, GadgetError,
    GadgetModeReconfigurer, GadgetProvider, GadgetResult, GadgetSchema, GadgetTier,
};
