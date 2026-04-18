//! Default in-process workbench projection — W3-WEB-2.
//!
//! `InProcessWorkbenchProjection` reads from an optional `KnowledgeService`
//! to build the bootstrap response. Activity and evidence stubs are wired
//! in PSL-1 (Penny trace source).
//!
//! Authority: `docs/design/gateway/workbench-projection-and-actions.md` §2.2.2

use std::sync::Arc;

use async_trait::async_trait;
use gadgetron_core::workbench::{
    PlugHealth, WorkbenchActivityResponse, WorkbenchBootstrapResponse, WorkbenchKnowledgeSummary,
    WorkbenchRequestEvidenceResponse,
};
use gadgetron_knowledge::service::KnowledgeService;
use uuid::Uuid;

use super::workbench::{WorkbenchHttpError, WorkbenchProjectionService};

// ---------------------------------------------------------------------------
// InProcessWorkbenchProjection
// ---------------------------------------------------------------------------

/// Default workbench projection that reads directly from the in-process
/// `KnowledgeService`.
///
/// When `knowledge` is `None` (e.g. headless test builds), the bootstrap
/// response marks the service as degraded rather than panicking.
pub struct InProcessWorkbenchProjection {
    /// Optional knowledge service. `None` → degraded mode.
    pub knowledge: Option<Arc<KnowledgeService>>,
    /// Gateway crate version string (use `env!("CARGO_PKG_VERSION")`).
    pub gateway_version: &'static str,
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
}
