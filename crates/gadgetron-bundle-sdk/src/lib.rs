//! Public contracts shared by Gadgetron Core and independently shipped Bundles.
//!
//! This crate intentionally contains no runtime, database, policy engine, or
//! domain implementation. A Bundle may describe capabilities and submit
//! advisory observations, but Core remains authoritative for identity,
//! authorization, policy, audit, secrets, and canonical knowledge state.

mod broker;
mod bundle_set;
mod dependency;
mod error;
mod id;
mod intelligence;
mod manifest;
mod ontology;
mod protocol;

pub use broker::*;
pub use bundle_set::*;
pub use dependency::*;
pub use error::{BundleSdkError, Result};
pub use id::{BundleId, CapabilityId, GadgetName, LocalId, RelativePath};
pub use intelligence::*;
pub use manifest::*;
pub use ontology::*;
pub use protocol::*;

/// Oldest package-manifest version this SDK can read.
pub const BUNDLE_PACKAGE_MANIFEST_VERSION_MIN: u32 = 1;

/// Latest package-manifest version emitted by this SDK release.
///
/// Version 2 adds signed, typed UI contributions. Version 3 adds the signed
/// Operational/Intelligence product class. Older manifests remain readable
/// but do not acquire a class by inference.
pub const BUNDLE_PACKAGE_MANIFEST_VERSION: u32 = 3;

/// The only signed Bundle Set deployment-plan manifest understood by v1.
pub const BUNDLE_SET_MANIFEST_VERSION: u32 = 1;

/// The only Core↔Bundle host-protocol version understood by this SDK release.
pub const BUNDLE_HOST_PROTOCOL_VERSION: u32 = 1;

/// The only Bundle→Core broker-protocol version understood by this SDK release.
///
/// Broker traffic uses a separate channel and version from the Core→Bundle
/// host protocol, so either contract can evolve without multiplexing frames.
pub const BUNDLE_BROKER_PROTOCOL_VERSION: u32 = 1;
