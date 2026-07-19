//! Core-owned transport for external Bundle runtimes.
//!
//! This crate attaches the public SDK protocol to an already-sandboxed byte
//! channel. It deliberately does not spawn arbitrary executables: process,
//! filesystem, network and resource isolation belong to a separate supervisor
//! that must prove the security floor before handing its stdio to this layer.

mod broker;
mod error;
mod installed;
mod package;
mod session;

pub use broker::{
    serve_broker_channel, BrokerCaller, BrokerChannelLimits, BrokerHostError, BundleBroker,
    DenyAllBundleBroker, DEFAULT_BROKER_FRAME_BYTES, DEFAULT_BROKER_REQUEST_TIMEOUT,
};
pub use error::{BundleHostError, Result};
pub use installed::{verify_required_detached_signature, SignedInstalledPackage};
pub use package::ValidatedPackageContract;
pub use session::{BundleHostSession, DEFAULT_FRAME_BYTES, DEFAULT_REQUEST_TIMEOUT};
