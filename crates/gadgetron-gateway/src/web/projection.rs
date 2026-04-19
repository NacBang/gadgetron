//! Default in-process workbench projection — W3-WEB-2 + W3-WEB-2b.
//!
//! `InProcessWorkbenchProjection` reads from an optional `KnowledgeService`
//! to build the bootstrap and knowledge-status responses. Descriptor listing
//! delegates to the embedded `DescriptorCatalog`. Activity and evidence stubs
//! are wired in PSL-1 (Penny trace source).
//!
//! Authority: `docs/design/gateway/workbench-projection-and-actions.md` §2.2.2

use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use gadgetron_core::workbench::{
    PlugHealth, WorkbenchActivityResponse, WorkbenchBootstrapResponse,
    WorkbenchKnowledgeStatusResponse, WorkbenchKnowledgeSummary,
    WorkbenchRegisteredActionsResponse, WorkbenchRegisteredViewsResponse,
    WorkbenchRequestEvidenceResponse, WorkbenchViewData,
};
use gadgetron_knowledge::service::KnowledgeService;
use uuid::Uuid;

use super::workbench::{WorkbenchHttpError, WorkbenchProjectionService};
use crate::web::catalog::DescriptorCatalog;

// ---------------------------------------------------------------------------
// InProcessWorkbenchProjection
// ---------------------------------------------------------------------------

/// Default workbench projection that reads directly from the in-process
/// `KnowledgeService` and the descriptor catalog snapshot.
///
/// When `knowledge` is `None` (e.g. headless test builds), the bootstrap
/// response marks the service as degraded rather than panicking.
pub struct InProcessWorkbenchProjection {
    /// Optional knowledge service. `None` → degraded mode.
    pub knowledge: Option<Arc<KnowledgeService>>,
    /// Gateway crate version string (use `env!("CARGO_PKG_VERSION")`).
    pub gateway_version: &'static str,
    /// Shared descriptor snapshot — atomically swappable via
    /// `POST /api/v1/web/workbench/admin/reload-catalog` (ISSUE 8
    /// TASK 8.1). Every read loads the current `Arc<DescriptorCatalog>`
    /// so in-flight requests keep reading their snapshot while a
    /// reload swaps the pointer for future requests. O(1) `Arc::clone`
    /// on reload; no allocation on read.
    pub descriptor_catalog: Arc<ArcSwap<DescriptorCatalog>>,
}

#[async_trait]
impl WorkbenchProjectionService for InProcessWorkbenchProjection {
    async fn bootstrap(&self) -> Result<WorkbenchBootstrapResponse, WorkbenchHttpError> {
        match &self.knowledge {
            None => Ok(WorkbenchBootstrapResponse {
                gateway_version: self.gateway_version.to_string(),
                default_model: None,
                active_plugs: vec![],
                degraded_reasons: vec!["knowledge service not wired".into()],
                knowledge: WorkbenchKnowledgeSummary {
                    canonical_ready: false,
                    search_ready: false,
                    relation_ready: false,
                    last_ingest_at: None,
                },
            }),
            Some(svc) => {
                let snapshot = svc.plug_health_snapshot();
                let active_plugs: Vec<PlugHealth> = snapshot
                    .into_iter()
                    .map(|p| PlugHealth {
                        id: p.id,
                        role: p.role,
                        healthy: p.healthy,
                        note: p.note,
                    })
                    .collect();

                let canonical_ready = active_plugs
                    .iter()
                    .any(|p| p.role == "canonical" && p.healthy);
                let search_ready = active_plugs.iter().any(|p| p.role == "search" && p.healthy);
                let relation_ready = active_plugs
                    .iter()
                    .any(|p| p.role == "relation" && p.healthy);

                let degraded_reasons = if !canonical_ready {
                    vec!["no healthy canonical store".into()]
                } else {
                    vec![]
                };

                Ok(WorkbenchBootstrapResponse {
                    gateway_version: self.gateway_version.to_string(),
                    default_model: None,
                    active_plugs,
                    degraded_reasons,
                    knowledge: WorkbenchKnowledgeSummary {
                        canonical_ready,
                        search_ready,
                        relation_ready,
                        last_ingest_at: None,
                    },
                })
            }
        }
    }

    async fn activity(&self, _limit: u32) -> Result<WorkbenchActivityResponse, WorkbenchHttpError> {
        // Activity source (Penny trace) wires in PSL-1.
        Ok(WorkbenchActivityResponse {
            entries: vec![],
            is_truncated: false,
        })
    }

    async fn request_evidence(
        &self,
        request_id: Uuid,
    ) -> Result<WorkbenchRequestEvidenceResponse, WorkbenchHttpError> {
        // Evidence projection wires in PSL-1.
        Err(WorkbenchHttpError::RequestNotFound { request_id })
    }

    async fn knowledge_status(
        &self,
    ) -> Result<WorkbenchKnowledgeStatusResponse, WorkbenchHttpError> {
        match &self.knowledge {
            None => Ok(WorkbenchKnowledgeStatusResponse {
                canonical_ready: false,
                search_ready: false,
                relation_ready: false,
                stale_reasons: vec!["knowledge service not wired".into()],
                last_ingest_at: None,
            }),
            Some(svc) => {
                let snapshot = svc.plug_health_snapshot();
                let canonical_ready = snapshot.iter().any(|p| p.role == "canonical" && p.healthy);
                let search_ready = snapshot.iter().any(|p| p.role == "search" && p.healthy);
                let relation_ready = snapshot.iter().any(|p| p.role == "relation" && p.healthy);

                let mut stale_reasons = vec![];
                if !canonical_ready {
                    stale_reasons.push("canonical store not healthy".into());
                }
                if !search_ready {
                    stale_reasons.push("search index not healthy".into());
                }

                Ok(WorkbenchKnowledgeStatusResponse {
                    canonical_ready,
                    search_ready,
                    relation_ready,
                    stale_reasons,
                    last_ingest_at: None,
                })
            }
        }
    }

    async fn views(
        &self,
        actor_scopes: &[gadgetron_core::context::Scope],
    ) -> Result<WorkbenchRegisteredViewsResponse, WorkbenchHttpError> {
        // Drift-fix follow-up to PR 7 (doc-10): the handler now threads
        // the caller's real scopes through instead of the old hardcoded
        // `[Scope::OpenAiCompat]` placeholder.
        let catalog = self.descriptor_catalog.load();
        let views = catalog.visible_views(actor_scopes);
        Ok(WorkbenchRegisteredViewsResponse { views })
    }

    async fn view_data(
        &self,
        actor_scopes: &[gadgetron_core::context::Scope],
        view_id: &str,
    ) -> Result<WorkbenchViewData, WorkbenchHttpError> {
        // Scope-gated lookup: if the caller's scopes do not admit the
        // view, surface `ViewNotFound` (404) rather than 403 so we
        // don't leak existence of scope-restricted views per doc
        // §2.4.1.
        let catalog = self.descriptor_catalog.load();
        let descriptor = catalog
            .visible_views(actor_scopes)
            .into_iter()
            .find(|v| v.id == view_id)
            .ok_or_else(|| WorkbenchHttpError::ViewNotFound {
                view_id: view_id.to_string(),
            })?;

        // Seed view: knowledge-activity-recent → stub empty timeline payload.
        // Real data wiring is a follow-up (W3-WEB-3 / activity source integration).
        let payload = match descriptor.id.as_str() {
            "knowledge-activity-recent" => serde_json::json!({ "entries": [] }),
            _ => serde_json::json!({}),
        };

        Ok(WorkbenchViewData {
            view_id: view_id.to_string(),
            payload,
        })
    }

    async fn actions(
        &self,
        actor_scopes: &[gadgetron_core::context::Scope],
    ) -> Result<WorkbenchRegisteredActionsResponse, WorkbenchHttpError> {
        let catalog = self.descriptor_catalog.load();
        let actions = catalog.visible_actions(actor_scopes);
        Ok(WorkbenchRegisteredActionsResponse { actions })
    }
}
