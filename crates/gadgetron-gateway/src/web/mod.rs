//! Gateway web surface — workbench projection routes.
//!
//! Read-only workbench endpoints mounted at `/api/v1/web/workbench/`,
//! plus descriptor catalog, view data, and action invoke endpoints.

pub mod action_service;
pub mod approval_store;
pub mod catalog;
pub mod projection;
pub mod replay_cache;
pub mod server_metrics;
pub mod workbench;
