//! Gateway web surface — workbench projection routes.
//!
//! W3-WEB-2:   read-only workbench endpoints mounted at `/api/v1/web/workbench/`.
//! W3-WEB-2b:  descriptor catalog, view data, and action invoke endpoints.

pub mod action_service;
pub mod catalog;
pub mod projection;
pub mod replay_cache;
pub mod workbench;
