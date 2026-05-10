//! Penny-facing gateway services — shared surface awareness.
//!
//! This module exposes the `PennySharedSurfaceService` trait and its
//! `InProcessPennySharedSurfaceService` default implementation that
//! delegates to `WorkbenchProjectionService`.
//!
//! Also provides `DefaultPennyTurnContextAssembler` which implements the
//! `PennyTurnContextAssembler` trait from `gadgetron-core`.
//!
//! `render_penny_shared_context` is a pure deterministic function that
//! converts a `PennyTurnBootstrap` into the `<gadgetron_shared_context>`
//! prompt block injected before each Penny turn.

pub mod shared_context;

pub use shared_context::{
    render_penny_shared_context, DefaultPennyTurnContextAssembler,
    InProcessPennySharedSurfaceService, PennySharedSurfaceService,
};
