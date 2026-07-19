use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, SecondsFormat, Utc};
use gadgetron_bundle_runtime::SharedBundleBroker;
use gadgetron_bundle_sdk::{
    BrokerResource, DatabaseDeleteRequest, DatabaseInsertRequest, DatabaseOrderDirection,
    DatabaseRows, DatabaseSelectRequest, DatabaseUpdateRequest, GadgetInvocation, HostResponse,
    InvocationLeaseToken, KnowledgeCollectionAction, KnowledgeCollectionRequest,
    KnowledgeCollectionResult, LocalId,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{broker_error, gadget_result, host_error};

const READ_PERMISSION: &str = "social-read";
const WRITE_PERMISSION: &str = "social-write";
const PURGE_PERMISSION: &str = "social-purge";
const COLLECTION_PERMISSION: &str = "social-collections";

#[derive(Clone, Copy)]
struct EntitySpec {
    table: &'static str,
    id_column: &'static str,
    columns: fn() -> Vec<String>,
}

const POST: EntitySpec = EntitySpec {
    table: "social_posts",
    id_column: "post_id",
    columns: post_columns,
};
const CONVERSATION: EntitySpec = EntitySpec {
    table: "social_conversations",
    id_column: "conversation_id",
    columns: conversation_columns,
};
const SIGNAL: EntitySpec = EntitySpec {
    table: "social_signals",
    id_column: "signal_id",
    columns: signal_columns,
};
const BRIEFING: EntitySpec = EntitySpec {
    table: "social_briefings",
    id_column: "briefing_id",
    columns: briefing_columns,
};
const DRAFT: EntitySpec = EntitySpec {
    table: "social_response_drafts",
    id_column: "draft_id",
    columns: draft_columns,
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
        "social.watchlist-list" => watchlist_list(invocation.input, lease, broker).await,
        "social.watchlist-collect" => watchlist_collect(invocation.input, lease, broker).await,
        "social.post-list" => post_list(invocation.input, lease, broker).await,
        "social.post-upsert" => post_upsert(invocation.input, lease, broker).await,
        "social.conversation-list" => conversation_list(invocation.input, lease, broker).await,
        "social.conversation-upsert" => conversation_upsert(invocation.input, lease, broker).await,
        "social.signal-list" => signal_list(invocation.input, lease, broker).await,
        "social.signal-upsert" => signal_upsert(invocation.input, lease, broker).await,
        "social.briefing-list" => briefing_list(invocation.input, lease, broker).await,
        "social.briefing-upsert" => briefing_upsert(invocation.input, lease, broker).await,
        "social.response-draft-list" => draft_list(invocation.input, lease, broker).await,
        "social.response-draft-upsert" => draft_upsert(invocation.input, lease, broker).await,
        "social.response-draft-handoff" => draft_handoff(invocation.input, lease, broker).await,
        "social.source-purge" => source_purge(invocation.input, lease, broker).await,
        "social.signal-graph" => signal_graph(invocation.input, lease, broker).await,
        "social.subject-context" => subject_context(invocation.input, lease, broker).await,
        "social.dashboard-summary" => dashboard_summary(lease, broker).await,
        _ => host_error(
            "capability-not-found",
            "requested Social Media Intelligence capability is not available",
        ),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicListInput {
    #[serde(default)]
    topic_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationListInput {
    #[serde(default)]
    conversation_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BriefingListInput {
    #[serde(default)]
    briefing_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchlistCollectInput {
    collection_id: String,
    expected_revision: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostUpsertInput {
    topic_id: String,
    provider: String,
    external_uri: String,
    cid: String,
    author_handle: String,
    text_excerpt: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    reply_to_uri: Option<String>,
    #[serde(default)]
    quote_uri: Option<String>,
    state: String,
    engagement: Value,
    #[serde(default)]
    moderation_labels: Vec<String>,
    source_id: String,
    source_revision: i64,
    fetched_at: String,
    content_hash: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationUpsertInput {
    #[serde(default)]
    conversation_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    topic_id: String,
    title: String,
    summary: String,
    origin_uri: String,
    post_count: i32,
    status: String,
    first_seen_at: String,
    last_seen_at: String,
    source_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignalUpsertInput {
    #[serde(default)]
    signal_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    conversation_id: String,
    kind: String,
    statement: String,
    confidence_basis: String,
    status: String,
    supporting_posts: Vec<String>,
    contradicting_posts: Vec<String>,
    window_start: String,
    window_end: String,
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
    citations: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftUpsertInput {
    #[serde(default)]
    draft_id: Option<String>,
    #[serde(default)]
    revision: Option<i64>,
    briefing_id: String,
    provider: String,
    target_account: String,
    audience: String,
    objective: String,
    body: String,
    impact_preview: String,
    risk_notes: String,
    status: String,
    citations: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftHandoffInput {
    draft_id: String,
    revision: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourcePurgeInput {
    source_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BriefingInput {
    briefing_id: String,
}

async fn watchlist_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: TopicListInput = match serde_json::from_value::<TopicListInput>(input) {
        Ok(input) if input.topic_id.is_none() && (1..=200).contains(&input.limit) => input,
        _ => return invalid("Watchlist list input is outside the signed bound"),
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
                .map(|watchlist| {
                    json!({
                        "collection_id": watchlist.collection_id,
                        "title": watchlist.topic,
                        "status": watchlist.status,
                        "providers": watchlist.queries.iter().map(|query| query.provider.as_str()).collect::<Vec<_>>(),
                        "source_coverage": watchlist.locators.len(),
                        "schedule_enabled": watchlist.schedule_enabled,
                        "next_collect": watchlist.next_run_at,
                        "last_collect": watchlist.last_run_at,
                        "expected_revision": watchlist.revision,
                        "updated_at": watchlist.updated_at,
                    })
                })
                .collect::<Vec<_>>();
            gadget_result(json!({"count": rows.len(), "rows": rows, "truncated": truncated}))
        }
        Ok(_) => host_error(
            "collection-response-invalid",
            "Core returned an unexpected watchlist list result",
        ),
        Err(error) => broker_error(error),
    }
}

async fn watchlist_collect(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: WatchlistCollectInput = match serde_json::from_value::<WatchlistCollectInput>(input)
    {
        Ok(input) if valid_uuid(&input.collection_id) && input.expected_revision > 0 => input,
        _ => return invalid("Watchlist collection id or revision is invalid"),
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
            "watchlist_id": collection_id,
            "run_id": run_id,
            "status": status,
            "started": created,
        })),
        Ok(_) => host_error(
            "collection-response-invalid",
            "Core returned an unexpected watchlist collection result",
        ),
        Err(error) => broker_error(error),
    }
}

async fn post_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    list_by_topic(input, lease, broker, POST, "fetched_at", 500).await
}

async fn conversation_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    list_by_topic(input, lease, broker, CONVERSATION, "last_seen_at", 500).await
}

async fn signal_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: ConversationListInput = match serde_json::from_value::<ConversationListInput>(input)
    {
        Ok(input) if (1..=500).contains(&input.limit) => input,
        _ => return invalid("Signal list input is invalid"),
    };
    let mut request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table(SIGNAL.table),
        signal_columns(),
    )
    .with_order("window_end", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(conversation_id) = input.conversation_id {
        if !valid_uuid(&conversation_id) {
            return invalid("Conversation must be a UUID");
        }
        request = request.with_filter("conversation_id", json!(conversation_id));
    }
    query_rows(broker, request).await
}

async fn briefing_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    list_by_topic(input, lease, broker, BRIEFING, "window_end", 500).await
}

async fn draft_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: BriefingListInput = match serde_json::from_value::<BriefingListInput>(input) {
        Ok(input) if (1..=500).contains(&input.limit) => input,
        _ => return invalid("Response draft list input is invalid"),
    };
    let mut request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table(DRAFT.table),
        draft_columns(),
    )
    .with_order("updated_at", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(briefing_id) = input.briefing_id {
        if !valid_uuid(&briefing_id) {
            return invalid("Briefing must be a UUID");
        }
        request = request.with_filter("briefing_id", json!(briefing_id));
    }
    query_rows(broker, request).await
}

async fn list_by_topic(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
    entity: EntitySpec,
    order: &str,
    maximum: u32,
) -> HostResponse {
    let input: TopicListInput = match serde_json::from_value::<TopicListInput>(input) {
        Ok(input) if (1..=maximum).contains(&input.limit) => input,
        _ => return invalid("Topic list input is invalid"),
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

async fn post_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: PostUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Public post snapshot input is invalid"),
    };
    if !valid_uuid(&input.topic_id)
        || input.provider != "bluesky"
        || !valid_at_uri(&input.external_uri)
        || !bounded(&input.cid, 1, 200)
        || !valid_handle(&input.author_handle)
        || !bounded(&input.text_excerpt, 1, 1_000)
        || !optional_bounded(input.language.as_deref(), 2, 35)
        || !optional_at_uri(input.reply_to_uri.as_deref())
        || !optional_at_uri(input.quote_uri.as_deref())
        || !matches!(
            input.state.as_str(),
            "current" | "edited" | "deleted" | "moderated"
        )
        || !valid_engagement(&input.engagement)
        || !valid_string_array(&input.moderation_labels, 50, 100)
        || !valid_source(&input.source_id, input.source_revision, &input.fetched_at)
        || !valid_hash(&input.content_hash)
    {
        return invalid("Public post fields are outside the signed contract");
    }
    let values = BTreeMap::from([
        ("topic_id".into(), json!(input.topic_id.clone())),
        ("provider".into(), json!(input.provider.clone())),
        ("external_uri".into(), json!(input.external_uri.clone())),
        ("cid".into(), json!(input.cid)),
        ("author_handle".into(), json!(input.author_handle)),
        ("text_excerpt".into(), json!(input.text_excerpt)),
        ("language".into(), json!(input.language)),
        ("reply_to_uri".into(), json!(input.reply_to_uri)),
        ("quote_uri".into(), json!(input.quote_uri)),
        ("state".into(), json!(input.state)),
        ("engagement".into(), input.engagement),
        ("moderation_labels".into(), json!(input.moderation_labels)),
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
            table(POST.table),
            post_columns(),
        )
        .with_filter("topic_id", json!(input.topic_id))
        .with_filter("provider", json!(input.provider))
        .with_filter("external_uri", json!(input.external_uri))
        .with_filter("source_id", json!(input.source_id))
        .with_filter("source_revision", json!(input.source_revision))
        .with_limit(2),
    )
    .await
    {
        Ok(rows) if rows.rows.len() <= 1 => rows.rows.into_iter().next(),
        Ok(_) => {
            return host_error(
                "post-source-identity-conflict",
                "more than one post has the same provider Source revision",
            )
        }
        Err(error) => return error,
    };
    if let Some(existing) = existing {
        if entity_values_match(&existing, &values) {
            return gadget_result(json!(existing));
        }
        let Some(post_id) = text(&existing, "post_id") else {
            return host_error("post-state-invalid", "post identity is unavailable");
        };
        let Some(revision) = existing.get("revision").and_then(Value::as_i64) else {
            return host_error("post-state-invalid", "post revision is unavailable");
        };
        return upsert(
            broker,
            lease,
            POST,
            Some(post_id.to_string()),
            Some(revision),
            values,
        )
        .await;
    }
    upsert(broker, lease, POST, None, None, values).await
}

async fn conversation_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: ConversationUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Conversation input is invalid"),
    };
    if !valid_uuid(&input.topic_id)
        || !bounded(&input.title, 1, 300)
        || !bounded(&input.summary, 1, 4_000)
        || !valid_at_uri(&input.origin_uri)
        || !(1..=10_000).contains(&input.post_count)
        || !matches!(
            input.status.as_str(),
            "current" | "aging" | "stale" | "conflicted"
        )
        || !valid_window(&input.first_seen_at, &input.last_seen_at)
        || !valid_uuid_array(&input.source_ids, 200)
    {
        return invalid("Conversation fields are outside the signed contract");
    }
    let values = BTreeMap::from([
        ("topic_id".into(), json!(input.topic_id)),
        ("title".into(), json!(input.title)),
        ("summary".into(), json!(input.summary)),
        ("origin_uri".into(), json!(input.origin_uri)),
        ("post_count".into(), json!(input.post_count)),
        ("status".into(), json!(input.status)),
        ("first_seen_at".into(), json!(input.first_seen_at)),
        ("last_seen_at".into(), json!(input.last_seen_at)),
        ("source_ids".into(), json!(input.source_ids)),
    ]);
    upsert(
        broker,
        lease,
        CONVERSATION,
        input.conversation_id,
        input.revision,
        values,
    )
    .await
}

async fn signal_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: SignalUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Social signal input is invalid"),
    };
    let overlap = input
        .supporting_posts
        .iter()
        .any(|post| input.contradicting_posts.contains(post));
    if !valid_uuid(&input.conversation_id)
        || !matches!(
            input.kind.as_str(),
            "claim" | "trend" | "audience" | "question" | "correction"
        )
        || !bounded(&input.statement, 1, 2_000)
        || !bounded(&input.confidence_basis, 1, 2_000)
        || !matches!(
            input.status.as_str(),
            "observed" | "speculative" | "corroborated" | "contradicted"
        )
        || !valid_optional_uuid_array(&input.supporting_posts, 200)
        || !valid_optional_uuid_array(&input.contradicting_posts, 200)
        || input.supporting_posts.is_empty() && input.contradicting_posts.is_empty()
        || overlap
        || !valid_window(&input.window_start, &input.window_end)
    {
        return invalid("Social signal fields are outside the signed contract");
    }
    let values = BTreeMap::from([
        ("conversation_id".into(), json!(input.conversation_id)),
        ("kind".into(), json!(input.kind)),
        ("statement".into(), json!(input.statement)),
        ("confidence_basis".into(), json!(input.confidence_basis)),
        ("status".into(), json!(input.status)),
        ("supporting_posts".into(), json!(input.supporting_posts)),
        (
            "contradicting_posts".into(),
            json!(input.contradicting_posts),
        ),
        ("window_start".into(), json!(input.window_start)),
        ("window_end".into(), json!(input.window_end)),
    ]);
    upsert(
        broker,
        lease,
        SIGNAL,
        input.signal_id,
        input.revision,
        values,
    )
    .await
}

async fn briefing_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: BriefingUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Social briefing input is invalid"),
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
        || !valid_window(&input.window_start, &input.window_end)
        || !valid_briefing_citations(&input.citations, 200)
    {
        return invalid("Social briefing fields are outside the signed contract");
    }
    let values = BTreeMap::from([
        ("topic_id".into(), json!(input.topic_id)),
        ("title".into(), json!(input.title)),
        ("key_changes".into(), json!(input.key_changes)),
        ("why_it_matters".into(), json!(input.why_it_matters)),
        ("open_questions".into(), json!(input.open_questions)),
        ("status".into(), json!(input.status)),
        ("window_start".into(), json!(input.window_start)),
        ("window_end".into(), json!(input.window_end)),
        ("citations".into(), json!(input.citations)),
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

async fn draft_upsert(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: DraftUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("Response draft input is invalid"),
    };
    if !valid_uuid(&input.briefing_id)
        || input.provider != "bluesky"
        || !valid_handle(&input.target_account)
        || !bounded(&input.audience, 1, 500)
        || !bounded(&input.objective, 1, 1_000)
        || !bounded(&input.body, 1, 300)
        || !bounded(&input.impact_preview, 1, 2_000)
        || !bounded(&input.risk_notes, 1, 2_000)
        || !matches!(
            input.status.as_str(),
            "draft" | "reviewed" | "handed_off" | "withdrawn"
        )
        || !valid_citations(&input.citations, 200)
    {
        return invalid("Response draft fields are outside the signed contract");
    }
    let values = BTreeMap::from([
        ("briefing_id".into(), json!(input.briefing_id)),
        ("provider".into(), json!(input.provider)),
        ("target_account".into(), json!(input.target_account)),
        ("audience".into(), json!(input.audience)),
        ("objective".into(), json!(input.objective)),
        ("body".into(), json!(input.body)),
        ("impact_preview".into(), json!(input.impact_preview)),
        ("risk_notes".into(), json!(input.risk_notes)),
        ("status".into(), json!(input.status)),
        ("citations".into(), json!(input.citations)),
    ]);
    upsert(broker, lease, DRAFT, input.draft_id, input.revision, values).await
}

async fn draft_handoff(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: DraftHandoffInput = match serde_json::from_value::<DraftHandoffInput>(input) {
        Ok(input) if valid_uuid(&input.draft_id) && input.revision > 0 => input,
        _ => return invalid("Draft id and current revision are required"),
    };
    let draft = match select_one(broker, lease, DRAFT, &input.draft_id).await {
        Ok(Some(draft)) => draft,
        Ok(None) => return host_error("draft-not-found", "Response draft is not available"),
        Err(error) => return error,
    };
    if draft.get("revision").and_then(Value::as_i64) != Some(input.revision) {
        return host_error(
            "revision-conflict",
            "the response draft changed; read its current revision before handoff",
        );
    }
    if !matches!(text(&draft, "status"), Some("draft" | "reviewed")) {
        return host_error(
            "draft-state-invalid",
            "only an active draft can be handed to a human reviewer",
        );
    }
    gadget_result(json!({
        "draft_id": draft.get("draft_id"),
        "draft_revision": draft.get("revision"),
        "provider": draft.get("provider"),
        "target_account": draft.get("target_account"),
        "audience": draft.get("audience"),
        "objective": draft.get("objective"),
        "body": draft.get("body"),
        "impact_preview": draft.get("impact_preview"),
        "risk_notes": draft.get("risk_notes"),
        "citations": draft.get("citations"),
        "publish_allowed": false,
        "next_step": "A human may copy or revise this text outside Gadgetron after reviewing the cited evidence.",
    }))
}

async fn source_purge(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: SourcePurgeInput = match serde_json::from_value::<SourcePurgeInput>(input) {
        Ok(input) if valid_uuid(&input.source_id) => input,
        _ => return invalid("Source id must be a UUID"),
    };
    let posts = match select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table(POST.table),
            columns(&["post_id", "topic_id", "source_id"]),
        )
        .with_filter("source_id", json!(input.source_id.clone()))
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let topic_ids = posts
        .iter()
        .filter_map(|post| text(post, "topic_id").map(str::to_owned))
        .collect::<BTreeSet<_>>();
    let mut briefings_deleted = 0;
    let mut conversations_deleted = 0;
    for topic_id in &topic_ids {
        briefings_deleted += match delete_all(
            broker,
            lease.clone(),
            BRIEFING.table,
            BTreeMap::from([("topic_id".into(), json!(topic_id))]),
        )
        .await
        {
            Ok(count) => count,
            Err(error) => return error,
        };
        conversations_deleted += match delete_all(
            broker,
            lease.clone(),
            CONVERSATION.table,
            BTreeMap::from([("topic_id".into(), json!(topic_id))]),
        )
        .await
        {
            Ok(count) => count,
            Err(error) => return error,
        };
    }
    let posts_deleted = match delete_all(
        broker,
        lease,
        POST.table,
        BTreeMap::from([("source_id".into(), json!(input.source_id.clone()))]),
    )
    .await
    {
        Ok(count) => count,
        Err(error) => return error,
    };
    gadget_result(json!({
        "source_id": input.source_id,
        "posts_deleted": posts_deleted,
        "conversations_deleted": conversations_deleted,
        "briefings_deleted": briefings_deleted,
        "invalidated_topic_ids": topic_ids,
        "core_source_purge_required": true,
        "next_step": "Delete the same Source in Knowledge to remove its purgeable Core blob and metadata note.",
    }))
}

async fn signal_graph(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: TopicListInput = match serde_json::from_value::<TopicListInput>(input) {
        Ok(input) if (1..=200).contains(&input.limit) => input,
        _ => return invalid("Signal graph input is invalid"),
    };
    if input.topic_id.as_deref().is_some_and(|id| !valid_uuid(id)) {
        return invalid("Topic must be a UUID");
    }
    let mut post_request = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table(POST.table),
        post_columns(),
    )
    .with_order("fetched_at", DatabaseOrderDirection::Descending)
    .with_limit(500);
    let mut conversation_request = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table(CONVERSATION.table),
        conversation_columns(),
    )
    .with_order("last_seen_at", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    let mut briefing_request = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table(BRIEFING.table),
        briefing_columns(),
    )
    .with_order("window_end", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(topic_id) = input.topic_id {
        post_request = post_request.with_filter("topic_id", json!(topic_id.clone()));
        conversation_request =
            conversation_request.with_filter("topic_id", json!(topic_id.clone()));
        briefing_request = briefing_request.with_filter("topic_id", json!(topic_id));
    }
    let posts = match select(broker, post_request).await {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let conversations = match select(broker, conversation_request).await {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let conversation_ids = ids(&conversations, "conversation_id");
    let signals = match select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table(SIGNAL.table),
            signal_columns(),
        )
        .with_order("window_end", DatabaseOrderDirection::Descending)
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows
            .rows
            .into_iter()
            .filter(|row| {
                text(row, "conversation_id").is_some_and(|id| conversation_ids.contains(id))
            })
            .collect::<Vec<_>>(),
        Err(error) => return error,
    };
    let briefings = match select(broker, briefing_request).await {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let drafts = match select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table(DRAFT.table),
            draft_columns(),
        )
        .with_order("updated_at", DatabaseOrderDirection::Descending)
        .with_limit(200),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let post_ids = ids(&posts, "post_id");
    let signal_ids = ids(&signals, "signal_id");
    let briefing_ids = ids(&briefings, "briefing_id");
    let origins = conversations
        .iter()
        .filter_map(|row| {
            text(row, "origin_uri")
                .zip(text(row, "conversation_id"))
                .map(|(uri, id)| (uri.to_string(), id.to_string()))
        })
        .collect::<BTreeMap<_, _>>();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for post in &posts {
        let (Some(post_id), Some(uri)) = (text(post, "post_id"), text(post, "external_uri")) else {
            continue;
        };
        nodes.push(json!({"id": post_id, "label": text(post, "text_excerpt").unwrap_or("Public post"), "kind": "social_post", "status": post.get("state")}));
        if let Some(conversation_id) = origins.get(uri) {
            edges.push(json!({"source": post_id, "target": conversation_id, "label": "starts conversation"}));
        }
    }
    for conversation in &conversations {
        let Some(conversation_id) = text(conversation, "conversation_id") else {
            continue;
        };
        nodes.push(json!({"id": conversation_id, "label": text(conversation, "title").unwrap_or("Conversation"), "kind": "conversation", "status": conversation.get("status")}));
    }
    for signal in &signals {
        let (Some(signal_id), Some(conversation_id)) =
            (text(signal, "signal_id"), text(signal, "conversation_id"))
        else {
            continue;
        };
        nodes.push(json!({"id": signal_id, "label": text(signal, "statement").unwrap_or("Signal"), "kind": "signal", "status": signal.get("status")}));
        edges
            .push(json!({"source": signal_id, "target": conversation_id, "label": "derived from"}));
        for post_id in string_values(signal.get("supporting_posts")) {
            if post_ids.contains(post_id) {
                edges.push(json!({"source": post_id, "target": signal_id, "label": "supports"}));
            }
        }
        for post_id in string_values(signal.get("contradicting_posts")) {
            if post_ids.contains(post_id) {
                edges.push(json!({"source": post_id, "target": signal_id, "label": "contradicts"}));
            }
        }
    }
    for briefing in &briefings {
        let Some(briefing_id) = text(briefing, "briefing_id") else {
            continue;
        };
        nodes.push(json!({"id": briefing_id, "label": text(briefing, "title").unwrap_or("Briefing"), "kind": "social_briefing", "status": briefing.get("status")}));
        for signal_id in citation_ids(briefing.get("citations"), "signal_id") {
            if signal_ids.contains(signal_id) {
                edges.push(json!({"source": briefing_id, "target": signal_id, "label": "cites"}));
            }
        }
    }
    for draft in drafts {
        let (Some(draft_id), Some(briefing_id)) =
            (text(&draft, "draft_id"), text(&draft, "briefing_id"))
        else {
            continue;
        };
        if !briefing_ids.contains(briefing_id) {
            continue;
        }
        nodes.push(json!({"id": draft_id, "label": text(&draft, "objective").unwrap_or("Response draft"), "kind": "response_draft", "status": draft.get("status")}));
        edges.push(json!({"source": draft_id, "target": briefing_id, "label": "drafted from"}));
    }
    gadget_result(json!({"nodes": nodes, "edges": edges}))
}

async fn subject_context(
    input: Value,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let input: BriefingInput = match serde_json::from_value::<BriefingInput>(input) {
        Ok(input) if valid_uuid(&input.briefing_id) => input,
        _ => return invalid("Briefing id must be a UUID"),
    };
    let briefing = match select_one(broker, lease, BRIEFING, &input.briefing_id).await {
        Ok(Some(briefing)) => briefing,
        Ok(None) => return host_error("briefing-not-found", "Social briefing is not available"),
        Err(error) => return error,
    };
    gadget_result(json!({
        "id": briefing.get("briefing_id"),
        "kind": "social_briefing",
        "bundle": "social-media-intelligence",
        "title": briefing.get("title"),
        "subtitle": briefing.get("status"),
        "href": format!("/web/workspace?id=social-media-intelligence.briefings&briefing={}", input.briefing_id),
        "facts": {
            "topic_id": briefing.get("topic_id"),
            "key_changes": briefing.get("key_changes"),
            "why_it_matters": briefing.get("why_it_matters"),
            "open_questions": briefing.get("open_questions"),
            "window_start": briefing.get("window_start"),
            "window_end": briefing.get("window_end"),
            "citations": briefing.get("citations"),
            "revision": briefing.get("revision"),
        },
        "prompt": "이 공개 소셜 대화에서 무엇이 변했는지, 근거와 반례, 영향 범위, 아직 모르는 점을 인용과 함께 검토해줘.",
    }))
}

async fn dashboard_summary(
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let conversations = match select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table(CONVERSATION.table),
            columns(&["status", "last_seen_at"]),
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let signals = match select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table(SIGNAL.table),
            columns(&["status", "window_end"]),
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let drafts = match select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table(DRAFT.table),
            columns(&["status", "updated_at"]),
        )
        .with_limit(200),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(error) => return error,
    };
    let count = |rows: &[BTreeMap<String, Value>], status: &str| {
        rows.iter()
            .filter(|row| text(row, "status") == Some(status))
            .count()
    };
    gadget_result(json!({
        "active conversations": count(&conversations, "current"),
        "signals": signals.len(),
        "corroborated": count(&signals, "corroborated"),
        "contradicted or speculative": count(&signals, "contradicted") + count(&signals, "speculative"),
        "response drafts awaiting review": count(&drafts, "draft"),
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

async fn delete_all(
    broker: &SharedBundleBroker,
    lease: InvocationLeaseToken,
    table_name: &str,
    filters: BTreeMap<String, Value>,
) -> Result<u32, HostResponse> {
    let mut deleted = 0_u32;
    for _ in 0..10 {
        let result = broker
            .lock()
            .await
            .database_delete(
                DatabaseDeleteRequest::new(
                    lease.clone(),
                    id(PURGE_PERMISSION),
                    table(table_name),
                    filters.clone(),
                )
                .with_limit(100),
            )
            .await
            .map_err(broker_error)?;
        deleted = deleted.saturating_add(result.affected_rows);
        if result.affected_rows < 100 {
            return Ok(deleted);
        }
    }
    Err(host_error(
        "purge-bound-exceeded",
        "source purge exceeded the signed 1000-row bound",
    ))
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

fn entity_values_match(
    current: &BTreeMap<String, Value>,
    expected: &BTreeMap<String, Value>,
) -> bool {
    expected.iter().all(|(key, expected)| {
        current.get(key).is_some_and(|current| {
            current == expected
                || key.ends_with("_at")
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

fn ids(rows: &[BTreeMap<String, Value>], key: &str) -> BTreeSet<String> {
    rows.iter()
        .filter_map(|row| text(row, key).map(str::to_owned))
        .collect()
}

fn text<'a>(row: &'a BTreeMap<String, Value>, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

fn string_values(value: Option<&Value>) -> impl Iterator<Item = &str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

fn citation_ids<'a>(value: Option<&'a Value>, key: &'a str) -> impl Iterator<Item = &'a str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(move |citation| citation.get(key).and_then(Value::as_str))
}

fn valid_source(source_id: &str, revision: i64, observed_at: &str) -> bool {
    valid_uuid(source_id) && revision > 0 && DateTime::parse_from_rfc3339(observed_at).is_ok()
}

fn valid_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}

fn valid_uuid_array(values: &[String], maximum: usize) -> bool {
    !values.is_empty()
        && values.len() <= maximum
        && values.iter().all(|value| valid_uuid(value))
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

fn valid_optional_uuid_array(values: &[String], maximum: usize) -> bool {
    values.len() <= maximum
        && values.iter().all(|value| valid_uuid(value))
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_at_uri(value: &str) -> bool {
    value.starts_with("at://") && bounded(value, 6, 2_000) && !value.contains(char::is_whitespace)
}

fn optional_at_uri(value: Option<&str>) -> bool {
    match value {
        Some(value) => valid_at_uri(value),
        None => true,
    }
}

fn valid_handle(value: &str) -> bool {
    bounded(value, 3, 253)
        && value.contains('.')
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn valid_window(start: &str, end: &str) -> bool {
    DateTime::parse_from_rfc3339(start)
        .ok()
        .zip(DateTime::parse_from_rfc3339(end).ok())
        .is_some_and(|(start, end)| end >= start)
}

fn valid_engagement(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() <= 20
        && object.iter().all(|(key, value)| {
            bounded(key, 1, 50) && value.as_u64().is_some_and(|count| count <= 1_000_000_000)
        })
}

fn valid_string_array(values: &[String], maximum_items: usize, maximum_chars: usize) -> bool {
    values.len() <= maximum_items && values.iter().all(|value| bounded(value, 1, maximum_chars))
}

fn valid_citations(values: &[Value], maximum: usize) -> bool {
    !values.is_empty()
        && values.len() <= maximum
        && serde_json::to_vec(values).is_ok_and(|bytes| bytes.len() <= 64 * 1024)
        && values.iter().all(|value| {
            let Some(object) = value.as_object() else {
                return false;
            };
            object.len() <= 8
                && object
                    .get("source_id")
                    .and_then(Value::as_str)
                    .is_some_and(valid_uuid)
                && object
                    .get("source_revision")
                    .and_then(Value::as_i64)
                    .is_some_and(|revision| revision > 0)
                && object.values().all(|value| match value {
                    Value::String(value) => bounded(value, 1, 2_000),
                    Value::Number(_) | Value::Bool(_) | Value::Null => true,
                    Value::Array(_) | Value::Object(_) => false,
                })
        })
}

fn valid_briefing_citations(values: &[Value], maximum: usize) -> bool {
    valid_citations(values, maximum)
        && values.iter().all(|value| {
            value
                .get("signal_id")
                .and_then(Value::as_str)
                .is_some_and(valid_uuid)
        })
}

fn optional_bounded(value: Option<&str>, minimum: usize, maximum: usize) -> bool {
    match value {
        Some(value) => bounded(value, minimum, maximum),
        None => true,
    }
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

fn post_columns() -> Vec<String> {
    columns(&[
        "post_id",
        "topic_id",
        "provider",
        "external_uri",
        "cid",
        "author_handle",
        "text_excerpt",
        "language",
        "reply_to_uri",
        "quote_uri",
        "state",
        "engagement",
        "moderation_labels",
        "source_id",
        "source_revision",
        "fetched_at",
        "content_hash",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn conversation_columns() -> Vec<String> {
    columns(&[
        "conversation_id",
        "topic_id",
        "title",
        "summary",
        "origin_uri",
        "post_count",
        "status",
        "first_seen_at",
        "last_seen_at",
        "source_ids",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn signal_columns() -> Vec<String> {
    columns(&[
        "signal_id",
        "conversation_id",
        "kind",
        "statement",
        "confidence_basis",
        "status",
        "supporting_posts",
        "contradicting_posts",
        "window_start",
        "window_end",
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
        "citations",
        "revision",
        "created_at",
        "updated_at",
    ])
}

fn draft_columns() -> Vec<String> {
    columns(&[
        "draft_id",
        "briefing_id",
        "provider",
        "target_account",
        "audience",
        "objective",
        "body",
        "impact_preview",
        "risk_notes",
        "status",
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
    fn public_identity_and_pinned_citations_are_bounded() {
        assert!(valid_at_uri("at://did:plc:abc/app.bsky.feed.post/123"));
        assert!(valid_handle("example.bsky.social"));
        assert!(valid_hash(&"a".repeat(64)));
        assert!(valid_optional_uuid_array(&[], 200));
        assert!(valid_optional_uuid_array(
            &["11111111-1111-1111-1111-111111111111".to_string()],
            200,
        ));
        assert!(valid_citations(
            &[json!({
                "source_id": "11111111-1111-1111-1111-111111111111",
                "source_revision": 1,
                "signal_id": "22222222-2222-2222-2222-222222222222",
                "claim": "Observed in the pinned public response"
            })],
            10,
        ));
        assert!(valid_briefing_citations(
            &[json!({
                "source_id": "11111111-1111-1111-1111-111111111111",
                "source_revision": 1,
                "signal_id": "22222222-2222-2222-2222-222222222222",
                "stance": "supports"
            })],
            10,
        ));
        assert!(!valid_handle("private-account"));
    }
}
