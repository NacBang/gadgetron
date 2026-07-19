//! Core-owned, domain-neutral process isolation for external Bundle runtimes.
//!
//! The transport host intentionally cannot spawn processes. This crate is the
//! separate security boundary that turns a validated package contract into an
//! isolated stdio channel, or fails closed before any capability is enabled.

mod error;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod spec;
#[cfg(not(target_os = "linux"))]
mod unsupported;

pub use error::{BundleSupervisorError, Result};
#[cfg(target_os = "linux")]
pub use linux::{
    run_internal_helper, LinuxSandboxSupervisor, SandboxedBundle, INTERNAL_HELPER_MARKER,
};
#[cfg(not(target_os = "linux"))]
pub use unsupported::{
    run_internal_helper, LinuxSandboxSupervisor, SandboxedBundle, INTERNAL_HELPER_MARKER,
};
