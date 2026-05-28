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
    AgentBackend, AgentBrainSettings, AgentBrainSettingsSource, AgentConfig, AgentEffort,
    BrainConfig, BrainMode, BrainShimConfig, CodexApprovalPolicy, CodexAuthMode, CodexConfig,
    CodexSandboxMode, DestructiveGadgetsConfig, ExtraConfirmation, GadgetMode, GadgetsConfig,
    ModelSource, UpdateAgentBrainSettingsRequest, WriteGadgetsConfig,
};
pub use tools::{
    GadgetDispatcher, GadgetError, GadgetModeReconfigurer, GadgetProvider, GadgetResult,
    GadgetSchema, GadgetTier,
};
