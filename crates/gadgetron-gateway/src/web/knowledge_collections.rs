use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Extension, Json, Router,
};
use gadgetron_core::{context::TenantContext, error::GadgetronError};
use gadgetron_xaas::{
    knowledge_collections::{
        self as collections, CollectionLocator, CollectionQuery, CollectionRunTrigger,
        CreateKnowledgeCollection, EnqueueCollectionRun, KnowledgeCollectionError,
        UpdateKnowledgeCollection,
    },
    knowledge_spaces::SpaceActor,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    knowledge_collections::next_schedule_after,
    server::AppState,
    web::{
        bundle_runtime::{
            BundleCollectionProfileProjection, BundleKnowledgeProfilesProjection,
            BundleRuntimeState,
        },
        workbench::WorkbenchHttpError,
    },
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/knowledge/collection-profiles",
            get(list_available_profiles_handler),
        )
        .route(
            "/knowledge/spaces/{space_id}/collections",
            get(list_collections_handler).post(create_collection_handler),
        )
        .route(
            "/knowledge/collections/{collection_id}",
            get(get_collection_handler)
                .put(update_collection_handler)
                .delete(archive_collection_handler),
        )
        .route(
            "/knowledge/collections/{collection_id}/runs",
            get(list_runs_handler).post(enqueue_run_handler),
        )
        .route(
            "/knowledge/collections/{collection_id}/source-health",
            get(source_health_handler),
        )
        .route("/knowledge/collection-runs/{run_id}", get(get_run_handler))
        .route(
            "/knowledge/collection-runs/{run_id}/cancel",
            post(cancel_run_handler),
        )
        .route(
            "/knowledge/collection-runs/{run_id}/retry",
            post(retry_run_handler),
        )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCollectionRequest {
    pub output_vault_id: Uuid,
    pub bundle_id: String,
    pub profile_id: String,
    pub topic: String,
    #[serde(default)]
    pub schedule_enabled: bool,
    #[serde(default)]
    pub locators: Vec<CollectionLocator>,
    #[serde(default)]
    pub queries: Vec<CollectionQuery>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateCollectionRequest {
    pub expected_revision: i64,
    pub topic: String,
    pub status: String,
    #[serde(default)]
    pub schedule_enabled: bool,
    #[serde(default)]
    pub locators: Vec<CollectionLocator>,
    #[serde(default)]
    pub queries: Vec<CollectionQuery>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedRevision {
    pub expected_revision: i64,
}

#[derive(Debug, Deserialize)]
pub struct ListRunsQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
}

const fn default_limit() -> i64 {
    50
}

#[derive(Debug, Serialize)]
pub struct AvailableCollectionProfile {
    pub bundle_id: String,
    pub package_manifest_sha256: String,
    #[serde(flatten)]
    pub collection: BundleCollectionProfileProjection,
    pub query_provider_status: Vec<QueryProviderStatus>,
}

#[derive(Debug, Serialize)]
pub struct QueryProviderStatus {
    pub id: String,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct AvailableCollectionProfilesResponse {
    pub profiles: Vec<AvailableCollectionProfile>,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
pub struct CollectionListResponse {
    pub collections: Vec<collections::KnowledgeCollectionRow>,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
pub struct CollectionRunListResponse {
    pub runs: Vec<collections::KnowledgeCollectionRunRow>,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
pub struct CollectionRunDetailResponse {
    pub run: collections::KnowledgeCollectionRunRow,
    pub items: Vec<collections::KnowledgeCollectionRunItemRow>,
}

#[derive(Debug, Serialize)]
pub struct CollectionSourceHealthResponse {
    pub sources: Vec<collections::CollectionSourceHealthRow>,
    pub returned: usize,
}

pub async fn list_available_profiles_handler(
    State(state): State<AppState>,
) -> Result<Json<AvailableCollectionProfilesResponse>, WorkbenchHttpError> {
    let manager = runtime_manager(&state)?;
    let mut profiles = Vec::new();
    for status in manager.list().await? {
        if status.state != BundleRuntimeState::Enabled {
            continue;
        }
        let projection = manager.knowledge_profiles(&status.bundle_id)?;
        profiles.extend(projection.collections.into_iter().map(|collection| {
            let query_provider_status = collection
                .profile
                .query_providers
                .iter()
                .map(|provider| QueryProviderStatus {
                    id: provider.id.as_str().to_string(),
                    status: query_provider_readiness(provider.id.as_str()),
                })
                .collect();
            AvailableCollectionProfile {
                bundle_id: projection.bundle_id.clone(),
                package_manifest_sha256: projection.package_manifest_sha256.clone(),
                collection,
                query_provider_status,
            }
        }));
    }
    profiles.sort_by(|left, right| {
        (&left.bundle_id, left.collection.profile.id.as_str())
            .cmp(&(&right.bundle_id, right.collection.profile.id.as_str()))
    });
    let returned = profiles.len();
    Ok(Json(AvailableCollectionProfilesResponse {
        profiles,
        returned,
    }))
}

pub async fn create_collection_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
    Json(request): Json<CreateCollectionRequest>,
) -> Result<Json<collections::KnowledgeCollectionRow>, WorkbenchHttpError> {
    let (projection, profile) =
        signed_profile(&state, &request.bundle_id, &request.profile_id).await?;
    let locators =
        validated_collection_inputs(&profile.profile, &request.locators, &request.queries)?;
    let next_run_at = schedule_next(
        profile.profile.schedule.as_deref(),
        request.schedule_enabled,
    )?;
    let row = collections::create_collection(
        pool(&state)?,
        actor(&ctx)?,
        CreateKnowledgeCollection {
            space_id,
            output_vault_id: request.output_vault_id,
            bundle_id: request.bundle_id,
            profile_id: request.profile_id,
            label: profile.profile.label.clone(),
            topic: request.topic,
            connector: profile.profile.connector.as_str().to_string(),
            source_classes: profile
                .profile
                .source_classes
                .iter()
                .map(|value| value.as_str().to_string())
                .collect(),
            allowed_domains: profile.profile.allowlisted_domains.clone(),
            freshness_seconds: i64::try_from(profile.profile.freshness_seconds)
                .map_err(|_| invalid("Collection freshness exceeds the Core limit"))?,
            schedule: profile.profile.schedule.clone(),
            schedule_enabled: request.schedule_enabled,
            next_run_at,
            max_sources: i32::try_from(profile.profile.budget.max_sources)
                .map_err(|_| invalid("Collection source budget exceeds the Core limit"))?,
            max_bytes: i64::try_from(profile.profile.budget.max_bytes)
                .map_err(|_| invalid("Collection byte budget exceeds the Core limit"))?,
            max_wall_seconds: i32::try_from(profile.profile.budget.max_wall_seconds)
                .map_err(|_| invalid("Collection time budget exceeds the Core limit"))?,
            package_manifest_sha256: projection.package_manifest_sha256,
            recipe_asset_id: profile.profile.recipe_asset.as_str().to_string(),
            recipe_sha256: profile.recipe_sha256.clone(),
            locators,
            queries: request.queries,
        },
    )
    .await?;
    Ok(Json(row))
}

pub async fn update_collection_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(collection_id): Path<Uuid>,
    Json(request): Json<UpdateCollectionRequest>,
) -> Result<Json<collections::KnowledgeCollectionRow>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let current = collections::get_collection(
        pool(&state)?,
        actor,
        collection_id,
        gadgetron_xaas::knowledge_spaces::SpaceRole::Contributor,
        true,
    )
    .await?;
    let (current, _, profile) = ensure_current_snapshot(
        &state,
        pool(&state)?,
        actor,
        current,
        request.expected_revision,
    )
    .await?;
    let locators =
        validated_collection_inputs(&profile.profile, &request.locators, &request.queries)?;
    let next_run_at = schedule_next(current.schedule.as_deref(), request.schedule_enabled)?;
    Ok(Json(
        collections::update_collection(
            pool(&state)?,
            actor,
            collection_id,
            UpdateKnowledgeCollection {
                expected_revision: current.revision,
                topic: request.topic,
                status: request.status,
                schedule_enabled: request.schedule_enabled,
                next_run_at,
                locators,
                queries: request.queries,
            },
        )
        .await?,
    ))
}

pub async fn archive_collection_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(collection_id): Path<Uuid>,
    Json(request): Json<ExpectedRevision>,
) -> Result<Json<collections::KnowledgeCollectionRow>, WorkbenchHttpError> {
    Ok(Json(
        collections::archive_collection(
            pool(&state)?,
            actor(&ctx)?,
            collection_id,
            request.expected_revision,
        )
        .await?,
    ))
}

pub async fn get_collection_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(collection_id): Path<Uuid>,
) -> Result<Json<collections::KnowledgeCollectionRow>, WorkbenchHttpError> {
    Ok(Json(
        collections::get_collection(
            pool(&state)?,
            actor(&ctx)?,
            collection_id,
            gadgetron_xaas::knowledge_spaces::SpaceRole::Viewer,
            false,
        )
        .await?,
    ))
}

pub async fn list_collections_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
) -> Result<Json<CollectionListResponse>, WorkbenchHttpError> {
    let rows = collections::list_collections(pool(&state)?, actor(&ctx)?, space_id).await?;
    let returned = rows.len();
    Ok(Json(CollectionListResponse {
        collections: rows,
        returned,
    }))
}

pub async fn enqueue_run_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(collection_id): Path<Uuid>,
    Json(request): Json<ExpectedRevision>,
) -> Result<Json<collections::EnqueuedCollectionRun>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let collection = collections::get_collection(
        pool(&state)?,
        actor,
        collection_id,
        gadgetron_xaas::knowledge_spaces::SpaceRole::Contributor,
        true,
    )
    .await?;
    let (collection, _, _) = ensure_current_snapshot(
        &state,
        pool(&state)?,
        actor,
        collection,
        request.expected_revision,
    )
    .await?;
    let policy_revision = active_policy_revision(&state, actor.tenant_id).await?;
    Ok(Json(
        collections::enqueue_run(
            pool(&state)?,
            actor,
            collection_id,
            EnqueueCollectionRun {
                expected_collection_revision: collection.revision,
                trigger: CollectionRunTrigger::OnDemand,
                requested_by_user_id: actor.user_id,
                on_behalf_of_user_id: actor.user_id,
                tool_policy_revision: policy_revision,
                scheduled_at: None,
                next_schedule_at: None,
            },
        )
        .await?,
    ))
}

pub async fn list_runs_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(collection_id): Path<Uuid>,
    Query(query): Query<ListRunsQuery>,
) -> Result<Json<CollectionRunListResponse>, WorkbenchHttpError> {
    let rows =
        collections::list_runs(pool(&state)?, actor(&ctx)?, collection_id, query.limit).await?;
    let returned = rows.len();
    Ok(Json(CollectionRunListResponse {
        runs: rows,
        returned,
    }))
}

pub async fn get_run_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<CollectionRunDetailResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let run = collections::get_run(pool(&state)?, actor, run_id).await?;
    let items = collections::run_items(pool(&state)?, actor, run_id).await?;
    Ok(Json(CollectionRunDetailResponse { run, items }))
}

pub async fn cancel_run_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(run_id): Path<Uuid>,
    Json(request): Json<ExpectedRevision>,
) -> Result<Json<collections::KnowledgeCollectionRunRow>, WorkbenchHttpError> {
    Ok(Json(
        collections::request_cancel(
            pool(&state)?,
            actor(&ctx)?,
            run_id,
            request.expected_revision,
        )
        .await?,
    ))
}

pub async fn retry_run_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(run_id): Path<Uuid>,
    Json(request): Json<ExpectedRevision>,
) -> Result<Json<collections::EnqueuedCollectionRun>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let parent = collections::get_run(pool(&state)?, actor, run_id).await?;
    let (projection, profile) =
        signed_profile(&state, &parent.bundle_id, &parent.profile_id).await?;
    if projection.package_manifest_sha256 != parent.package_manifest_sha256
        || profile.recipe_sha256 != parent.recipe_sha256
    {
        return Err(invalid(
            "The exact signed collection package used by this run is no longer active",
        ));
    }
    Ok(Json(
        collections::retry_run(pool(&state)?, actor, run_id, request.expected_revision).await?,
    ))
}

pub async fn source_health_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(collection_id): Path<Uuid>,
) -> Result<Json<CollectionSourceHealthResponse>, WorkbenchHttpError> {
    let rows = collections::source_health(pool(&state)?, actor(&ctx)?, collection_id).await?;
    let returned = rows.len();
    Ok(Json(CollectionSourceHealthResponse {
        sources: rows,
        returned,
    }))
}

async fn signed_profile(
    state: &AppState,
    bundle_id: &str,
    profile_id: &str,
) -> Result<
    (
        BundleKnowledgeProfilesProjection,
        BundleCollectionProfileProjection,
    ),
    WorkbenchHttpError,
> {
    let manager = runtime_manager(state)?;
    if manager.status(bundle_id).await?.state != BundleRuntimeState::Enabled {
        return Err(invalid(
            "The Bundle that owns this collection is not enabled",
        ));
    }
    let projection = manager.knowledge_profiles(bundle_id)?;
    let profile = projection
        .collections
        .iter()
        .find(|candidate| candidate.profile.id.as_str() == profile_id)
        .cloned()
        .ok_or_else(|| invalid("The Bundle does not declare this collection profile"))?;
    Ok((projection, profile))
}

pub(super) async fn ensure_current_snapshot(
    state: &AppState,
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    collection: collections::KnowledgeCollectionRow,
    expected_revision: i64,
) -> Result<
    (
        collections::KnowledgeCollectionRow,
        BundleKnowledgeProfilesProjection,
        BundleCollectionProfileProjection,
    ),
    WorkbenchHttpError,
> {
    if collection.revision != expected_revision {
        return Err(WorkbenchHttpError::KnowledgeConflict);
    }
    let (projection, profile) =
        signed_profile(state, &collection.bundle_id, &collection.profile_id).await?;
    if profile.recipe_sha256 != collection.recipe_sha256
        || profile.profile.recipe_asset.as_str() != collection.recipe_asset_id
    {
        return Err(invalid(
            "The signed collection profile changed; archive this configuration and create a new one",
        ));
    }
    let collection = if projection.package_manifest_sha256 == collection.package_manifest_sha256 {
        collection
    } else {
        collections::rebind_collection_package(
            pool,
            actor,
            collection.id,
            collection.revision,
            projection.package_manifest_sha256.clone(),
        )
        .await?
    };
    Ok((collection, projection, profile))
}

fn validate_locators(
    locators: &[CollectionLocator],
    source_classes: &[gadgetron_bundle_sdk::LocalId],
    allowed_domains: &[String],
) -> Result<(), WorkbenchHttpError> {
    if locators.is_empty() {
        return Err(invalid("Add at least one approved source URL"));
    }
    for locator in locators {
        let url = Url::parse(&locator.url).map_err(|_| invalid("Source URL is invalid"))?;
        let host = url
            .host_str()
            .ok_or_else(|| invalid("Source URL has no host"))?;
        if url.scheme() != "https"
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !allowed_domains
                .iter()
                .any(|domain| host.eq_ignore_ascii_case(domain))
        {
            return Err(invalid(format!(
                "Source URL must be HTTPS and use one of the signed domains: {}",
                allowed_domains.join(", ")
            )));
        }
        if !source_classes
            .iter()
            .any(|class| class.as_str() == locator.source_class)
        {
            return Err(invalid("Source class is not declared by this collection"));
        }
    }
    Ok(())
}

pub(super) fn validated_collection_inputs(
    profile: &gadgetron_bundle_sdk::CollectionProfileDescriptor,
    locators: &[CollectionLocator],
    queries: &[CollectionQuery],
) -> Result<Vec<CollectionLocator>, WorkbenchHttpError> {
    match profile.connector.as_str() {
        "core-source-fetch" if queries.is_empty() => {
            validate_locators(
                locators,
                &profile.source_classes,
                &profile.allowlisted_domains,
            )?;
            Ok(locators.to_vec())
        }
        "core-community-api" | "core-social-api" if locators.is_empty() => {
            build_query_locators(profile, queries)
        }
        "core-source-fetch" | "core-community-api" | "core-social-api" => Err(invalid(
            "Choose either approved source URLs or provider queries for this profile",
        )),
        _ => Err(invalid(
            "This Core version does not support the collection connector",
        )),
    }
}

fn build_query_locators(
    profile: &gadgetron_bundle_sdk::CollectionProfileDescriptor,
    queries: &[CollectionQuery],
) -> Result<Vec<CollectionLocator>, WorkbenchHttpError> {
    if queries.is_empty() || queries.len() > profile.budget.max_sources as usize {
        return Err(invalid("Select at least one signed query provider"));
    }
    let mut providers = std::collections::BTreeSet::new();
    queries
        .iter()
        .map(|query| {
            let provider = profile
                .query_providers
                .iter()
                .find(|provider| provider.id.as_str() == query.provider)
                .ok_or_else(|| invalid("Query provider is not declared by this profile"))?;
            if !providers.insert(query.provider.as_str()) {
                return Err(invalid("Each query provider may be selected only once"));
            }
            validate_query(provider, query)?;
            let readiness = query_provider_readiness(&query.provider);
            if readiness != "ready" {
                let reason = match query.provider.as_str() {
                    "reddit" => "requires approved API access and purge-capable Source storage; current Git-backed Vaults cannot remove deleted content history".to_string(),
                    _ if readiness == "needs_connection" => {
                        "needs a Core connection before it can be selected".to_string()
                    }
                    _ => "is unavailable in this Core build".to_string(),
                };
                return Err(invalid(format!("{} {reason}", provider.label)));
            }
            let url = match query.provider.as_str() {
                "stack-exchange" => stack_exchange_query_url(profile, query)?,
                "reddit" => reddit_query_url(profile, query)?,
                "bluesky-search" | "bluesky-author" => {
                    bluesky_query_url(profile, query)?
                }
                _ => {
                    return Err(invalid(
                        "This Core build does not implement the query provider",
                    ))
                }
            };
            Ok(CollectionLocator {
                url,
                title: format!("{} · {}", provider.label, query.scope),
                source_class: provider.source_class.as_str().to_string(),
            })
        })
        .collect()
}

fn validate_query(
    provider: &gadgetron_bundle_sdk::CollectionQueryProviderDescriptor,
    query: &CollectionQuery,
) -> Result<(), WorkbenchHttpError> {
    if query.query.trim().is_empty()
        || query.query.chars().count() > 500
        || query.scope.trim().is_empty()
        || query.scope.len() > 80
        || !(1..=provider.max_window_days).contains(&query.window_days)
        || query.tags.len() > 8
        || (!provider.supports_tags && !query.tags.is_empty())
        || (!provider.supports_language && query.language.is_some())
        || query
            .tags
            .iter()
            .any(|tag| tag.trim().is_empty() || tag.len() > 35)
        || query
            .language
            .as_ref()
            .is_some_and(|language| language.trim().is_empty() || language.len() > 35)
    {
        return Err(invalid(
            "Provider query is outside the signed input boundary",
        ));
    }
    let scope_valid = match query.provider.as_str() {
        "stack-exchange" => query.scope.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        }),
        "reddit" => {
            query.scope == "all"
                || query
                    .scope
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        }
        "bluesky-search" => matches!(query.scope.as_str(), "latest" | "top"),
        "bluesky-author" => {
            matches!(query.scope.as_str(), "latest" | "top")
                && valid_at_identifier(query.query.trim())
        }
        _ => false,
    };
    if !scope_valid {
        return Err(invalid("Provider scope or account is invalid"));
    }
    Ok(())
}

fn valid_at_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'_'))
}

fn stack_exchange_query_url(
    profile: &gadgetron_bundle_sdk::CollectionProfileDescriptor,
    query: &CollectionQuery,
) -> Result<String, WorkbenchHttpError> {
    const FILTER: &str = "!T3Audpg2zUMHdQ1BCA";
    let mut url = Url::parse("https://api.stackexchange.com/2.3/search/advanced")
        .map_err(|_| invalid("Core Stack Exchange endpoint is invalid"))?;
    let page_size = profile.budget.max_sources.clamp(1, 10).to_string();
    {
        let mut pairs = url.query_pairs_mut();
        pairs
            .append_pair("site", &query.scope)
            .append_pair("q", query.query.trim())
            .append_pair("answers", "1")
            .append_pair("sort", "activity")
            .append_pair("order", "desc")
            .append_pair("pagesize", &page_size)
            .append_pair("filter", FILTER)
            .append_pair("gadgetron_window_days", &query.window_days.to_string());
        if !query.tags.is_empty() {
            pairs.append_pair("tagged", &query.tags.join(";"));
        }
    }
    validate_generated_locator(profile, url)
}

fn reddit_query_url(
    profile: &gadgetron_bundle_sdk::CollectionProfileDescriptor,
    query: &CollectionQuery,
) -> Result<String, WorkbenchHttpError> {
    let mut url = Url::parse("https://oauth.reddit.com")
        .map_err(|_| invalid("Core Reddit endpoint is invalid"))?;
    url.set_path(&format!("/r/{}/search.json", query.scope));
    {
        let mut pairs = url.query_pairs_mut();
        pairs
            .append_pair("q", query.query.trim())
            .append_pair(
                "restrict_sr",
                if query.scope != "all" { "on" } else { "off" },
            )
            .append_pair("sort", "relevance")
            .append_pair("raw_json", "1")
            .append_pair(
                "limit",
                &profile.budget.max_sources.clamp(1, 10).to_string(),
            )
            .append_pair("gadgetron_window_days", &query.window_days.to_string());
    }
    validate_generated_locator(profile, url)
}

fn bluesky_query_url(
    profile: &gadgetron_bundle_sdk::CollectionProfileDescriptor,
    query: &CollectionQuery,
) -> Result<String, WorkbenchHttpError> {
    let mut url = Url::parse("https://api.bsky.app/xrpc/app.bsky.feed.searchPosts")
        .map_err(|_| invalid("Core Bluesky endpoint is invalid"))?;
    let limit = profile.budget.max_sources.clamp(1, 10).to_string();
    {
        let mut pairs = url.query_pairs_mut();
        pairs
            .append_pair(
                "q",
                if query.provider == "bluesky-author" {
                    "*"
                } else {
                    query.query.trim()
                },
            )
            .append_pair("sort", &query.scope)
            .append_pair("limit", &limit)
            .append_pair("gadgetron_window_days", &query.window_days.to_string());
        if query.provider == "bluesky-author" {
            pairs.append_pair("author", query.query.trim());
        }
        if let Some(language) = query.language.as_deref() {
            pairs.append_pair("lang", language);
        }
        for tag in &query.tags {
            pairs.append_pair("tag", tag.trim_start_matches('#'));
        }
    }
    validate_generated_locator(profile, url)
}

fn validate_generated_locator(
    profile: &gadgetron_bundle_sdk::CollectionProfileDescriptor,
    url: Url,
) -> Result<String, WorkbenchHttpError> {
    let locator = CollectionLocator {
        url: url.to_string(),
        title: String::new(),
        source_class: profile
            .source_classes
            .first()
            .map(|value| value.as_str().to_string())
            .unwrap_or_default(),
    };
    validate_locators(
        std::slice::from_ref(&locator),
        &profile.source_classes,
        &profile.allowlisted_domains,
    )?;
    Ok(locator.url)
}

fn query_provider_readiness(provider: &str) -> &'static str {
    match provider {
        "stack-exchange" | "bluesky-search" | "bluesky-author" => "ready",
        // Current Git-backed Domain Vaults cannot purge historical note bytes.
        // Keep Reddit fail-closed until a purge-capable Source store exists.
        "reddit" => "unavailable",
        _ => "unavailable",
    }
}

fn schedule_next(
    schedule: Option<&str>,
    enabled: bool,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, WorkbenchHttpError> {
    if !enabled {
        return Ok(None);
    }
    let schedule = schedule.ok_or_else(|| invalid("This collection has no signed schedule"))?;
    next_schedule_after(schedule, chrono::Utc::now())
        .map(Some)
        .map_err(invalid)
}

async fn active_policy_revision(
    state: &AppState,
    tenant_id: Uuid,
) -> Result<String, WorkbenchHttpError> {
    state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.policy_evaluator.as_ref())
        .ok_or_else(|| WorkbenchHttpError::PolicyUnavailable {
            detail: "Collections require the common policy evaluator".into(),
        })?
        .active_identity(tenant_id)
        .await
        .map(|identity| identity.to_revision_ref())
        .map_err(|error| WorkbenchHttpError::PolicyUnavailable {
            detail: error.detail,
        })
}

fn actor(ctx: &TenantContext) -> Result<SpaceActor, WorkbenchHttpError> {
    Ok(SpaceActor {
        tenant_id: ctx.tenant_id,
        user_id: ctx
            .actor_user_id
            .ok_or(WorkbenchHttpError::KnowledgeForbidden)?,
    })
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, WorkbenchHttpError> {
    state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "Knowledge collections require PostgreSQL".into(),
        ))
    })
}

fn runtime_manager(
    state: &AppState,
) -> Result<&crate::web::bundle_runtime::BundleRuntimeManager, WorkbenchHttpError> {
    state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.runtime_manager.as_deref())
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "Knowledge collections require the Bundle runtime".into(),
            ))
        })
}

fn invalid(detail: impl Into<String>) -> WorkbenchHttpError {
    WorkbenchHttpError::KnowledgeInvalidInput {
        detail: detail.into(),
    }
}

impl From<KnowledgeCollectionError> for WorkbenchHttpError {
    fn from(error: KnowledgeCollectionError) -> Self {
        match error {
            KnowledgeCollectionError::InvalidInput(detail)
            | KnowledgeCollectionError::InvalidPersisted(detail) => {
                Self::KnowledgeInvalidInput { detail }
            }
            KnowledgeCollectionError::NotFound => Self::KnowledgeNotFound,
            KnowledgeCollectionError::Conflict | KnowledgeCollectionError::LeaseLost => {
                Self::KnowledgeConflict
            }
            KnowledgeCollectionError::Space(error) => error.into(),
            KnowledgeCollectionError::ServicePrincipal(_) => Self::KnowledgeForbidden,
            KnowledgeCollectionError::Database(error) => Self::Core(GadgetronError::Config(
                format!("Knowledge collection database: {error}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use gadgetron_bundle_sdk::BundlePackageManifest;

    use super::*;

    fn community_profile() -> gadgetron_bundle_sdk::CollectionProfileDescriptor {
        let source =
            include_str!("../../../../bundles/community-intelligence/package.template.toml")
                .replace(
                    "@ENTRY_SHA256@",
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                );
        BundlePackageManifest::parse_toml(&source)
            .unwrap()
            .capabilities
            .collection_profiles
            .into_iter()
            .next()
            .unwrap()
    }

    fn social_profile() -> gadgetron_bundle_sdk::CollectionProfileDescriptor {
        let source =
            include_str!("../../../../bundles/social-media-intelligence/package.template.toml")
                .replace(
                    "@ENTRY_SHA256@",
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                );
        BundlePackageManifest::parse_toml(&source)
            .unwrap()
            .capabilities
            .collection_profiles
            .into_iter()
            .next()
            .unwrap()
    }

    fn stack_query() -> CollectionQuery {
        CollectionQuery {
            provider: "stack-exchange".into(),
            query: "rust async cancellation".into(),
            scope: "stackoverflow".into(),
            tags: vec!["rust".into()],
            language: None,
            window_days: 30,
        }
    }

    #[test]
    fn community_query_becomes_a_bounded_core_owned_provider_locator() {
        let profile = community_profile();
        let locators = validated_collection_inputs(&profile, &[], &[stack_query()]).unwrap();
        assert_eq!(locators.len(), 1);
        assert_eq!(locators[0].source_class, "community");
        let url = Url::parse(&locators[0].url).unwrap();
        assert_eq!(url.host_str(), Some("api.stackexchange.com"));
        let pairs = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            pairs.get("site").map(|value| value.as_ref()),
            Some("stackoverflow")
        );
        assert_eq!(
            pairs.get("tagged").map(|value| value.as_ref()),
            Some("rust")
        );
        assert_eq!(
            pairs
                .get("gadgetron_window_days")
                .map(|value| value.as_ref()),
            Some("30")
        );
        assert!(pairs.contains_key("filter"));
    }

    #[test]
    fn provider_queries_do_not_accept_raw_urls_or_unsupported_retention() {
        let profile = community_profile();
        let locator = CollectionLocator {
            url: "https://api.stackexchange.com/2.3/questions".into(),
            title: "raw".into(),
            source_class: "community".into(),
        };
        assert!(validated_collection_inputs(&profile, &[locator], &[stack_query()]).is_err());

        let mut reddit = stack_query();
        reddit.provider = "reddit".into();
        reddit.scope = "rust".into();
        reddit.tags.clear();
        assert!(validated_collection_inputs(&profile, &[], &[reddit]).is_err());
    }

    #[test]
    fn social_queries_use_only_the_fixed_public_bluesky_endpoint() {
        let profile = social_profile();
        let search = CollectionQuery {
            provider: "bluesky-search".into(),
            query: "linux server monitoring".into(),
            scope: "latest".into(),
            tags: vec!["sysadmin".into()],
            language: Some("en".into()),
            window_days: 7,
        };
        let author = CollectionQuery {
            provider: "bluesky-author".into(),
            query: "bsky.app".into(),
            scope: "top".into(),
            tags: Vec::new(),
            language: None,
            window_days: 30,
        };
        let locators = validated_collection_inputs(&profile, &[], &[search, author]).unwrap();
        assert_eq!(locators.len(), 2);
        for locator in &locators {
            let url = Url::parse(&locator.url).unwrap();
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.host_str(), Some("api.bsky.app"));
            assert_eq!(url.path(), "/xrpc/app.bsky.feed.searchPosts");
        }
        let author_url = Url::parse(&locators[1].url).unwrap();
        let author_pairs = author_url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(author_pairs.get("q").map(|value| value.as_ref()), Some("*"));
        assert_eq!(
            author_pairs.get("author").map(|value| value.as_ref()),
            Some("bsky.app")
        );
    }

    #[test]
    fn social_author_query_rejects_urls_and_ambiguous_identifiers() {
        let profile = social_profile();
        for query in [
            "https://bsky.app/profile/example",
            "two..dots",
            "white space",
        ] {
            let author = CollectionQuery {
                provider: "bluesky-author".into(),
                query: query.into(),
                scope: "latest".into(),
                tags: Vec::new(),
                language: None,
                window_days: 7,
            };
            assert!(validated_collection_inputs(&profile, &[], &[author]).is_err());
        }
    }
}
