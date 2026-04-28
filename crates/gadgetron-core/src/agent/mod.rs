//! Agent-Centric Control Plane types.
//!
//! Agent (Claude Code) permission model, brain model selection, Gadget
//! registry provider interface. See:
//!
//! - D-20260414-04 (decision log)
//! - ADR-P2A-05 (agent-centric control plane)
//! - ADR-P2A-10 (Bundle / Plug / Gadget terminology)
//! - docs/design/phase2/04-gadget-registry.md
//!
//! Submodules:
//! - [`config`] — `AgentConfig`, `BrainConfig`, `GadgetsConfig`, + validation
//! - [`tools`] — `GadgetProvider` trait, `GadgetSchema`, `GadgetTier`,
//!   `GadgetResult`, `GadgetError`

pub mod config;
pub mod shared_context;
pub mod tools;

pub use config::{
    AgentConfig, BrainConfig, BrainMode, BrainShimConfig, DestructiveGadgetsConfig,
    ExtraConfirmation, GadgetMode, GadgetsConfig, WriteGadgetsConfig,
};
pub use tools::{
    GadgetDispatcher, GadgetError, GadgetModeReconfigurer, GadgetProvider, GadgetResult,
    GadgetSchema, GadgetTier,
};
