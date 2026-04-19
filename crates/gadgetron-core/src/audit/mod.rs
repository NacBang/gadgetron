//! Cross-crate audit event types + sink trait.
//!
//! P2A (Path 1) introduces a Gadget-level audit stream distinct from the
//! request-level `AuditEntry` in `gadgetron-xaas`. The types live here so
//! every consumer (the sink producer in `gadgetron-penny`, the concrete
//! writer in `gadgetron-xaas`, and the composition root in
//! `gadgetron-cli`) can depend on a common vocabulary without pulling in
//! the persistence layer's dependencies.
//!
//! Spec: ADR-P2A-06 Implementation status addendum item 1 (and
//! `04-gadget-registry.md §10`).

mod action;
mod event;

pub use action::{
    ActionAuditEvent, ActionAuditOutcome, ActionAuditSink, NoopActionAuditSink,
};
pub use event::{
    CoreAuditEvent, CoreAuditEventSink, GadgetAuditEvent, GadgetAuditEventSink, GadgetCallOutcome,
    GadgetMetadata, GadgetTier, NoopCoreAuditEventSink, NoopGadgetAuditEventSink,
};
