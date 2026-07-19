//! Gateway web surface — workbench projection routes.
//!
//! Read-only workbench endpoints mounted at `/api/v1/web/workbench/`,
//! plus descriptor catalog, view data, and action invoke endpoints.

pub mod action_service;
pub mod approval_store;
pub mod autonomy;
pub mod bundle_broker;
pub mod bundle_grants;
pub mod bundle_runtime;
pub mod bundle_scheduler;
pub mod bundle_targets;
pub mod catalog;
pub mod intelligence_context;
pub mod knowledge_collections;
pub mod knowledge_graph;
pub mod knowledge_jobs;
pub mod knowledge_ontology;
pub mod knowledge_sources;
pub mod knowledge_spaces;
pub mod manager_oversight;
pub mod projection;
pub mod replay_cache;
pub mod safe_fetch;
pub mod workbench;
