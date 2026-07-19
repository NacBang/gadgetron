use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, SecondsFormat, Utc};
use gadgetron_bundle_sdk::{
    BrokerResource, DatabaseInsertRequest, DatabaseOrderDirection, DatabaseRows,
    DatabaseSelectRequest, DatabaseUpdateRequest, GadgetInvocation, HostResponse,
    InvocationLeaseToken, LocalId,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{broker_error, gadget_result, host_error, SharedBroker};

const READ_PERMISSION: &str = "restaurant-read";
const WRITE_PERMISSION: &str = "restaurant-write";

#[derive(Clone, Copy)]
struct EntitySpec {
    table: &'static str,
    id_column: &'static str,
    columns: fn() -> Vec<String>,
}

const BRANCH: EntitySpec = EntitySpec {
    table: "restaurant_branches",
    id_column: "branch_id",
    columns: branch_columns,
};
const MENU_ITEM: EntitySpec = EntitySpec {
    table: "restaurant_menu_items",
    id_column: "menu_item_id",
    columns: menu_columns,
};
const REVIEW: EntitySpec = EntitySpec {
    table: "restaurant_review_snapshots",
    id_column: "review_id",
    columns: review_columns,
};
const RECOMMENDATION: EntitySpec = EntitySpec {
    table: "restaurant_recommendations",
    id_column: "recommendation_id",
    columns: recommendation_columns,
};
const OUTCOME: EntitySpec = EntitySpec {
    table: "restaurant_visit_outcomes",
    id_column: "outcome_id",
    columns: outcome_columns,
};

pub(crate) async fn invoke(invocation: GadgetInvocation, broker: SharedBroker) -> HostResponse {
    let Some(lease) = invocation.context.broker_lease else {
        return host_error(
            "broker-lease-required",
            "Core did not attach an invocation-scoped broker lease",
        );
    };
    match invocation.gadget.as_str() {
        "restaurant.branch-list" => branch_list(invocation.input, lease, broker).await,
        "restaurant.branch-get" => branch_get(invocation.input, lease, broker).await,
        "restaurant.branch-upsert" => branch_upsert(invocation.input, lease, broker).await,
        "restaurant.menu-list" => {
            child_list(
                invocation.input,
                lease,
                broker,
                "restaurant_menu_items",
                menu_columns(),
                "observed_at",
                500,
            )
            .await
        }
        "restaurant.menu-upsert" => menu_upsert(invocation.input, lease, broker).await,
        "restaurant.review-list" => {
            child_list(
                invocation.input,
                lease,
                broker,
                "restaurant_review_snapshots",
                review_columns(),
                "captured_at",
                200,
            )
            .await
        }
        "restaurant.review-upsert" => review_upsert(invocation.input, lease, broker).await,
        "restaurant.recommendation-list" => {
            recommendation_list(invocation.input, lease, broker).await
        }
        "restaurant.recommend" => recommend(invocation.input, lease, broker).await,
        "restaurant.compare" => compare(invocation.input, lease, broker).await,
        "restaurant.research" => research_handoff(invocation.input),
        "restaurant.record-visit-outcome" => visit_outcome(invocation.input, lease, broker).await,
        "restaurant.subject-context" => subject_context(invocation.input, lease, broker).await,
        "restaurant.plan-summary" => plan_summary(lease, broker).await,
        _ => host_error(
            "capability-not-found",
            "requested Restaurant Research capability is not available",
        ),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListInput {
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchIdInput {
    branch_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildListInput {
    branch_id: String,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecommendationIdInput {
    recommendation_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompareInput {
    branch_id: String,
    compare_to_branch_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResearchInput {
    question: String,
    source_id: String,
    source_revision: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchUpsertInput {
    #[serde(default)]
    branch_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    name: String,
    address: String,
    cuisine: String,
    status: String,
    source_id: String,
    source_revision: i64,
    observed_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MenuUpsertInput {
    #[serde(default)]
    menu_item_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    branch_id: String,
    name: String,
    category: String,
    #[serde(default)]
    price_minor: Option<i64>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    dietary_notes: String,
    #[serde(default)]
    allergen_notes: String,
    source_id: String,
    source_revision: i64,
    observed_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewUpsertInput {
    #[serde(default)]
    review_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    branch_id: String,
    source_name: String,
    passage: String,
    #[serde(default)]
    bias_context: String,
    sentiment: String,
    source_id: String,
    source_revision: i64,
    captured_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecommendInput {
    #[serde(default)]
    recommendation_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    branch_id: String,
    query: String,
    reason: String,
    #[serde(default)]
    conditions: String,
    freshness: String,
    supporting_source_id: String,
    supporting_source_revision: i64,
    #[serde(default)]
    contradicting_source_id: Option<String>,
    #[serde(default)]
    contradicting_source_revision: Option<i64>,
    valid_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VisitOutcomeInput {
    #[serde(default)]
    outcome_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    recommendation_id: String,
    visited_at: String,
    result: String,
    #[serde(default)]
    feedback: String,
    #[serde(default)]
    actual_cost_minor: Option<i64>,
    #[serde(default)]
    currency: Option<String>,
}

async fn branch_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let limit = match list_limit(input, 500) {
        Ok(limit) => limit,
        Err(message) => return invalid(message),
    };
    query_rows(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("restaurant_branches"),
            branch_columns(),
        )
        .with_order("name", DatabaseOrderDirection::Ascending)
        .with_order("branch_id", DatabaseOrderDirection::Ascending)
        .with_limit(limit),
    )
    .await
}

async fn branch_get(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: BranchIdInput = match serde_json::from_value::<BranchIdInput>(input) {
        Ok(input) if valid_uuid(&input.branch_id) => input,
        _ => return invalid("branch_id must be a UUID"),
    };
    row_result(
        select_one(
            &broker,
            lease,
            "restaurant_branches",
            "branch_id",
            &input.branch_id,
            branch_columns(),
        )
        .await,
        "branch-not-found",
        "the requested branch does not exist",
    )
}

async fn branch_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: BranchUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Branch fields do not match the signed schema"),
    };
    if !bounded(&input.name, 1, 200)
        || !bounded(&input.address, 1, 500)
        || !bounded(&input.cuisine, 1, 120)
        || !matches!(
            input.status.as_str(),
            "open" | "temporarily_closed" | "closed" | "unknown"
        )
        || !valid_uuid(&input.source_id)
        || input.source_revision <= 0
        || !valid_time(&input.observed_at)
    {
        return invalid("Branch values violate the signed domain contract");
    }
    let values = BTreeMap::from([
        ("name".into(), json!(input.name)),
        ("address".into(), json!(input.address)),
        ("cuisine".into(), json!(input.cuisine)),
        ("status".into(), json!(input.status)),
        ("source_id".into(), json!(input.source_id)),
        ("source_revision".into(), json!(input.source_revision)),
        ("observed_at".into(), json!(input.observed_at)),
    ]);
    upsert(
        &broker,
        lease,
        BRANCH,
        input.branch_id,
        input.revision,
        values,
    )
    .await
}

async fn child_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
    table_name: &str,
    columns: Vec<String>,
    order: &str,
    maximum: u32,
) -> HostResponse {
    let input: ChildListInput = match serde_json::from_value::<ChildListInput>(input) {
        Ok(input) if valid_uuid(&input.branch_id) && (1..=maximum).contains(&input.limit) => input,
        _ => return invalid("branch_id and bounded limit are required"),
    };
    query_rows(
        &broker,
        DatabaseSelectRequest::new(lease, id(READ_PERMISSION), table(table_name), columns)
            .with_filter("branch_id", json!(input.branch_id))
            .with_order(order, DatabaseOrderDirection::Descending)
            .with_limit(input.limit),
    )
    .await
}

async fn menu_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: MenuUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Menu fields do not match the signed schema"),
    };
    if !valid_uuid(&input.branch_id)
        || !bounded(&input.name, 1, 200)
        || !bounded(&input.category, 1, 100)
        || input.price_minor.is_some_and(|value| value < 0)
        || input
            .currency
            .as_deref()
            .is_some_and(|value| !valid_currency(value))
        || input.price_minor.is_some() != input.currency.is_some()
        || !bounded(&input.dietary_notes, 0, 500)
        || !bounded(&input.allergen_notes, 0, 500)
        || !valid_source(&input.source_id, input.source_revision, &input.observed_at)
    {
        return invalid("Menu values violate the signed domain contract");
    }
    let values = BTreeMap::from([
        ("branch_id".into(), json!(input.branch_id)),
        ("name".into(), json!(input.name)),
        ("category".into(), json!(input.category)),
        ("price_minor".into(), option_json(input.price_minor)),
        ("currency".into(), option_json(input.currency)),
        ("dietary_notes".into(), json!(input.dietary_notes)),
        ("allergen_notes".into(), json!(input.allergen_notes)),
        ("source_id".into(), json!(input.source_id)),
        ("source_revision".into(), json!(input.source_revision)),
        ("observed_at".into(), json!(input.observed_at)),
    ]);
    upsert(
        &broker,
        lease,
        MENU_ITEM,
        input.menu_item_id,
        input.revision,
        values,
    )
    .await
}

async fn review_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ReviewUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Review fields do not match the signed schema"),
    };
    if !valid_uuid(&input.branch_id)
        || !bounded(&input.source_name, 1, 120)
        || !bounded(&input.passage, 1, 2000)
        || !bounded(&input.bias_context, 0, 500)
        || !matches!(
            input.sentiment.as_str(),
            "positive" | "mixed" | "negative" | "unrated"
        )
        || !valid_source(&input.source_id, input.source_revision, &input.captured_at)
    {
        return invalid("Review values violate the signed domain contract");
    }
    let values = BTreeMap::from([
        ("branch_id".into(), json!(input.branch_id)),
        ("source_name".into(), json!(input.source_name)),
        ("passage".into(), json!(input.passage)),
        ("bias_context".into(), json!(input.bias_context)),
        ("sentiment".into(), json!(input.sentiment)),
        ("source_id".into(), json!(input.source_id)),
        ("source_revision".into(), json!(input.source_revision)),
        ("captured_at".into(), json!(input.captured_at)),
    ]);
    upsert(
        &broker,
        lease,
        REVIEW,
        input.review_id,
        input.revision,
        values,
    )
    .await
}

async fn recommendation_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let limit = match list_limit(input, 200) {
        Ok(limit) => limit,
        Err(message) => return invalid(message),
    };
    let mut recommendations = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("restaurant_recommendations"),
            recommendation_columns(),
        )
        .with_order("valid_at", DatabaseOrderDirection::Descending)
        .with_order("recommendation_id", DatabaseOrderDirection::Ascending)
        .with_limit(limit),
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let branches = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("restaurant_branches"),
            [
                "branch_id".into(),
                "name".into(),
                "address".into(),
                "cuisine".into(),
                "status".into(),
            ],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let by_id: HashMap<String, BTreeMap<String, Value>> = branches
        .rows
        .into_iter()
        .filter_map(|row| Some((row.get("branch_id")?.as_str()?.to_string(), row)))
        .collect();
    for row in &mut recommendations.rows {
        if let Some(branch) = row
            .get("branch_id")
            .and_then(Value::as_str)
            .and_then(|branch_id| by_id.get(branch_id))
        {
            row.insert(
                "title".into(),
                branch.get("name").cloned().unwrap_or(Value::Null),
            );
            row.insert(
                "address".into(),
                branch.get("address").cloned().unwrap_or(Value::Null),
            );
            row.insert(
                "cuisine".into(),
                branch.get("cuisine").cloned().unwrap_or(Value::Null),
            );
            row.insert(
                "branch_status".into(),
                branch.get("status").cloned().unwrap_or(Value::Null),
            );
        }
    }
    rows_result(recommendations)
}

async fn recommend(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: RecommendInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Recommendation fields do not match the signed schema"),
    };
    if !valid_uuid(&input.branch_id)
        || !bounded(&input.query, 1, 500)
        || !bounded(&input.reason, 1, 1000)
        || !bounded(&input.conditions, 0, 1000)
        || !matches!(
            input.freshness.as_str(),
            "current" | "aging" | "stale" | "conflicted"
        )
        || !valid_uuid(&input.supporting_source_id)
        || input.supporting_source_revision <= 0
        || input
            .contradicting_source_id
            .as_deref()
            .is_some_and(|value| !valid_uuid(value))
        || input.contradicting_source_id.is_some() != input.contradicting_source_revision.is_some()
        || input
            .contradicting_source_revision
            .is_some_and(|value| value <= 0)
        || !valid_time(&input.valid_at)
    {
        return invalid("Recommendation values violate the signed domain contract");
    }
    let values = BTreeMap::from([
        ("branch_id".into(), json!(input.branch_id)),
        ("query".into(), json!(input.query)),
        ("reason".into(), json!(input.reason)),
        ("conditions".into(), json!(input.conditions)),
        ("freshness".into(), json!(input.freshness)),
        (
            "supporting_source_id".into(),
            json!(input.supporting_source_id),
        ),
        (
            "supporting_source_revision".into(),
            json!(input.supporting_source_revision),
        ),
        (
            "contradicting_source_id".into(),
            option_json(input.contradicting_source_id),
        ),
        (
            "contradicting_source_revision".into(),
            option_json(input.contradicting_source_revision),
        ),
        ("valid_at".into(), json!(input.valid_at)),
    ]);
    upsert(
        &broker,
        lease,
        RECOMMENDATION,
        input.recommendation_id,
        input.revision,
        values,
    )
    .await
}

async fn compare(input: Value, lease: InvocationLeaseToken, broker: SharedBroker) -> HostResponse {
    let input: CompareInput = match serde_json::from_value::<CompareInput>(input) {
        Ok(input)
            if valid_uuid(&input.branch_id)
                && valid_uuid(&input.compare_to_branch_id)
                && input.branch_id != input.compare_to_branch_id =>
        {
            input
        }
        _ => return invalid("two different branch UUIDs are required"),
    };
    let mut compared = Vec::new();
    for branch_id in [&input.branch_id, &input.compare_to_branch_id] {
        let Some(branch) = (match select_one(
            &broker,
            lease.clone(),
            "restaurant_branches",
            "branch_id",
            branch_id,
            branch_columns(),
        )
        .await
        {
            Ok(row) => row,
            Err(error) => return error,
        }) else {
            return host_error("branch-not-found", "a compared branch no longer exists");
        };
        let menus = match select(
            &broker,
            DatabaseSelectRequest::new(
                lease.clone(),
                id(READ_PERMISSION),
                table("restaurant_menu_items"),
                menu_columns(),
            )
            .with_filter("branch_id", json!(branch_id))
            .with_limit(200),
        )
        .await
        {
            Ok(rows) => rows.rows,
            Err(error) => return error,
        };
        let recommendations = match select(
            &broker,
            DatabaseSelectRequest::new(
                lease.clone(),
                id(READ_PERMISSION),
                table("restaurant_recommendations"),
                recommendation_columns(),
            )
            .with_filter("branch_id", json!(branch_id))
            .with_limit(100),
        )
        .await
        {
            Ok(rows) => rows.rows,
            Err(error) => return error,
        };
        compared.push(json!({"branch": branch, "menu": menus, "recommendations": recommendations}));
    }
    gadget_result(json!({"rows": compared, "count": compared.len()}))
}

fn research_handoff(input: Value) -> HostResponse {
    let input: ResearchInput = match serde_json::from_value::<ResearchInput>(input) {
        Ok(input)
            if bounded(&input.question, 1, 500)
                && valid_uuid(&input.source_id)
                && input.source_revision > 0 =>
        {
            input
        }
        _ => return invalid("question and a pinned Core source revision are required"),
    };
    gadget_result(json!({
        "recipe": "restaurant-core-source-research",
        "network": "core_source_fetch_only",
        "question": input.question,
        "source_id": input.source_id,
        "source_revision": input.source_revision,
        "next_gadgets": ["restaurant.branch-upsert", "restaurant.menu-upsert", "restaurant.review-upsert", "restaurant.recommend"]
    }))
}

async fn recommendation_context(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: RecommendationIdInput = match serde_json::from_value::<RecommendationIdInput>(input)
    {
        Ok(input) if valid_uuid(&input.recommendation_id) => input,
        _ => return invalid("recommendation_id must be a UUID"),
    };
    let Some(recommendation) = (match select_one(
        &broker,
        lease.clone(),
        "restaurant_recommendations",
        "recommendation_id",
        &input.recommendation_id,
        recommendation_columns(),
    )
    .await
    {
        Ok(row) => row,
        Err(error) => return error,
    }) else {
        return host_error(
            "recommendation-not-found",
            "the requested recommendation does not exist",
        );
    };
    let branch_id = recommendation
        .get("branch_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(branch) = (match select_one(
        &broker,
        lease,
        "restaurant_branches",
        "branch_id",
        branch_id,
        branch_columns(),
    )
    .await
    {
        Ok(row) => row,
        Err(error) => return error,
    }) else {
        return host_error(
            "branch-not-found",
            "the recommendation branch does not exist",
        );
    };
    gadget_result(json!({
        "recommendation_id": input.recommendation_id,
        "recommendation_revision": recommendation.get("revision"),
        "branch_id": branch_id,
        "branch_revision": branch.get("revision"),
        "title": branch.get("name"),
        "place": branch.get("address"),
        "reason": recommendation.get("reason"),
        "freshness": recommendation.get("freshness"),
        "supporting_source_id": recommendation.get("supporting_source_id"),
        "supporting_source_revision": recommendation.get("supporting_source_revision"),
        "contradicting_source_id": recommendation.get("contradicting_source_id"),
        "contradicting_source_revision": recommendation.get("contradicting_source_revision")
    }))
}

async fn visit_outcome(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: VisitOutcomeInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Visit outcome fields do not match the signed schema"),
    };
    if !valid_uuid(&input.recommendation_id)
        || !valid_time(&input.visited_at)
        || !matches!(
            input.result.as_str(),
            "better_than_expected" | "as_expected" | "worse_than_expected" | "not_visited"
        )
        || !bounded(&input.feedback, 0, 2000)
        || input.actual_cost_minor.is_some_and(|value| value < 0)
        || input
            .currency
            .as_deref()
            .is_some_and(|value| !valid_currency(value))
        || input.actual_cost_minor.is_some() != input.currency.is_some()
    {
        return invalid("Visit outcome values violate the signed domain contract");
    }
    let values = BTreeMap::from([
        ("recommendation_id".into(), json!(input.recommendation_id)),
        ("visited_at".into(), json!(input.visited_at)),
        ("result".into(), json!(input.result)),
        ("feedback".into(), json!(input.feedback)),
        (
            "actual_cost_minor".into(),
            option_json(input.actual_cost_minor),
        ),
        ("currency".into(), option_json(input.currency)),
    ]);
    upsert(
        &broker,
        lease,
        OUTCOME,
        input.outcome_id,
        input.revision,
        values,
    )
    .await
}

async fn subject_context(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let handoff = recommendation_context(input, lease, broker).await;
    let HostResponse::GadgetResult(result) = handoff else {
        return handoff;
    };
    let value = result.output;
    gadget_result(json!({
        "id": value.get("recommendation_id"),
        "kind": "restaurant_recommendation",
        "bundle": "restaurant-research",
        "title": value.get("title"),
        "subtitle": value.get("place"),
        "href": "/web/workspace?id=restaurant-research:restaurants",
        "summary": value.get("reason"),
        "facts": value,
        "related": [],
        "prompt": "이 추천의 최신성, 지지·상충 근거와 여행 일정에 넣을 조건을 검토해줘."
    }))
}

async fn plan_summary(lease: InvocationLeaseToken, broker: SharedBroker) -> HostResponse {
    let rows = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("restaurant_recommendations"),
            ["freshness".into()],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let count = |status: &str| {
        rows.rows
            .iter()
            .filter(|row| row.get("freshness").and_then(Value::as_str) == Some(status))
            .count()
    };
    gadget_result(json!({
        "saved recommendations": rows.rows.len(),
        "current": count("current"),
        "needs refresh": count("aging") + count("stale"),
        "conflicts": count("conflicted")
    }))
}

async fn upsert(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    entity: EntitySpec,
    entity_id: Option<String>,
    revision: Option<i64>,
    mut values: BTreeMap<String, Value>,
) -> HostResponse {
    let is_create = entity_id.is_none();
    let entity_id = match entity_id {
        Some(value) if valid_uuid(&value) => value,
        Some(_) => return invalid("entity id must be a UUID"),
        None => Uuid::new_v4().to_string(),
    };
    let revision = if is_create {
        if revision.is_some() {
            return invalid("revision must be omitted when creating a record");
        }
        1
    } else {
        match revision {
            Some(value) if value > 0 && value < i64::MAX => value,
            _ => return invalid("a positive current revision is required when updating"),
        }
    };
    let timestamp = now();
    values.insert("updated_at".into(), json!(timestamp));
    let affected = if is_create {
        values.insert(entity.id_column.into(), json!(entity_id));
        values.insert("revision".into(), json!(1));
        values.insert("created_at".into(), json!(timestamp));
        insert(
            broker,
            DatabaseInsertRequest::new(
                lease.clone(),
                id(WRITE_PERMISSION),
                table(entity.table),
                values,
            ),
        )
        .await
    } else {
        values.insert("revision".into(), json!(revision + 1));
        update(
            broker,
            DatabaseUpdateRequest::new(
                lease.clone(),
                id(WRITE_PERMISSION),
                table(entity.table),
                values,
                BTreeMap::from([
                    (entity.id_column.into(), json!(entity_id)),
                    ("revision".into(), json!(revision)),
                ]),
            ),
        )
        .await
    };
    match affected {
        Ok(1) => row_result(
            select_one(
                broker,
                lease,
                entity.table,
                entity.id_column,
                &entity_id,
                (entity.columns)(),
            )
            .await,
            "write-verification-failed",
            "the mutation could not be read back",
        ),
        Ok(0) if !is_create => host_error(
            "revision-conflict",
            "the record changed; read its current revision before retrying",
        ),
        Ok(_) => host_error(
            "write-cardinality-invalid",
            "mutation affected an unexpected row count",
        ),
        Err(error) => error,
    }
}

async fn select_one(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    table_name: &str,
    id_column: &str,
    entity_id: &str,
    columns: Vec<String>,
) -> Result<Option<BTreeMap<String, Value>>, HostResponse> {
    select(
        broker,
        DatabaseSelectRequest::new(lease, id(READ_PERMISSION), table(table_name), columns)
            .with_filter(id_column, json!(entity_id))
            .with_limit(1),
    )
    .await
    .map(|rows| rows.rows.into_iter().next())
}

async fn query_rows(broker: &SharedBroker, request: DatabaseSelectRequest) -> HostResponse {
    match select(broker, request).await {
        Ok(rows) => rows_result(rows),
        Err(error) => error,
    }
}

async fn select(
    broker: &SharedBroker,
    request: DatabaseSelectRequest,
) -> Result<DatabaseRows, HostResponse> {
    broker
        .lock()
        .await
        .database_select(request)
        .await
        .map_err(broker_error)
}

async fn insert(
    broker: &SharedBroker,
    request: DatabaseInsertRequest,
) -> Result<u32, HostResponse> {
    broker
        .lock()
        .await
        .database_insert(request)
        .await
        .map(|result| result.affected_rows)
        .map_err(broker_error)
}

async fn update(
    broker: &SharedBroker,
    request: DatabaseUpdateRequest,
) -> Result<u32, HostResponse> {
    broker
        .lock()
        .await
        .database_update(request)
        .await
        .map(|result| result.affected_rows)
        .map_err(broker_error)
}

fn row_result(
    result: Result<Option<BTreeMap<String, Value>>, HostResponse>,
    code: &str,
    message: &str,
) -> HostResponse {
    match result {
        Ok(Some(row)) => gadget_result(json!(row)),
        Ok(None) => host_error(code, message),
        Err(error) => error,
    }
}

fn rows_result(rows: DatabaseRows) -> HostResponse {
    gadget_result(json!({"count": rows.rows.len(), "rows": rows.rows, "truncated": rows.truncated}))
}

fn list_limit(input: Value, maximum: u32) -> Result<u32, &'static str> {
    let input: ListInput = serde_json::from_value(input).map_err(|_| "list input is invalid")?;
    (1..=maximum)
        .contains(&input.limit)
        .then_some(input.limit)
        .ok_or("limit is outside the signed bound")
}

fn valid_source(source_id: &str, source_revision: i64, observed_at: &str) -> bool {
    valid_uuid(source_id) && source_revision > 0 && valid_time(observed_at)
}

fn valid_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}

fn valid_time(value: &str) -> bool {
    DateTime::parse_from_rfc3339(value).is_ok()
}

fn valid_currency(value: &str) -> bool {
    value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase())
}

fn bounded(value: &str, minimum: usize, maximum: usize) -> bool {
    let length = value.chars().count();
    (minimum..=maximum).contains(&length) && !value.contains('\0')
}

fn option_json<T: serde::Serialize>(value: Option<T>) -> Value {
    value.map_or(Value::Null, |value| json!(value))
}

fn invalid(message: &str) -> HostResponse {
    host_error("invalid-arguments", message)
}

fn id(value: &str) -> LocalId {
    LocalId::new(value).expect("static local id is valid")
}

fn table(value: &str) -> BrokerResource {
    BrokerResource::database_table(value).expect("static database table is valid")
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn default_limit() -> u32 {
    100
}

fn columns(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn branch_columns() -> Vec<String> {
    columns(&[
        "branch_id",
        "name",
        "address",
        "cuisine",
        "status",
        "source_id",
        "source_revision",
        "observed_at",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn menu_columns() -> Vec<String> {
    columns(&[
        "menu_item_id",
        "branch_id",
        "name",
        "category",
        "price_minor",
        "currency",
        "dietary_notes",
        "allergen_notes",
        "source_id",
        "source_revision",
        "observed_at",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn review_columns() -> Vec<String> {
    columns(&[
        "review_id",
        "branch_id",
        "source_name",
        "passage",
        "bias_context",
        "sentiment",
        "source_id",
        "source_revision",
        "captured_at",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn recommendation_columns() -> Vec<String> {
    columns(&[
        "recommendation_id",
        "branch_id",
        "query",
        "reason",
        "conditions",
        "freshness",
        "supporting_source_id",
        "supporting_source_revision",
        "contradicting_source_id",
        "contradicting_source_revision",
        "valid_at",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn outcome_columns() -> Vec<String> {
    columns(&[
        "outcome_id",
        "recommendation_id",
        "visited_at",
        "result",
        "feedback",
        "actual_cost_minor",
        "currency",
        "revision",
        "created_at",
        "updated_at",
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_and_currency_contracts_are_bounded() {
        assert!(valid_source(
            "11111111-1111-1111-1111-111111111111",
            1,
            "2026-07-12T00:00:00Z"
        ));
        assert!(!valid_source("not-an-id", 1, "2026-07-12T00:00:00Z"));
        assert!(valid_currency("KRW"));
        assert!(!valid_currency("krw"));
    }
}
