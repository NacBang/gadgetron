use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use gadgetron_bundle_sdk::{
    BrokerResource, BundleId, CapabilityId, CitationUseRef, ContextUseRef, DatabaseDeleteRequest,
    DatabaseInsertRequest, DatabaseOrderDirection, DatabaseRows, DatabaseSelectRequest,
    DatabaseUpdateRequest, GadgetInvocation, HostResponse, IntelligenceBudget,
    IntelligenceContextRequest, IntelligenceQueryDraft, InvocationLeaseToken, LocalId,
    ObservedOutcome, OutcomeFeedbackDraft, OutcomeFeedbackRequest, OutcomeObservation,
    OutcomePredicateResult, SubjectRevisionRef,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{broker_error, gadget_result, host_error, SharedBroker};

const READ_PERMISSION: &str = "travel-read";
const WRITE_PERMISSION: &str = "travel-write";
const KNOWLEDGE_READ_PERMISSION: &str = "travel-knowledge-read";
const KNOWLEDGE_FEEDBACK_PERMISSION: &str = "travel-knowledge-feedback";

pub(crate) async fn invoke(invocation: GadgetInvocation, broker: SharedBroker) -> HostResponse {
    let Some(lease) = invocation.context.broker_lease else {
        return host_error(
            "broker-lease-required",
            "Core did not attach an invocation-scoped broker lease",
        );
    };
    let actor_ref = invocation.context.actor_id.clone();
    match invocation.gadget.as_str() {
        "travel.trip-list" => trip_list(invocation.input, lease, broker).await,
        "travel.trip-get" => trip_get(invocation.input, lease, broker).await,
        "travel.trip-upsert" => trip_upsert(invocation.input, lease, broker).await,
        "travel.trip-delete" => {
            delete_entity(invocation.input, lease, broker, "travel_trips", "trip_id").await
        }
        "travel.itinerary-list" => itinerary_list(invocation.input, lease, broker).await,
        "travel.itinerary-item-upsert" => itinerary_upsert(invocation.input, lease, broker).await,
        "travel.itinerary-item-delete" => {
            delete_entity(
                invocation.input,
                lease,
                broker,
                "travel_itinerary_items",
                "item_id",
            )
            .await
        }
        "travel.constraint-list" => constraint_list(invocation.input, lease, broker).await,
        "travel.constraint-upsert" => constraint_upsert(invocation.input, lease, broker).await,
        "travel.constraint-delete" => {
            delete_entity(
                invocation.input,
                lease,
                broker,
                "travel_constraints",
                "constraint_id",
            )
            .await
        }
        "travel.budget-summary" => budget_summary(invocation.input, lease, broker).await,
        "travel.budget-item-upsert" => budget_upsert(invocation.input, lease, broker).await,
        "travel.budget-item-delete" => {
            delete_entity(
                invocation.input,
                lease,
                broker,
                "travel_budget_items",
                "budget_item_id",
            )
            .await
        }
        "travel.plan-summary" => plan_summary(lease, broker).await,
        "travel.export" => export_trip(invocation.input, lease, broker).await,
        "travel.knowledge-context" => knowledge_context(invocation.input, lease, broker).await,
        "travel.restaurant-attach" => restaurant_attach(invocation.input, lease, broker).await,
        "travel.restaurant-bridge-list" => {
            restaurant_bridge_list(invocation.input, lease, broker).await
        }
        "travel.disruption-record" => disruption_record(invocation.input, lease, broker).await,
        "travel.monitor-plan" => monitor_plan(invocation.input, lease, broker).await,
        "travel.propose-replan" => propose_replan(invocation.input, lease, broker).await,
        "travel.apply-replan" => apply_replan(invocation.input, actor_ref, lease, broker).await,
        "travel.rollback-replan" => {
            rollback_replan(invocation.input, actor_ref, lease, broker).await
        }
        "travel.operation-outcomes-list" => {
            operation_outcomes_list(invocation.input, lease, broker).await
        }
        _ => host_error(
            "capability-not-found",
            "requested Travel Planner capability is not available",
        ),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListInput {
    #[serde(default)]
    trip_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdInput {
    trip_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteInput {
    id: String,
    revision: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TripUpsertInput {
    #[serde(default)]
    trip_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    title: String,
    origin: String,
    start_date: String,
    end_date: String,
    timezone: String,
    traveler_count: i64,
    status: String,
    currency: String,
    budget_amount_minor: i64,
    #[serde(default)]
    notes: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItineraryUpsertInput {
    #[serde(default)]
    item_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    trip_id: String,
    title: String,
    kind: String,
    starts_at: String,
    ends_at: String,
    timezone: String,
    place: String,
    status: String,
    #[serde(default)]
    notes: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConstraintUpsertInput {
    #[serde(default)]
    constraint_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    trip_id: String,
    strength: String,
    scope: String,
    rule_text: String,
    #[serde(default)]
    provenance: String,
    conflict_status: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetUpsertInput {
    #[serde(default)]
    budget_item_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    trip_id: String,
    category: String,
    label: String,
    quoted_amount_minor: i64,
    #[serde(default)]
    actual_amount_minor: Option<i64>,
    currency: String,
    observed_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportInput {
    trip_id: String,
    format: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestaurantAttachInput {
    trip_id: String,
    #[serde(default)]
    trip_revision: Option<i64>,
    starts_at: String,
    ends_at: String,
    timezone: String,
    recommendation_id: String,
    recommendation_revision: i64,
    branch_id: String,
    title: String,
    place: String,
    reason: String,
    freshness: String,
    supporting_source_id: String,
    supporting_source_revision: i64,
    #[serde(default)]
    contradicting_source_id: Option<String>,
    #[serde(default)]
    contradicting_source_revision: Option<i64>,
    #[serde(default)]
    context_query_id: Option<String>,
    #[serde(default)]
    context_revision: Option<String>,
    #[serde(default)]
    used_citation_id: Option<String>,
    #[serde(default)]
    used_source_revision: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KnowledgeContextInput {
    trip_id: String,
    trip_revision: i64,
    question: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DisruptionRecordInput {
    trip_id: String,
    #[serde(default)]
    affected_item_id: Option<String>,
    kind: String,
    severity: String,
    summary: String,
    impact: String,
    source_ref: String,
    observed_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplanProposalInput {
    disruption_id: String,
    item_id: String,
    expected_item_revision: i64,
    reason: String,
    evidence_ref: String,
    title: String,
    starts_at: String,
    ends_at: String,
    timezone: String,
    place: String,
    status: String,
    cost_change_minor: i64,
    booking_impact: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalIdInput {
    proposal_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplanRollbackInput {
    proposal_id: String,
    operation_id: String,
}

async fn trip_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input = match parse_list(input) {
        Ok(input) if input.trip_id.is_none() => input,
        _ => return invalid("trip-list accepts only an optional bounded limit"),
    };
    match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("travel_trips"),
            trip_columns(),
        )
        .with_order("start_date", DatabaseOrderDirection::Ascending)
        .with_order("trip_id", DatabaseOrderDirection::Ascending)
        .with_limit(input.limit),
    )
    .await
    {
        Ok(rows) => rows_result(rows),
        Err(error) => error,
    }
}

async fn trip_get(input: Value, lease: InvocationLeaseToken, broker: SharedBroker) -> HostResponse {
    let input: IdInput = match serde_json::from_value::<IdInput>(input) {
        Ok(input) if uuid(&input.trip_id).is_ok() => input,
        _ => return invalid("trip_id must be a UUID"),
    };
    match get_trip_row(&broker, lease, &input.trip_id).await {
        Ok(Some(row)) => gadget_result(json!(row)),
        Ok(None) => host_error("trip-not-found", "the requested Trip does not exist"),
        Err(error) => error,
    }
}

async fn trip_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: TripUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Trip fields do not match the signed schema"),
    };
    if let Err(message) = validate_trip(&input) {
        return invalid(message);
    }
    let now = now();
    let is_create = input.trip_id.is_none();
    let trip_id = match input.trip_id.as_deref() {
        Some(value) => match uuid(value) {
            Ok(value) => value.to_string(),
            Err(message) => return invalid(message),
        },
        None => Uuid::new_v4().to_string(),
    };
    let revision = if is_create {
        if input.revision.is_some() {
            return invalid("revision must be omitted when creating a Trip");
        }
        1
    } else {
        match valid_revision(input.revision) {
            Ok(revision) => revision,
            Err(message) => return invalid(message),
        }
    };
    let mut values = BTreeMap::from([
        ("title".into(), json!(input.title)),
        ("origin".into(), json!(input.origin)),
        ("start_date".into(), json!(input.start_date)),
        ("end_date".into(), json!(input.end_date)),
        ("timezone".into(), json!(input.timezone)),
        ("traveler_count".into(), json!(input.traveler_count)),
        ("status".into(), json!(input.status)),
        ("currency".into(), json!(input.currency)),
        (
            "budget_amount_minor".into(),
            json!(input.budget_amount_minor),
        ),
        ("notes".into(), json!(input.notes)),
        ("updated_at".into(), json!(now)),
    ]);
    let affected = if is_create {
        values.insert("trip_id".into(), json!(trip_id));
        values.insert("revision".into(), json!(1));
        values.insert("created_at".into(), json!(now));
        insert(
            &broker,
            DatabaseInsertRequest::new(
                lease.clone(),
                id(WRITE_PERMISSION),
                table("travel_trips"),
                values,
            ),
        )
        .await
    } else {
        values.insert("revision".into(), json!(revision + 1));
        update(
            &broker,
            DatabaseUpdateRequest::new(
                lease.clone(),
                id(WRITE_PERMISSION),
                table("travel_trips"),
                values,
                BTreeMap::from([
                    ("trip_id".into(), json!(trip_id)),
                    ("revision".into(), json!(revision)),
                ]),
            ),
        )
        .await
    };
    match affected {
        Ok(1) => match get_trip_row(&broker, lease, &trip_id).await {
            Ok(Some(row)) => gadget_result(json!(row)),
            Ok(None) => host_error(
                "write-verification-failed",
                "Trip mutation could not be read back",
            ),
            Err(error) => error,
        },
        Ok(0) if !is_create => revision_conflict(),
        Ok(_) => host_error(
            "write-cardinality-invalid",
            "Trip mutation affected an unexpected row count",
        ),
        Err(error) => error,
    }
}

async fn itinerary_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input = match parse_list(input) {
        Ok(input) => input,
        Err(message) => return invalid(message),
    };
    let mut request = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table("travel_itinerary_items"),
        itinerary_columns(),
    )
    .with_order("starts_at", DatabaseOrderDirection::Ascending)
    .with_order("item_id", DatabaseOrderDirection::Ascending)
    .with_limit(input.limit);
    if let Some(trip_id) = input.trip_id.as_deref() {
        if uuid(trip_id).is_err() {
            return invalid("trip_id must be a UUID");
        }
        request = request.with_filter("trip_id", json!(trip_id));
    }
    let mut rows = match select(&broker, request).await {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let trips = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("travel_trips"),
            ["trip_id".into(), "title".into()],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let titles: HashMap<String, Value> = trips
        .rows
        .into_iter()
        .filter_map(|row| {
            Some((
                row.get("trip_id")?.as_str()?.to_string(),
                row.get("title")?.clone(),
            ))
        })
        .collect();
    for row in &mut rows.rows {
        let title = row
            .get("trip_id")
            .and_then(Value::as_str)
            .and_then(|id| titles.get(id))
            .cloned()
            .unwrap_or_else(|| json!("Unavailable Trip"));
        row.insert("trip_title".into(), title);
        if let Some(start) = row.get("starts_at").cloned() {
            row.insert("start".into(), start);
        }
    }
    rows_result(rows)
}

async fn itinerary_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ItineraryUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Itinerary fields do not match the signed schema"),
    };
    if let Err(message) = validate_itinerary(&input) {
        return invalid(message);
    }
    let now = now();
    let is_create = input.item_id.is_none();
    let item_id = match entity_id(input.item_id.as_deref(), is_create) {
        Ok(id) => id,
        Err(message) => return invalid(message),
    };
    let revision = match mutation_revision(input.revision, is_create) {
        Ok(revision) => revision,
        Err(message) => return invalid(message),
    };
    let mut values = BTreeMap::from([
        ("trip_id".into(), json!(input.trip_id)),
        ("title".into(), json!(input.title)),
        ("kind".into(), json!(input.kind)),
        ("starts_at".into(), json!(input.starts_at)),
        ("ends_at".into(), json!(input.ends_at)),
        ("timezone".into(), json!(input.timezone)),
        ("place".into(), json!(input.place)),
        ("status".into(), json!(input.status)),
        ("notes".into(), json!(input.notes)),
        ("updated_at".into(), json!(now)),
    ]);
    let result = mutate_entity(
        &broker,
        lease,
        "travel_itinerary_items",
        "item_id",
        &item_id,
        revision,
        is_create,
        &now,
        &mut values,
    )
    .await;
    mutation_result(result, is_create, "item_id", &item_id, revision)
}

async fn knowledge_context(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: KnowledgeContextInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Trip, revision and question do not match the signed schema"),
    };
    if uuid(&input.trip_id).is_err()
        || input.trip_revision <= 0
        || !bounded(&input.question, 2, 2048)
    {
        return invalid("Trip context requires a UUID, positive revision and bounded question");
    }
    match get_trip_row(&broker, lease.clone(), &input.trip_id).await {
        Ok(Some(row))
            if row.get("revision").and_then(Value::as_i64) == Some(input.trip_revision) => {}
        Ok(Some(_)) => {
            return host_error(
                "trip-revision-conflict",
                "the Trip changed before knowledge context was requested",
            )
        }
        Ok(None) => return host_error("trip-not-found", "the requested Trip does not exist"),
        Err(error) => return error,
    }
    let subject = match SubjectRevisionRef::new(
        BundleId::new("travel-planner").expect("static Bundle id is valid"),
        CapabilityId::new("travel.trip").expect("static subject kind is valid"),
        input.trip_id,
        input.trip_revision.to_string(),
    ) {
        Ok(subject) => subject,
        Err(_) => return invalid("Trip subject revision is invalid"),
    };
    let budget = IntelligenceBudget::new(8, 100, 65_536, 8_000, 10)
        .expect("fixed Travel context budget is valid");
    let draft = match IntelligenceQueryDraft::new(
        Uuid::new_v4().to_string(),
        subject,
        input.question,
        60 * 60 * 24 * 30,
        budget,
    ) {
        Ok(draft) => draft,
        Err(_) => return invalid("Trip knowledge query is invalid"),
    };
    match broker
        .lock()
        .await
        .intelligence_context(IntelligenceContextRequest::new(
            lease,
            id(KNOWLEDGE_READ_PERMISSION),
            draft,
        ))
        .await
    {
        Ok(pack) => gadget_result(
            serde_json::to_value(pack).expect("validated knowledge context serializes"),
        ),
        Err(error) => broker_error(error),
    }
}

async fn restaurant_attach(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: RestaurantAttachInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Restaurant handoff fields do not match the signed schema"),
    };
    let start = match timestamp(&input.starts_at) {
        Ok(value) => value,
        Err(_) => return invalid("starts_at must be RFC 3339"),
    };
    let end = match timestamp(&input.ends_at) {
        Ok(value) => value,
        Err(_) => return invalid("ends_at must be RFC 3339"),
    };
    let context = match (
        input.trip_revision,
        input.context_query_id.as_ref(),
        input.context_revision.as_ref(),
        input.used_citation_id.as_ref(),
        input.used_source_revision.as_ref(),
    ) {
        (None, None, None, None, None) => None,
        (
            Some(revision),
            Some(query_id),
            Some(context_revision),
            Some(citation_id),
            Some(source_revision),
        ) if revision > 0
            && bounded(query_id, 1, 256)
            && bounded(context_revision, 1, 256)
            && bounded(citation_id, 1, 256)
            && bounded(source_revision, 1, 256) =>
        {
            Some((
                revision,
                query_id.clone(),
                context_revision.clone(),
                citation_id.clone(),
                source_revision.clone(),
            ))
        }
        _ => {
            return invalid(
                "Trip revision and all knowledge context references must be supplied together",
            )
        }
    };
    if uuid(&input.trip_id).is_err()
        || uuid(&input.recommendation_id).is_err()
        || uuid(&input.branch_id).is_err()
        || uuid(&input.supporting_source_id).is_err()
        || input
            .contradicting_source_id
            .as_deref()
            .is_some_and(|value| uuid(value).is_err())
        || input.recommendation_revision <= 0
        || input.supporting_source_revision <= 0
        || input.contradicting_source_id.is_some() != input.contradicting_source_revision.is_some()
        || input
            .contradicting_source_revision
            .is_some_and(|value| value <= 0)
        || end <= start
        || input.timezone.parse::<chrono_tz::Tz>().is_err()
        || !bounded(&input.title, 1, 200)
        || !bounded(&input.place, 1, 300)
        || !bounded(&input.reason, 1, 1000)
        || !matches!(
            input.freshness.as_str(),
            "current" | "aging" | "stale" | "conflicted"
        )
    {
        return invalid("Restaurant handoff violates the signed bridge contract");
    }
    let item_id = Uuid::new_v4().to_string();
    let now = now();
    let snapshot = json!({
        "title": input.title,
        "place": input.place,
        "reason": input.reason,
        "freshness": input.freshness,
    });
    let mut values = BTreeMap::from([
        ("trip_id".into(), json!(input.trip_id)),
        ("title".into(), snapshot["title"].clone()),
        ("kind".into(), json!("meal")),
        ("starts_at".into(), json!(input.starts_at)),
        ("ends_at".into(), json!(input.ends_at)),
        ("timezone".into(), json!(input.timezone)),
        ("place".into(), snapshot["place"].clone()),
        ("status".into(), json!("proposed")),
        ("notes".into(), snapshot["reason"].clone()),
        ("external_owner_bundle".into(), json!("restaurant-research")),
        ("external_entity_id".into(), json!(input.recommendation_id)),
        (
            "external_entity_revision".into(),
            json!(input.recommendation_revision),
        ),
        ("external_branch_id".into(), json!(input.branch_id)),
        ("external_snapshot".into(), snapshot.clone()),
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
            input
                .contradicting_source_id
                .map_or(Value::Null, |value| json!(value)),
        ),
        (
            "contradicting_source_revision".into(),
            input
                .contradicting_source_revision
                .map_or(Value::Null, |value| json!(value)),
        ),
        ("updated_at".into(), json!(now)),
    ]);
    let result = mutate_entity(
        &broker,
        lease.clone(),
        "travel_itinerary_items",
        "item_id",
        &item_id,
        1,
        true,
        &now,
        &mut values,
    )
    .await;
    match result {
        Ok(1) => {
            let experience = if let Some((
                trip_revision,
                query_id,
                context_revision,
                citation_id,
                source_revision,
            )) = context
            {
                let subject = SubjectRevisionRef::new(
                    BundleId::new("travel-planner").expect("static Bundle id is valid"),
                    CapabilityId::new("travel.trip").expect("static subject kind is valid"),
                    input.trip_id,
                    trip_revision.to_string(),
                )
                .expect("validated Travel subject is valid");
                let draft = OutcomeFeedbackDraft::new(
                    format!("restaurant-attach-{item_id}"),
                    subject,
                    item_id.clone(),
                    Some(ContextUseRef::new(query_id, context_revision)),
                    json!({}),
                    json!({
                        "item_id": item_id,
                        "revision": 1,
                        "status": "proposed",
                        "restaurant": snapshot,
                    }),
                    OutcomePredicateResult::Satisfied,
                    "Restaurant recommendation was added to the itinerary and persisted",
                    vec![CitationUseRef::new(citation_id, source_revision)],
                );
                match broker
                    .lock()
                    .await
                    .outcome_feedback(OutcomeFeedbackRequest::new(
                        lease,
                        id(KNOWLEDGE_FEEDBACK_PERMISSION),
                        draft,
                    ))
                    .await
                {
                    Ok(receipt) => json!({
                        "state": "recorded",
                        "revision": receipt.experience_revision,
                        "duplicate": receipt.duplicate,
                    }),
                    Err(error) => json!({
                        "state": "not_recorded",
                        "reason": error.public_message(),
                    }),
                }
            } else {
                json!({"state": "not_requested"})
            };
            gadget_result(json!({
                "item_id": item_id,
                "revision": 1,
                "created": true,
                "experience": experience,
            }))
        }
        Ok(_) => host_error(
            "write-cardinality-invalid",
            "restaurant attachment affected an unexpected row count",
        ),
        Err(error) => error,
    }
}

async fn restaurant_bridge_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input = match parse_list(input) {
        Ok(input) => input,
        Err(message) => return invalid(message),
    };
    let mut request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("travel_itinerary_items"),
        itinerary_columns(),
    )
    .with_filter("external_owner_bundle", json!("restaurant-research"))
    .with_order("starts_at", DatabaseOrderDirection::Ascending)
    .with_limit(input.limit);
    if let Some(trip_id) = input.trip_id {
        if uuid(&trip_id).is_err() {
            return invalid("trip_id must be a UUID");
        }
        request = request.with_filter("trip_id", json!(trip_id));
    }
    match select(&broker, request).await {
        Ok(rows) => rows_result(rows),
        Err(error) => error,
    }
}

async fn constraint_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    list_for_trip(
        input,
        lease,
        broker,
        "travel_constraints",
        [
            "constraint_id",
            "trip_id",
            "strength",
            "scope",
            "rule_text",
            "provenance",
            "conflict_status",
            "revision",
            "created_at",
            "updated_at",
        ],
        "strength",
    )
    .await
}

async fn constraint_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ConstraintUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Constraint fields do not match the signed schema"),
    };
    if uuid(&input.trip_id).is_err()
        || !matches!(input.strength.as_str(), "hard" | "soft")
        || !matches!(
            input.conflict_status.as_str(),
            "clear" | "potential" | "violated" | "resolved"
        )
        || !bounded(&input.scope, 1, 100)
        || !bounded(&input.rule_text, 1, 1000)
        || !bounded(&input.provenance, 0, 1000)
    {
        return invalid("Constraint values violate the signed domain contract");
    }
    let now = now();
    let is_create = input.constraint_id.is_none();
    let entity_id = match entity_id(input.constraint_id.as_deref(), is_create) {
        Ok(id) => id,
        Err(message) => return invalid(message),
    };
    let revision = match mutation_revision(input.revision, is_create) {
        Ok(revision) => revision,
        Err(message) => return invalid(message),
    };
    let mut values = BTreeMap::from([
        ("trip_id".into(), json!(input.trip_id)),
        ("strength".into(), json!(input.strength)),
        ("scope".into(), json!(input.scope)),
        ("rule_text".into(), json!(input.rule_text)),
        ("provenance".into(), json!(input.provenance)),
        ("conflict_status".into(), json!(input.conflict_status)),
        ("updated_at".into(), json!(now)),
    ]);
    let result = mutate_entity(
        &broker,
        lease,
        "travel_constraints",
        "constraint_id",
        &entity_id,
        revision,
        is_create,
        &now,
        &mut values,
    )
    .await;
    mutation_result(result, is_create, "constraint_id", &entity_id, revision)
}

async fn budget_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: BudgetUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Budget fields do not match the signed schema"),
    };
    if uuid(&input.trip_id).is_err()
        || !matches!(
            input.category.as_str(),
            "transport" | "lodging" | "food" | "activity" | "fees" | "other"
        )
        || !bounded(&input.label, 1, 200)
        || input.quoted_amount_minor < 0
        || input.actual_amount_minor.is_some_and(|value| value < 0)
        || !currency(&input.currency)
        || timestamp(&input.observed_at).is_err()
    {
        return invalid("Budget values violate the signed domain contract");
    }
    let now = now();
    let is_create = input.budget_item_id.is_none();
    let entity_id = match entity_id(input.budget_item_id.as_deref(), is_create) {
        Ok(id) => id,
        Err(message) => return invalid(message),
    };
    let revision = match mutation_revision(input.revision, is_create) {
        Ok(revision) => revision,
        Err(message) => return invalid(message),
    };
    let mut values = BTreeMap::from([
        ("trip_id".into(), json!(input.trip_id)),
        ("category".into(), json!(input.category)),
        ("label".into(), json!(input.label)),
        (
            "quoted_amount_minor".into(),
            json!(input.quoted_amount_minor),
        ),
        (
            "actual_amount_minor".into(),
            input
                .actual_amount_minor
                .map_or(Value::Null, |value| json!(value)),
        ),
        ("currency".into(), json!(input.currency)),
        ("observed_at".into(), json!(input.observed_at)),
        ("updated_at".into(), json!(now)),
    ]);
    let result = mutate_entity(
        &broker,
        lease,
        "travel_budget_items",
        "budget_item_id",
        &entity_id,
        revision,
        is_create,
        &now,
        &mut values,
    )
    .await;
    mutation_result(result, is_create, "budget_item_id", &entity_id, revision)
}

async fn budget_summary(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input = match parse_list(input) {
        Ok(input) => input,
        Err(message) => return invalid(message),
    };
    let mut request = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table("travel_budget_items"),
        [
            "budget_item_id".into(),
            "trip_id".into(),
            "category".into(),
            "label".into(),
            "quoted_amount_minor".into(),
            "actual_amount_minor".into(),
            "currency".into(),
            "observed_at".into(),
            "revision".into(),
        ],
    )
    .with_order("observed_at", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(trip_id) = input.trip_id.as_deref() {
        if uuid(trip_id).is_err() {
            return invalid("trip_id must be a UUID");
        }
        request = request.with_filter("trip_id", json!(trip_id));
    }
    let rows = match select(&broker, request).await {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let mut totals: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for row in &rows.rows {
        let Some(currency) = row.get("currency").and_then(Value::as_str) else {
            continue;
        };
        let quoted = number(row.get("quoted_amount_minor"));
        let actual = number(row.get("actual_amount_minor"));
        let entry = totals.entry(currency.to_string()).or_default();
        entry.0 = entry.0.saturating_add(quoted);
        entry.1 = entry.1.saturating_add(actual);
    }
    let totals: Vec<Value> = totals
        .into_iter()
        .map(|(currency, (quoted, actual))| {
            json!({
                "currency": currency,
                "quoted_amount_minor": quoted,
                "actual_amount_minor": actual,
            })
        })
        .collect();
    gadget_result(json!({
        "trip_id": input.trip_id,
        "totals": totals,
        "items": rows.rows,
        "count": rows.rows.len(),
        "truncated": rows.truncated,
    }))
}

async fn plan_summary(lease: InvocationLeaseToken, broker: SharedBroker) -> HostResponse {
    let trips = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("travel_trips"),
            [
                "trip_id".into(),
                "title".into(),
                "start_date".into(),
                "status".into(),
                "currency".into(),
                "budget_amount_minor".into(),
            ],
        )
        .with_order("start_date", DatabaseOrderDirection::Ascending)
        .with_limit(200),
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let items = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("travel_itinerary_items"),
            [
                "item_id".into(),
                "trip_id".into(),
                "title".into(),
                "starts_at".into(),
                "timezone".into(),
                "status".into(),
            ],
        )
        .with_order("starts_at", DatabaseOrderDirection::Ascending)
        .with_limit(200),
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let today = Utc::now().date_naive().to_string();
    let next_trip = trips.rows.iter().find(|row| {
        row.get("start_date")
            .and_then(Value::as_str)
            .is_some_and(|date| date >= today.as_str())
            && !matches!(
                row.get("status").and_then(Value::as_str),
                Some("cancelled" | "completed")
            )
    });
    let current = Utc::now();
    let next_item = items.rows.iter().find(|row| {
        row.get("starts_at")
            .and_then(Value::as_str)
            .and_then(|value| timestamp(value).ok())
            .is_some_and(|at| at > current)
            && row.get("status").and_then(Value::as_str) != Some("cancelled")
    });
    gadget_result(json!({
        "trip_count": trips.rows.len(),
        "upcoming_trip_count": trips.rows.iter().filter(|row| row.get("start_date").and_then(Value::as_str).is_some_and(|date| date >= today.as_str())).count(),
        "next_trip": next_trip.and_then(|row| row.get("title")).cloned().unwrap_or(Value::Null),
        "next_trip_start": next_trip.and_then(|row| row.get("start_date")).cloned().unwrap_or(Value::Null),
        "next_item": next_item.and_then(|row| row.get("title")).cloned().unwrap_or(Value::Null),
        "next_item_start": next_item.and_then(|row| row.get("starts_at")).cloned().unwrap_or(Value::Null),
    }))
}

async fn export_trip(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ExportInput = match serde_json::from_value::<ExportInput>(input) {
        Ok(input)
            if uuid(&input.trip_id).is_ok()
                && matches!(input.format.as_str(), "markdown" | "json" | "ical") =>
        {
            input
        }
        _ => return invalid("trip_id and format (markdown, json, ical) are required"),
    };
    let trip = match get_trip_row(&broker, lease.clone(), &input.trip_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return host_error("trip-not-found", "the requested Trip does not exist"),
        Err(error) => return error,
    };
    let items = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("travel_itinerary_items"),
            itinerary_columns(),
        )
        .with_filter("trip_id", json!(input.trip_id))
        .with_order("starts_at", DatabaseOrderDirection::Ascending)
        .with_limit(200),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let revision = trip.get("revision").cloned().unwrap_or_else(|| json!(1));
    let slug = filename_slug(trip.get("title").and_then(Value::as_str).unwrap_or("trip"));
    let (extension, media_type, content) = match input.format.as_str() {
        "markdown" => ("md", "text/markdown", markdown_export(&trip, &items)),
        "json" => (
            "json",
            "application/json",
            serde_json::to_string_pretty(&json!({"trip": trip, "itinerary": items}))
                .expect("bounded JSON export serializes"),
        ),
        "ical" => ("ics", "text/calendar", ical_export(&trip, &items)),
        _ => unreachable!(),
    };
    gadget_result(json!({
        "format": input.format,
        "filename": format!("{slug}.{extension}"),
        "media_type": media_type,
        "trip_revision": revision,
        "content": content,
    }))
}

async fn disruption_record(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: DisruptionRecordInput = match serde_json::from_value::<DisruptionRecordInput>(input)
    {
        Ok(input) => input,
        Err(_) => return invalid("Disruption fields do not match the signed schema"),
    };
    if uuid(&input.trip_id).is_err()
        || input
            .affected_item_id
            .as_deref()
            .is_some_and(|value| uuid(value).is_err())
        || !matches!(
            input.kind.as_str(),
            "delay" | "cancellation" | "closure" | "advisory" | "availability" | "other"
        )
        || !matches!(
            input.severity.as_str(),
            "low" | "medium" | "high" | "critical"
        )
        || !bounded(&input.summary, 1, 500)
        || !bounded(&input.impact, 1, 1000)
        || !bounded(&input.source_ref, 1, 1000)
        || timestamp(&input.observed_at).is_err()
    {
        return invalid("Disruption values violate the signed domain contract");
    }
    match get_trip_row(&broker, lease.clone(), &input.trip_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return host_error("trip-not-found", "the selected Trip does not exist"),
        Err(error) => return error,
    }
    if let Some(item_id) = input.affected_item_id.as_deref() {
        match get_itinerary_row(&broker, lease.clone(), item_id).await {
            Ok(Some(row))
                if row.get("trip_id").and_then(Value::as_str) == Some(input.trip_id.as_str()) => {}
            Ok(_) => {
                return host_error(
                    "itinerary-item-not-found",
                    "the affected itinerary item is not part of this Trip",
                )
            }
            Err(error) => return error,
        }
    }
    let disruption_id = Uuid::new_v4().to_string();
    let now = now();
    match insert(
        &broker,
        DatabaseInsertRequest::new(
            lease,
            id(WRITE_PERMISSION),
            table("travel_disruptions"),
            BTreeMap::from([
                ("disruption_id".into(), json!(disruption_id)),
                ("trip_id".into(), json!(input.trip_id)),
                (
                    "affected_item_id".into(),
                    input
                        .affected_item_id
                        .map_or(Value::Null, |value| json!(value)),
                ),
                ("kind".into(), json!(input.kind)),
                ("severity".into(), json!(input.severity)),
                ("summary".into(), json!(input.summary)),
                ("impact".into(), json!(input.impact)),
                ("source_ref".into(), json!(input.source_ref)),
                ("observed_at".into(), json!(input.observed_at)),
                ("state".into(), json!("open")),
                ("revision".into(), json!(1)),
                ("created_at".into(), json!(now)),
                ("updated_at".into(), json!(now)),
            ]),
        ),
    )
    .await
    {
        Ok(1) => gadget_result(json!({
            "disruption_id": disruption_id,
            "state": "open",
            "revision": 1,
        })),
        Ok(_) => host_error(
            "write-cardinality-invalid",
            "disruption record affected an unexpected row count",
        ),
        Err(error) => error,
    }
}

async fn monitor_plan(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input = match parse_list(input) {
        Ok(input) => input,
        Err(message) => return invalid(message),
    };
    let mut disruption_request = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table("travel_disruptions"),
        [
            "disruption_id".into(),
            "trip_id".into(),
            "affected_item_id".into(),
            "kind".into(),
            "severity".into(),
            "summary".into(),
            "impact".into(),
            "observed_at".into(),
            "state".into(),
            "revision".into(),
        ],
    )
    .with_filter("state", json!("open"))
    .with_order("observed_at", DatabaseOrderDirection::Descending)
    .with_limit(input.limit.min(200));
    if let Some(trip_id) = input.trip_id.as_deref() {
        if uuid(trip_id).is_err() {
            return invalid("trip_id must be a UUID");
        }
        disruption_request = disruption_request.with_filter("trip_id", json!(trip_id));
    }
    let disruptions = match select(&broker, disruption_request).await {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let replans = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("travel_replans"),
            [
                "proposal_id".into(),
                "disruption_id".into(),
                "item_id".into(),
                "status".into(),
                "reason".into(),
                "booking_impact".into(),
                "cost_change_minor".into(),
                "operation_id".into(),
                "created_at".into(),
            ],
        )
        .with_order("created_at", DatabaseOrderDirection::Descending)
        .with_limit(200),
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let items = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("travel_itinerary_items"),
            ["item_id".into(), "title".into(), "place".into()],
        )
        .with_order("starts_at", DatabaseOrderDirection::Ascending)
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let latest_replan: HashMap<String, &BTreeMap<String, Value>> = replans
        .rows
        .iter()
        .filter_map(|row| {
            row.get("disruption_id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), row))
        })
        .fold(HashMap::new(), |mut map, (id, row)| {
            map.entry(id).or_insert(row);
            map
        });
    let item_labels: HashMap<String, (&str, &str)> = items
        .rows
        .iter()
        .filter_map(|row| {
            Some((
                row.get("item_id")?.as_str()?.to_string(),
                (
                    row.get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Itinerary item"),
                    row.get("place").and_then(Value::as_str).unwrap_or(""),
                ),
            ))
        })
        .collect();
    let rows = disruptions
        .rows
        .iter()
        .map(|disruption| {
            let disruption_id = disruption
                .get("disruption_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let replan = latest_replan.get(disruption_id).copied();
            let item_id = replan
                .and_then(|row| row.get("item_id"))
                .or_else(|| disruption.get("affected_item_id"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let (item_title, place) = item_labels
                .get(item_id)
                .copied()
                .unwrap_or(("Trip plan", ""));
            let state = replan
                .and_then(|row| row.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("action_required");
            json!({
                "title": disruption.get("summary").cloned().unwrap_or_else(|| json!("Travel change")),
                "status": human_replan_status(state),
                "severity": disruption.get("severity").cloned().unwrap_or_else(|| json!("medium")),
                "impact": disruption.get("impact").cloned().unwrap_or(Value::Null),
                "observed_at": disruption.get("observed_at").cloned().unwrap_or(Value::Null),
                "affected_plan": item_title,
                "place": place,
                "reason": replan.and_then(|row| row.get("reason")).cloned().unwrap_or(Value::Null),
                "booking_impact": replan.and_then(|row| row.get("booking_impact")).cloned().unwrap_or(Value::Null),
                "cost_change_minor": replan.and_then(|row| row.get("cost_change_minor")).cloned().unwrap_or(Value::Null),
                "disruption_id": disruption_id,
                "trip_id": disruption.get("trip_id").cloned().unwrap_or(Value::Null),
                "item_id": if item_id.is_empty() { Value::Null } else { json!(item_id) },
                "proposal_id": replan.and_then(|row| row.get("proposal_id")).cloned().unwrap_or(Value::Null),
                "operation_id": replan.and_then(|row| row.get("operation_id")).cloned().unwrap_or(Value::Null),
            })
        })
        .collect::<Vec<_>>();
    gadget_result(json!({
        "count": rows.len(),
        "rows": rows,
        "truncated": disruptions.truncated || replans.truncated || items.truncated,
    }))
}

async fn propose_replan(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ReplanProposalInput = match serde_json::from_value::<ReplanProposalInput>(input) {
        Ok(input) => input,
        Err(_) => return invalid("Re-plan fields do not match the signed schema"),
    };
    if uuid(&input.disruption_id).is_err()
        || uuid(&input.item_id).is_err()
        || input.expected_item_revision <= 0
        || !bounded(&input.reason, 1, 1000)
        || !bounded(&input.evidence_ref, 1, 1000)
        || !bounded(&input.title, 1, 200)
        || !bounded(&input.place, 1, 300)
        || input.timezone.parse::<chrono_tz::Tz>().is_err()
        || !matches!(
            input.status.as_str(),
            "proposed" | "planned" | "confirmed" | "completed" | "cancelled"
        )
        || !matches!(
            input.booking_impact.as_str(),
            "none" | "manual_change" | "cancellation"
        )
    {
        return invalid("Re-plan values violate the signed domain contract");
    }
    let starts_at = match timestamp(&input.starts_at) {
        Ok(value) => value,
        Err(_) => return invalid("starts_at must be RFC 3339"),
    };
    let _ends_at = match timestamp(&input.ends_at) {
        Ok(value) if value > starts_at => value,
        _ => return invalid("ends_at must be after starts_at"),
    };
    let disruption = match get_disruption_row(&broker, lease.clone(), &input.disruption_id).await {
        Ok(Some(row)) if row.get("state").and_then(Value::as_str) == Some("open") => row,
        Ok(Some(_)) => {
            return host_error(
                "disruption-closed",
                "the disruption no longer needs a plan change",
            )
        }
        Ok(None) => return host_error("disruption-not-found", "the disruption was not found"),
        Err(error) => return error,
    };
    let item = match get_itinerary_row(&broker, lease.clone(), &input.item_id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return host_error(
                "itinerary-item-not-found",
                "the itinerary item was not found",
            )
        }
        Err(error) => return error,
    };
    let trip_id = disruption
        .get("trip_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if item.get("trip_id").and_then(Value::as_str) != Some(trip_id)
        || disruption
            .get("affected_item_id")
            .and_then(Value::as_str)
            .is_some_and(|affected| affected != input.item_id)
        || item.get("revision").and_then(Value::as_i64) != Some(input.expected_item_revision)
    {
        return revision_conflict();
    }
    let before = itinerary_state(&item);
    let proposed = json!({
        "title": input.title,
        "starts_at": input.starts_at,
        "ends_at": input.ends_at,
        "timezone": input.timezone,
        "place": input.place,
        "status": input.status,
        "revision": input.expected_item_revision + 1,
    });
    let proposal_id = Uuid::new_v4().to_string();
    let now = now();
    match insert(
        &broker,
        DatabaseInsertRequest::new(
            lease,
            id(WRITE_PERMISSION),
            table("travel_replans"),
            BTreeMap::from([
                ("proposal_id".into(), json!(proposal_id)),
                ("disruption_id".into(), json!(input.disruption_id)),
                ("trip_id".into(), json!(trip_id)),
                ("item_id".into(), json!(input.item_id)),
                ("status".into(), json!("proposed")),
                ("reason".into(), json!(input.reason)),
                ("evidence_ref".into(), json!(input.evidence_ref)),
                ("booking_impact".into(), json!(input.booking_impact)),
                ("cost_change_minor".into(), json!(input.cost_change_minor)),
                (
                    "expected_item_revision".into(),
                    json!(input.expected_item_revision),
                ),
                ("before_state".into(), before.clone()),
                ("proposed_state".into(), proposed.clone()),
                ("applied_item_revision".into(), Value::Null),
                ("operation_id".into(), Value::Null),
                ("revision".into(), json!(1)),
                ("created_at".into(), json!(now)),
                ("updated_at".into(), json!(now)),
            ]),
        ),
    )
    .await
    {
        Ok(1) => gadget_result(json!({
            "proposal_id": proposal_id,
            "status": "proposed",
            "title": item.get("title").cloned().unwrap_or_else(|| json!("Trip plan")),
            "issue": disruption.get("summary").cloned().unwrap_or_else(|| json!("Travel disruption")),
            "action": "Review revised itinerary",
            "before": before,
            "after": proposed,
            "rollback_available": false,
        })),
        Ok(_) => host_error(
            "write-cardinality-invalid",
            "re-plan proposal affected an unexpected row count",
        ),
        Err(error) => error,
    }
}

async fn apply_replan(
    input: Value,
    actor_ref: String,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ProposalIdInput = match serde_json::from_value::<ProposalIdInput>(input) {
        Ok(input) if uuid(&input.proposal_id).is_ok() => input,
        _ => return invalid("proposal_id must be a UUID"),
    };
    let proposal = match get_replan_row(&broker, lease.clone(), &input.proposal_id).await {
        Ok(Some(row)) if row.get("status").and_then(Value::as_str) == Some("proposed") => row,
        Ok(Some(_)) => {
            return host_error(
                "replan-not-applicable",
                "only a current proposed re-plan can be applied",
            )
        }
        Ok(None) => return host_error("replan-not-found", "the re-plan was not found"),
        Err(error) => return error,
    };
    let item_id = proposal
        .get("item_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let trip_id = proposal
        .get("trip_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let expected_revision = proposal
        .get("expected_item_revision")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let before = proposal.get("before_state").cloned().unwrap_or(Value::Null);
    let proposed = proposal
        .get("proposed_state")
        .cloned()
        .unwrap_or(Value::Null);
    let item = match get_itinerary_row(&broker, lease.clone(), item_id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return host_error(
                "itinerary-item-not-found",
                "the itinerary item was not found",
            )
        }
        Err(error) => return error,
    };
    let operation_id = Uuid::new_v4().to_string();
    let current_state = itinerary_state(&item);
    let stale = item.get("revision").and_then(Value::as_i64) != Some(expected_revision)
        || !itinerary_states_match(&current_state, &before);
    let hard_conflict = match has_open_hard_conflict(&broker, lease.clone(), trip_id).await {
        Ok(value) => value,
        Err(error) => return error,
    };
    if stale || hard_conflict {
        return safe_stop_replan(
            &broker,
            lease,
            &proposal,
            &operation_id,
            current_state,
            if stale {
                "The itinerary changed after this re-plan was prepared"
            } else {
                "A hard travel constraint is currently violated"
            },
            &actor_ref,
            0,
        )
        .await;
    }
    let update_count = match update(
        &broker,
        DatabaseUpdateRequest::new(
            lease.clone(),
            id(WRITE_PERMISSION),
            table("travel_itinerary_items"),
            itinerary_update_values(&proposed, expected_revision + 1),
            BTreeMap::from([
                ("item_id".into(), json!(item_id)),
                ("revision".into(), json!(expected_revision)),
            ]),
        ),
    )
    .await
    {
        Ok(count) => count,
        Err(error) => return error,
    };
    if update_count != 1 {
        return safe_stop_replan(
            &broker,
            lease,
            &proposal,
            &operation_id,
            current_state,
            "The itinerary changed before the update could be applied",
            &actor_ref,
            1,
        )
        .await;
    }
    let mut verified = None;
    for _ in 0..2 {
        match get_itinerary_row(&broker, lease.clone(), item_id).await {
            Ok(Some(row)) => {
                if let Some(state) =
                    verified_itinerary_state(&row, expected_revision + 1, &proposed)
                {
                    verified = Some(state);
                    break;
                }
            }
            Ok(None) => {}
            Err(error) => return error,
        }
    }
    let Some(after) = verified else {
        let rollback_count = update(
            &broker,
            DatabaseUpdateRequest::new(
                lease.clone(),
                id(WRITE_PERMISSION),
                table("travel_itinerary_items"),
                itinerary_update_values(&before, expected_revision + 2),
                BTreeMap::from([
                    ("item_id".into(), json!(item_id)),
                    ("revision".into(), json!(expected_revision + 1)),
                ]),
            ),
        )
        .await
        .unwrap_or(0);
        let recovered_after = if rollback_count == 1 {
            match get_itinerary_row(&broker, lease.clone(), item_id).await {
                Ok(Some(row)) => verified_itinerary_state(&row, expected_revision + 2, &before),
                _ => None,
            }
        } else {
            None
        };
        let recovered = recovered_after.is_some();
        let after =
            recovered_after.unwrap_or_else(|| json!({"state":"unknown","predicate_met":false}));
        if let Err(error) = update_replan_terminal(
            &broker,
            lease.clone(),
            &proposal,
            "safe_stopped",
            &operation_id,
            None,
        )
        .await
        {
            return error;
        }
        if let Err(error) = record_travel_outcome(
            &broker,
            lease,
            &operation_id,
            &input.proposal_id,
            item_id,
            "apply_replan",
            before.clone(),
            after.clone(),
            if recovered { "failed" } else { "indeterminate" },
            2,
            &actor_ref,
        )
        .await
        {
            return error;
        }
        return travel_operation_result(
            "safe_stopped",
            &proposal,
            "The revised itinerary could not be verified",
            if recovered {
                "Original itinerary restored"
            } else {
                "Stopped without further changes"
            },
            before,
            after,
            2,
            false,
            operation_id,
            if recovered {
                ObservedOutcome::Failed
            } else {
                ObservedOutcome::Indeterminate
            },
        );
    };
    if let Err(error) = update_replan_terminal(
        &broker,
        lease.clone(),
        &proposal,
        "applied",
        &operation_id,
        Some(expected_revision + 1),
    )
    .await
    {
        return error;
    }
    if let Err(error) = record_travel_outcome(
        &broker,
        lease,
        &operation_id,
        &input.proposal_id,
        item_id,
        "apply_replan",
        before.clone(),
        after.clone(),
        "succeeded",
        1,
        &actor_ref,
    )
    .await
    {
        return error;
    }
    travel_operation_result(
        "applied",
        &proposal,
        "Travel disruption required a plan change",
        "Revised itinerary applied and verified",
        before,
        after,
        1,
        true,
        operation_id,
        ObservedOutcome::Succeeded,
    )
}

async fn rollback_replan(
    input: Value,
    actor_ref: String,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ReplanRollbackInput = match serde_json::from_value::<ReplanRollbackInput>(input) {
        Ok(input) if uuid(&input.proposal_id).is_ok() && uuid(&input.operation_id).is_ok() => input,
        _ => return invalid("proposal_id and operation_id must be UUIDs"),
    };
    let proposal = match get_replan_row(&broker, lease.clone(), &input.proposal_id).await {
        Ok(Some(row))
            if row.get("status").and_then(Value::as_str) == Some("applied")
                && row.get("operation_id").and_then(Value::as_str)
                    == Some(input.operation_id.as_str()) =>
        {
            row
        }
        Ok(Some(_)) => {
            return host_error(
                "replan-not-reversible",
                "the selected applied re-plan is no longer reversible",
            )
        }
        Ok(None) => return host_error("replan-not-found", "the re-plan was not found"),
        Err(error) => return error,
    };
    let item_id = proposal
        .get("item_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let applied_revision = proposal
        .get("applied_item_revision")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let before = proposal.get("before_state").cloned().unwrap_or(Value::Null);
    let proposed = proposal
        .get("proposed_state")
        .cloned()
        .unwrap_or(Value::Null);
    let current = match get_itinerary_row(&broker, lease.clone(), item_id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return host_error(
                "itinerary-item-not-found",
                "the itinerary item was not found",
            )
        }
        Err(error) => return error,
    };
    let current_state = itinerary_state(&current);
    let rollback_operation_id = Uuid::new_v4().to_string();
    if current.get("revision").and_then(Value::as_i64) != Some(applied_revision)
        || !itinerary_states_match(&current_state, &proposed)
    {
        if let Err(error) = record_travel_outcome(
            &broker,
            lease,
            &rollback_operation_id,
            &input.proposal_id,
            item_id,
            "rollback_replan",
            current_state.clone(),
            current_state.clone(),
            "failed",
            0,
            &actor_ref,
        )
        .await
        {
            return error;
        }
        return travel_operation_result(
            "safe_stopped",
            &proposal,
            "The itinerary changed after this re-plan",
            "No rollback performed",
            current_state.clone(),
            current_state,
            0,
            false,
            rollback_operation_id,
            ObservedOutcome::Failed,
        );
    }
    let count = match update(
        &broker,
        DatabaseUpdateRequest::new(
            lease.clone(),
            id(WRITE_PERMISSION),
            table("travel_itinerary_items"),
            itinerary_update_values(&before, applied_revision + 1),
            BTreeMap::from([
                ("item_id".into(), json!(item_id)),
                ("revision".into(), json!(applied_revision)),
            ]),
        ),
    )
    .await
    {
        Ok(count) => count,
        Err(error) => return error,
    };
    let restored_after = if count == 1 {
        match get_itinerary_row(&broker, lease.clone(), item_id).await {
            Ok(Some(row)) => verified_itinerary_state(&row, applied_revision + 1, &before),
            _ => None,
        }
    } else {
        None
    };
    let restored = restored_after.is_some();
    let after = restored_after.unwrap_or_else(|| json!({"state":"unknown","predicate_met":false}));
    if restored {
        if let Err(error) = update_replan_terminal(
            &broker,
            lease.clone(),
            &proposal,
            "rolled_back",
            &rollback_operation_id,
            Some(applied_revision + 1),
        )
        .await
        {
            return error;
        }
    }
    if let Err(error) = record_travel_outcome(
        &broker,
        lease,
        &rollback_operation_id,
        &input.proposal_id,
        item_id,
        "rollback_replan",
        current_state.clone(),
        after.clone(),
        if restored {
            "succeeded"
        } else {
            "indeterminate"
        },
        1,
        &actor_ref,
    )
    .await
    {
        return error;
    }
    travel_operation_result(
        if restored {
            "rolled_back"
        } else {
            "safe_stopped"
        },
        &proposal,
        if restored {
            "The applied travel change was reversed"
        } else {
            "The previous itinerary could not be verified"
        },
        if restored {
            "Previous itinerary restored"
        } else {
            "Stopped without further changes"
        },
        current_state,
        after,
        1,
        false,
        rollback_operation_id,
        if restored {
            ObservedOutcome::Succeeded
        } else {
            ObservedOutcome::Indeterminate
        },
    )
}

async fn operation_outcomes_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let limit = match parse_list(input) {
        Ok(input) if input.trip_id.is_none() => input.limit.min(200),
        _ => return invalid("operation outcome list accepts only a bounded limit"),
    };
    match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("travel_operation_outcomes"),
            [
                "operation_id".into(),
                "proposal_id".into(),
                "target_item_id".into(),
                "action".into(),
                "before_state".into(),
                "after_state".into(),
                "observed_outcome".into(),
                "attempts".into(),
                "actor_ref".into(),
                "created_at".into(),
            ],
        )
        .with_order("created_at", DatabaseOrderDirection::Descending)
        .with_limit(limit),
    )
    .await
    {
        Ok(rows) => rows_result(rows),
        Err(error) => error,
    }
}

async fn get_disruption_row(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    disruption_id: &str,
) -> Result<Option<BTreeMap<String, Value>>, HostResponse> {
    select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("travel_disruptions"),
            [
                "disruption_id".into(),
                "trip_id".into(),
                "affected_item_id".into(),
                "summary".into(),
                "state".into(),
                "revision".into(),
            ],
        )
        .with_filter("disruption_id", json!(disruption_id))
        .with_limit(1),
    )
    .await
    .map(|rows| rows.rows.into_iter().next())
}

async fn get_replan_row(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    proposal_id: &str,
) -> Result<Option<BTreeMap<String, Value>>, HostResponse> {
    select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("travel_replans"),
            [
                "proposal_id".into(),
                "disruption_id".into(),
                "trip_id".into(),
                "item_id".into(),
                "status".into(),
                "reason".into(),
                "evidence_ref".into(),
                "booking_impact".into(),
                "cost_change_minor".into(),
                "expected_item_revision".into(),
                "before_state".into(),
                "proposed_state".into(),
                "applied_item_revision".into(),
                "operation_id".into(),
                "revision".into(),
            ],
        )
        .with_filter("proposal_id", json!(proposal_id))
        .with_limit(1),
    )
    .await
    .map(|rows| rows.rows.into_iter().next())
}

async fn get_itinerary_row(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    item_id: &str,
) -> Result<Option<BTreeMap<String, Value>>, HostResponse> {
    select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("travel_itinerary_items"),
            itinerary_columns(),
        )
        .with_filter("item_id", json!(item_id))
        .with_limit(1),
    )
    .await
    .map(|rows| rows.rows.into_iter().next())
}

async fn has_open_hard_conflict(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    trip_id: &str,
) -> Result<bool, HostResponse> {
    select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("travel_constraints"),
            ["constraint_id".into()],
        )
        .with_filter("trip_id", json!(trip_id))
        .with_filter("strength", json!("hard"))
        .with_filter("conflict_status", json!("violated"))
        .with_limit(1),
    )
    .await
    .map(|rows| !rows.rows.is_empty())
}

fn itinerary_state(row: &BTreeMap<String, Value>) -> Value {
    json!({
        "title": row.get("title").cloned().unwrap_or(Value::Null),
        "starts_at": row.get("starts_at").cloned().unwrap_or(Value::Null),
        "ends_at": row.get("ends_at").cloned().unwrap_or(Value::Null),
        "timezone": row.get("timezone").cloned().unwrap_or(Value::Null),
        "place": row.get("place").cloned().unwrap_or(Value::Null),
        "status": row.get("status").cloned().unwrap_or(Value::Null),
        "revision": row.get("revision").cloned().unwrap_or(Value::Null),
    })
}

fn itinerary_update_values(state: &Value, revision: i64) -> BTreeMap<String, Value> {
    let mut values = BTreeMap::new();
    for field in [
        "title",
        "starts_at",
        "ends_at",
        "timezone",
        "place",
        "status",
    ] {
        values.insert(
            field.into(),
            state.get(field).cloned().unwrap_or(Value::Null),
        );
    }
    values.insert("revision".into(), json!(revision));
    values.insert("updated_at".into(), json!(now()));
    values
}

fn itinerary_states_match(left: &Value, right: &Value) -> bool {
    ["title", "timezone", "place", "status"]
        .into_iter()
        .all(|field| left.get(field) == right.get(field))
        && ["starts_at", "ends_at"].into_iter().all(|field| {
            match (
                left.get(field).and_then(Value::as_str),
                right.get(field).and_then(Value::as_str),
            ) {
                (Some(left), Some(right)) => timestamp(left)
                    .ok()
                    .zip(timestamp(right).ok())
                    .is_some_and(|(left, right)| left == right),
                _ => false,
            }
        })
}

fn verified_itinerary_state(
    row: &BTreeMap<String, Value>,
    expected_revision: i64,
    expected_state: &Value,
) -> Option<Value> {
    let state = itinerary_state(row);
    (row.get("revision").and_then(Value::as_i64) == Some(expected_revision)
        && itinerary_states_match(&state, expected_state))
    .then_some(state)
}

async fn update_replan_terminal(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    proposal: &BTreeMap<String, Value>,
    status: &str,
    operation_id: &str,
    applied_item_revision: Option<i64>,
) -> Result<(), HostResponse> {
    let proposal_id = proposal
        .get("proposal_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let revision = proposal
        .get("revision")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let count = update(
        broker,
        DatabaseUpdateRequest::new(
            lease,
            id(WRITE_PERMISSION),
            table("travel_replans"),
            BTreeMap::from([
                ("status".into(), json!(status)),
                ("operation_id".into(), json!(operation_id)),
                (
                    "applied_item_revision".into(),
                    applied_item_revision.map_or(Value::Null, |value| json!(value)),
                ),
                ("revision".into(), json!(revision + 1)),
                ("updated_at".into(), json!(now())),
            ]),
            BTreeMap::from([
                ("proposal_id".into(), json!(proposal_id)),
                ("revision".into(), json!(revision)),
            ]),
        ),
    )
    .await?;
    match count {
        1 => Ok(()),
        0 => Err(revision_conflict()),
        _ => Err(host_error(
            "write-cardinality-invalid",
            "re-plan state affected an unexpected row count",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_travel_outcome(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    operation_id: &str,
    proposal_id: &str,
    item_id: &str,
    action: &str,
    before_state: Value,
    after_state: Value,
    observed_outcome: &str,
    attempts: i64,
    actor_ref: &str,
) -> Result<(), HostResponse> {
    insert(
        broker,
        DatabaseInsertRequest::new(
            lease,
            id(WRITE_PERMISSION),
            table("travel_operation_outcomes"),
            BTreeMap::from([
                ("operation_id".into(), json!(operation_id)),
                ("proposal_id".into(), json!(proposal_id)),
                ("target_item_id".into(), json!(item_id)),
                ("action".into(), json!(action)),
                ("before_state".into(), before_state),
                ("after_state".into(), after_state),
                ("observed_outcome".into(), json!(observed_outcome)),
                ("attempts".into(), json!(attempts)),
                ("actor_ref".into(), json!(actor_ref)),
                ("created_at".into(), json!(now())),
            ]),
        ),
    )
    .await
    .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
async fn safe_stop_replan(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    proposal: &BTreeMap<String, Value>,
    operation_id: &str,
    current_state: Value,
    issue: &str,
    actor_ref: &str,
    attempts: i64,
) -> HostResponse {
    if let Err(error) = update_replan_terminal(
        broker,
        lease.clone(),
        proposal,
        "safe_stopped",
        operation_id,
        None,
    )
    .await
    {
        return error;
    }
    let proposal_id = proposal
        .get("proposal_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let item_id = proposal
        .get("item_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Err(error) = record_travel_outcome(
        broker,
        lease,
        operation_id,
        proposal_id,
        item_id,
        "apply_replan",
        current_state.clone(),
        current_state.clone(),
        "failed",
        attempts,
        actor_ref,
    )
    .await
    {
        return error;
    }
    travel_operation_result(
        "safe_stopped",
        proposal,
        issue,
        "No itinerary change applied",
        current_state.clone(),
        current_state,
        attempts,
        false,
        operation_id.to_string(),
        ObservedOutcome::Failed,
    )
}

#[allow(clippy::too_many_arguments)]
fn travel_operation_result(
    status: &str,
    proposal: &BTreeMap<String, Value>,
    issue: &str,
    action: &str,
    before: Value,
    after: Value,
    attempts: i64,
    rollback_available: bool,
    operation_id: String,
    observed: ObservedOutcome,
) -> HostResponse {
    let title = proposal
        .get("proposed_state")
        .and_then(|state| state.get("title"))
        .cloned()
        .or_else(|| {
            proposal
                .get("before_state")
                .and_then(|state| state.get("title"))
                .cloned()
        })
        .unwrap_or_else(|| json!("Trip plan"));
    let mut observation = OutcomeObservation::new(observed, action);
    observation.details = json!({
        "proposal_id": proposal.get("proposal_id").cloned().unwrap_or(Value::Null),
        "operation_id": operation_id,
        "before": before,
        "after": after,
        "attempts": attempts,
    });
    HostResponse::GadgetResult(
        gadgetron_bundle_sdk::GadgetResult::new(json!({
            "status": status,
            "title": title,
            "issue": issue,
            "action": action,
            "before": before,
            "after": after,
            "attempts": attempts,
            "rollback_available": rollback_available,
            "proposal_id": proposal.get("proposal_id").cloned().unwrap_or(Value::Null),
            "operation_id": operation_id,
        }))
        .with_outcome(observation),
    )
}

fn human_replan_status(status: &str) -> &'static str {
    match status {
        "proposed" => "Ready for review",
        "applied" => "Plan updated",
        "rolled_back" => "Previous plan restored",
        "safe_stopped" => "Stopped safely",
        _ => "Needs re-plan",
    }
}

async fn list_for_trip<const N: usize>(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
    table_name: &str,
    columns: [&str; N],
    order_field: &str,
) -> HostResponse {
    let input = match parse_list(input) {
        Ok(input) => input,
        Err(message) => return invalid(message),
    };
    let mut request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table(table_name),
        columns.into_iter().map(str::to_string),
    )
    .with_order(order_field, DatabaseOrderDirection::Ascending)
    .with_limit(input.limit);
    if let Some(trip_id) = input.trip_id {
        if uuid(&trip_id).is_err() {
            return invalid("trip_id must be a UUID");
        }
        request = request.with_filter("trip_id", json!(trip_id));
    }
    match select(&broker, request).await {
        Ok(rows) => rows_result(rows),
        Err(error) => error,
    }
}

#[allow(clippy::too_many_arguments)]
async fn mutate_entity(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    table_name: &str,
    id_field: &str,
    entity_id: &str,
    revision: i64,
    is_create: bool,
    now: &str,
    values: &mut BTreeMap<String, Value>,
) -> Result<u32, HostResponse> {
    if is_create {
        values.insert(id_field.into(), json!(entity_id));
        values.insert("revision".into(), json!(1));
        values.insert("created_at".into(), json!(now));
        insert(
            broker,
            DatabaseInsertRequest::new(
                lease,
                id(WRITE_PERMISSION),
                table(table_name),
                values.clone(),
            ),
        )
        .await
    } else {
        values.insert("revision".into(), json!(revision + 1));
        update(
            broker,
            DatabaseUpdateRequest::new(
                lease,
                id(WRITE_PERMISSION),
                table(table_name),
                values.clone(),
                BTreeMap::from([
                    (id_field.into(), json!(entity_id)),
                    ("revision".into(), json!(revision)),
                ]),
            ),
        )
        .await
    }
}

fn mutation_result(
    result: Result<u32, HostResponse>,
    is_create: bool,
    id_field: &str,
    entity_id: &str,
    revision: i64,
) -> HostResponse {
    match result {
        Ok(1) => gadget_result(json!({
            id_field: entity_id,
            "revision": if is_create { 1 } else { revision + 1 },
            "created": is_create,
        })),
        Ok(0) if !is_create => revision_conflict(),
        Ok(_) => host_error(
            "write-cardinality-invalid",
            "mutation affected an unexpected row count",
        ),
        Err(error) => error,
    }
}

async fn delete_entity(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
    table_name: &str,
    id_field: &str,
) -> HostResponse {
    let input: DeleteInput = match serde_json::from_value::<DeleteInput>(input) {
        Ok(input) if uuid(&input.id).is_ok() && input.revision > 0 => input,
        _ => return invalid("id must be a UUID and revision must be positive"),
    };
    let request = DatabaseDeleteRequest::new(
        lease,
        id(WRITE_PERMISSION),
        table(table_name),
        BTreeMap::from([
            (id_field.into(), json!(input.id)),
            ("revision".into(), json!(input.revision)),
        ]),
    );
    match delete(&broker, request).await {
        Ok(1) => gadget_result(json!({"id": input.id, "deleted": true})),
        Ok(0) => revision_conflict(),
        Ok(_) => host_error(
            "write-cardinality-invalid",
            "delete affected an unexpected row count",
        ),
        Err(error) => error,
    }
}

async fn get_trip_row(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    trip_id: &str,
) -> Result<Option<BTreeMap<String, Value>>, HostResponse> {
    select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("travel_trips"),
            trip_columns(),
        )
        .with_filter("trip_id", json!(trip_id))
        .with_limit(1),
    )
    .await
    .map(|rows| rows.rows.into_iter().next())
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

async fn delete(
    broker: &SharedBroker,
    request: DatabaseDeleteRequest,
) -> Result<u32, HostResponse> {
    broker
        .lock()
        .await
        .database_delete(request)
        .await
        .map(|result| result.affected_rows)
        .map_err(broker_error)
}

fn parse_list(value: Value) -> Result<ListInput, &'static str> {
    let input: ListInput = serde_json::from_value(value).map_err(|_| "list input is invalid")?;
    if !(1..=500).contains(&input.limit) {
        return Err("limit must be between 1 and 500");
    }
    Ok(input)
}

fn validate_trip(input: &TripUpsertInput) -> Result<(), &'static str> {
    let start = NaiveDate::parse_from_str(&input.start_date, "%Y-%m-%d")
        .map_err(|_| "start_date must be YYYY-MM-DD")?;
    let end = NaiveDate::parse_from_str(&input.end_date, "%Y-%m-%d")
        .map_err(|_| "end_date must be YYYY-MM-DD")?;
    if end < start
        || !bounded(&input.title, 1, 200)
        || !bounded(&input.origin, 1, 200)
        || !bounded(&input.notes, 0, 4096)
        || input.timezone.parse::<chrono_tz::Tz>().is_err()
        || !(1..=100).contains(&input.traveler_count)
        || !matches!(
            input.status.as_str(),
            "draft" | "planned" | "active" | "completed" | "cancelled"
        )
        || !currency(&input.currency)
        || input.budget_amount_minor < 0
    {
        return Err("Trip values violate the signed domain contract");
    }
    Ok(())
}

fn validate_itinerary(input: &ItineraryUpsertInput) -> Result<(), &'static str> {
    let start = timestamp(&input.starts_at).map_err(|_| "starts_at must be RFC 3339")?;
    let end = timestamp(&input.ends_at).map_err(|_| "ends_at must be RFC 3339")?;
    if uuid(&input.trip_id).is_err()
        || end <= start
        || !bounded(&input.title, 1, 200)
        || !matches!(
            input.kind.as_str(),
            "transport" | "lodging" | "activity" | "meal" | "buffer" | "other"
        )
        || input.timezone.parse::<chrono_tz::Tz>().is_err()
        || !bounded(&input.place, 1, 300)
        || !matches!(
            input.status.as_str(),
            "proposed" | "planned" | "confirmed" | "completed" | "cancelled"
        )
        || !bounded(&input.notes, 0, 4096)
    {
        return Err("Itinerary values violate the signed domain contract");
    }
    Ok(())
}

fn mutation_revision(revision: Option<i64>, is_create: bool) -> Result<i64, &'static str> {
    if is_create {
        if revision.is_some() {
            return Err("revision must be omitted when creating an entity");
        }
        Ok(1)
    } else {
        valid_revision(revision)
    }
}

fn valid_revision(revision: Option<i64>) -> Result<i64, &'static str> {
    match revision {
        Some(revision) if revision > 0 && revision < i64::MAX => Ok(revision),
        _ => Err("a positive current revision is required when updating"),
    }
}

fn entity_id(value: Option<&str>, is_create: bool) -> Result<String, &'static str> {
    if is_create {
        Ok(Uuid::new_v4().to_string())
    } else {
        uuid(value.unwrap_or_default()).map(|id| id.to_string())
    }
}

fn uuid(value: &str) -> Result<Uuid, &'static str> {
    Uuid::parse_str(value).map_err(|_| "id must be a UUID")
}

fn timestamp(value: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(value).map(|value| value.with_timezone(&Utc))
}

fn bounded(value: &str, minimum: usize, maximum: usize) -> bool {
    let length = value.chars().count();
    (minimum..=maximum).contains(&length) && !value.chars().any(|character| character == '\0')
}

fn currency(value: &str) -> bool {
    value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase())
}

fn number(value: Option<&Value>) -> i64 {
    value.and_then(Value::as_i64).unwrap_or(0)
}

fn rows_result(rows: DatabaseRows) -> HostResponse {
    gadget_result(json!({
        "count": rows.rows.len(),
        "rows": rows.rows,
        "truncated": rows.truncated,
    }))
}

fn invalid(message: &str) -> HostResponse {
    host_error("invalid-arguments", message)
}

fn revision_conflict() -> HostResponse {
    host_error(
        "revision-conflict",
        "the record changed or no longer exists; read the current revision before retrying",
    )
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

fn trip_columns() -> impl IntoIterator<Item = String> {
    [
        "trip_id",
        "title",
        "origin",
        "start_date",
        "end_date",
        "timezone",
        "traveler_count",
        "status",
        "currency",
        "budget_amount_minor",
        "notes",
        "revision",
        "created_at",
        "updated_at",
    ]
    .into_iter()
    .map(str::to_string)
}

fn itinerary_columns() -> impl IntoIterator<Item = String> {
    [
        "item_id",
        "trip_id",
        "title",
        "kind",
        "starts_at",
        "ends_at",
        "timezone",
        "place",
        "status",
        "notes",
        "external_owner_bundle",
        "external_entity_id",
        "external_entity_revision",
        "external_branch_id",
        "external_snapshot",
        "supporting_source_id",
        "supporting_source_revision",
        "contradicting_source_id",
        "contradicting_source_revision",
        "revision",
        "created_at",
        "updated_at",
    ]
    .into_iter()
    .map(str::to_string)
}

fn markdown_export(trip: &BTreeMap<String, Value>, items: &[BTreeMap<String, Value>]) -> String {
    let mut output = format!(
        "# {}\n\n- Origin: {}\n- Dates: {} — {}\n- Timezone: {}\n- Status: {}\n- Budget: {} {} minor units\n- Revision: {}\n\n## Itinerary\n",
        text(trip, "title"), text(trip, "origin"), text(trip, "start_date"), text(trip, "end_date"),
        text(trip, "timezone"), text(trip, "status"), text(trip, "budget_amount_minor"),
        text(trip, "currency"), text(trip, "revision"),
    );
    if items.is_empty() {
        output.push_str("\nNo itinerary items.\n");
    } else {
        for item in items {
            output.push_str(&format!(
                "\n### {}\n\n- Time: {} — {} ({})\n- Place: {}\n- Kind/status: {} / {}\n",
                text(item, "title"),
                text(item, "starts_at"),
                text(item, "ends_at"),
                text(item, "timezone"),
                text(item, "place"),
                text(item, "kind"),
                text(item, "status"),
            ));
        }
    }
    output
}

fn ical_export(trip: &BTreeMap<String, Value>, items: &[BTreeMap<String, Value>]) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//Gadgetron//Travel Planner 0.1//EN".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
        format!("X-WR-CALNAME:{}", ical_text(&text(trip, "title"))),
    ];
    for item in items {
        let (Some(start), Some(end)) = (
            item.get("starts_at")
                .and_then(Value::as_str)
                .and_then(|value| timestamp(value).ok()),
            item.get("ends_at")
                .and_then(Value::as_str)
                .and_then(|value| timestamp(value).ok()),
        ) else {
            continue;
        };
        let updated = item
            .get("updated_at")
            .and_then(Value::as_str)
            .and_then(|value| timestamp(value).ok())
            .unwrap_or(start);
        lines.extend([
            "BEGIN:VEVENT".to_string(),
            format!("UID:{}@travel.gadgetron", text(item, "item_id")),
            format!("DTSTAMP:{}", ical_time(updated)),
            format!("DTSTART:{}", ical_time(start)),
            format!("DTEND:{}", ical_time(end)),
            format!("SUMMARY:{}", ical_text(&text(item, "title"))),
            format!("LOCATION:{}", ical_text(&text(item, "place"))),
            format!(
                "DESCRIPTION:Trip {} · {}",
                ical_text(&text(trip, "title")),
                ical_text(&text(item, "status"))
            ),
            "END:VEVENT".to_string(),
        ]);
    }
    lines.push("END:VCALENDAR".to_string());
    lines.join("\r\n") + "\r\n"
}

fn ical_time(value: DateTime<Utc>) -> String {
    value.format("%Y%m%dT%H%M%SZ").to_string()
}

fn ical_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace(['\r', '\n'], "\\n")
}

fn text(row: &BTreeMap<String, Value>, field: &str) -> String {
    match row.get(field) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn filename_slug(value: &str) -> String {
    let slug: String = value
        .chars()
        .take(80)
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug
        .trim_matches('-')
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "trip".into()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn itinerary_predicate_compares_human_state_and_normalizes_timestamps() {
        let left = json!({
            "title": "Museum",
            "starts_at": "2026-08-01T01:00:00Z",
            "ends_at": "2026-08-01T02:00:00Z",
            "timezone": "Asia/Seoul",
            "place": "Jongno",
            "status": "planned",
            "revision": 2,
        });
        let same = json!({
            "title": "Museum",
            "starts_at": "2026-08-01T10:00:00+09:00",
            "ends_at": "2026-08-01T11:00:00+09:00",
            "timezone": "Asia/Seoul",
            "place": "Jongno",
            "status": "planned",
            "revision": 99,
        });
        assert!(itinerary_states_match(&left, &same));
        let mut changed = left.clone();
        changed["place"] = json!("Gangnam");
        assert!(!itinerary_states_match(&left, &changed));
    }

    #[test]
    fn monitor_status_uses_terminal_user_language() {
        assert_eq!(human_replan_status("proposed"), "Ready for review");
        assert_eq!(human_replan_status("applied"), "Plan updated");
        assert_eq!(human_replan_status("safe_stopped"), "Stopped safely");
        assert_eq!(human_replan_status("unknown"), "Needs re-plan");
    }

    #[test]
    fn verified_state_keeps_the_authoritative_post_write_revision() {
        let row = BTreeMap::from([
            ("title".into(), json!("Museum")),
            ("starts_at".into(), json!("2026-08-01T01:00:00Z")),
            ("ends_at".into(), json!("2026-08-01T02:00:00Z")),
            ("timezone".into(), json!("Asia/Seoul")),
            ("place".into(), json!("Jongno")),
            ("status".into(), json!("planned")),
            ("revision".into(), json!(3)),
        ]);
        let expected = json!({
            "title": "Museum",
            "starts_at": "2026-08-01T10:00:00+09:00",
            "ends_at": "2026-08-01T11:00:00+09:00",
            "timezone": "Asia/Seoul",
            "place": "Jongno",
            "status": "planned",
            "revision": 1,
        });

        let verified = verified_itinerary_state(&row, 3, &expected).unwrap();
        assert_eq!(verified["revision"], json!(3));
        assert!(verified_itinerary_state(&row, 2, &expected).is_none());
    }
}
