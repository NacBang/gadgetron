use axum::{
    extract::{Path, State},
    routing::{get, post},
    Extension, Json, Router,
};
use gadgetron_core::{
    context::TenantContext,
    error::{DatabaseErrorKind, GadgetronError},
};
use gadgetron_knowledge::{
    OntologyActivationCommand, OntologyActivationReceipt, OntologyKernel, OntologyMappingCommand,
    OntologyMappingDisposition, OntologyMappingEvent, OntologyRegistryEntry, OntologyRegistryError,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::AppState;

use super::workbench::WorkbenchHttpError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/knowledge/ontologies", get(list_registry_handler))
        .route(
            "/knowledge/ontologies/{revision_id}/activate",
            post(activate_handler),
        )
        .route(
            "/knowledge/ontologies/{revision_id}/deactivate",
            post(deactivate_handler),
        )
        .route(
            "/knowledge/objects/{object_id}/ontology-mappings",
            get(list_mapping_history_handler).post(append_mapping_handler),
        )
}

#[derive(Debug, Serialize)]
pub struct OntologyRegistryResponse {
    pub revisions: Vec<OntologyRegistryEntry>,
    pub returned: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationRequest {
    pub expected_activation_revision: i64,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappingRequest {
    pub object_revision: i64,
    pub expected_mapping_revision: i64,
    pub disposition: OntologyMappingDisposition,
    #[serde(default)]
    pub ontology_revision_id: Option<Uuid>,
    #[serde(default)]
    pub type_id: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default = "empty_object")]
    pub evidence: serde_json::Value,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct MappingHistoryResponse {
    pub mappings: Vec<OntologyMappingEvent>,
    pub returned: usize,
}

pub async fn list_registry_handler(
    State(state): State<AppState>,
    Extension(context): Extension<TenantContext>,
) -> Result<Json<OntologyRegistryResponse>, WorkbenchHttpError> {
    let actor = require_admin(&state, &context).await?;
    let revisions = OntologyKernel::new(pool(&state)?.clone())
        .list_registry(actor.0)
        .await
        .map_err(map_error)?;
    let returned = revisions.len();
    Ok(Json(OntologyRegistryResponse {
        revisions,
        returned,
    }))
}

pub async fn activate_handler(
    State(state): State<AppState>,
    Extension(context): Extension<TenantContext>,
    Path(revision_id): Path<Uuid>,
    Json(request): Json<ActivationRequest>,
) -> Result<Json<OntologyActivationReceipt>, WorkbenchHttpError> {
    set_activation(&state, &context, revision_id, request, true)
        .await
        .map(Json)
}

pub async fn deactivate_handler(
    State(state): State<AppState>,
    Extension(context): Extension<TenantContext>,
    Path(revision_id): Path<Uuid>,
    Json(request): Json<ActivationRequest>,
) -> Result<Json<OntologyActivationReceipt>, WorkbenchHttpError> {
    set_activation(&state, &context, revision_id, request, false)
        .await
        .map(Json)
}

async fn set_activation(
    state: &AppState,
    context: &TenantContext,
    revision_id: Uuid,
    request: ActivationRequest,
    activate: bool,
) -> Result<OntologyActivationReceipt, WorkbenchHttpError> {
    let (tenant_id, user_id) = require_admin(state, context).await?;
    let kernel = OntologyKernel::new(pool(state)?.clone());
    let command = OntologyActivationCommand {
        tenant_id,
        actor_user_id: user_id,
        ontology_revision_id: revision_id,
        expected_activation_revision: request.expected_activation_revision,
        reason: &request.reason,
    };
    if activate {
        kernel.activate(command).await
    } else {
        kernel.deactivate(command).await
    }
    .map_err(map_error)
}

pub async fn append_mapping_handler(
    State(state): State<AppState>,
    Extension(context): Extension<TenantContext>,
    Path(object_id): Path<Uuid>,
    Json(request): Json<MappingRequest>,
) -> Result<Json<OntologyMappingEvent>, WorkbenchHttpError> {
    let (tenant_id, user_id) = require_admin(&state, &context).await?;
    let event = OntologyKernel::new(pool(&state)?.clone())
        .append_mapping(OntologyMappingCommand {
            tenant_id,
            recorded_by: user_id,
            object_id,
            object_revision: request.object_revision,
            expected_mapping_revision: request.expected_mapping_revision,
            disposition: request.disposition,
            ontology_revision_id: request.ontology_revision_id,
            type_id: request.type_id.as_deref(),
            confidence: request.confidence,
            evidence: request.evidence,
            reason: &request.reason,
        })
        .await
        .map_err(map_error)?;
    Ok(Json(event))
}

pub async fn list_mapping_history_handler(
    State(state): State<AppState>,
    Extension(context): Extension<TenantContext>,
    Path(object_id): Path<Uuid>,
) -> Result<Json<MappingHistoryResponse>, WorkbenchHttpError> {
    let (tenant_id, _) = require_admin(&state, &context).await?;
    let mappings = OntologyKernel::new(pool(&state)?.clone())
        .list_mapping_history(tenant_id, object_id)
        .await
        .map_err(map_error)?;
    let returned = mappings.len();
    Ok(Json(MappingHistoryResponse { mappings, returned }))
}

async fn require_admin(
    state: &AppState,
    context: &TenantContext,
) -> Result<(Uuid, Uuid), WorkbenchHttpError> {
    let user_id = context
        .actor_user_id
        .ok_or(WorkbenchHttpError::KnowledgeForbidden)?;
    gadgetron_xaas::knowledge_graph::require_tenant_admin(pool(state)?, context.tenant_id, user_id)
        .await?;
    Ok((context.tenant_id, user_id))
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, WorkbenchHttpError> {
    state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "Knowledge ontology requires PostgreSQL".to_string(),
        ))
    })
}

fn map_error(error: OntologyRegistryError) -> WorkbenchHttpError {
    match error {
        OntologyRegistryError::RevisionNotFound(_) | OntologyRegistryError::ObjectNotFound(_) => {
            WorkbenchHttpError::KnowledgeNotFound
        }
        OntologyRegistryError::ActivationConflict { .. }
        | OntologyRegistryError::ObjectRevisionConflict { .. }
        | OntologyRegistryError::MappingConflict { .. }
        | OntologyRegistryError::ActivationTargetChanged => WorkbenchHttpError::KnowledgeConflict,
        OntologyRegistryError::Database(error) => {
            WorkbenchHttpError::Core(GadgetronError::Database {
                kind: DatabaseErrorKind::QueryFailed,
                message: format!("Knowledge ontology operation failed: {error}"),
            })
        }
        error => WorkbenchHttpError::KnowledgeInvalidInput {
            detail: error.to_string(),
        },
    }
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}
