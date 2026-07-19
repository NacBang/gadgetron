use std::collections::BTreeMap;

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

const READ_PERMISSION: &str = "news-read";
const WRITE_PERMISSION: &str = "news-write";
const COLLECTION_PERMISSION: &str = "news-collections";

#[derive(Clone, Copy)]
struct EntitySpec {
    table: &'static str,
    id_column: &'static str,
    columns: fn() -> Vec<String>,
}

const ARTICLE: EntitySpec = EntitySpec {
    table: "news_article_snapshots",
    id_column: "article_id",
    columns: article_columns,
};
const EVENT: EntitySpec = EntitySpec {
    table: "news_events",
    id_column: "event_id",
    columns: event_columns,
};
const CLAIM: EntitySpec = EntitySpec {
    table: "news_claims",
    id_column: "claim_id",
    columns: claim_columns,
};
const BRIEFING: EntitySpec = EntitySpec {
    table: "news_briefings",
    id_column: "briefing_id",
    columns: briefing_columns,
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
        "news.topic-list" => topic_list(invocation.input, lease, broker).await,
        "news.topic-collect" => topic_collect(invocation.input, lease, broker).await,
        "news.article-upsert" => article_upsert(invocation.input, lease, broker).await,
        "news.event-list" => event_list(invocation.input, lease, broker).await,
        "news.event-upsert" => event_upsert(invocation.input, lease, broker).await,
        "news.claim-list" => claim_list(invocation.input, lease, broker).await,
        "news.claim-upsert" => claim_upsert(invocation.input, lease, broker).await,
        "news.briefing-list" => briefing_list(invocation.input, lease, broker).await,
        "news.briefing-upsert" => briefing_upsert(invocation.input, lease, broker).await,
        "news.event-graph" => event_graph(invocation.input, lease, broker).await,
        "news.subject-context" => subject_context(invocation.input, lease, broker).await,
        "news.dashboard-summary" => dashboard_summary(lease, broker).await,
        _ => host_error(
            "capability-not-found",
            "requested News Intelligence capability is not available",
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
struct EventIdInput {
    event_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArticleUpsertInput {
    topic_id: String,
    canonical_url: String,
    headline: String,
    publisher: String,
    source_class: String,
    source_id: String,
    source_revision: i64,
    #[serde(default)]
    published_at: Option<String>,
    fetched_at: String,
    content_hash: String,
    summary: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventUpsertInput {
    #[serde(default)]
    event_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    topic_id: String,
    title: String,
    summary: String,
    state: String,
    first_seen_at: String,
    last_seen_at: String,
    official_sources: i32,
    editorial_sources: i32,
    community_sources: i32,
    supporting_claims: i32,
    contradicting_claims: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimUpsertInput {
    #[serde(default)]
    claim_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    event_id: String,
    #[serde(default)]
    article_id: Option<String>,
    statement: String,
    #[serde(default)]
    speaker: String,
    status: String,
    source_id: String,
    source_revision: i64,
    observed_at: String,
    #[serde(default)]
    supersedes_claim_id: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct CitationInput {
    source_id: String,
    source_revision: i64,
    title: String,
    locator: String,
    stance: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BriefingUpsertInput {
    #[serde(default)]
    briefing_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    topic_id: String,
    title: String,
    key_changes: String,
    why_it_matters: String,
    open_questions: String,
    status: String,
    window_start: String,
    window_end: String,
    official_sources: i32,
    editorial_sources: i32,
    community_sources: i32,
    supporting_claims: i32,
    contradicting_claims: i32,
    citations: Vec<CitationInput>,
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
            let rows: Vec<_> = collections
                .into_iter()
                .map(|topic| {
                    json!({
                        "collection_id": topic.collection_id,
                        "title": topic.topic,
                        "status": topic.status,
                        "source_coverage": topic.locators.len(),
                        "source_classes": topic.source_classes,
                        "schedule_enabled": topic.schedule_enabled,
                        "next_collect": topic.next_run_at,
                        "last_collect": topic.last_run_at,
                        "expected_revision": topic.revision,
                        "updated_at": topic.updated_at,
                    })
                })
                .collect();
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

async fn event_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input = match list_input(input, 500) {
        Ok(input) => input,
        Err(message) => return invalid(message),
    };
    let mut request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table(EVENT.table),
        event_columns(),
    )
    .with_order("last_seen_at", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(topic_id) = input.topic_id {
        if !valid_uuid(&topic_id) {
            return invalid("Topic must be a UUID");
        }
        request = request.with_filter("topic_id", json!(topic_id));
    }
    query_rows(broker, request).await
}

async fn claim_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        event_id: String,
        #[serde(default = "default_limit")]
        limit: u32,
    }
    let input: Input = match serde_json::from_value::<Input>(input) {
        Ok(input) if valid_uuid(&input.event_id) && (1..=500).contains(&input.limit) => input,
        _ => return invalid("Claim list input is invalid"),
    };
    query_rows(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table(CLAIM.table),
            claim_columns(),
        )
        .with_filter("event_id", json!(input.event_id))
        .with_order("observed_at", DatabaseOrderDirection::Descending)
        .with_limit(input.limit),
    )
    .await
}

async fn briefing_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input = match list_input(input, 200) {
        Ok(input) => input,
        Err(message) => return invalid(message),
    };
    let mut request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table(BRIEFING.table),
        briefing_columns(),
    )
    .with_order("window_end", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(topic_id) = input.topic_id {
        if !valid_uuid(&topic_id) {
            return invalid("Topic must be a UUID");
        }
        request = request.with_filter("topic_id", json!(topic_id));
    }
    query_rows(broker, request).await
}

async fn article_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: ArticleUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Article snapshot input is invalid"),
    };
    if !valid_uuid(&input.topic_id)
        || !valid_source(&input.source_id, input.source_revision, &input.fetched_at)
        || input
            .published_at
            .as_deref()
            .is_some_and(|time| !valid_time(time))
        || !input.canonical_url.starts_with("https://")
        || !bounded(&input.canonical_url, 10, 2_000)
        || !bounded(&input.headline, 1, 500)
        || !bounded(&input.publisher, 1, 200)
        || !matches!(
            input.source_class.as_str(),
            "official" | "editorial" | "community"
        )
        || input.content_hash.len() != 64
        || !input
            .content_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || !bounded(&input.summary, 1, 4_000)
    {
        return invalid("Article snapshot fields are outside the signed contract");
    }
    let mut article_id = None;
    let mut revision = None;
    let mut existing = None;
    let rows = match select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table(ARTICLE.table),
            article_columns(),
        )
        .with_filter("topic_id", json!(input.topic_id.clone()))
        .with_filter("source_id", json!(input.source_id.clone()))
        .with_filter("source_revision", json!(input.source_revision))
        .with_limit(2),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    if rows.len() > 1 {
        return host_error(
            "article-source-identity-conflict",
            "more than one article snapshot has the same Source revision",
        );
    }
    if let Some(row) = rows.into_iter().next() {
        article_id = row
            .get("article_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        revision = row.get("revision").and_then(Value::as_i64);
        if article_id.is_none() || revision.is_none() {
            return host_error(
                "article-state-invalid",
                "the existing article snapshot has no identity or revision",
            );
        }
        existing = Some(row);
    }
    let values = BTreeMap::from([
        ("topic_id".into(), json!(input.topic_id)),
        ("canonical_url".into(), json!(input.canonical_url)),
        ("headline".into(), json!(input.headline)),
        ("publisher".into(), json!(input.publisher)),
        ("source_class".into(), json!(input.source_class)),
        ("source_id".into(), json!(input.source_id)),
        ("source_revision".into(), json!(input.source_revision)),
        ("published_at".into(), option_json(input.published_at)),
        ("fetched_at".into(), json!(input.fetched_at)),
        (
            "content_hash".into(),
            json!(input.content_hash.to_ascii_lowercase()),
        ),
        ("summary".into(), json!(input.summary)),
    ]);
    if existing
        .as_ref()
        .is_some_and(|row| entity_values_match(row, &values))
    {
        return gadget_result(json!(existing.expect("existing row was checked")));
    }
    upsert(broker, lease, ARTICLE, article_id, revision, values).await
}

async fn event_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: EventUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("News event input is invalid"),
    };
    if !valid_uuid(&input.topic_id)
        || !bounded(&input.title, 1, 300)
        || !bounded(&input.summary, 1, 4_000)
        || !matches!(
            input.state.as_str(),
            "developing" | "confirmed" | "corrected" | "uncertain" | "closed"
        )
        || !valid_time(&input.first_seen_at)
        || !valid_time(&input.last_seen_at)
        || DateTime::parse_from_rfc3339(&input.first_seen_at).ok()
            > DateTime::parse_from_rfc3339(&input.last_seen_at).ok()
        || [
            input.official_sources,
            input.editorial_sources,
            input.community_sources,
            input.supporting_claims,
            input.contradicting_claims,
        ]
        .into_iter()
        .any(|count| !(0..=10_000).contains(&count))
    {
        return invalid("News event fields are outside the signed contract");
    }
    let values = BTreeMap::from([
        ("topic_id".into(), json!(input.topic_id)),
        ("title".into(), json!(input.title)),
        ("summary".into(), json!(input.summary)),
        ("state".into(), json!(input.state)),
        ("first_seen_at".into(), json!(input.first_seen_at)),
        ("last_seen_at".into(), json!(input.last_seen_at)),
        ("official_sources".into(), json!(input.official_sources)),
        ("editorial_sources".into(), json!(input.editorial_sources)),
        ("community_sources".into(), json!(input.community_sources)),
        ("supporting_claims".into(), json!(input.supporting_claims)),
        (
            "contradicting_claims".into(),
            json!(input.contradicting_claims),
        ),
    ]);
    upsert(broker, lease, EVENT, input.event_id, input.revision, values).await
}

async fn claim_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: ClaimUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("News claim input is invalid"),
    };
    if !valid_uuid(&input.event_id)
        || input
            .article_id
            .as_deref()
            .is_some_and(|id| !valid_uuid(id))
        || input
            .supersedes_claim_id
            .as_deref()
            .is_some_and(|id| !valid_uuid(id))
        || !bounded(&input.statement, 1, 2_000)
        || !bounded(&input.speaker, 0, 300)
        || !matches!(
            input.status.as_str(),
            "reported" | "corroborated" | "contradicted" | "corrected" | "unverified"
        )
        || !valid_source(&input.source_id, input.source_revision, &input.observed_at)
    {
        return invalid("News claim fields are outside the signed contract");
    }
    if input.status == "corrected" && input.supersedes_claim_id.is_none() {
        return invalid("A corrected claim must identify the claim it supersedes");
    }
    let values = BTreeMap::from([
        ("event_id".into(), json!(input.event_id)),
        ("article_id".into(), option_json(input.article_id)),
        ("statement".into(), json!(input.statement)),
        ("speaker".into(), json!(input.speaker)),
        ("status".into(), json!(input.status)),
        ("source_id".into(), json!(input.source_id)),
        ("source_revision".into(), json!(input.source_revision)),
        ("observed_at".into(), json!(input.observed_at)),
        (
            "supersedes_claim_id".into(),
            option_json(input.supersedes_claim_id),
        ),
    ]);
    upsert(broker, lease, CLAIM, input.claim_id, input.revision, values).await
}

async fn briefing_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: BriefingUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("News briefing input is invalid"),
    };
    if !valid_uuid(&input.topic_id)
        || !bounded(&input.title, 1, 300)
        || !bounded(&input.key_changes, 1, 6_000)
        || !bounded(&input.why_it_matters, 1, 4_000)
        || !bounded(&input.open_questions, 0, 4_000)
        || !matches!(
            input.status.as_str(),
            "current" | "aging" | "stale" | "conflicted"
        )
        || !valid_time(&input.window_start)
        || !valid_time(&input.window_end)
        || input.citations.len() < 2
        || input.citations.len() > 64
        || input.citations.iter().any(|citation| {
            !valid_uuid(&citation.source_id)
                || citation.source_revision < 1
                || !bounded(&citation.title, 1, 500)
                || !bounded(&citation.locator, 1, 2_000)
                || !matches!(
                    citation.stance.as_str(),
                    "supports" | "contradicts" | "context"
                )
        })
        || [
            input.official_sources,
            input.editorial_sources,
            input.community_sources,
            input.supporting_claims,
            input.contradicting_claims,
        ]
        .into_iter()
        .any(|count| !(0..=10_000).contains(&count))
    {
        return invalid("News briefing fields are outside the signed contract");
    }
    let citations = serde_json::to_value(&input.citations)
        .expect("validated citation records are serializable");
    let values = BTreeMap::from([
        ("topic_id".into(), json!(input.topic_id)),
        ("title".into(), json!(input.title)),
        ("key_changes".into(), json!(input.key_changes)),
        ("why_it_matters".into(), json!(input.why_it_matters)),
        ("open_questions".into(), json!(input.open_questions)),
        ("status".into(), json!(input.status)),
        ("window_start".into(), json!(input.window_start)),
        ("window_end".into(), json!(input.window_end)),
        ("official_sources".into(), json!(input.official_sources)),
        ("editorial_sources".into(), json!(input.editorial_sources)),
        ("community_sources".into(), json!(input.community_sources)),
        ("supporting_claims".into(), json!(input.supporting_claims)),
        (
            "contradicting_claims".into(),
            json!(input.contradicting_claims),
        ),
        ("citations".into(), citations),
    ]);
    upsert(
        broker,
        lease,
        BRIEFING,
        input.briefing_id,
        input.revision,
        values,
    )
    .await
}

async fn event_graph(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input = match list_input(input, 200) {
        Ok(input) => input,
        Err(message) => return invalid(message),
    };
    let mut event_request = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table(EVENT.table),
        event_columns(),
    )
    .with_order("last_seen_at", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(topic_id) = input.topic_id {
        if !valid_uuid(&topic_id) {
            return invalid("Topic must be a UUID");
        }
        event_request = event_request.with_filter("topic_id", json!(topic_id));
    }
    let events = match select(broker, event_request).await {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let claims = match select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table(CLAIM.table),
            claim_columns(),
        )
        .with_order("observed_at", DatabaseOrderDirection::Descending)
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let event_ids: std::collections::BTreeSet<_> = events
        .iter()
        .filter_map(|event| event.get("event_id").and_then(Value::as_str))
        .collect();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for event in &events {
        let Some(event_id) = event.get("event_id").and_then(Value::as_str) else {
            continue;
        };
        nodes.push(json!({
            "id": event_id,
            "label": event.get("title").and_then(Value::as_str).unwrap_or("News event"),
            "kind": "event",
            "status": event.get("state"),
        }));
    }
    for claim in &claims {
        let (Some(claim_id), Some(event_id)) = (
            claim.get("claim_id").and_then(Value::as_str),
            claim.get("event_id").and_then(Value::as_str),
        ) else {
            continue;
        };
        if !event_ids.contains(event_id) {
            continue;
        }
        nodes.push(json!({
            "id": claim_id,
            "label": claim.get("statement").and_then(Value::as_str).unwrap_or("Claim"),
            "kind": "claim",
            "status": claim.get("status"),
        }));
        edges.push(json!({
            "source": claim_id,
            "target": event_id,
            "label": claim.get("status").and_then(Value::as_str).unwrap_or("reports on"),
        }));
        if let Some(previous) = claim.get("supersedes_claim_id").and_then(Value::as_str) {
            edges.push(json!({
                "source": claim_id,
                "target": previous,
                "label": "corrects",
            }));
        }
    }
    gadget_result(json!({"nodes": nodes, "edges": edges}))
}

async fn subject_context(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: EventIdInput = match serde_json::from_value::<EventIdInput>(input) {
        Ok(input) if valid_uuid(&input.event_id) => input,
        _ => return invalid("Event id must be a UUID"),
    };
    let event = match select_one(broker, lease.clone(), EVENT, &input.event_id).await {
        Ok(Some(event)) => event,
        Ok(None) => return host_error("event-not-found", "News event is not available"),
        Err(error) => return error,
    };
    let claims = match select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table(CLAIM.table),
            claim_columns(),
        )
        .with_filter("event_id", json!(input.event_id))
        .with_order("observed_at", DatabaseOrderDirection::Descending)
        .with_limit(50),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    gadget_result(json!({
        "id": event.get("event_id"),
        "kind": "news_event",
        "bundle": "news-intelligence",
        "title": event.get("title"),
        "subtitle": event.get("state"),
        "href": format!("/web/workspace?id=news-intelligence.news&event={}", input.event_id),
        "facts": {
            "topic_id": event.get("topic_id"),
            "summary": event.get("summary"),
            "last_seen_at": event.get("last_seen_at"),
            "official_sources": event.get("official_sources"),
            "editorial_sources": event.get("editorial_sources"),
            "community_sources": event.get("community_sources"),
            "claims": claims,
        },
        "prompt": "이 뉴스 사건의 변화, 근거 다양성, 상충·정정 사항과 관련 지식 영향을 인용과 함께 검토해줘.",
    }))
}

async fn dashboard_summary(
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let events = match select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table(EVENT.table),
            columns(&["state", "last_seen_at"]),
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let briefings = match select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table(BRIEFING.table),
            columns(&["status", "window_end"]),
        )
        .with_limit(200),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let count = |status: &str| {
        events
            .iter()
            .filter(|event| event.get("state").and_then(Value::as_str) == Some(status))
            .count()
    };
    let stale = briefings
        .iter()
        .filter(|briefing| {
            matches!(
                briefing.get("status").and_then(Value::as_str),
                Some("aging" | "stale" | "conflicted")
            )
        })
        .count();
    gadget_result(json!({
        "active events": events.len(),
        "developing": count("developing"),
        "uncertain or corrected": count("uncertain") + count("corrected"),
        "briefings needing attention": stale,
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
                || matches!(key.as_str(), "published_at" | "fetched_at")
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
        Ok(rows) => rows_result(rows),
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

fn rows_result(rows: DatabaseRows) -> HostResponse {
    gadget_result(json!({
        "count": rows.rows.len(),
        "rows": rows.rows,
        "truncated": rows.truncated,
    }))
}

fn list_input(input: Value, maximum: u32) -> Result<ListInput, &'static str> {
    let input: ListInput = serde_json::from_value(input).map_err(|_| "list input is invalid")?;
    (1..=maximum)
        .contains(&input.limit)
        .then_some(input)
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

fn article_columns() -> Vec<String> {
    columns(&[
        "article_id",
        "topic_id",
        "canonical_url",
        "headline",
        "publisher",
        "source_class",
        "source_id",
        "source_revision",
        "published_at",
        "fetched_at",
        "content_hash",
        "summary",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn event_columns() -> Vec<String> {
    columns(&[
        "event_id",
        "topic_id",
        "title",
        "summary",
        "state",
        "first_seen_at",
        "last_seen_at",
        "official_sources",
        "editorial_sources",
        "community_sources",
        "supporting_claims",
        "contradicting_claims",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn claim_columns() -> Vec<String> {
    columns(&[
        "claim_id",
        "event_id",
        "article_id",
        "statement",
        "speaker",
        "status",
        "source_id",
        "source_revision",
        "observed_at",
        "supersedes_claim_id",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn briefing_columns() -> Vec<String> {
    columns(&[
        "briefing_id",
        "topic_id",
        "title",
        "key_changes",
        "why_it_matters",
        "open_questions",
        "status",
        "window_start",
        "window_end",
        "official_sources",
        "editorial_sources",
        "community_sources",
        "supporting_claims",
        "contradicting_claims",
        "citations",
        "revision",
        "created_at",
        "updated_at",
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_and_time_inputs_are_bounded() {
        assert!(valid_source(
            "11111111-1111-1111-1111-111111111111",
            1,
            "2026-07-13T00:00:00Z",
        ));
        assert!(!valid_source("not-an-id", 1, "2026-07-13T00:00:00Z"));
        assert!(bounded("A concise claim", 1, 2_000));
        assert!(!bounded("", 1, 2_000));
    }
}
