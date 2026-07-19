//! Manager-facing projection and continuation controls for durable autonomy.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Extension, Json, Router,
};
use gadgetron_core::context::TenantContext;
use gadgetron_xaas::{
    autonomy::{self as store, AutonomyError},
    knowledge_spaces::SpaceActor,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::server::AppState;

use super::workbench::WorkbenchHttpError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/autonomy/goals", get(list_goals_handler))
        .route("/admin/autonomy/goals/{goal_id}", get(get_goal_handler))
        .route(
            "/admin/autonomy/goals/{goal_id}/resume",
            post(resume_goal_handler),
        )
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
}

const fn default_limit() -> i64 {
    100
}

#[derive(Debug, Serialize)]
struct GoalListResponse {
    goals: Vec<GoalProjection>,
    returned: usize,
}

#[derive(Debug, Serialize)]
struct GoalDetailResponse {
    goal: GoalProjection,
    runs: Vec<store::AutonomyRunRow>,
}

#[derive(Debug, Serialize)]
struct GoalProjection {
    #[serde(flatten)]
    goal: store::AutonomyGoalRow,
    acting_space_title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResumeRequest {
    expected_revision: i64,
}

async fn list_goals_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Query(query): Query<ListQuery>,
) -> Result<Json<GoalListResponse>, WorkbenchHttpError> {
    let goals = store::list_goals(pool(&state)?, ctx.tenant_id, query.limit)
        .await
        .map_err(autonomy_error)?;
    let goals = project_goals(&state, ctx.tenant_id, goals).await?;
    let returned = goals.len();
    Ok(Json(GoalListResponse { goals, returned }))
}

async fn get_goal_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(goal_id): Path<Uuid>,
) -> Result<Json<GoalDetailResponse>, WorkbenchHttpError> {
    let goal = store::get_goal(pool(&state)?, ctx.tenant_id, goal_id)
        .await
        .map_err(autonomy_error)?;
    let goal = project_goals(&state, ctx.tenant_id, vec![goal])
        .await?
        .pop()
        .ok_or(WorkbenchHttpError::ManagerNotFound)?;
    let runs = store::list_runs(pool(&state)?, ctx.tenant_id, goal_id, 100)
        .await
        .map_err(autonomy_error)?;
    Ok(Json(GoalDetailResponse { goal, runs }))
}

async fn project_goals(
    state: &AppState,
    tenant_id: Uuid,
    goals: Vec<store::AutonomyGoalRow>,
) -> Result<Vec<GoalProjection>, WorkbenchHttpError> {
    let pool = pool(state)?;
    let space_ids: Vec<_> = goals
        .iter()
        .filter_map(|goal| goal.acting_space_id)
        .collect();
    let titles: BTreeMap<Uuid, String> = if space_ids.is_empty() {
        BTreeMap::new()
    } else {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, title FROM knowledge_spaces WHERE tenant_id = $1 AND id = ANY($2)",
        )
        .bind(tenant_id)
        .bind(&space_ids)
        .fetch_all(pool)
        .await
        .map_err(|error| autonomy_error(AutonomyError::Database(error)))?
        .into_iter()
        .collect()
    };
    Ok(goals
        .into_iter()
        .map(|goal| {
            let acting_space_title = goal
                .acting_space_id
                .and_then(|space_id| titles.get(&space_id).cloned());
            GoalProjection {
                goal,
                acting_space_title,
            }
        })
        .collect())
}

async fn resume_goal_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(goal_id): Path<Uuid>,
    Json(request): Json<ResumeRequest>,
) -> Result<Json<GoalProjection>, WorkbenchHttpError> {
    let actor = SpaceActor {
        tenant_id: ctx.tenant_id,
        user_id: ctx
            .actor_user_id
            .ok_or(WorkbenchHttpError::ManagerIdentityRequired)?,
    };
    let goal = store::resume_goal(pool(&state)?, actor, goal_id, request.expected_revision)
        .await
        .map_err(autonomy_error)?;
    let goal = project_goals(&state, ctx.tenant_id, vec![goal])
        .await?
        .pop()
        .ok_or(WorkbenchHttpError::ManagerNotFound)?;
    Ok(Json(goal))
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, WorkbenchHttpError> {
    state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(gadgetron_core::error::GadgetronError::Config(
            "Autonomous duty-cycle oversight requires PostgreSQL".into(),
        ))
    })
}

fn autonomy_error(error: AutonomyError) -> WorkbenchHttpError {
    match error {
        AutonomyError::NotFound => WorkbenchHttpError::ManagerNotFound,
        AutonomyError::Conflict
        | AutonomyError::LeaseLost
        | AutonomyError::ExecutionSnapshotChanged => WorkbenchHttpError::ManagerConflict,
        AutonomyError::ContextForbidden => WorkbenchHttpError::KnowledgeForbidden,
        AutonomyError::InvalidInput(detail) => WorkbenchHttpError::ManagerInvalidInput { detail },
        AutonomyError::Database(error) => {
            WorkbenchHttpError::Core(gadgetron_core::error::GadgetronError::Config(format!(
                "Autonomous duty-cycle database operation failed: {error}"
            )))
        }
        AutonomyError::ServicePrincipal(error) => {
            WorkbenchHttpError::Core(gadgetron_core::error::GadgetronError::Config(format!(
                "Autonomous duty-cycle identity operation failed: {error}"
            )))
        }
    }
}
