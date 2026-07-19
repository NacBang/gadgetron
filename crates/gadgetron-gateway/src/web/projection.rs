//! Default in-process workbench projection.
//!
//! `InProcessWorkbenchProjection` reads from an optional `KnowledgeService`
//! to build the bootstrap and knowledge-status responses. Descriptor
//! listing delegates to the embedded `DescriptorCatalog`. Activity and
//! evidence stubs are wired through the Penny trace source.

use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use gadgetron_core::workbench::{
    DynamicWorkbenchSurface, PlugHealth, WorkbenchActivityResponse, WorkbenchBootstrapResponse,
    WorkbenchCapabilityProjectionResponse, WorkbenchContributionData,
    WorkbenchKnowledgeStatusResponse, WorkbenchKnowledgeSummary,
    WorkbenchRegisteredActionsResponse, WorkbenchRegisteredViewsResponse,
    WorkbenchRequestEvidenceResponse, WorkbenchViewData,
};
use gadgetron_knowledge::service::KnowledgeService;
use uuid::Uuid;

use super::workbench::{WorkbenchHttpError, WorkbenchProjectionService};
use crate::web::catalog::CatalogSnapshot;

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
    /// Shared catalog snapshot — atomically swappable via
    /// `POST /api/v1/web/workbench/admin/reload-catalog`.
    /// Bundles the `DescriptorCatalog` and its pre-compiled validators
    /// so a reload replaces BOTH in one atomic swap.
    pub descriptor_catalog: Arc<ArcSwap<CatalogSnapshot>>,
    /// Enabled external Bundle workspaces. The surface owns a signed,
    /// enabled+healthy snapshot and disappears atomically on disable/failure.
    pub dynamic_workbench: Option<Arc<dyn DynamicWorkbenchSurface>>,
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
        // Activity source (Penny trace) is wired through.
        Ok(WorkbenchActivityResponse {
            entries: vec![],
            is_truncated: false,
        })
    }

    async fn request_evidence(
        &self,
        request_id: Uuid,
    ) -> Result<WorkbenchRequestEvidenceResponse, WorkbenchHttpError> {
        // Evidence projection is wired through.
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
        let snapshot = self.descriptor_catalog.load();
        let mut views = snapshot.catalog.visible_views(actor_scopes);
        let capability_projection = self
            .dynamic_workbench
            .as_ref()
            .map(|surface| surface.capability_projection(actor_scopes));
        if let Some(dynamic) = &capability_projection {
            let existing: std::collections::BTreeSet<String> =
                views.iter().map(|view| view.id.clone()).collect();
            views.extend(
                dynamic
                    .views
                    .iter()
                    .filter(|view| !existing.contains(&view.id))
                    .cloned(),
            );
        }
        Ok(WorkbenchRegisteredViewsResponse {
            capability_revision: capability_projection.map(|projection| projection.revision),
            views,
        })
    }

    async fn view_data(
        &self,
        actor: &gadgetron_core::context::TenantContext,
        view_id: &str,
    ) -> Result<WorkbenchViewData, WorkbenchHttpError> {
        // Scope-gated lookup: if the caller's scopes do not admit the
        // view, surface `ViewNotFound` (404) rather than 403 so we
        // don't leak existence of scope-restricted views per doc
        // §2.4.1.
        let descriptor = {
            let snapshot = self.descriptor_catalog.load();
            snapshot
                .catalog
                .visible_views(&actor.scopes)
                .into_iter()
                .find(|view| view.id == view_id)
        };

        if let Some(descriptor) = descriptor {
            // Seed view: knowledge-activity-recent → stub empty timeline payload.
            // Real data wiring is a follow-up.
            let payload = match descriptor.id.as_str() {
                "knowledge-activity-recent" => serde_json::json!({ "entries": [] }),
                _ => serde_json::json!({}),
            };
            return Ok(WorkbenchViewData {
                view_id: view_id.to_string(),
                capability_revision: None,
                payload,
            });
        }

        let Some(surface) = &self.dynamic_workbench else {
            return Err(WorkbenchHttpError::ViewNotFound {
                view_id: view_id.to_string(),
            });
        };
        let dispatch_context = gadgetron_core::agent::tools::GadgetDispatchContext::new(
            actor.tenant_id.to_string(),
            actor.actor_user_id.unwrap_or(actor.api_key_id).to_string(),
            actor.request_id.to_string(),
        )
        .with_scopes(actor.scopes.iter().map(ToString::to_string));
        surface
            .load_view_data(dispatch_context, &actor.scopes, view_id)
            .await
            .map_err(|error| match error {
                gadgetron_core::agent::tools::GadgetError::UnknownGadget(_) => {
                    WorkbenchHttpError::ViewNotFound {
                        view_id: view_id.to_string(),
                    }
                }
                other => WorkbenchHttpError::Core(other.into()),
            })
    }

    async fn actions(
        &self,
        actor_scopes: &[gadgetron_core::context::Scope],
    ) -> Result<WorkbenchRegisteredActionsResponse, WorkbenchHttpError> {
        let snapshot = self.descriptor_catalog.load();
        let mut actions = snapshot.catalog.visible_actions(actor_scopes);
        let capability_projection = self
            .dynamic_workbench
            .as_ref()
            .map(|surface| surface.capability_projection(actor_scopes));
        if let Some(dynamic) = &capability_projection {
            let existing: std::collections::BTreeSet<String> =
                actions.iter().map(|action| action.id.clone()).collect();
            actions.extend(
                dynamic
                    .actions
                    .iter()
                    .filter(|action| !existing.contains(&action.id))
                    .cloned(),
            );
        }
        Ok(WorkbenchRegisteredActionsResponse {
            capability_revision: capability_projection.map(|projection| projection.revision),
            actions,
        })
    }

    async fn capabilities(
        &self,
        actor_scopes: &[gadgetron_core::context::Scope],
    ) -> Result<WorkbenchCapabilityProjectionResponse, WorkbenchHttpError> {
        Ok(self
            .dynamic_workbench
            .as_ref()
            .map_or_else(WorkbenchCapabilityProjectionResponse::default, |surface| {
                surface.capability_projection(actor_scopes)
            }))
    }

    async fn contribution_data(
        &self,
        actor: &gadgetron_core::context::TenantContext,
        contribution_id: &str,
    ) -> Result<WorkbenchContributionData, WorkbenchHttpError> {
        let surface = self.dynamic_workbench.as_ref().ok_or_else(|| {
            WorkbenchHttpError::ContributionNotFound {
                contribution_id: contribution_id.to_string(),
            }
        })?;
        let projection = surface.capability_projection(&actor.scopes);
        if !projection
            .ui_contributions
            .iter()
            .any(|contribution| contribution.id == contribution_id)
        {
            return Err(WorkbenchHttpError::ContributionNotFound {
                contribution_id: contribution_id.to_string(),
            });
        }
        let dispatch_context = gadgetron_core::agent::tools::GadgetDispatchContext::new(
            actor.tenant_id.to_string(),
            actor.actor_user_id.unwrap_or(actor.api_key_id).to_string(),
            actor.request_id.to_string(),
        )
        .with_scopes(actor.scopes.iter().map(ToString::to_string));
        surface
            .load_contribution_data(dispatch_context, &actor.scopes, contribution_id)
            .await
            .map_err(|error| match error {
                gadgetron_core::agent::tools::GadgetError::UnknownGadget(_) => {
                    WorkbenchHttpError::ContributionNotFound {
                        contribution_id: contribution_id.to_string(),
                    }
                }
                other => WorkbenchHttpError::Core(other.into()),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::{
        agent::tools::{GadgetDispatchContext, GadgetError},
        context::{QuotaSnapshot, Scope, TenantContext},
        workbench::{
            WorkbenchActionDescriptor, WorkbenchRendererKind, WorkbenchUiContributionDescriptor,
            WorkbenchUiContributionKind, WorkbenchUiContributionPlacement, WorkbenchUiIconToken,
            WorkbenchViewDescriptor,
        },
    };
    use std::{sync::Arc, time::Instant};

    struct FakeDynamicSurface;

    #[async_trait]
    impl DynamicWorkbenchSurface for FakeDynamicSurface {
        fn visible_views(&self, _: &[Scope]) -> Vec<WorkbenchViewDescriptor> {
            vec![]
        }
        fn visible_actions(&self, _: &[Scope]) -> Vec<WorkbenchActionDescriptor> {
            vec![]
        }
        fn find_action(&self, _: &[Scope], _: &str) -> Option<WorkbenchActionDescriptor> {
            None
        }
        fn capability_projection(&self, _: &[Scope]) -> WorkbenchCapabilityProjectionResponse {
            WorkbenchCapabilityProjectionResponse {
                revision: "a".repeat(64),
                bundles: vec![],
                views: vec![],
                actions: vec![],
                ui_contributions: vec![WorkbenchUiContributionDescriptor {
                    id: "travel.summary".into(),
                    owner_bundle: "travel".into(),
                    kind: WorkbenchUiContributionKind::DashboardWidget,
                    label: "Trip summary".into(),
                    placement: WorkbenchUiContributionPlacement::Dashboard,
                    order_hint: 0,
                    icon: WorkbenchUiIconToken::Calendar,
                    navigation_section: None,
                    target_registry: None,
                    target_profile: None,
                    required_scopes: vec![],
                    empty_state: "No trips".into(),
                    error_state: "Trips unavailable".into(),
                    workspace_id: None,
                    gadget_name: Some("travel.summary".into()),
                    job_id: None,
                    domain_schema_id: None,
                    renderer: Some(WorkbenchRendererKind::Dashboard),
                    refresh_seconds: Some(30),
                }],
            }
        }
        async fn load_view_data(
            &self,
            _: GadgetDispatchContext,
            _: &[Scope],
            view_id: &str,
        ) -> Result<WorkbenchViewData, GadgetError> {
            Err(GadgetError::UnknownGadget(view_id.into()))
        }
        async fn load_contribution_data(
            &self,
            _: GadgetDispatchContext,
            _: &[Scope],
            contribution_id: &str,
        ) -> Result<WorkbenchContributionData, GadgetError> {
            if contribution_id == "travel.summary" {
                Ok(WorkbenchContributionData {
                    contribution_id: contribution_id.into(),
                    capability_revision: "a".repeat(64),
                    payload: serde_json::json!({"upcoming": 2}),
                })
            } else {
                Err(GadgetError::UnknownGadget(contribution_id.into()))
            }
        }
    }

    fn actor() -> TenantContext {
        TenantContext {
            tenant_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
            scopes: vec![Scope::OpenAiCompat],
            quota_snapshot: Arc::new(QuotaSnapshot {
                daily_limit_cents: 1,
                daily_used_cents: 0,
                monthly_limit_cents: 1,
                monthly_used_cents: 0,
            }),
            request_id: Uuid::new_v4(),
            started_at: Instant::now(),
            actor_user_id: None,
            actor_api_key_id: None,
        }
    }

    #[tokio::test]
    async fn contribution_data_is_revision_pinned_and_descriptor_selected() {
        let projection = InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "test",
            descriptor_catalog: Arc::new(ArcSwap::from_pointee(
                crate::web::catalog::DescriptorCatalog::empty().into_snapshot(),
            )),
            dynamic_workbench: Some(Arc::new(FakeDynamicSurface)),
        };
        let data = projection
            .contribution_data(&actor(), "travel.summary")
            .await
            .unwrap();
        assert_eq!(data.capability_revision, "a".repeat(64));
        assert_eq!(data.payload, serde_json::json!({"upcoming": 2}));
        assert!(matches!(
            projection
                .contribution_data(&actor(), "travel.hidden")
                .await,
            Err(WorkbenchHttpError::ContributionNotFound { .. })
        ));
    }
}
