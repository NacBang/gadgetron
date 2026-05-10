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
    AgentBrainSettings, AgentBrainSettingsSource, AgentConfig, BrainConfig, BrainMode,
    BrainShimConfig, DestructiveGadgetsConfig, ExtraConfirmation, GadgetMode, GadgetsConfig,
    UpdateAgentBrainSettingsRequest, WriteGadgetsConfig,
};
pub use tools::{
    GadgetDispatcher, GadgetError, GadgetModeReconfigurer, GadgetProvider, GadgetResult,
    GadgetSchema, GadgetTier,
};
