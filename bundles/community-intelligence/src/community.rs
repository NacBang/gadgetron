use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, SecondsFormat, Utc};
use gadgetron_bundle_runtime::SharedBundleBroker;
use gadgetron_bundle_sdk::{
    BrokerResource, DatabaseInsertRequest, DatabaseOrderDirection, DatabaseRows,
    DatabaseSelectRequest, DatabaseUpdateRequest, GadgetInvocation, HostResponse,
    InvocationLeaseToken, KnowledgeCollectionAction, KnowledgeCollectionRequest,
    KnowledgeCollectionResult, LocalId,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{broker_error, gadget_result, host_error};

const READ_PERMISSION: &str = "community-read";
const WRITE_PERMISSION: &str = "community-write";
const COLLECTION_PERMISSION: &str = "community-collections";

#[derive(Clone, Copy)]
struct EntitySpec {
    table: &'static str,
    id_column: &'static str,
    columns: fn() -> Vec<String>,
}

const DISCUSSION: EntitySpec = EntitySpec {
    table: "community_discussions",
    id_column: "discussion_id",
    columns: discussion_columns,
};
const PATTERN: EntitySpec = EntitySpec {
    table: "community_solution_patterns",
    id_column: "pattern_id",
    columns: pattern_columns,
};
const EVIDENCE: EntitySpec = EntitySpec {
    table: "community_pattern_evidence",
    id_column: "evidence_id",
    columns: evidence_columns,
};

pub(crate) async fn invoke(
    invocation: GadgetInvocation,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let Some(lease) = invocation.context.broker_lease else {
        return host_error(
            "broker-lease-required",
            "Core did not attach an invocation-scoped broker lease",
        );
    };
    match invocation.gadget.as_str() {
        "community.topic-list" => topic_list(invocation.input, lease, broker).await,
        "community.topic-collect" => topic_collect(invocation.input, lease, broker).await,
        "community.discussion-list" => discussion_list(invocation.input, lease, broker).await,
        "community.discussion-upsert" => discussion_upsert(invocation.input, lease, broker).await,
        "community.solution-pattern-list" => pattern_list(invocation.input, lease, broker).await,
        "community.solution-pattern-upsert" => {
            pattern_upsert(invocation.input, lease, broker).await
        }
        "community.pattern-evidence-list" => evidence_list(invocation.input, lease, broker).await,
        "community.pattern-evidence-upsert" => {
            evidence_upsert(invocation.input, lease, broker).await
        }
        "community.solution-graph" => solution_graph(invocation.input, lease, broker).await,
        "community.subject-context" => subject_context(invocation.input, lease, broker).await,
        "community.dashboard-summary" => dashboard_summary(lease, broker).await,
        _ => host_error(
            "capability-not-found",
            "requested Community Intelligence capability is not available",
        ),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListInput {
    #[serde(default)]
    topic_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicCollectInput {
    collection_id: String,
    expected_revision: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscussionUpsertInput {
    topic_id: String,
    provider: String,
    external_id: String,
    canonical_url: String,
    title: String,
    summary: String,
    state: String,
    score_snapshot: i32,
    accepted_answer_observed: bool,
    source_id: String,
    source_revision: i64,
    fetched_at: String,
    content_hash: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatternUpsertInput {
    #[serde(default)]
    pattern_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    topic_id: String,
    title: String,
    problem_signature: String,
    environment: String,
    procedure: String,
    rollback: String,
    status: String,
    supporting_evidence: i32,
    contradicting_evidence: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceUpsertInput {
    #[serde(default)]
    evidence_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    pattern_id: String,
    discussion_id: String,
    statement: String,
    stance: String,
    source_id: String,
    source_revision: i64,
    observed_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatternInput {
    pattern_id: String,
}

async fn topic_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: ListInput = match serde_json::from_value::<ListInput>(input) {
        Ok(input) if input.topic_id.is_none() && (1..=200).contains(&input.limit) => input,
        _ => return invalid("Topic list input is outside the signed bound"),
    };
    let request = KnowledgeCollectionRequest::new(
        lease,
        id(COLLECTION_PERMISSION),
        KnowledgeCollectionAction::List { limit: input.limit },
    );
    match broker.lock().await.knowledge_collection(request).await {
        Ok(KnowledgeCollectionResult::Listed {
            collections,
            truncated,
        }) => {
            let rows = collections
                .into_iter()
                .map(|topic| {
                    json!({
                        "collection_id": topic.collection_id,
                        "title": topic.topic,
                        "status": topic.status,
                        "providers": topic.queries.iter().map(|query| query.provider.as_str()).collect::<Vec<_>>(),
                        "source_coverage": topic.locators.len(),
                        "schedule_enabled": topic.schedule_enabled,
                        "next_collect": topic.next_run_at,
                        "last_collect": topic.last_run_at,
                        "expected_revision": topic.revision,
                        "updated_at": topic.updated_at,
                    })
                })
                .collect::<Vec<_>>();
            gadget_result(json!({"count": rows.len(), "rows": rows, "truncated": truncated}))
        }
        Ok(_) => host_error(
            "collection-response-invalid",
            "Core returned an unexpected Topic list result",
        ),
        Err(error) => broker_error(error),
    }
}

async fn topic_collect(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: TopicCollectInput = match serde_json::from_value::<TopicCollectInput>(input) {
        Ok(input) if valid_uuid(&input.collection_id) && input.expected_revision > 0 => input,
        _ => return invalid("Topic collection id or revision is invalid"),
    };
    let request = KnowledgeCollectionRequest::new(
        lease,
        id(COLLECTION_PERMISSION),
        KnowledgeCollectionAction::Enqueue {
            collection_id: input.collection_id,
            expected_revision: input.expected_revision,
        },
    );
    match broker.lock().await.knowledge_collection(request).await {
        Ok(KnowledgeCollectionResult::Enqueued {
            collection_id,
            run_id,
            status,
            created,
        }) => gadget_result(json!({
            "topic_id": collection_id,
            "run_id": run_id,
            "status": status,
            "started": created,
        })),
        Ok(_) => host_error(
            "collection-response-invalid",
            "Core returned an unexpected Topic collection result",
        ),
        Err(error) => broker_error(error),
    }
}

async fn discussion_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    list_entity(input, lease, broker, DISCUSSION, "fetched_at", 500).await
}

async fn pattern_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    list_entity(input, lease, broker, PATTERN, "updated_at", 500).await
}

async fn evidence_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        pattern_id: String,
        #[serde(default = "default_limit")]
        limit: u32,
    }
    let input: Input = match serde_json::from_value::<Input>(input) {
        Ok(input) if valid_uuid(&input.pattern_id) && (1..=500).contains(&input.limit) => input,
        _ => return invalid("Pattern evidence list input is invalid"),
    };
    query_rows(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table(EVIDENCE.table),
            evidence_columns(),
        )
        .with_filter("pattern_id", json!(input.pattern_id))
        .with_order("observed_at", DatabaseOrderDirection::Descending)
        .with_limit(input.limit),
    )
    .await
}

async fn list_entity(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
    entity: EntitySpec,
    order: &str,
    maximum: u32,
) -> HostResponse {
    let input = match list_input(input, maximum) {
        Ok(input) => input,
        Err(message) => return invalid(message),
    };
    let mut request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table(entity.table),
        (entity.columns)(),
    )
    .with_order(order, DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(topic_id) = input.topic_id {
        if !valid_uuid(&topic_id) {
            return invalid("Topic must be a UUID");
        }
        request = request.with_filter("topic_id", json!(topic_id));
    }
    query_rows(broker, request).await
}

async fn discussion_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: DiscussionUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Discussion snapshot input is invalid"),
    };
    if !valid_uuid(&input.topic_id)
        || !matches!(
            input.provider.as_str(),
            "stack-exchange" | "reddit" | "forum"
        )
        || !bounded(&input.external_id, 1, 200)
        || !input.canonical_url.starts_with("https://")
        || !bounded(&input.canonical_url, 10, 2_000)
        || !bounded(&input.title, 1, 500)
        || !bounded(&input.summary, 1, 4_000)
        || !matches!(
            input.state.as_str(),
            "active" | "edited" | "deleted" | "locked"
        )
        || !(-1_000_000..=1_000_000).contains(&input.score_snapshot)
        || !valid_source(&input.source_id, input.source_revision, &input.fetched_at)
        || !valid_hash(&input.content_hash)
    {
        return invalid("Discussion snapshot fields are outside the signed contract");
    }
    let values = BTreeMap::from([
        ("topic_id".into(), json!(input.topic_id.clone())),
        ("provider".into(), json!(input.provider.clone())),
        ("external_id".into(), json!(input.external_id.clone())),
        ("canonical_url".into(), json!(input.canonical_url)),
        ("title".into(), json!(input.title)),
        ("summary".into(), json!(input.summary)),
        ("state".into(), json!(input.state)),
        ("score_snapshot".into(), json!(input.score_snapshot)),
        (
            "accepted_answer_observed".into(),
            json!(input.accepted_answer_observed),
        ),
        ("source_id".into(), json!(input.source_id.clone())),
        ("source_revision".into(), json!(input.source_revision)),
        ("fetched_at".into(), json!(input.fetched_at)),
        ("content_hash".into(), json!(input.content_hash)),
    ]);
    let existing = match select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table(DISCUSSION.table),
            discussion_columns(),
        )
        .with_filter("topic_id", json!(input.topic_id))
        .with_filter("provider", json!(input.provider))
        .with_filter("external_id", json!(input.external_id))
        .with_filter("source_id", json!(input.source_id))
        .with_filter("source_revision", json!(input.source_revision))
        .with_limit(2),
    )
    .await
    {
        Ok(rows) if rows.rows.len() <= 1 => rows.rows.into_iter().next(),
        Ok(_) => {
            return host_error(
                "discussion-source-identity-conflict",
                "more than one discussion has the same provider Source revision",
            )
        }
        Err(error) => return error,
    };
    if let Some(existing) = existing {
        if entity_values_match(&existing, &values) {
            return gadget_result(json!(existing));
        }
        let Some(discussion_id) = existing.get("discussion_id").and_then(Value::as_str) else {
            return host_error(
                "discussion-state-invalid",
                "discussion identity is unavailable",
            );
        };
        let Some(revision) = existing.get("revision").and_then(Value::as_i64) else {
            return host_error(
                "discussion-state-invalid",
                "discussion revision is unavailable",
            );
        };
        return upsert(
            broker,
            lease,
            DISCUSSION,
            Some(discussion_id.to_string()),
            Some(revision),
            values,
        )
        .await;
    }
    upsert(broker, lease, DISCUSSION, None, None, values).await
}

async fn pattern_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: PatternUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Solution pattern input is invalid"),
    };
    if !valid_uuid(&input.topic_id)
        || !bounded(&input.title, 1, 300)
        || !bounded(&input.problem_signature, 1, 2_000)
        || !bounded(&input.environment, 1, 2_000)
        || !bounded(&input.procedure, 1, 6_000)
        || !bounded(&input.rollback, 1, 4_000)
        || !matches!(
            input.status.as_str(),
            "reproduced" | "environment_dependent" | "contradicted" | "speculative" | "obsolete"
        )
        || !(0..=10_000).contains(&input.supporting_evidence)
        || !(0..=10_000).contains(&input.contradicting_evidence)
    {
        return invalid("Solution pattern fields are outside the signed contract");
    }
    let values = BTreeMap::from([
        ("topic_id".into(), json!(input.topic_id)),
        ("title".into(), json!(input.title)),
        ("problem_signature".into(), json!(input.problem_signature)),
        ("environment".into(), json!(input.environment)),
        ("procedure".into(), json!(input.procedure)),
        ("rollback".into(), json!(input.rollback)),
        ("status".into(), json!(input.status)),
        (
            "supporting_evidence".into(),
            json!(input.supporting_evidence),
        ),
        (
            "contradicting_evidence".into(),
            json!(input.contradicting_evidence),
        ),
    ]);
    upsert(
        broker,
        lease,
        PATTERN,
        input.pattern_id,
        input.revision,
        values,
    )
    .await
}

async fn evidence_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: EvidenceUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Pattern evidence input is invalid"),
    };
    if !valid_uuid(&input.pattern_id)
        || !valid_uuid(&input.discussion_id)
        || !bounded(&input.statement, 1, 2_000)
        || !matches!(
            input.stance.as_str(),
            "supports" | "contradicts" | "context"
        )
        || !valid_source(&input.source_id, input.source_revision, &input.observed_at)
    {
        return invalid("Pattern evidence fields are outside the signed contract");
    }
    let values = BTreeMap::from([
        ("pattern_id".into(), json!(input.pattern_id)),
        ("discussion_id".into(), json!(input.discussion_id)),
        ("statement".into(), json!(input.statement)),
        ("stance".into(), json!(input.stance)),
        ("source_id".into(), json!(input.source_id)),
        ("source_revision".into(), json!(input.source_revision)),
        ("observed_at".into(), json!(input.observed_at)),
    ]);
    upsert(
        broker,
        lease,
        EVIDENCE,
        input.evidence_id,
        input.revision,
        values,
    )
    .await
}

async fn solution_graph(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input = match list_input(input, 200) {
        Ok(input) => input,
        Err(message) => return invalid(message),
    };
    let mut pattern_request = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table(PATTERN.table),
        pattern_columns(),
    )
    .with_order("updated_at", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    let mut discussion_request = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table(DISCUSSION.table),
        discussion_columns(),
    )
    .with_order("fetched_at", DatabaseOrderDirection::Descending)
    .with_limit(500);
    if let Some(topic_id) = input.topic_id {
        if !valid_uuid(&topic_id) {
            return invalid("Topic must be a UUID");
        }
        pattern_request = pattern_request.with_filter("topic_id", json!(topic_id.clone()));
        discussion_request = discussion_request.with_filter("topic_id", json!(topic_id));
    }
    let patterns = match select(broker, pattern_request).await {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let discussions = match select(broker, discussion_request).await {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let evidence = match select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table(EVIDENCE.table),
            evidence_columns(),
        )
        .with_order("observed_at", DatabaseOrderDirection::Descending)
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let pattern_ids = ids(&patterns, "pattern_id");
    let discussion_ids = ids(&discussions, "discussion_id");
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for pattern in patterns {
        let Some(pattern_id) = text(&pattern, "pattern_id") else {
            continue;
        };
        nodes.push(json!({"id": pattern_id, "label": text(&pattern, "title").unwrap_or("Solution pattern"), "kind": "solution_pattern", "status": pattern.get("status")}));
    }
    for discussion in discussions {
        let Some(discussion_id) = text(&discussion, "discussion_id") else {
            continue;
        };
        nodes.push(json!({"id": discussion_id, "label": text(&discussion, "title").unwrap_or("Discussion"), "kind": "discussion", "status": discussion.get("state")}));
    }
    for item in evidence {
        let (Some(evidence_id), Some(pattern_id), Some(discussion_id)) = (
            text(&item, "evidence_id"),
            text(&item, "pattern_id"),
            text(&item, "discussion_id"),
        ) else {
            continue;
        };
        if !pattern_ids.contains(pattern_id) || !discussion_ids.contains(discussion_id) {
            continue;
        }
        let stance = text(&item, "stance").unwrap_or("context");
        nodes.push(json!({"id": evidence_id, "label": text(&item, "statement").unwrap_or("Evidence"), "kind": "evidence", "status": stance}));
        edges.push(json!({"source": evidence_id, "target": pattern_id, "label": stance}));
        edges.push(json!({"source": evidence_id, "target": discussion_id, "label": "observed in"}));
    }
    gadget_result(json!({"nodes": nodes, "edges": edges}))
}

async fn subject_context(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: PatternInput = match serde_json::from_value::<PatternInput>(input) {
        Ok(input) if valid_uuid(&input.pattern_id) => input,
        _ => return invalid("Pattern id must be a UUID"),
    };
    let pattern = match select_one(broker, lease.clone(), PATTERN, &input.pattern_id).await {
        Ok(Some(pattern)) => pattern,
        Ok(None) => return host_error("pattern-not-found", "Solution pattern is not available"),
        Err(error) => return error,
    };
    let evidence = match select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table(EVIDENCE.table),
            evidence_columns(),
        )
        .with_filter("pattern_id", json!(input.pattern_id.clone()))
        .with_order("observed_at", DatabaseOrderDirection::Descending)
        .with_limit(50),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    gadget_result(json!({
        "id": pattern.get("pattern_id"),
        "kind": "community_solution_pattern",
        "bundle": "community-intelligence",
        "title": pattern.get("title"),
        "subtitle": pattern.get("status"),
        "href": format!("/web/workspace?id=community-intelligence.solutions&pattern={}", input.pattern_id),
        "facts": {
            "problem_signature": pattern.get("problem_signature"),
            "environment": pattern.get("environment"),
            "procedure": pattern.get("procedure"),
            "rollback": pattern.get("rollback"),
            "evidence": evidence,
        },
        "prompt": "이 해결 패턴의 적용 환경, 상충 근거, rollback과 현재 서버 상황에 대한 적용 가능성을 인용과 함께 검토해줘.",
    }))
}

async fn dashboard_summary(
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let patterns = match select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table(PATTERN.table),
            columns(&["status", "updated_at"]),
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let count = |status: &str| {
        patterns
            .iter()
            .filter(|pattern| text(pattern, "status") == Some(status))
            .count()
    };
    gadget_result(json!({
        "solution patterns": patterns.len(),
        "reproduced": count("reproduced"),
        "environment dependent": count("environment_dependent"),
        "contradicted or obsolete": count("contradicted") + count("obsolete"),
    }))
}

async fn upsert(
    broker: &SharedBundleBroker,
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
        broker
            .lock()
            .await
            .database_insert(DatabaseInsertRequest::new(
                lease.clone(),
                id(WRITE_PERMISSION),
                table(entity.table),
                values,
            ))
            .await
            .map(|result| result.affected_rows)
    } else {
        values.insert("revision".into(), json!(revision + 1));
        broker
            .lock()
            .await
            .database_update(DatabaseUpdateRequest::new(
                lease.clone(),
                id(WRITE_PERMISSION),
                table(entity.table),
                values,
                BTreeMap::from([
                    (entity.id_column.into(), json!(entity_id)),
                    ("revision".into(), json!(revision)),
                ]),
            ))
            .await
            .map(|result| result.affected_rows)
    };
    match affected {
        Ok(1) => match select_one(broker, lease, entity, &entity_id).await {
            Ok(Some(row)) => gadget_result(json!(row)),
            Ok(None) => host_error(
                "write-verification-failed",
                "the mutation could not be read back",
            ),
            Err(error) => error,
        },
        Ok(0) if !is_create => host_error(
            "revision-conflict",
            "the record changed; read its current revision before retrying",
        ),
        Ok(_) => host_error(
            "write-cardinality-invalid",
            "mutation affected an unexpected row count",
        ),
        Err(error) => broker_error(error),
    }
}

async fn select_one(
    broker: &SharedBundleBroker,
    lease: InvocationLeaseToken,
    entity: EntitySpec,
    entity_id: &str,
) -> Result<Option<BTreeMap<String, Value>>, HostResponse> {
    select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table(entity.table),
            (entity.columns)(),
        )
        .with_filter(entity.id_column, json!(entity_id))
        .with_limit(1),
    )
    .await
    .map(|rows| rows.rows.into_iter().next())
}

fn entity_values_match(
    current: &BTreeMap<String, Value>,
    expected: &BTreeMap<String, Value>,
) -> bool {
    expected.iter().all(|(key, expected)| {
        current.get(key).is_some_and(|current| {
            current == expected
                || key == "fetched_at"
                    && current
                        .as_str()
                        .zip(expected.as_str())
                        .is_some_and(|(current, expected)| {
                            DateTime::parse_from_rfc3339(current).ok()
                                == DateTime::parse_from_rfc3339(expected).ok()
                        })
        })
    })
}

async fn query_rows(broker: &SharedBundleBroker, request: DatabaseSelectRequest) -> HostResponse {
    match select(broker, request).await {
        Ok(rows) => gadget_result(json!({
            "count": rows.rows.len(),
            "rows": rows.rows,
            "truncated": rows.truncated,
        })),
        Err(error) => error,
    }
}

async fn select(
    broker: &SharedBundleBroker,
    request: DatabaseSelectRequest,
) -> Result<DatabaseRows, HostResponse> {
    broker
        .lock()
        .await
        .database_select(request)
        .await
        .map_err(broker_error)
}

fn list_input(input: Value, maximum: u32) -> Result<ListInput, &'static str> {
    let input: ListInput = serde_json::from_value(input).map_err(|_| "list input is invalid")?;
    (1..=maximum)
        .contains(&input.limit)
        .then_some(input)
        .ok_or("limit is outside the signed bound")
}

fn ids(rows: &[BTreeMap<String, Value>], key: &str) -> BTreeSet<String> {
    rows.iter()
        .filter_map(|row| text(row, key).map(str::to_owned))
        .collect()
}

fn text<'a>(row: &'a BTreeMap<String, Value>, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

fn valid_source(source_id: &str, revision: i64, observed_at: &str) -> bool {
    valid_uuid(source_id) && revision > 0 && DateTime::parse_from_rfc3339(observed_at).is_ok()
}

fn valid_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn bounded(value: &str, minimum: usize, maximum: usize) -> bool {
    let length = value.chars().count();
    (minimum..=maximum).contains(&length) && !value.contains('\0')
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

fn discussion_columns() -> Vec<String> {
    columns(&[
        "discussion_id",
        "topic_id",
        "provider",
        "external_id",
        "canonical_url",
        "title",
        "summary",
        "state",
        "score_snapshot",
        "accepted_answer_observed",
        "source_id",
        "source_revision",
        "fetched_at",
        "content_hash",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn pattern_columns() -> Vec<String> {
    columns(&[
        "pattern_id",
        "topic_id",
        "title",
        "problem_signature",
        "environment",
        "procedure",
        "rollback",
        "status",
        "supporting_evidence",
        "contradicting_evidence",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn evidence_columns() -> Vec<String> {
    columns(&[
        "evidence_id",
        "pattern_id",
        "discussion_id",
        "statement",
        "stance",
        "source_id",
        "source_revision",
        "observed_at",
        "revision",
        "created_at",
        "updated_at",
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_identity_and_pattern_text_are_bounded() {
        assert!(valid_source(
            "11111111-1111-1111-1111-111111111111",
            1,
            "2026-07-14T00:00:00Z",
        ));
        assert!(valid_hash(&"a".repeat(64)));
        assert!(bounded("rollback before retry", 1, 4_000));
        assert!(!bounded("", 1, 4_000));
    }
}
