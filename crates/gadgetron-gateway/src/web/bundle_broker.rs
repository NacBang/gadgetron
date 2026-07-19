use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use gadgetron_bundle_host::{BrokerCaller, BundleBroker, ValidatedPackageContract};
use gadgetron_bundle_sdk::{
    AgentRole, BrokerError, BrokerOperationKind, BrokerProbeResult, BrokerRequest, BrokerResponse,
    DatabaseDeleteRequest, DatabaseInsertRequest, DatabaseMutationEvent, DatabaseMutationResult,
    DatabaseRows, DatabaseSelectRequest, DatabaseUpdateRequest, EventJobDescriptor,
    IntelligenceContextRequest, InvocationContext, InvocationLeaseToken, KnowledgeCollectionAction,
    KnowledgeCollectionLocator, KnowledgeCollectionQuery, KnowledgeCollectionRecord,
    KnowledgeCollectionRequest, KnowledgeCollectionResult, KnowledgeEventDescriptor, LocalId,
    OutcomeFeedbackRequest, PermissionKind, SshExecuteRequest,
};
use gadgetron_core::{
    agent::config::{AgentConfig, ConversationAgentProfile, ModelSource},
    policy::PolicyEvaluator,
};
use gadgetron_xaas::{
    autonomy::EnqueueBundleEvent,
    knowledge_agent_profiles,
    knowledge_collections::{
        self as collections, CollectionLocator, CollectionQuery, CollectionRunTrigger,
        CreateKnowledgeCollection, EnqueueCollectionRun, KnowledgeCollectionError,
        UpdateKnowledgeCollection,
    },
    knowledge_events::EnqueueKnowledgeEvent,
    knowledge_spaces::{SpaceActor, SpaceRole},
};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::web::bundle_grants::BundlePermissionGrantStore;
use crate::web::bundle_targets::{BundleSshControlPlane, BundleSshError};
use crate::web::intelligence_context::{
    IntelligenceActorBinding, IntelligenceContextError, IntelligenceContextService,
};

const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(90);
const MAX_ACTIVE_LEASES: usize = 4_096;
const DATABASE_STATEMENT_TIMEOUT: &str = "5s";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BundleEventRoute {
    pub subject_owner_bundle: String,
    pub subject_kind: String,
    pub event_kind: String,
}

#[derive(Debug, Clone)]
pub(crate) struct BundleEventJobContract {
    pub descriptor: EventJobDescriptor,
    pub owner_bundle_id: String,
    pub package_manifest_sha256: String,
    pub core_role: AgentRole,
    pub recipe_id: String,
    pub prompt_contract_revision: String,
    pub goal: String,
    pub max_wall_seconds: i32,
    pub max_attempts: i32,
}

#[derive(Debug, Clone)]
struct PreparedKnowledgeEvent {
    descriptor: KnowledgeEventDescriptor,
    tenant_id: Uuid,
    publisher_bundle_id: String,
    snapshot_filters: BTreeMap<String, serde_json::Value>,
    acting_space_id: Option<Uuid>,
    requested_by_user_id: Uuid,
    service_actor_user_id: Uuid,
    effective_role: Option<String>,
}

#[derive(Clone)]
pub struct BundleBrokerRuntime {
    pool: Option<PgPool>,
    grants: Arc<BundlePermissionGrantStore>,
    leases: Arc<InvocationLeaseStore>,
    ssh: BundleSshControlPlane,
    intelligence: Option<Arc<IntelligenceContextService>>,
    policy_evaluator: Option<Arc<dyn PolicyEvaluator>>,
    agent_brain: Option<Arc<arc_swap::ArcSwap<AgentConfig>>>,
    event_jobs: Arc<arc_swap::ArcSwap<BTreeMap<BundleEventRoute, BundleEventJobContract>>>,
}

impl BundleBrokerRuntime {
    pub fn open(
        state_root: impl AsRef<std::path::Path>,
        pool: Option<PgPool>,
    ) -> Result<Self, String> {
        Self::open_with_lease_ttl(state_root, pool, None, DEFAULT_LEASE_TTL)
    }

    pub fn open_with_intelligence(
        state_root: impl AsRef<std::path::Path>,
        pool: Option<PgPool>,
        intelligence: Option<Arc<IntelligenceContextService>>,
        policy_evaluator: Option<Arc<dyn PolicyEvaluator>>,
        agent_brain: Option<Arc<arc_swap::ArcSwap<AgentConfig>>>,
    ) -> Result<Self, String> {
        Self::open_with_services(
            state_root,
            pool,
            intelligence,
            policy_evaluator,
            agent_brain,
            DEFAULT_LEASE_TTL,
        )
    }

    fn open_with_lease_ttl(
        state_root: impl AsRef<std::path::Path>,
        pool: Option<PgPool>,
        intelligence: Option<Arc<IntelligenceContextService>>,
        lease_ttl: Duration,
    ) -> Result<Self, String> {
        Self::open_with_services(state_root, pool, intelligence, None, None, lease_ttl)
    }

    fn open_with_services(
        state_root: impl AsRef<std::path::Path>,
        pool: Option<PgPool>,
        intelligence: Option<Arc<IntelligenceContextService>>,
        policy_evaluator: Option<Arc<dyn PolicyEvaluator>>,
        agent_brain: Option<Arc<arc_swap::ArcSwap<AgentConfig>>>,
        lease_ttl: Duration,
    ) -> Result<Self, String> {
        let state_root = state_root.as_ref();
        Ok(Self {
            pool,
            grants: Arc::new(BundlePermissionGrantStore::open(state_root)?),
            leases: Arc::new(InvocationLeaseStore::new(lease_ttl)),
            ssh: BundleSshControlPlane::open(state_root)?,
            intelligence,
            policy_evaluator,
            agent_brain,
            event_jobs: Arc::new(arc_swap::ArcSwap::from_pointee(BTreeMap::new())),
        })
    }

    pub fn grants(&self) -> &Arc<BundlePermissionGrantStore> {
        &self.grants
    }

    pub fn ssh(&self) -> &BundleSshControlPlane {
        &self.ssh
    }

    pub(crate) fn database_pool(&self) -> Option<&PgPool> {
        self.pool.as_ref()
    }

    pub(crate) fn publish_event_jobs(
        &self,
        jobs: BTreeMap<BundleEventRoute, BundleEventJobContract>,
    ) {
        self.event_jobs.store(Arc::new(jobs));
    }

    pub fn broker_for(&self, contract: &ValidatedPackageContract) -> Arc<dyn BundleBroker> {
        Arc::new(GatewayBundleBroker {
            contract: contract.clone(),
            runtime: self.clone(),
        })
    }

    pub fn issue_lease(
        &self,
        bundle_id: impl Into<String>,
        package_manifest_sha256: impl Into<String>,
        context: &InvocationContext,
    ) -> Result<InvocationLeaseGuard, String> {
        self.leases.issue(
            bundle_id.into(),
            package_manifest_sha256.into(),
            context,
            None,
        )
    }

    pub(crate) fn issue_delegated_lease(
        &self,
        bundle_id: impl Into<String>,
        package_manifest_sha256: impl Into<String>,
        context: &InvocationContext,
        delegated_actor_id: Uuid,
    ) -> Result<InvocationLeaseGuard, String> {
        self.leases.issue(
            bundle_id.into(),
            package_manifest_sha256.into(),
            context,
            Some(delegated_actor_id),
        )
    }
}

#[derive(Clone)]
struct InvocationLeaseBinding {
    bundle_id: String,
    package_manifest_sha256: String,
    tenant_id: String,
    actor_id: String,
    acting_space_id: Option<String>,
    delegated_actor_id: Option<Uuid>,
    _request_id: String,
    _scopes: Vec<String>,
    expires_at: Instant,
}

struct InvocationLeaseStore {
    ttl: Duration,
    active: Mutex<BTreeMap<String, InvocationLeaseBinding>>,
}

impl InvocationLeaseStore {
    fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            active: Mutex::new(BTreeMap::new()),
        }
    }

    fn issue(
        self: &Arc<Self>,
        bundle_id: String,
        package_manifest_sha256: String,
        context: &InvocationContext,
        delegated_actor_id: Option<Uuid>,
    ) -> Result<InvocationLeaseGuard, String> {
        let mut active = self.active.lock().expect("Bundle lease lock poisoned");
        let now = Instant::now();
        active.retain(|_, binding| binding.expires_at > now);
        if active.len() >= MAX_ACTIVE_LEASES {
            return Err("Bundle broker active lease ceiling reached".into());
        }
        let token = loop {
            let mut bytes = [0_u8; 32];
            OsRng.fill_bytes(&mut bytes);
            let token = InvocationLeaseToken::new(URL_SAFE_NO_PAD.encode(bytes))
                .expect("32 random bytes have a canonical 43-byte base64url encoding");
            if !active.contains_key(token.as_str()) {
                break token;
            }
        };
        active.insert(
            token.as_str().to_string(),
            InvocationLeaseBinding {
                bundle_id,
                package_manifest_sha256,
                tenant_id: context.tenant_id.clone(),
                actor_id: context.actor_id.clone(),
                acting_space_id: context.acting_space_id.clone(),
                delegated_actor_id,
                _request_id: context.request_id.clone(),
                _scopes: context.scopes.clone(),
                expires_at: now + self.ttl,
            },
        );
        drop(active);
        Ok(InvocationLeaseGuard {
            store: Arc::clone(self),
            token,
        })
    }

    fn validate(
        &self,
        caller: &BrokerCaller,
        token: &InvocationLeaseToken,
    ) -> Result<InvocationLeaseBinding, BrokerError> {
        let mut active = self.active.lock().expect("Bundle lease lock poisoned");
        let now = Instant::now();
        active.retain(|_, binding| binding.expires_at > now);
        let binding = active.get(token.as_str()).cloned().ok_or_else(|| {
            broker_error(
                "lease-invalid",
                "invocation lease is absent or expired",
                false,
            )
        })?;
        if binding.bundle_id != caller.identity().id.as_str()
            || binding.package_manifest_sha256 != caller.package_manifest_sha256()
        {
            return Err(broker_error(
                "lease-binding-mismatch",
                "invocation lease does not belong to this signed Bundle runtime",
                false,
            ));
        }
        Ok(binding)
    }

    fn revoke(&self, token: &InvocationLeaseToken) {
        self.active
            .lock()
            .expect("Bundle lease lock poisoned")
            .remove(token.as_str());
    }
}

pub struct InvocationLeaseGuard {
    store: Arc<InvocationLeaseStore>,
    token: InvocationLeaseToken,
}

impl InvocationLeaseGuard {
    pub fn token(&self) -> &InvocationLeaseToken {
        &self.token
    }
}

impl Drop for InvocationLeaseGuard {
    fn drop(&mut self) {
        self.store.revoke(&self.token);
    }
}

struct GatewayBundleBroker {
    contract: ValidatedPackageContract,
    runtime: BundleBrokerRuntime,
}

#[async_trait]
impl BundleBroker for GatewayBundleBroker {
    async fn handle(&self, caller: &BrokerCaller, request: BrokerRequest) -> BrokerResponse {
        if caller.identity() != &self.contract.runtime_identity()
            || caller.package_manifest_sha256() != self.contract.manifest_sha256()
        {
            return BrokerResponse::Error(broker_error(
                "runtime-identity-mismatch",
                "broker executor is bound to a different signed package",
                false,
            ));
        }
        match request {
            BrokerRequest::Probe(request) => self.handle_probe(request).await,
            BrokerRequest::DatabaseSelect(request) => {
                self.handle_database_select(caller, request).await
            }
            BrokerRequest::DatabaseInsert(request) => {
                self.handle_database_insert(caller, request).await
            }
            BrokerRequest::DatabaseUpdate(request) => {
                self.handle_database_update(caller, request).await
            }
            BrokerRequest::DatabaseDelete(request) => {
                self.handle_database_delete(caller, request).await
            }
            BrokerRequest::SshExecute(request) => self.handle_ssh_execute(caller, request).await,
            BrokerRequest::IntelligenceContext(request) => {
                self.handle_intelligence_context(caller, request).await
            }
            BrokerRequest::OutcomeFeedback(request) => {
                self.handle_outcome_feedback(caller, request).await
            }
            BrokerRequest::KnowledgeCollection(request) => {
                self.handle_knowledge_collection(caller, request).await
            }
            _ => BrokerResponse::Error(broker_error(
                "operation-not-supported",
                "broker operation is not supported by this Core build",
                false,
            )),
        }
    }
}

impl GatewayBundleBroker {
    async fn handle_probe(
        &self,
        request: gadgetron_bundle_sdk::BrokerProbeRequest,
    ) -> BrokerResponse {
        let kind = match self.authorize_permission(
            &request.permission_id,
            request.resource.as_str(),
            None,
        ) {
            Ok(kind) => kind,
            Err(error) => return BrokerResponse::Error(error),
        };
        match kind {
            PermissionKind::Database => {
                let Some(pool) = &self.runtime.pool else {
                    return BrokerResponse::Probe(BrokerProbeResult::unavailable(
                        request.permission_id,
                        request.resource,
                        "Core database broker is not configured",
                    ));
                };
                let Some(table) = request.resource.database_table_name() else {
                    return BrokerResponse::Error(broker_error(
                        "resource-not-supported",
                        "database probe requires an exact postgres table resource",
                        false,
                    ));
                };
                if relation_has_tenant_uuid(pool, table, true).await {
                    BrokerResponse::Probe(BrokerProbeResult::ready(
                        request.permission_id,
                        request.resource,
                    ))
                } else {
                    BrokerResponse::Probe(BrokerProbeResult::unavailable(
                        request.permission_id,
                        request.resource,
                        "database resource is absent or lacks a UUID tenant_id boundary",
                    ))
                }
            }
            PermissionKind::Network => {
                let declared = self
                    .contract
                    .manifest()
                    .broker_operations
                    .iter()
                    .any(|operation| {
                        operation.kind == BrokerOperationKind::SshExecute
                            && operation.network_permission_id == request.permission_id
                            && operation.network_resource == request.resource.as_str()
                    });
                if !declared {
                    return BrokerResponse::Error(broker_error(
                        "resource-not-supported",
                        "network resource is not bound to a signed broker operation",
                        false,
                    ));
                }
                if self.runtime.ssh.dependency_ready() {
                    BrokerResponse::Probe(BrokerProbeResult::ready(
                        request.permission_id,
                        request.resource,
                    ))
                } else {
                    BrokerResponse::Probe(BrokerProbeResult::unavailable(
                        request.permission_id,
                        request.resource,
                        "Core SSH broker dependencies are unavailable",
                    ))
                }
            }
            PermissionKind::SecretUse => {
                let declared = self
                    .contract
                    .manifest()
                    .broker_operations
                    .iter()
                    .any(|operation| {
                        operation.kind == BrokerOperationKind::SshExecute
                            && operation.secret_permission_id == request.permission_id
                            && operation.secret_resource == request.resource.as_str()
                    });
                if !declared {
                    return BrokerResponse::Error(broker_error(
                        "resource-not-supported",
                        "secret resource is not bound to a signed broker operation",
                        false,
                    ));
                }
                if self.runtime.ssh.dependency_ready() {
                    BrokerResponse::Probe(BrokerProbeResult::ready(
                        request.permission_id,
                        request.resource,
                    ))
                } else {
                    BrokerResponse::Probe(BrokerProbeResult::unavailable(
                        request.permission_id,
                        request.resource,
                        "Core SSH secret provider is unavailable",
                    ))
                }
            }
            PermissionKind::KnowledgeRead
            | PermissionKind::KnowledgeFeedback
            | PermissionKind::KnowledgeCollection => {
                let expected = match kind {
                    PermissionKind::KnowledgeRead => "knowledge:context",
                    PermissionKind::KnowledgeFeedback => "knowledge:feedback",
                    PermissionKind::KnowledgeCollection => "knowledge:collection",
                    _ => unreachable!(),
                };
                if request.resource.as_str() != expected {
                    return BrokerResponse::Error(broker_error(
                        "resource-not-supported",
                        "knowledge broker probe requires its exact Core resource",
                        false,
                    ));
                }
                let ready = match kind {
                    PermissionKind::KnowledgeCollection => self.runtime.pool.is_some(),
                    _ => self.runtime.intelligence.is_some(),
                };
                if ready {
                    BrokerResponse::Probe(BrokerProbeResult::ready(
                        request.permission_id,
                        request.resource,
                    ))
                } else {
                    BrokerResponse::Probe(BrokerProbeResult::unavailable(
                        request.permission_id,
                        request.resource,
                        "Core knowledge broker is not configured",
                    ))
                }
            }
            _ => BrokerResponse::Error(broker_error(
                "operation-not-supported",
                "this permission kind has no v1 broker probe executor",
                false,
            )),
        }
    }

    async fn handle_database_select(
        &self,
        caller: &BrokerCaller,
        request: DatabaseSelectRequest,
    ) -> BrokerResponse {
        if let Err(error) = self.authorize_permission(
            &request.permission_id,
            request.resource.as_str(),
            Some(PermissionKind::Database),
        ) {
            return BrokerResponse::Error(error);
        }
        let lease = match self.runtime.leases.validate(caller, &request.lease) {
            Ok(lease) => lease,
            Err(error) => return BrokerResponse::Error(error),
        };
        let Some(pool) = &self.runtime.pool else {
            return BrokerResponse::Error(broker_error(
                "dependency-unavailable",
                "Core database broker is not configured",
                true,
            ));
        };
        match execute_database_select(pool, &lease.tenant_id, &request).await {
            Ok(rows) => BrokerResponse::DatabaseRows(rows),
            Err(error) => BrokerResponse::Error(error),
        }
    }

    async fn handle_database_insert(
        &self,
        caller: &BrokerCaller,
        request: DatabaseInsertRequest,
    ) -> BrokerResponse {
        if let Err(error) = self.authorize_permission(
            &request.permission_id,
            request.resource.as_str(),
            Some(PermissionKind::Database),
        ) {
            return BrokerResponse::Error(error);
        }
        let lease = match self.runtime.leases.validate(caller, &request.lease) {
            Ok(lease) => lease,
            Err(error) => return BrokerResponse::Error(error),
        };
        let Some(pool) = &self.runtime.pool else {
            return BrokerResponse::Error(database_dependency_unavailable());
        };
        let event = match request.event.as_ref() {
            Some(event) => match self.resolve_bundle_event(pool, caller, &lease, event).await {
                Ok(event) => event,
                Err(error) => {
                    tracing::warn!(
                        target: "bundle_broker",
                        bundle_id = caller.identity().id.as_str(),
                        event_kind = event.event_kind.as_str(),
                        error_code = error.code.as_str(),
                        retryable = error.retryable,
                        "optional Bundle enrichment preparation failed and will not block the authoritative database mutation"
                    );
                    None
                }
            },
            None => None,
        };
        match execute_database_insert(
            pool,
            Some(self.runtime.ssh()),
            Some(caller.identity().id.as_str()),
            &lease.tenant_id,
            &request,
            event,
        )
        .await
        {
            Ok(result) => BrokerResponse::DatabaseMutation(result),
            Err(error) => BrokerResponse::Error(error),
        }
    }

    async fn resolve_bundle_event(
        &self,
        pool: &PgPool,
        caller: &BrokerCaller,
        lease: &InvocationLeaseBinding,
        event: &DatabaseMutationEvent,
    ) -> Result<Option<EnqueueBundleEvent>, BrokerError> {
        let route = BundleEventRoute {
            subject_owner_bundle: caller.identity().id.as_str().to_string(),
            subject_kind: event.subject_kind.as_str().to_string(),
            event_kind: event.event_kind.as_str().to_string(),
        };
        let Some(contract) = self.runtime.event_jobs.load().get(&route).cloned() else {
            // An optional Intelligence package may be disabled. The operational
            // mutation remains authoritative and must not wait for enrichment.
            return Ok(None);
        };
        let validator =
            jsonschema::validator_for(&contract.descriptor.input_schema).map_err(|_| {
                broker_error(
                    "event-contract-invalid",
                    "enabled Bundle event input schema could not be compiled",
                    false,
                )
            })?;
        if validator.iter_errors(&event.input).next().is_some() {
            return Err(broker_error(
                "event-input-invalid",
                "Bundle event input does not match the enabled signed descriptor",
                false,
            ));
        }
        let tenant_id = Uuid::parse_str(&lease.tenant_id).map_err(|_| {
            broker_error(
                "lease-context-invalid",
                "invocation lease tenant is invalid",
                false,
            )
        })?;
        let service_actor_user_id = Uuid::parse_str(&lease.actor_id).map_err(|_| {
            broker_error(
                "lease-context-invalid",
                "invocation lease actor is invalid",
                false,
            )
        })?;
        let requested_by_user_id = lease.delegated_actor_id.unwrap_or(service_actor_user_id);
        let acting_space_id = lease
            .acting_space_id
            .as_deref()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| {
                broker_error(
                    "event-context-required",
                    "Bundle event enrichment requires the inherited Team or Project context",
                    false,
                )
            })?;
        let requested_actor = SpaceActor {
            tenant_id,
            user_id: requested_by_user_id,
        };
        let requested_space =
            gadgetron_xaas::knowledge_spaces::effective_spaces(pool, requested_actor)
                .await
                .map_err(|_| event_context_forbidden())?
                .into_iter()
                .find(|candidate| candidate.space.id == acting_space_id)
                .filter(|candidate| candidate.effective_role != SpaceRole::Viewer)
                .ok_or_else(event_context_forbidden)?;
        if requested_space.space.status != "active"
            || !matches!(requested_space.space.kind.as_str(), "project" | "team")
        {
            return Err(event_context_forbidden());
        }
        let service_space = gadgetron_xaas::knowledge_spaces::effective_spaces(
            pool,
            SpaceActor {
                tenant_id,
                user_id: service_actor_user_id,
            },
        )
        .await
        .map_err(|_| event_context_forbidden())?
        .into_iter()
        .find(|candidate| candidate.space.id == acting_space_id)
        .ok_or_else(event_context_forbidden)?;
        if service_space.space.status != "active"
            || service_space.effective_role == SpaceRole::Viewer
        {
            return Err(event_context_forbidden());
        }
        let effective_role = requested_space.effective_role.as_str().to_string();
        let brain = self.runtime.agent_brain.as_ref().ok_or_else(|| {
            broker_error(
                "event-executor-unavailable",
                "Core AI profile source is unavailable",
                true,
            )
        })?;
        let global = ConversationAgentProfile::from_agent(&brain.load());
        let effective = knowledge_agent_profiles::resolve_role_profile(
            pool,
            tenant_id,
            &global,
            contract.core_role.as_str(),
            Some((
                &contract.owner_bundle_id,
                contract.descriptor.agent_role.as_str(),
            )),
        )
        .await
        .map_err(|_| {
            broker_error(
                "event-profile-unavailable",
                "effective Bundle AI role profile could not be resolved",
                true,
            )
        })?;
        let prompt_seed = serde_json::to_string(&event.input).map_err(|_| {
            broker_error(
                "event-input-invalid",
                "Bundle event input could not be serialized",
                false,
            )
        })?;
        let profile = super::workbench::canonicalize_registered_endpoint_profile(
            pool,
            tenant_id,
            effective.selection.clone().into_profile(),
        )
        .await
        .map_err(|_| {
            broker_error(
                "event-profile-unavailable",
                "effective Bundle AI endpoint is unavailable",
                true,
            )
        })?
        .resolve_auto(&prompt_seed);
        let policy_revision = match self.runtime.policy_evaluator.as_ref() {
            Some(evaluator) => evaluator
                .active_identity(tenant_id)
                .await
                .map(|identity| identity.to_revision_ref())
                .map_err(|_| {
                    broker_error(
                        "event-policy-unavailable",
                        "active Core policy revision could not be pinned",
                        true,
                    )
                })?,
            None => "policy-unavailable".to_string(),
        };
        let profile_snapshot = serde_json::json!({
            "backend": profile.backend.as_str(),
            "model": profile.model,
            "effort": profile.effort.as_str(),
            "endpoint_id": profile.llm_endpoint_id,
            "model_source": match profile.model_source {
                ModelSource::Default => "default",
                ModelSource::Local => "local",
            },
            "local_base_url": profile.local_base_url,
            "local_api_key_env": profile.local_api_key_env,
            "prompt_contract_revision": contract.prompt_contract_revision,
            "tool_policy_revision": policy_revision,
            "role_profile_source": effective.source.as_str(),
            "role_profile_ref": effective.profile_ref,
        });
        Ok(Some(EnqueueBundleEvent {
            tenant_id,
            event_kind: event.event_kind.as_str().to_string(),
            subject_bundle_id: caller.identity().id.as_str().to_string(),
            subject_kind: event.subject_kind.as_str().to_string(),
            subject_id: event.subject_id.clone(),
            subject_revision: event.subject_revision.clone(),
            event_payload: event.input.clone(),
            owner_bundle_id: contract.owner_bundle_id,
            recipe_id: contract.recipe_id,
            package_manifest_sha256: contract.package_manifest_sha256,
            agent_role_id: contract.descriptor.agent_role.as_str().to_string(),
            result_gadget: contract.descriptor.result_gadget.as_str().to_string(),
            goal: contract.goal,
            acting_space_id,
            requested_by_user_id,
            service_actor_user_id,
            effective_role,
            max_wall_seconds: contract.max_wall_seconds,
            max_attempts: contract.max_attempts,
            agent_profile_snapshot: profile_snapshot,
        }))
    }

    async fn handle_database_update(
        &self,
        caller: &BrokerCaller,
        request: DatabaseUpdateRequest,
    ) -> BrokerResponse {
        if let Err(error) = self.authorize_permission(
            &request.permission_id,
            request.resource.as_str(),
            Some(PermissionKind::Database),
        ) {
            return BrokerResponse::Error(error);
        }
        let lease = match self.runtime.leases.validate(caller, &request.lease) {
            Ok(lease) => lease,
            Err(error) => return BrokerResponse::Error(error),
        };
        let Some(pool) = &self.runtime.pool else {
            return BrokerResponse::Error(database_dependency_unavailable());
        };
        let event = match request.event.as_ref() {
            Some(event) => match self
                .resolve_knowledge_event(pool, caller, &lease, event, request.limit)
                .await
            {
                Ok(event) => Some(event),
                Err(error) => return BrokerResponse::Error(error),
            },
            None => None,
        };
        match execute_database_update(pool, &lease.tenant_id, &request, event).await {
            Ok(result) => BrokerResponse::DatabaseMutation(result),
            Err(error) => BrokerResponse::Error(error),
        }
    }

    async fn handle_database_delete(
        &self,
        caller: &BrokerCaller,
        request: DatabaseDeleteRequest,
    ) -> BrokerResponse {
        if let Err(error) = self.authorize_permission(
            &request.permission_id,
            request.resource.as_str(),
            Some(PermissionKind::Database),
        ) {
            return BrokerResponse::Error(error);
        }
        let lease = match self.runtime.leases.validate(caller, &request.lease) {
            Ok(lease) => lease,
            Err(error) => return BrokerResponse::Error(error),
        };
        let Some(pool) = &self.runtime.pool else {
            return BrokerResponse::Error(database_dependency_unavailable());
        };
        let event = match request.event.as_ref() {
            Some(event) => match self
                .resolve_knowledge_event(pool, caller, &lease, event, request.limit)
                .await
            {
                Ok(event) => Some(event),
                Err(error) => return BrokerResponse::Error(error),
            },
            None => None,
        };
        match execute_database_delete(pool, &lease.tenant_id, &request, event).await {
            Ok(result) => BrokerResponse::DatabaseMutation(result),
            Err(error) => BrokerResponse::Error(error),
        }
    }

    async fn resolve_knowledge_event(
        &self,
        pool: &PgPool,
        caller: &BrokerCaller,
        lease: &InvocationLeaseBinding,
        event: &DatabaseMutationEvent,
        mutation_limit: u32,
    ) -> Result<PreparedKnowledgeEvent, BrokerError> {
        if mutation_limit != 1 {
            return Err(broker_error(
                "knowledge-event-limit-invalid",
                "a Knowledge event mutation must target exactly one row",
                false,
            ));
        }
        let snapshot = event.post_mutation_snapshot.as_ref().ok_or_else(|| {
            broker_error(
                "knowledge-event-snapshot-required",
                "a Knowledge event requires a post-mutation snapshot reference",
                false,
            )
        })?;
        let descriptor = self
            .contract
            .manifest()
            .capabilities
            .knowledge_events
            .iter()
            .find(|descriptor| {
                descriptor.event_kind == event.event_kind
                    && descriptor.subject_kind == event.subject_kind
            })
            .cloned()
            .ok_or_else(|| {
                broker_error(
                    "knowledge-event-not-declared",
                    "signed package did not declare this Knowledge event route",
                    false,
                )
            })?;
        self.authorize_permission(
            &descriptor.snapshot_permission_id,
            descriptor.snapshot_resource.as_str(),
            Some(PermissionKind::Database),
        )?;
        let table = descriptor
            .snapshot_resource
            .database_table_name()
            .ok_or_else(|| {
                broker_error(
                    "knowledge-event-contract-invalid",
                    "signed Knowledge snapshot resource is invalid",
                    false,
                )
            })?;
        if !relation_has_tenant_uuid(pool, table, true).await {
            return Err(broker_error(
                "knowledge-event-projection-unavailable",
                "signed Knowledge snapshot projection is unavailable",
                false,
            ));
        }
        let tenant_id = Uuid::parse_str(&lease.tenant_id).map_err(|_| {
            broker_error(
                "lease-context-invalid",
                "invocation lease tenant is invalid",
                false,
            )
        })?;
        let service_actor_user_id = Uuid::parse_str(&lease.actor_id).map_err(|_| {
            broker_error(
                "lease-context-invalid",
                "invocation lease actor is invalid",
                false,
            )
        })?;
        let requested_by_user_id = lease.delegated_actor_id.unwrap_or(service_actor_user_id);
        let (acting_space_id, effective_role) = if descriptor.acting_space_id_field.is_some() {
            // This descriptor pins its context in a signed post-mutation
            // snapshot. That row only exists after the mutation, so the
            // membership guard is applied in capture_knowledge_event.
            (None, None)
        } else {
            let acting_space_id = lease
                .acting_space_id
                .as_deref()
                .and_then(|value| Uuid::parse_str(value).ok())
                .ok_or_else(|| {
                    broker_error(
                        "knowledge-event-context-required",
                        "Knowledge events require an inherited Team or Project context",
                        false,
                    )
                })?;
            let effective_role = validate_knowledge_event_context(
                pool,
                tenant_id,
                requested_by_user_id,
                service_actor_user_id,
                acting_space_id,
            )
            .await?;
            (Some(acting_space_id), Some(effective_role))
        };
        Ok(PreparedKnowledgeEvent {
            descriptor,
            tenant_id,
            publisher_bundle_id: caller.identity().id.as_str().to_string(),
            snapshot_filters: snapshot.filters.clone(),
            acting_space_id,
            requested_by_user_id,
            service_actor_user_id,
            effective_role,
        })
    }

    async fn handle_ssh_execute(
        &self,
        caller: &BrokerCaller,
        request: SshExecuteRequest,
    ) -> BrokerResponse {
        let Some(operation) = self
            .contract
            .manifest()
            .broker_operations
            .iter()
            .find(|operation| operation.id == request.operation_id)
        else {
            return BrokerResponse::Error(broker_error(
                "operation-not-requested",
                "signed package did not declare this broker operation",
                false,
            ));
        };
        if operation.kind != BrokerOperationKind::SshExecute {
            return BrokerResponse::Error(broker_error(
                "operation-not-supported",
                "signed broker operation is not an SSH executor profile",
                false,
            ));
        }
        for (permission_id, resource, kind) in [
            (
                &operation.network_permission_id,
                operation.network_resource.as_str(),
                PermissionKind::Network,
            ),
            (
                &operation.secret_permission_id,
                operation.secret_resource.as_str(),
                PermissionKind::SecretUse,
            ),
        ] {
            if let Err(error) = self.authorize_permission(permission_id, resource, Some(kind)) {
                return BrokerResponse::Error(error);
            }
        }
        let lease = match self.runtime.leases.validate(caller, &request.lease) {
            Ok(lease) => lease,
            Err(error) => return BrokerResponse::Error(error),
        };
        let tenant_id = match Uuid::parse_str(&lease.tenant_id) {
            Ok(tenant_id) if !tenant_id.is_nil() => tenant_id,
            _ => {
                return BrokerResponse::Error(broker_error(
                    "invalid-invocation-context",
                    "authenticated invocation tenant is not a non-nil UUID",
                    false,
                ))
            }
        };
        match self
            .runtime
            .ssh
            .execute(
                tenant_id,
                caller.identity().id.as_str(),
                &request.target_id,
                operation,
            )
            .await
        {
            Ok(result) => BrokerResponse::SshExecution(result),
            Err(error) => BrokerResponse::Error(ssh_broker_error(error)),
        }
    }

    async fn handle_intelligence_context(
        &self,
        caller: &BrokerCaller,
        request: IntelligenceContextRequest,
    ) -> BrokerResponse {
        if let Err(error) = self.authorize_permission(
            &request.permission_id,
            "knowledge:context",
            Some(PermissionKind::KnowledgeRead),
        ) {
            return BrokerResponse::Error(error);
        }
        let lease = match self.runtime.leases.validate(caller, &request.lease) {
            Ok(lease) => lease,
            Err(error) => return BrokerResponse::Error(error),
        };
        let binding = match intelligence_binding(&lease) {
            Ok(binding) => binding,
            Err(error) => return BrokerResponse::Error(error),
        };
        let Some(service) = &self.runtime.intelligence else {
            return BrokerResponse::Error(broker_error(
                "dependency-unavailable",
                "Core intelligence broker is not configured",
                true,
            ));
        };
        match service
            .resolve(binding, &caller.identity().id, request.draft)
            .await
        {
            Ok(pack) => BrokerResponse::KnowledgeContext(pack),
            Err(error) => BrokerResponse::Error(intelligence_broker_error(error)),
        }
    }

    async fn handle_outcome_feedback(
        &self,
        caller: &BrokerCaller,
        request: OutcomeFeedbackRequest,
    ) -> BrokerResponse {
        if let Err(error) = self.authorize_permission(
            &request.permission_id,
            "knowledge:feedback",
            Some(PermissionKind::KnowledgeFeedback),
        ) {
            return BrokerResponse::Error(error);
        }
        let lease = match self.runtime.leases.validate(caller, &request.lease) {
            Ok(lease) => lease,
            Err(error) => return BrokerResponse::Error(error),
        };
        let binding = match intelligence_binding(&lease) {
            Ok(binding) => binding,
            Err(error) => return BrokerResponse::Error(error),
        };
        let Some(service) = &self.runtime.intelligence else {
            return BrokerResponse::Error(broker_error(
                "dependency-unavailable",
                "Core intelligence broker is not configured",
                true,
            ));
        };
        match service
            .record_feedback(binding, &caller.identity().id, request.draft)
            .await
        {
            Ok(receipt) => BrokerResponse::OutcomeFeedbackAccepted(receipt),
            Err(error) => BrokerResponse::Error(intelligence_broker_error(error)),
        }
    }

    async fn handle_knowledge_collection(
        &self,
        caller: &BrokerCaller,
        request: KnowledgeCollectionRequest,
    ) -> BrokerResponse {
        if let Err(error) = self.authorize_permission(
            &request.permission_id,
            "knowledge:collection",
            Some(PermissionKind::KnowledgeCollection),
        ) {
            return BrokerResponse::Error(error);
        }
        let lease = match self.runtime.leases.validate(caller, &request.lease) {
            Ok(lease) => lease,
            Err(error) => return BrokerResponse::Error(error),
        };
        let actor = match knowledge_collection_actor(&lease) {
            Ok(actor) => actor,
            Err(error) => return BrokerResponse::Error(error),
        };
        let Some(pool) = &self.runtime.pool else {
            return BrokerResponse::Error(broker_error(
                "dependency-unavailable",
                "Core knowledge collection storage is not configured",
                true,
            ));
        };
        let result = match request.action {
            KnowledgeCollectionAction::List { limit } => {
                let mut rows = match collections::list_bundle_collections(
                    pool,
                    actor,
                    caller.identity().id.as_str(),
                    i64::from(limit) + 1,
                )
                .await
                {
                    Ok(rows) => rows,
                    Err(error) => return BrokerResponse::Error(knowledge_collection_error(error)),
                };
                let truncated = rows.len() > limit as usize;
                rows.truncate(limit as usize);
                let collections = match rows
                    .iter()
                    .map(collection_record)
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(records) => records,
                    Err(error) => return BrokerResponse::Error(error),
                };
                KnowledgeCollectionResult::Listed {
                    collections,
                    truncated,
                }
            }
            KnowledgeCollectionAction::Create {
                space_id,
                output_vault_id,
                profile_id,
                topic,
                schedule_enabled,
                locators,
                queries,
            } => {
                let (profile, recipe_sha256) = match self.collection_profile(&profile_id) {
                    Ok(profile) => profile,
                    Err(error) => return BrokerResponse::Error(error),
                };
                let (locators, queries) = match collection_inputs(profile, locators, queries) {
                    Ok(inputs) => inputs,
                    Err(error) => return BrokerResponse::Error(error),
                };
                let next_run_at =
                    match collection_schedule_next(profile.schedule.as_deref(), schedule_enabled) {
                        Ok(next_run_at) => next_run_at,
                        Err(error) => return BrokerResponse::Error(error),
                    };
                let space_id = match parse_uuid("space", &space_id) {
                    Ok(id) => id,
                    Err(error) => return BrokerResponse::Error(error),
                };
                let output_vault_id = match parse_uuid("output Vault", &output_vault_id) {
                    Ok(id) => id,
                    Err(error) => return BrokerResponse::Error(error),
                };
                let row = match collections::create_collection(
                    pool,
                    actor,
                    CreateKnowledgeCollection {
                        space_id,
                        output_vault_id,
                        bundle_id: caller.identity().id.to_string(),
                        profile_id: profile_id.to_string(),
                        label: profile.label.clone(),
                        topic,
                        connector: profile.connector.to_string(),
                        source_classes: profile
                            .source_classes
                            .iter()
                            .map(ToString::to_string)
                            .collect(),
                        allowed_domains: profile.allowlisted_domains.clone(),
                        freshness_seconds: match i64::try_from(profile.freshness_seconds) {
                            Ok(value) => value,
                            Err(_) => return BrokerResponse::Error(collection_profile_invalid()),
                        },
                        schedule: profile.schedule.clone(),
                        schedule_enabled,
                        next_run_at,
                        max_sources: match i32::try_from(profile.budget.max_sources) {
                            Ok(value) => value,
                            Err(_) => return BrokerResponse::Error(collection_profile_invalid()),
                        },
                        max_bytes: match i64::try_from(profile.budget.max_bytes) {
                            Ok(value) => value,
                            Err(_) => return BrokerResponse::Error(collection_profile_invalid()),
                        },
                        max_wall_seconds: match i32::try_from(profile.budget.max_wall_seconds) {
                            Ok(value) => value,
                            Err(_) => return BrokerResponse::Error(collection_profile_invalid()),
                        },
                        package_manifest_sha256: self.contract.manifest_sha256().to_string(),
                        recipe_asset_id: profile.recipe_asset.to_string(),
                        recipe_sha256,
                        locators,
                        queries,
                    },
                )
                .await
                {
                    Ok(row) => row,
                    Err(error) => return BrokerResponse::Error(knowledge_collection_error(error)),
                };
                match collection_record(&row) {
                    Ok(collection) => KnowledgeCollectionResult::Saved {
                        collection: Box::new(collection),
                    },
                    Err(error) => return BrokerResponse::Error(error),
                }
            }
            KnowledgeCollectionAction::Update {
                collection_id,
                expected_revision,
                topic,
                status,
                schedule_enabled,
                locators,
                queries,
            } => {
                let collection_id = match parse_uuid("collection", &collection_id) {
                    Ok(id) => id,
                    Err(error) => return BrokerResponse::Error(error),
                };
                let current = match collections::get_collection(
                    pool,
                    actor,
                    collection_id,
                    SpaceRole::Contributor,
                    true,
                )
                .await
                {
                    Ok(row) => row,
                    Err(error) => return BrokerResponse::Error(knowledge_collection_error(error)),
                };
                if let Err(error) = self.require_owned_collection(&current, true) {
                    return BrokerResponse::Error(error);
                }
                let profile_id = match LocalId::new(current.profile_id.clone()) {
                    Ok(id) => id,
                    Err(_) => return BrokerResponse::Error(collection_profile_invalid()),
                };
                let (profile, _) = match self.collection_profile(&profile_id) {
                    Ok(profile) => profile,
                    Err(error) => return BrokerResponse::Error(error),
                };
                let (locators, queries) = match collection_inputs(profile, locators, queries) {
                    Ok(inputs) => inputs,
                    Err(error) => return BrokerResponse::Error(error),
                };
                let next_run_at =
                    match collection_schedule_next(current.schedule.as_deref(), schedule_enabled) {
                        Ok(next_run_at) => next_run_at,
                        Err(error) => return BrokerResponse::Error(error),
                    };
                let row = match collections::update_collection(
                    pool,
                    actor,
                    collection_id,
                    UpdateKnowledgeCollection {
                        expected_revision,
                        topic,
                        status: status.as_str().to_string(),
                        schedule_enabled,
                        next_run_at,
                        locators,
                        queries,
                    },
                )
                .await
                {
                    Ok(row) => row,
                    Err(error) => return BrokerResponse::Error(knowledge_collection_error(error)),
                };
                match collection_record(&row) {
                    Ok(collection) => KnowledgeCollectionResult::Saved {
                        collection: Box::new(collection),
                    },
                    Err(error) => return BrokerResponse::Error(error),
                }
            }
            KnowledgeCollectionAction::Archive {
                collection_id,
                expected_revision,
            } => {
                let collection_id = match parse_uuid("collection", &collection_id) {
                    Ok(id) => id,
                    Err(error) => return BrokerResponse::Error(error),
                };
                let current = match collections::get_collection(
                    pool,
                    actor,
                    collection_id,
                    SpaceRole::Contributor,
                    true,
                )
                .await
                {
                    Ok(row) => row,
                    Err(error) => return BrokerResponse::Error(knowledge_collection_error(error)),
                };
                if let Err(error) = self.require_owned_collection(&current, false) {
                    return BrokerResponse::Error(error);
                }
                let row = match collections::archive_collection(
                    pool,
                    actor,
                    collection_id,
                    expected_revision,
                )
                .await
                {
                    Ok(row) => row,
                    Err(error) => return BrokerResponse::Error(knowledge_collection_error(error)),
                };
                match collection_record(&row) {
                    Ok(collection) => KnowledgeCollectionResult::Saved {
                        collection: Box::new(collection),
                    },
                    Err(error) => return BrokerResponse::Error(error),
                }
            }
            KnowledgeCollectionAction::Enqueue {
                collection_id,
                expected_revision,
            } => {
                let collection_id = match parse_uuid("collection", &collection_id) {
                    Ok(id) => id,
                    Err(error) => return BrokerResponse::Error(error),
                };
                let current = match collections::get_collection(
                    pool,
                    actor,
                    collection_id,
                    SpaceRole::Contributor,
                    true,
                )
                .await
                {
                    Ok(row) => row,
                    Err(error) => return BrokerResponse::Error(knowledge_collection_error(error)),
                };
                if let Err(error) = self.require_owned_collection(&current, true) {
                    return BrokerResponse::Error(error);
                }
                let Some(evaluator) = &self.runtime.policy_evaluator else {
                    return BrokerResponse::Error(broker_error(
                        "dependency-unavailable",
                        "Core collection policy evaluator is not configured",
                        true,
                    ));
                };
                let policy_revision = match evaluator.active_identity(actor.tenant_id).await {
                    Ok(identity) => identity.to_revision_ref(),
                    Err(_) => {
                        return BrokerResponse::Error(broker_error(
                            "dependency-unavailable",
                            "Core collection policy revision is unavailable",
                            true,
                        ))
                    }
                };
                let enqueued = match collections::enqueue_run(
                    pool,
                    actor,
                    collection_id,
                    EnqueueCollectionRun {
                        expected_collection_revision: expected_revision,
                        trigger: CollectionRunTrigger::OnDemand,
                        requested_by_user_id: actor.user_id,
                        on_behalf_of_user_id: actor.user_id,
                        tool_policy_revision: policy_revision,
                        scheduled_at: None,
                        next_schedule_at: None,
                    },
                )
                .await
                {
                    Ok(run) => run,
                    Err(error) => return BrokerResponse::Error(knowledge_collection_error(error)),
                };
                KnowledgeCollectionResult::Enqueued {
                    collection_id: enqueued.run.collection_id.to_string(),
                    run_id: enqueued.run.id.to_string(),
                    status: enqueued.run.status,
                    created: enqueued.created,
                }
            }
            _ => {
                return BrokerResponse::Error(broker_error(
                    "operation-not-supported",
                    "knowledge collection action is not supported by this Core build",
                    false,
                ))
            }
        };
        BrokerResponse::KnowledgeCollection(result)
    }

    fn collection_profile(
        &self,
        profile_id: &LocalId,
    ) -> Result<(&gadgetron_bundle_sdk::CollectionProfileDescriptor, String), BrokerError> {
        let profile = self
            .contract
            .manifest()
            .capabilities
            .collection_profiles
            .iter()
            .find(|profile| &profile.id == profile_id)
            .ok_or_else(|| {
                broker_error(
                    "collection-profile-not-signed",
                    "signed package does not declare this collection profile",
                    false,
                )
            })?;
        let asset = self
            .contract
            .manifest()
            .capabilities
            .seed_assets
            .iter()
            .find(|asset| asset.id == profile.recipe_asset)
            .ok_or_else(collection_profile_invalid)?;
        Ok((profile, asset.sha256.clone()))
    }

    fn require_owned_collection(
        &self,
        collection: &collections::KnowledgeCollectionRow,
        require_current_profile: bool,
    ) -> Result<(), BrokerError> {
        if collection.bundle_id != self.contract.runtime_identity().id.as_str() {
            return Err(broker_error(
                "collection-not-owned",
                "collection is not owned by this signed Bundle",
                false,
            ));
        }
        if !require_current_profile {
            return Ok(());
        }
        let profile_id = LocalId::new(collection.profile_id.clone())
            .map_err(|_| collection_profile_invalid())?;
        let (profile, recipe_sha256) = self.collection_profile(&profile_id)?;
        if collection.package_manifest_sha256 != self.contract.manifest_sha256()
            || collection.recipe_asset_id != profile.recipe_asset.as_str()
            || collection.recipe_sha256 != recipe_sha256
        {
            return Err(broker_error(
                "collection-profile-changed",
                "signed collection profile changed; archive this Topic and create a new one",
                false,
            ));
        }
        Ok(())
    }

    fn authorize_permission(
        &self,
        permission_id: &LocalId,
        resource: &str,
        required_kind: Option<PermissionKind>,
    ) -> Result<PermissionKind, BrokerError> {
        let Some(permission) = self
            .contract
            .manifest()
            .permissions
            .iter()
            .find(|permission| &permission.id == permission_id)
        else {
            return Err(broker_error(
                "permission-not-requested",
                "signed package did not request this broker permission",
                false,
            ));
        };
        if required_kind.is_some_and(|kind| permission.kind != kind)
            || !permission.resources.iter().any(|item| item == resource)
        {
            return Err(broker_error(
                "resource-not-requested",
                "signed package did not request this operation and exact resource",
                false,
            ));
        }
        let granted = self
            .runtime
            .grants
            .get(self.contract.runtime_identity().id.as_str())
            .is_some_and(|grant| {
                grant.allows(
                    self.contract.manifest_sha256(),
                    permission_id,
                    permission.kind,
                    resource,
                )
            });
        if !granted {
            return Err(broker_error(
                "permission-not-granted",
                "operator grant is absent, revoked or pinned to another package digest",
                false,
            ));
        }
        Ok(permission.kind)
    }
}

fn knowledge_collection_actor(lease: &InvocationLeaseBinding) -> Result<SpaceActor, BrokerError> {
    let tenant_id = parse_uuid("invocation tenant", &lease.tenant_id)?;
    let user_id = parse_uuid("invocation actor", &lease.actor_id)?;
    if tenant_id.is_nil() || user_id.is_nil() {
        return Err(broker_error(
            "invalid-invocation-context",
            "authenticated invocation tenant and actor must not be nil",
            false,
        ));
    }
    Ok(SpaceActor { tenant_id, user_id })
}

fn parse_uuid(label: &str, value: &str) -> Result<Uuid, BrokerError> {
    Uuid::parse_str(value).map_err(|_| {
        broker_error(
            "invalid-invocation-context",
            &format!("authenticated {label} is not a UUID"),
            false,
        )
    })
}

fn collection_inputs(
    profile: &gadgetron_bundle_sdk::CollectionProfileDescriptor,
    locators: Vec<KnowledgeCollectionLocator>,
    queries: Vec<KnowledgeCollectionQuery>,
) -> Result<(Vec<CollectionLocator>, Vec<CollectionQuery>), BrokerError> {
    let locators = locators
        .into_iter()
        .map(|locator| CollectionLocator {
            url: locator.url,
            title: locator.title,
            source_class: locator.source_class.to_string(),
        })
        .collect::<Vec<_>>();
    let queries = queries
        .into_iter()
        .map(|query| CollectionQuery {
            provider: query.provider.to_string(),
            query: query.query,
            scope: query.scope,
            tags: query.tags,
            language: query.language,
            window_days: query.window_days,
        })
        .collect::<Vec<_>>();
    let generated =
        super::knowledge_collections::validated_collection_inputs(profile, &locators, &queries)
            .map_err(|_| {
                broker_error(
                    "collection-input-not-signed",
                    "collection sources are outside the signed profile or Core connector boundary",
                    false,
                )
            })?;
    Ok((generated, queries))
}

fn collection_schedule_next(
    schedule: Option<&str>,
    enabled: bool,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, BrokerError> {
    if !enabled {
        return Ok(None);
    }
    let schedule = schedule.ok_or_else(|| {
        broker_error(
            "collection-schedule-not-signed",
            "this collection profile has no signed schedule",
            false,
        )
    })?;
    crate::knowledge_collections::next_schedule_after(schedule, chrono::Utc::now())
        .map(Some)
        .map_err(|_| collection_profile_invalid())
}

fn collection_record(
    row: &collections::KnowledgeCollectionRow,
) -> Result<KnowledgeCollectionRecord, BrokerError> {
    let profile_id = LocalId::new(row.profile_id.clone()).map_err(|_| {
        broker_error(
            "collection-state-invalid",
            "persisted collection profile id is invalid",
            false,
        )
    })?;
    let source_classes = row
        .source_classes
        .iter()
        .map(|class| {
            LocalId::new(class.clone()).map_err(|_| {
                broker_error(
                    "collection-state-invalid",
                    "persisted collection source class is invalid",
                    false,
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let locators = row
        .parsed_locators()
        .map_err(knowledge_collection_error)?
        .into_iter()
        .map(|locator| {
            let source_class = LocalId::new(locator.source_class).map_err(|_| {
                broker_error(
                    "collection-state-invalid",
                    "persisted collection locator source class is invalid",
                    false,
                )
            })?;
            Ok(KnowledgeCollectionLocator::new(
                locator.url,
                locator.title,
                source_class,
            ))
        })
        .collect::<Result<Vec<_>, BrokerError>>()?;
    let queries =
        row.parsed_queries()
            .map_err(knowledge_collection_error)?
            .into_iter()
            .map(|query| {
                let provider = LocalId::new(query.provider).map_err(|_| {
                    broker_error(
                        "collection-state-invalid",
                        "persisted collection query provider is invalid",
                        false,
                    )
                })?;
                Ok(KnowledgeCollectionQuery::new(
                    provider,
                    query.query,
                    query.scope,
                    query.window_days,
                )
                .with_tags(query.tags)
                .with_language(query.language))
            })
            .collect::<Result<Vec<_>, BrokerError>>()?;
    Ok(KnowledgeCollectionRecord::new(
        row.id.to_string(),
        row.space_id.to_string(),
        row.output_vault_id.to_string(),
        profile_id,
        row.label.clone(),
        row.topic.clone(),
        row.status.clone(),
        source_classes,
        row.schedule_enabled,
        row.next_run_at.map(|time| time.to_rfc3339()),
        row.last_enqueued_at.map(|time| time.to_rfc3339()),
        row.last_run_at.map(|time| time.to_rfc3339()),
        locators,
        queries,
        row.revision,
        row.updated_at.to_rfc3339(),
    ))
}

fn collection_profile_invalid() -> BrokerError {
    broker_error(
        "collection-profile-invalid",
        "signed collection profile is not executable by this Core build",
        false,
    )
}

fn knowledge_collection_error(error: KnowledgeCollectionError) -> BrokerError {
    match error {
        KnowledgeCollectionError::InvalidInput(_)
        | KnowledgeCollectionError::InvalidPersisted(_) => broker_error(
            "collection-request-invalid",
            "knowledge collection request or stored state is invalid",
            false,
        ),
        KnowledgeCollectionError::NotFound => broker_error(
            "collection-not-found",
            "knowledge collection is absent or not visible",
            false,
        ),
        KnowledgeCollectionError::Conflict | KnowledgeCollectionError::LeaseLost => broker_error(
            "collection-revision-conflict",
            "knowledge collection changed; read its current revision and retry",
            false,
        ),
        KnowledgeCollectionError::Space(error) => match error {
            gadgetron_xaas::knowledge_spaces::KnowledgeSpaceError::Forbidden
            | gadgetron_xaas::knowledge_spaces::KnowledgeSpaceError::NotFound => broker_error(
                "collection-forbidden",
                "Knowledge Space or Vault is absent or not visible",
                false,
            ),
            gadgetron_xaas::knowledge_spaces::KnowledgeSpaceError::RevisionConflict
            | gadgetron_xaas::knowledge_spaces::KnowledgeSpaceError::Conflict => broker_error(
                "collection-revision-conflict",
                "Knowledge Space or Vault state changed",
                false,
            ),
            gadgetron_xaas::knowledge_spaces::KnowledgeSpaceError::InvalidInput(_) => broker_error(
                "collection-request-invalid",
                "Knowledge Space or Vault input is invalid",
                false,
            ),
            gadgetron_xaas::knowledge_spaces::KnowledgeSpaceError::Database(_) => broker_error(
                "dependency-unavailable",
                "knowledge storage is unavailable",
                true,
            ),
        },
        KnowledgeCollectionError::ServicePrincipal(_) | KnowledgeCollectionError::Database(_) => {
            broker_error(
                "dependency-unavailable",
                "knowledge collection dependency is unavailable",
                true,
            )
        }
    }
}

async fn relation_has_tenant_uuid(pool: &PgPool, relation: &str, allow_views: bool) -> bool {
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM information_schema.tables AS t
            JOIN information_schema.columns AS c
              ON c.table_schema = t.table_schema
             AND c.table_name = t.table_name
            WHERE t.table_schema = 'public'
              AND t.table_name = $1
              AND t.table_type IN ('BASE TABLE', 'VIEW')
              AND ($2 OR t.table_type = 'BASE TABLE')
              AND c.column_name = 'tenant_id'
              AND c.udt_name = 'uuid'
        )
        "#,
    )
    .bind(relation)
    .bind(allow_views)
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

async fn execute_database_select(
    pool: &PgPool,
    tenant_id: &str,
    request: &DatabaseSelectRequest,
) -> Result<DatabaseRows, BrokerError> {
    let tenant_id = Uuid::parse_str(tenant_id).map_err(|_| {
        broker_error(
            "invalid-invocation-context",
            "authenticated invocation tenant is not a UUID",
            false,
        )
    })?;
    if tenant_id.is_nil() {
        return Err(broker_error(
            "invalid-invocation-context",
            "authenticated invocation tenant must not be nil",
            false,
        ));
    }
    let table = request.resource.database_table_name().ok_or_else(|| {
        broker_error("resource-not-supported", "invalid database resource", false)
    })?;
    if !relation_has_tenant_uuid(pool, table, true).await {
        return Err(broker_error(
            "resource-unavailable",
            "database resource is absent or lacks a UUID tenant_id boundary",
            false,
        ));
    }

    let mut transaction = pool.begin().await.map_err(|_| {
        broker_error(
            "dependency-unavailable",
            "database broker could not start a transaction",
            true,
        )
    })?;
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *transaction)
        .await
        .map_err(|_| {
            broker_error(
                "dependency-unavailable",
                "database broker could not enforce a read-only transaction",
                true,
            )
        })?;
    sqlx::query(&format!(
        "SET LOCAL statement_timeout = '{DATABASE_STATEMENT_TIMEOUT}'"
    ))
    .execute(&mut *transaction)
    .await
    .map_err(|_| {
        broker_error(
            "dependency-unavailable",
            "database broker could not enforce its statement timeout",
            true,
        )
    })?;

    let mut query = QueryBuilder::<Postgres>::new("SELECT jsonb_build_object(");
    for (index, column) in request.columns.iter().enumerate() {
        if index > 0 {
            query.push(", ");
        }
        query
            .push("'")
            .push(column.as_str())
            .push("', ")
            .push(quoted_identifier(column));
    }
    query
        .push(") FROM ")
        .push(quoted_identifier(table))
        .push(" WHERE \"tenant_id\" = ")
        .push_bind(tenant_id);
    for (field, value) in &request.filters {
        if value.is_null() {
            query
                .push(" AND ")
                .push(quoted_identifier(field))
                .push(" IS NULL");
        } else {
            query
                .push(" AND to_jsonb(")
                .push(quoted_identifier(field))
                .push(") = ")
                .push_bind(sqlx::types::Json(value.clone()));
        }
    }
    for (index, order) in request.order_by.iter().enumerate() {
        query.push(if index == 0 { " ORDER BY " } else { ", " });
        query.push(quoted_identifier(&order.field));
        match order.direction {
            gadgetron_bundle_sdk::DatabaseOrderDirection::Ascending => query.push(" ASC"),
            gadgetron_bundle_sdk::DatabaseOrderDirection::Descending => query.push(" DESC"),
            _ => query.push(" ASC"),
        };
    }
    query
        .push(" LIMIT ")
        .push_bind(i64::from(request.limit) + 1);

    let mut rows = query
        .build_query_scalar::<serde_json::Value>()
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| {
            broker_error(
                "database-request-rejected",
                "database broker rejected the structured read",
                false,
            )
        })?;
    transaction.commit().await.map_err(|_| {
        broker_error(
            "dependency-unavailable",
            "database broker could not complete the read-only transaction",
            true,
        )
    })?;

    let truncated = rows.len() > request.limit as usize;
    if truncated {
        rows.truncate(request.limit as usize);
    }
    let mut object_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let serde_json::Value::Object(row) = row else {
            return Err(broker_error(
                "database-response-invalid",
                "database broker produced a non-object row",
                false,
            ));
        };
        object_rows.push(row.into_iter().collect());
    }
    let rows = DatabaseRows::new(object_rows, truncated);
    let response = BrokerResponse::DatabaseRows(rows.clone());
    if response.validate().is_err() {
        return Err(broker_error(
            "database-response-too-large",
            "database broker response exceeded its row or byte ceiling",
            false,
        ));
    }
    Ok(rows)
}

async fn validate_knowledge_event_context(
    pool: &PgPool,
    tenant_id: Uuid,
    requested_by_user_id: Uuid,
    service_actor_user_id: Uuid,
    acting_space_id: Uuid,
) -> Result<String, BrokerError> {
    let requested_space = gadgetron_xaas::knowledge_spaces::effective_spaces(
        pool,
        SpaceActor {
            tenant_id,
            user_id: requested_by_user_id,
        },
    )
    .await
    .map_err(|_| event_context_forbidden())?
    .into_iter()
    .find(|candidate| candidate.space.id == acting_space_id)
    .filter(|candidate| candidate.effective_role != SpaceRole::Viewer)
    .ok_or_else(event_context_forbidden)?;
    if requested_space.space.status != "active"
        || !matches!(requested_space.space.kind.as_str(), "project" | "team")
    {
        return Err(event_context_forbidden());
    }
    let service_space = gadgetron_xaas::knowledge_spaces::effective_spaces(
        pool,
        SpaceActor {
            tenant_id,
            user_id: service_actor_user_id,
        },
    )
    .await
    .map_err(|_| event_context_forbidden())?
    .into_iter()
    .find(|candidate| candidate.space.id == acting_space_id)
    .filter(|candidate| candidate.effective_role != SpaceRole::Viewer)
    .ok_or_else(event_context_forbidden)?;
    if service_space.space.status != "active" {
        return Err(event_context_forbidden());
    }
    Ok(requested_space.effective_role.as_str().to_string())
}

/// The Server package owns alert detection, while Core owns the target control
/// plane and its Space assignment.  Keep this first-consumer bridge here so a
/// package never accepts a caller-provided Space or guesses one from a target.
async fn stamp_server_incident_acting_space(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    ssh: Option<&BundleSshControlPlane>,
    caller_bundle_id: Option<&str>,
    tenant_id: Uuid,
    table: &str,
    request: &DatabaseInsertRequest,
) -> Result<(), BrokerError> {
    if caller_bundle_id != Some("server-administrator") || table != "alert_state" {
        return Ok(());
    }
    let Some(ssh) = ssh else {
        return Ok(());
    };
    let Some(fingerprint) = request
        .values
        .get("fingerprint")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(());
    };
    let Some(host_id) = request
        .values
        .get("host_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
    else {
        return Ok(());
    };
    let target_id = sqlx::query_scalar::<_, String>(
        "SELECT target_id FROM server_target_health WHERE tenant_id = $1 AND host_id = $2",
    )
    .bind(tenant_id)
    .bind(host_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| database_dependency_unavailable())?;
    let Some(target_id) = target_id else {
        return Ok(());
    };
    let acting_space_id = ssh
        .list_targets(tenant_id, "server-administrator")
        .targets
        .into_iter()
        .find(|target| target.target_id == target_id)
        .and_then(|target| target.acting_space_id);
    let Some(acting_space_id) = acting_space_id else {
        return Ok(());
    };
    sqlx::query(
        "UPDATE server_incidents \
         SET acting_space_id = $1 \
         WHERE tenant_id = $2 AND fingerprint = $3 \
           AND status = 'active' AND acting_space_id IS NULL",
    )
    .bind(acting_space_id)
    .bind(tenant_id)
    .bind(fingerprint)
    .execute(&mut **transaction)
    .await
    .map_err(|_| database_dependency_unavailable())?;
    Ok(())
}

async fn execute_database_insert(
    pool: &PgPool,
    ssh: Option<&BundleSshControlPlane>,
    caller_bundle_id: Option<&str>,
    tenant_id: &str,
    request: &DatabaseInsertRequest,
    event: Option<EnqueueBundleEvent>,
) -> Result<DatabaseMutationResult, BrokerError> {
    let (tenant_id, table) = validated_database_target(pool, tenant_id, &request.resource).await?;
    let mut object: serde_json::Map<String, serde_json::Value> =
        request.values.clone().into_iter().collect();
    object.insert(
        "tenant_id".into(),
        serde_json::Value::String(tenant_id.to_string()),
    );

    let mut transaction = begin_mutation_transaction(pool).await?;
    let mut query = QueryBuilder::<Postgres>::new("INSERT INTO ");
    query.push(quoted_identifier(table)).push(" (");
    let fields: Vec<&str> = std::iter::once("tenant_id")
        .chain(request.values.keys().map(String::as_str))
        .collect();
    push_identifier_list(&mut query, &fields);
    query.push(") SELECT ");
    push_identifier_list(&mut query, &fields);
    query
        .push(" FROM jsonb_populate_record(NULL::")
        .push(quoted_identifier(table))
        .push(", ")
        .push_bind(sqlx::types::Json(serde_json::Value::Object(object)))
        .push(")");
    if !request.conflict_keys.is_empty() {
        let conflict: Vec<&str> = std::iter::once("tenant_id")
            .chain(request.conflict_keys.iter().map(String::as_str))
            .collect();
        query.push(" ON CONFLICT (");
        push_identifier_list(&mut query, &conflict);
        query.push(") DO UPDATE SET ");
        let updates: Vec<&str> = request
            .values
            .keys()
            .filter(|field| !request.conflict_keys.contains(*field))
            .map(String::as_str)
            .collect();
        for (index, field) in updates.iter().enumerate() {
            if index > 0 {
                query.push(", ");
            }
            query
                .push(quoted_identifier(field))
                .push(" = EXCLUDED.")
                .push(quoted_identifier(field));
        }
    }
    let affected = query
        .build()
        .execute(&mut *transaction)
        .await
        .map_err(|_| database_request_rejected())?
        .rows_affected();
    if affected == 1 {
        stamp_server_incident_acting_space(
            &mut transaction,
            ssh,
            caller_bundle_id,
            tenant_id,
            table,
            request,
        )
        .await?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| database_dependency_unavailable())?;
    if let Some(event) = event {
        enqueue_optional_bundle_event(pool, event).await;
    }
    mutation_result(affected, 1)
}

async fn enqueue_optional_bundle_event(pool: &PgPool, event: EnqueueBundleEvent) {
    let mut transaction = match begin_mutation_transaction(pool).await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(
                target: "bundle_broker",
                bundle_id = event.subject_bundle_id,
                event_kind = event.event_kind,
                error_code = error.code.as_str(),
                "authoritative database mutation committed but optional Bundle enrichment dispatch could not start"
            );
            return;
        }
    };
    if gadgetron_xaas::autonomy::enqueue_bundle_event_in_transaction(
        &mut transaction,
        event.clone(),
    )
    .await
    .is_err()
    {
        tracing::warn!(
            target: "bundle_broker",
            bundle_id = event.subject_bundle_id,
            event_kind = event.event_kind,
            "authoritative database mutation committed but optional Bundle enrichment dispatch was rejected"
        );
        return;
    }
    if transaction.commit().await.is_err() {
        tracing::warn!(
            target: "bundle_broker",
            bundle_id = event.subject_bundle_id,
            event_kind = event.event_kind,
            "authoritative database mutation committed but optional Bundle enrichment dispatch was not persisted"
        );
    }
}

async fn execute_database_update(
    pool: &PgPool,
    tenant_id: &str,
    request: &DatabaseUpdateRequest,
    event: Option<PreparedKnowledgeEvent>,
) -> Result<DatabaseMutationResult, BrokerError> {
    let (tenant_id, table) = validated_database_target(pool, tenant_id, &request.resource).await?;
    let object: serde_json::Map<String, serde_json::Value> =
        request.values.clone().into_iter().collect();
    let mut transaction = begin_mutation_transaction(pool).await?;
    let mut query =
        QueryBuilder::<Postgres>::new("WITH patch AS (SELECT * FROM jsonb_populate_record(NULL::");
    query
        .push(quoted_identifier(table))
        .push(", ")
        .push_bind(sqlx::types::Json(serde_json::Value::Object(object)))
        .push(")), targets AS (SELECT ctid FROM ")
        .push(quoted_identifier(table))
        .push(" WHERE \"tenant_id\" = ")
        .push_bind(tenant_id);
    push_database_filters(&mut query, &request.filters, None);
    query
        .push(" LIMIT ")
        .push_bind(i64::from(request.limit))
        .push(" FOR UPDATE) UPDATE ")
        .push(quoted_identifier(table))
        .push(" AS target SET ");
    for (index, field) in request.values.keys().enumerate() {
        if index > 0 {
            query.push(", ");
        }
        query
            .push(quoted_identifier(field))
            .push(" = patch.")
            .push(quoted_identifier(field));
    }
    query.push(" FROM patch, targets WHERE target.ctid = targets.ctid");
    let affected = query
        .build()
        .execute(&mut *transaction)
        .await
        .map_err(|_| database_request_rejected())?
        .rows_affected();
    if affected == 1 {
        if let Some(event) = event {
            capture_knowledge_event(pool, &mut transaction, event).await?;
        }
    }
    transaction
        .commit()
        .await
        .map_err(|_| database_dependency_unavailable())?;
    mutation_result(affected, request.limit)
}

async fn execute_database_delete(
    pool: &PgPool,
    tenant_id: &str,
    request: &DatabaseDeleteRequest,
    event: Option<PreparedKnowledgeEvent>,
) -> Result<DatabaseMutationResult, BrokerError> {
    let (tenant_id, table) = validated_database_target(pool, tenant_id, &request.resource).await?;
    let mut transaction = begin_mutation_transaction(pool).await?;
    let mut query = QueryBuilder::<Postgres>::new("WITH targets AS (SELECT ctid FROM ");
    query
        .push(quoted_identifier(table))
        .push(" WHERE \"tenant_id\" = ")
        .push_bind(tenant_id);
    push_database_filters(&mut query, &request.filters, None);
    query
        .push(" LIMIT ")
        .push_bind(i64::from(request.limit))
        .push(" FOR UPDATE) DELETE FROM ")
        .push(quoted_identifier(table))
        .push(" AS target USING targets WHERE target.ctid = targets.ctid");
    let affected = query
        .build()
        .execute(&mut *transaction)
        .await
        .map_err(|_| database_request_rejected())?
        .rows_affected();
    if affected == 1 {
        if let Some(event) = event {
            capture_knowledge_event(pool, &mut transaction, event).await?;
        }
    }
    transaction
        .commit()
        .await
        .map_err(|_| database_dependency_unavailable())?;
    mutation_result(affected, request.limit)
}

async fn capture_knowledge_event(
    pool: &PgPool,
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    event: PreparedKnowledgeEvent,
) -> Result<(), BrokerError> {
    let table = event
        .descriptor
        .snapshot_resource
        .database_table_name()
        .expect("validated Knowledge snapshot resource");
    let mut query = QueryBuilder::<Postgres>::new("SELECT jsonb_build_object(");
    for (index, field) in event.descriptor.snapshot_fields.iter().enumerate() {
        if index > 0 {
            query.push(", ");
        }
        query
            .push("'")
            .push(field)
            .push("', ")
            .push(quoted_identifier(field));
    }
    query
        .push(") FROM ")
        .push(quoted_identifier(table))
        .push(" WHERE \"tenant_id\" = ")
        .push_bind(event.tenant_id);
    push_database_filters(&mut query, &event.snapshot_filters, None);
    query.push(" LIMIT 2");
    let rows = query
        .build_query_scalar::<serde_json::Value>()
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| {
            broker_error(
                "knowledge-event-snapshot-failed",
                "post-mutation Knowledge snapshot could not be read",
                true,
            )
        })?;
    if rows.is_empty() {
        // The mutation may end one signal without closing its incident. The
        // signed projection contains closed incidents only, so no row means
        // there is no domain event to enqueue in this transaction.
        return Ok(());
    }
    if rows.len() != 1 {
        return Err(broker_error(
            "knowledge-event-snapshot-ambiguous",
            "post-mutation Knowledge snapshot must resolve exactly one subject",
            false,
        ));
    }
    let snapshot = canonical_json(rows.into_iter().next().expect("one snapshot row"));
    let subject_id = snapshot
        .get(&event.descriptor.subject_id_field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or_else(|| invalid_knowledge_snapshot("subject id"))?
        .to_string();
    let subject_revision = snapshot
        .get(&event.descriptor.subject_revision_field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or_else(|| invalid_knowledge_snapshot("subject revision"))?
        .to_string();
    let source_title = snapshot
        .get(&event.descriptor.title_field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().count() <= 512)
        .ok_or_else(|| invalid_knowledge_snapshot("title"))?
        .to_string();
    let (acting_space_id, effective_role) = match &event.descriptor.acting_space_id_field {
        Some(field) => {
            let Some(space_value) = snapshot.get(field).filter(|value| !value.is_null()) else {
                // A legacy incident without a resolvable Core Space remains
                // operational, but cannot create a Knowledge event.
                return Ok(());
            };
            let acting_space_id = space_value
                .as_str()
                .and_then(|value| Uuid::parse_str(value).ok())
                .ok_or_else(|| invalid_knowledge_snapshot("acting Space id"))?;
            let effective_role = validate_knowledge_event_context(
                pool,
                event.tenant_id,
                event.requested_by_user_id,
                event.service_actor_user_id,
                acting_space_id,
            )
            .await?;
            (acting_space_id, effective_role)
        }
        None => (
            event
                .acting_space_id
                .expect("lease context validated before mutation"),
            event
                .effective_role
                .expect("lease context validated before mutation"),
        ),
    };
    let encoded =
        serde_json::to_vec(&snapshot).map_err(|_| invalid_knowledge_snapshot("canonical JSON"))?;
    let snapshot_hash = format!("sha256:{}", hex::encode(Sha256::digest(&encoded)));
    gadgetron_xaas::knowledge_events::enqueue_in_transaction(
        transaction,
        EnqueueKnowledgeEvent {
            tenant_id: event.tenant_id,
            descriptor_id: event.descriptor.id.as_str().to_string(),
            event_kind: event.descriptor.event_kind.as_str().to_string(),
            publisher_bundle_id: event.publisher_bundle_id,
            subject_kind: event.descriptor.subject_kind.as_str().to_string(),
            subject_id,
            subject_revision,
            snapshot,
            snapshot_hash,
            source_title,
            source_path_prefix: event.descriptor.source_path_prefix,
            acting_space_id,
            output_vault_bundle: event.descriptor.output_vault_bundle.as_str().to_string(),
            knowledge_schema_id: event.descriptor.knowledge_schema_id,
            researcher_bundle_id: event.descriptor.researcher_bundle.as_str().to_string(),
            researcher_role_id: event.descriptor.researcher_role.as_str().to_string(),
            requested_by_user_id: event.requested_by_user_id,
            service_actor_user_id: event.service_actor_user_id,
            effective_role,
        },
    )
    .await
    .map_err(|_| {
        broker_error(
            "knowledge-event-enqueue-failed",
            "post-mutation Knowledge event could not be durably enqueued",
            true,
        )
    })?;
    Ok(())
}

fn canonical_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, canonical_json(value)))
                .collect(),
        ),
        value => value,
    }
}

fn invalid_knowledge_snapshot(field: &str) -> BrokerError {
    broker_error(
        "knowledge-event-snapshot-invalid",
        &format!("post-mutation Knowledge snapshot has an invalid {field}"),
        false,
    )
}

async fn validated_database_target<'a>(
    pool: &PgPool,
    tenant_id: &str,
    resource: &'a gadgetron_bundle_sdk::BrokerResource,
) -> Result<(Uuid, &'a str), BrokerError> {
    let tenant_id = Uuid::parse_str(tenant_id).map_err(|_| {
        broker_error(
            "invalid-invocation-context",
            "authenticated invocation tenant is not a UUID",
            false,
        )
    })?;
    if tenant_id.is_nil() {
        return Err(broker_error(
            "invalid-invocation-context",
            "authenticated invocation tenant must not be nil",
            false,
        ));
    }
    let table = resource.database_table_name().ok_or_else(|| {
        broker_error("resource-not-supported", "invalid database resource", false)
    })?;
    if !relation_has_tenant_uuid(pool, table, false).await {
        return Err(broker_error(
            "resource-unavailable",
            "database resource is absent or lacks a UUID tenant_id boundary",
            false,
        ));
    }
    Ok((tenant_id, table))
}

async fn begin_mutation_transaction(
    pool: &PgPool,
) -> Result<sqlx::Transaction<'_, Postgres>, BrokerError> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| database_dependency_unavailable())?;
    sqlx::query(&format!(
        "SET LOCAL statement_timeout = '{DATABASE_STATEMENT_TIMEOUT}'"
    ))
    .execute(&mut *transaction)
    .await
    .map_err(|_| database_dependency_unavailable())?;
    Ok(transaction)
}

fn push_identifier_list(query: &mut QueryBuilder<'_, Postgres>, fields: &[&str]) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            query.push(", ");
        }
        query.push(quoted_identifier(field));
    }
}

fn push_database_filters(
    query: &mut QueryBuilder<'_, Postgres>,
    filters: &BTreeMap<String, serde_json::Value>,
    qualifier: Option<&str>,
) {
    for (field, value) in filters {
        query.push(" AND ");
        if let Some(qualifier) = qualifier {
            query.push(qualifier).push(".");
        }
        if value.is_null() {
            query.push(quoted_identifier(field)).push(" IS NULL");
        } else {
            query
                .push("to_jsonb(")
                .push(quoted_identifier(field))
                .push(") = ")
                .push_bind(sqlx::types::Json(value.clone()));
        }
    }
}

fn mutation_result(affected: u64, limit: u32) -> Result<DatabaseMutationResult, BrokerError> {
    if affected > u64::from(limit) {
        return Err(broker_error(
            "database-mutation-overflow",
            "database mutation exceeded its signed row ceiling",
            false,
        ));
    }
    let affected = u32::try_from(affected).map_err(|_| {
        broker_error(
            "database-mutation-overflow",
            "database mutation row count cannot be represented",
            false,
        )
    })?;
    Ok(DatabaseMutationResult::new(affected))
}

fn database_dependency_unavailable() -> BrokerError {
    broker_error(
        "dependency-unavailable",
        "database broker dependency is unavailable",
        true,
    )
}

fn event_context_forbidden() -> BrokerError {
    broker_error(
        "event-context-forbidden",
        "Bundle event enrichment did not inherit an authorized Team or Project context",
        false,
    )
}

fn database_request_rejected() -> BrokerError {
    broker_error(
        "database-request-rejected",
        "database broker rejected the structured mutation",
        false,
    )
}

fn quoted_identifier(identifier: &str) -> String {
    debug_assert!(identifier
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'));
    format!("\"{identifier}\"")
}

fn broker_error(code: &str, message: &str, retryable: bool) -> BrokerError {
    BrokerError::new(
        LocalId::new(code).expect("static broker error id is canonical"),
        message,
        retryable,
    )
}

fn intelligence_binding(
    lease: &InvocationLeaseBinding,
) -> Result<IntelligenceActorBinding, BrokerError> {
    let tenant_id = Uuid::parse_str(&lease.tenant_id).map_err(|_| {
        broker_error(
            "invalid-invocation-context",
            "authenticated invocation tenant is not a UUID",
            false,
        )
    })?;
    let actor_id = Uuid::parse_str(&lease.actor_id).map_err(|_| {
        broker_error(
            "invalid-invocation-context",
            "authenticated invocation actor is not a UUID",
            false,
        )
    })?;
    if tenant_id.is_nil() || actor_id.is_nil() {
        return Err(broker_error(
            "invalid-invocation-context",
            "authenticated invocation tenant and actor must not be nil",
            false,
        ));
    }
    Ok(IntelligenceActorBinding {
        tenant_id,
        actor_id,
        authority_actor_id: lease.delegated_actor_id,
        acting_space_id: lease
            .acting_space_id
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|_| {
                broker_error(
                    "invalid-invocation-context",
                    "authenticated invocation acting Space is not a UUID",
                    false,
                )
            })?,
    })
}

fn intelligence_broker_error(error: IntelligenceContextError) -> BrokerError {
    match error {
        IntelligenceContextError::Invalid(detail) => {
            tracing::warn!(
                target: "bundle_broker",
                error = %detail,
                "Core intelligence broker rejected a generated contract"
            );
            broker_error(
                "intelligence-request-invalid",
                "intelligence request failed contract validation",
                false,
            )
        }
        IntelligenceContextError::Forbidden => broker_error(
            "intelligence-context-forbidden",
            "knowledge context is not visible to this invocation actor",
            false,
        ),
        IntelligenceContextError::Unavailable => broker_error(
            "dependency-unavailable",
            "knowledge context dependency is unavailable",
            true,
        ),
        IntelligenceContextError::Conflict => broker_error(
            "intelligence-revision-conflict",
            "knowledge context id, subject or revision conflicts with prior evidence",
            false,
        ),
        IntelligenceContextError::Persistence => broker_error(
            "dependency-unavailable",
            "knowledge context persistence is unavailable",
            true,
        ),
    }
}

fn ssh_broker_error(error: BundleSshError) -> BrokerError {
    match error {
        BundleSshError::TargetNotFound => broker_error(
            "target-not-found",
            "approved SSH target does not exist for this tenant and Bundle",
            false,
        ),
        BundleSshError::SecretNotFound => broker_error(
            "secret-not-found",
            "approved SSH target secret is unavailable",
            false,
        ),
        BundleSshError::DnsChanged => broker_error(
            "target-address-changed",
            "SSH target address changed and requires Manager re-approval",
            false,
        ),
        BundleSshError::TargetRevisionChanged => broker_error(
            "target-revision-changed",
            "SSH target revision changed and requires a fresh setup plan",
            false,
        ),
        BundleSshError::Timeout => broker_error(
            "ssh-timeout",
            "signed SSH operation exceeded its wall-time ceiling",
            true,
        ),
        BundleSshError::OutputLimit => broker_error(
            "ssh-output-limit",
            "signed SSH operation exceeded its output ceiling",
            false,
        ),
        BundleSshError::NonUtf8Output => broker_error(
            "ssh-output-invalid",
            "signed SSH operation returned non-UTF-8 output",
            false,
        ),
        BundleSshError::DependencyUnavailable | BundleSshError::Persistence => broker_error(
            "dependency-unavailable",
            "Core SSH broker dependency is unavailable",
            true,
        ),
        BundleSshError::Invalid(_)
        | BundleSshError::SecretInUse
        | BundleSshError::Bootstrap { .. } => broker_error(
            "ssh-request-rejected",
            "SSH target or signed operation binding was rejected",
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::bundle_grants::{BundlePermissionGrant, GrantedBundlePermission};
    use gadgetron_bundle_sdk::{BrokerProbeRequest, BrokerResource, InvocationLeaseToken};
    use gadgetron_testing::harness::pg::PgHarness;
    use gadgetron_xaas::{knowledge_spaces, teams};
    use semver::Version;

    const PACKAGE: &str = r#"
manifest_version = 1

[bundle]
id = "server-administrator"
version = "0.1.0"
publisher = "gadgetron.project"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/server-administrator"
entry_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 30

[[permissions]]
id = "telemetry-read"
kind = "database"
description = "Read current host telemetry"
resources = ["postgres:table:host_stats_latest"]

[[permissions]]
id = "news-collections"
kind = "knowledge_collection"
description = "Manage signed knowledge collections"
resources = ["knowledge:collection"]

[[permissions]]
id = "ssh-inventory"
kind = "network"
description = "Run signed inventory"
resources = ["ssh:operation:inventory"]

[[permissions.secret_references]]
id = "ssh-identity"
purpose = "Authenticate to an approved target"
required = true

[[permissions]]
id = "ssh-key-use"
kind = "secret_use"
description = "Use an opaque SSH identity"
resources = ["secret:use:ssh-identity"]

[[broker_operations]]
id = "inventory"
kind = "ssh_execute"
network_permission_id = "ssh-inventory"
network_resource = "ssh:operation:inventory"
secret_permission_id = "ssh-key-use"
secret_resource = "secret:use:ssh-identity"
command = "uname -a"
timeout_seconds = 10
max_stdout_bytes = 65536
max_stderr_bytes = 8192
"#;

    fn contract() -> ValidatedPackageContract {
        ValidatedPackageContract::parse(PACKAGE, &Version::new(1, 0, 0)).unwrap()
    }

    #[tokio::test]
    async fn first_observation_and_degraded_retry_commit_without_enrichment_context() {
        let admin_url = std::env::var("DATABASE_URL")
            .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
            .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
        let Ok(admin) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
        else {
            eprintln!("skipping first observation event context test: PostgreSQL unavailable");
            return;
        };
        admin.close().await;
        let harness = PgHarness::new().await;
        let pool = harness.pool();
        let tenant_id = Uuid::new_v4();
        let actor_id = Uuid::new_v4();
        let table = format!("first_observation_{}", Uuid::new_v4().simple());
        sqlx::query(&format!(
            "CREATE TABLE \"{table}\" (tenant_id UUID NOT NULL, id UUID NOT NULL, status TEXT NOT NULL, PRIMARY KEY (tenant_id, id))"
        ))
        .execute(pool)
        .await
        .unwrap();
        let degraded_id = Uuid::new_v4();
        sqlx::query(&format!(
            "INSERT INTO \"{table}\" (tenant_id, id, status) VALUES ($1, $2, 'degraded')"
        ))
        .bind(tenant_id)
        .bind(degraded_id)
        .execute(pool)
        .await
        .unwrap();
        let package = ValidatedPackageContract::parse(
            &format!(
                r#"
manifest_version = 1

[bundle]
id = "server-administrator"
version = "0.1.0"
publisher = "gadgetron.project"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/server-administrator"
entry_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 30

[[permissions]]
id = "observation-write"
kind = "database"
description = "Write a signed first observation"
resources = ["postgres:table:{table}"]
"#,
            ),
            &Version::new(1, 0, 0),
        )
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let runtime = BundleBrokerRuntime::open(temp.path(), Some(pool.clone())).unwrap();
        runtime
            .grants()
            .put(
                BundlePermissionGrant::new(
                    &package.runtime_identity().id,
                    package.manifest_sha256(),
                    package
                        .manifest()
                        .permissions
                        .iter()
                        .map(GrantedBundlePermission::from),
                )
                .unwrap(),
            )
            .unwrap();
        let event_kind = LocalId::new("server-log-finding-created").unwrap();
        let subject_kind = LocalId::new("log-finding").unwrap();
        let descriptor: EventJobDescriptor = toml::from_str(
            r#"
id = "server-log-finding-enrichment"
event_kind = "server-log-finding-created"
subject_owner_bundle = "server-administrator"
subject_kind = "log-finding"
agent_role = "server-log-finding-enricher"
input_schema = { type = "object" }
result_gadget = "serverintelligence.finding-enrich-attach"
"#,
        )
        .unwrap();
        runtime.publish_event_jobs(BTreeMap::from([(
            BundleEventRoute {
                subject_owner_bundle: "server-administrator".into(),
                subject_kind: subject_kind.to_string(),
                event_kind: event_kind.to_string(),
            },
            BundleEventJobContract {
                descriptor,
                owner_bundle_id: "server-operations-intelligence".into(),
                package_manifest_sha256: "b".repeat(64),
                core_role: gadgetron_bundle_sdk::AgentRole::Researcher,
                recipe_id: "server-log-finding-enrichment".into(),
                prompt_contract_revision: "first-observation-test-v1".into(),
                goal: "Attach optional context".into(),
                max_wall_seconds: 120,
                max_attempts: 2,
            },
        )]));
        let broker = runtime.broker_for(&package);
        let caller = BrokerCaller::from_package(&package);
        let context = InvocationContext::new(
            tenant_id.to_string(),
            actor_id.to_string(),
            "first-observation-without-space",
        );
        let lease = runtime
            .issue_lease(
                package.runtime_identity().id.to_string(),
                package.manifest_sha256().to_string(),
                &context,
            )
            .unwrap();
        let observation_id = Uuid::new_v4();
        let request = DatabaseInsertRequest::new(
            lease.token().clone(),
            LocalId::new("observation-write").unwrap(),
            BrokerResource::database_table(&table).unwrap(),
            BTreeMap::from([
                ("id".into(), serde_json::json!(observation_id)),
                ("status".into(), serde_json::json!("healthy")),
            ]),
        )
        .with_event(DatabaseMutationEvent::new(
            event_kind.clone(),
            subject_kind.clone(),
            observation_id.to_string(),
            "c".repeat(64),
            serde_json::json!({}),
        ));
        let response = broker
            .handle(&caller, BrokerRequest::DatabaseInsert(request))
            .await;
        assert!(matches!(
            response,
            BrokerResponse::DatabaseMutation(ref result) if result.affected_rows == 1
        ));
        let stored: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS(SELECT 1 FROM \"{table}\" WHERE tenant_id = $1 AND id = $2)"
        ))
        .bind(tenant_id)
        .bind(observation_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert!(stored);
        let retry = DatabaseInsertRequest::new(
            lease.token().clone(),
            LocalId::new("observation-write").unwrap(),
            BrokerResource::database_table(&table).unwrap(),
            BTreeMap::from([
                ("id".into(), serde_json::json!(degraded_id)),
                ("status".into(), serde_json::json!("healthy")),
            ]),
        )
        .with_conflict_keys(["id".into()])
        .with_event(DatabaseMutationEvent::new(
            event_kind,
            subject_kind,
            degraded_id.to_string(),
            "d".repeat(64),
            serde_json::json!({}),
        ));
        let retry_response = broker
            .handle(&caller, BrokerRequest::DatabaseInsert(retry))
            .await;
        assert!(matches!(
            retry_response,
            BrokerResponse::DatabaseMutation(ref result) if result.affected_rows == 1
        ));
        let recovered: String = sqlx::query_scalar(&format!(
            "SELECT status FROM \"{table}\" WHERE tenant_id = $1 AND id = $2"
        ))
        .bind(tenant_id)
        .bind(degraded_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(recovered, "healthy");
        let dispatched: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM autonomy_goals WHERE tenant_id = $1 AND source_kind = 'bundle_event'",
        )
        .bind(tenant_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(dispatched, 0);
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn database_insert_commits_before_optional_bundle_event_dispatch() {
        let admin_url = std::env::var("DATABASE_URL")
            .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
            .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
        let Ok(admin) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
        else {
            eprintln!("skipping Bundle event transaction test: PostgreSQL unavailable");
            return;
        };
        admin.close().await;
        let harness = PgHarness::new().await;
        let pool = harness.pool();
        let tenant_id = Uuid::new_v4();
        let actor_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'event-broker-fixture')")
            .bind(tenant_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) VALUES ($1,$2,'event-broker@example.test','Event broker','admin','test')",
        )
        .bind(actor_id)
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
        teams::create_team(
            pool,
            tenant_id,
            "event-broker",
            "Event broker",
            None,
            Some(actor_id),
        )
        .await
        .unwrap();
        let space_id = knowledge_spaces::ensure_team_space(
            pool,
            SpaceActor {
                tenant_id,
                user_id: actor_id,
            },
            "event-broker",
            "Event broker",
        )
        .await
        .unwrap()
        .id;
        let table = format!("bundle_event_insert_{}", Uuid::new_v4().simple());
        sqlx::query(&format!(
            "CREATE TABLE \"{table}\" (tenant_id UUID NOT NULL, id UUID NOT NULL, label TEXT NOT NULL, PRIMARY KEY (tenant_id, id))"
        ))
        .execute(pool)
        .await
        .unwrap();
        let request = |id: Uuid| {
            DatabaseInsertRequest::new(
                InvocationLeaseToken::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap(),
                LocalId::new("event-write").unwrap(),
                BrokerResource::database_table(&table).unwrap(),
                BTreeMap::from([
                    ("id".into(), serde_json::json!(id)),
                    ("label".into(), serde_json::json!("rule finding")),
                ]),
            )
        };
        let event = EnqueueBundleEvent {
            tenant_id,
            event_kind: "server-log-finding-created".into(),
            subject_bundle_id: "server-administrator".into(),
            subject_kind: "log-finding".into(),
            subject_id: Uuid::new_v4().to_string(),
            subject_revision: "1".repeat(64),
            event_payload: serde_json::json!({"subject":{"summary":"rule finding"}}),
            owner_bundle_id: "server-operations-intelligence".into(),
            recipe_id: "server-log-finding-enrichment".into(),
            package_manifest_sha256: "a".repeat(64),
            agent_role_id: "server-log-finding-enricher".into(),
            result_gadget: "serverintelligence.finding-enrich-attach".into(),
            goal: "Attach bounded finding context".into(),
            acting_space_id: space_id,
            requested_by_user_id: actor_id,
            service_actor_user_id: actor_id,
            effective_role: "manager".into(),
            max_wall_seconds: 120,
            max_attempts: 2,
            agent_profile_snapshot: serde_json::json!({"model":"fast","revision":"profile:1"}),
        };
        execute_database_insert(
            pool,
            None,
            None,
            &tenant_id.to_string(),
            &request(Uuid::new_v4()),
            Some(event.clone()),
        )
        .await
        .unwrap();
        execute_database_insert(
            pool,
            None,
            None,
            &tenant_id.to_string(),
            &request(Uuid::new_v4()),
            Some(event.clone()),
        )
        .await
        .unwrap();
        let counts: (i64, i64) = sqlx::query_as(&format!(
            "SELECT (SELECT COUNT(*) FROM \"{table}\" WHERE tenant_id = $1), (SELECT COUNT(*) FROM autonomy_goals WHERE tenant_id = $1 AND source_kind = 'bundle_event')"
        ))
        .bind(tenant_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(counts, (2, 1));

        let lease = gadgetron_xaas::autonomy::lease_next(pool, "event-provider-1", 30)
            .await
            .unwrap()
            .unwrap();
        let retry = gadgetron_xaas::autonomy::finish_event_run(
            pool,
            &lease,
            "event-provider-1",
            gadgetron_xaas::autonomy::EventRunTerminal::ProviderFailure,
            "provider unavailable",
            None,
        )
        .await
        .unwrap();
        assert_eq!(retry.status, "retry_wait");
        sqlx::query("UPDATE autonomy_goals SET next_run_at = NOW() WHERE id = $1")
            .bind(retry.id)
            .execute(pool)
            .await
            .unwrap();
        let lease = gadgetron_xaas::autonomy::lease_next(pool, "event-provider-2", 30)
            .await
            .unwrap()
            .unwrap();
        let failed = gadgetron_xaas::autonomy::finish_event_run(
            pool,
            &lease,
            "event-provider-2",
            gadgetron_xaas::autonomy::EventRunTerminal::ProviderFailure,
            "provider retry budget exhausted",
            None,
        )
        .await
        .unwrap();
        assert_eq!(failed.status, "failed_provider");
        let preserved_rows: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM \"{table}\" WHERE tenant_id = $1"
        ))
        .bind(tenant_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(preserved_rows, 2);

        let preserved_id = Uuid::new_v4();
        let mut invalid_event = event;
        invalid_event.subject_id = Uuid::new_v4().to_string();
        invalid_event.max_wall_seconds = 1;
        assert!(execute_database_insert(
            pool,
            None,
            None,
            &tenant_id.to_string(),
            &request(preserved_id),
            Some(invalid_event),
        )
        .await
        .is_ok());
        let preserved: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS(SELECT 1 FROM \"{table}\" WHERE tenant_id = $1 AND id = $2)"
        ))
        .bind(tenant_id)
        .bind(preserved_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert!(preserved);
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn knowledge_event_revalidates_the_signed_snapshot_space() {
        let admin_url = std::env::var("DATABASE_URL")
            .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
            .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
        let Ok(admin) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
        else {
            eprintln!("skipping Knowledge snapshot Space test: PostgreSQL unavailable");
            return;
        };
        admin.close().await;
        let harness = PgHarness::new().await;
        let pool = harness.pool();
        let tenant_id = Uuid::new_v4();
        let actor_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'knowledge-space-fixture')")
            .bind(tenant_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) VALUES ($1,$2,'knowledge-space@example.test','Knowledge space','admin','test')",
        )
        .bind(actor_id)
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
        teams::create_team(
            pool,
            tenant_id,
            "knowledge-space",
            "Knowledge space",
            None,
            Some(actor_id),
        )
        .await
        .unwrap();
        let space_id = knowledge_spaces::ensure_team_space(
            pool,
            SpaceActor {
                tenant_id,
                user_id: actor_id,
            },
            "knowledge-space",
            "Knowledge space",
        )
        .await
        .unwrap()
        .id;
        let table = format!("knowledge_snapshot_{}", Uuid::new_v4().simple());
        sqlx::query(&format!(
            "CREATE TABLE \"{table}\" (tenant_id UUID NOT NULL, incident_id TEXT NOT NULL, revision TEXT NOT NULL, title TEXT NOT NULL, acting_space_id UUID, PRIMARY KEY (tenant_id, incident_id))"
        ))
        .execute(pool)
        .await
        .unwrap();
        let incident_id = Uuid::new_v4().to_string();
        let revision = Uuid::new_v4().to_string();
        sqlx::query(&format!(
            "INSERT INTO \"{table}\" (tenant_id, incident_id, revision, title, acting_space_id) VALUES ($1, $2, $3, 'Closed incident', $4)"
        ))
        .bind(tenant_id)
        .bind(&incident_id)
        .bind(&revision)
        .bind(space_id)
        .execute(pool)
        .await
        .unwrap();
        let descriptor: KnowledgeEventDescriptor = toml::from_str(&format!(
            r#"
id = "incident-closed"
event_kind = "server-incident-closed"
subject_kind = "server-incident"
snapshot_permission_id = "operations-read"
snapshot_resource = "postgres:table:{table}"
snapshot_fields = ["incident_id", "revision", "title", "acting_space_id"]
acting_space_id_field = "acting_space_id"
subject_id_field = "incident_id"
subject_revision_field = "revision"
title_field = "title"
researcher_bundle = "server-operations-intelligence"
researcher_role = "incident-researcher"
output_vault_bundle = "server-administrator"
knowledge_schema_id = "server-incident"
source_path_prefix = "incident"
"#
        ))
        .unwrap();
        let event = PreparedKnowledgeEvent {
            descriptor,
            tenant_id,
            publisher_bundle_id: "server-administrator".into(),
            snapshot_filters: BTreeMap::from([(
                "incident_id".into(),
                serde_json::json!(incident_id),
            )]),
            // A stored snapshot Space is intentionally sufficient; there is
            // no inherited action lease Space to fall back to here.
            acting_space_id: None,
            requested_by_user_id: actor_id,
            service_actor_user_id: actor_id,
            effective_role: None,
        };
        let mut transaction = begin_mutation_transaction(pool).await.unwrap();
        capture_knowledge_event(pool, &mut transaction, event)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        let stored_space: Uuid = sqlx::query_scalar(
            "SELECT acting_space_id FROM knowledge_event_outbox WHERE tenant_id = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(tenant_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(stored_space, space_id);
    }

    #[tokio::test]
    async fn signed_request_is_denied_until_exact_digest_grant_exists() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = BundleBrokerRuntime::open(temp.path(), None).unwrap();
        let contract = contract();
        let caller = BrokerCaller::from_package(&contract);
        let broker = runtime.broker_for(&contract);
        let resource = BrokerResource::database_table("host_stats_latest").unwrap();
        let request = BrokerRequest::Probe(BrokerProbeRequest::new(
            LocalId::new("telemetry-read").unwrap(),
            resource.clone(),
        ));

        let denied = broker.handle(&caller, request.clone()).await;
        assert!(matches!(
            denied,
            BrokerResponse::Error(ref error) if error.code.as_str() == "permission-not-granted"
        ));

        let permission = &contract.manifest().permissions[0];
        let grant = BundlePermissionGrant::new(
            &contract.runtime_identity().id,
            contract.manifest_sha256(),
            [GrantedBundlePermission::from(permission)],
        )
        .unwrap();
        runtime.grants().put(grant).unwrap();
        let unavailable = broker.handle(&caller, request).await;
        assert!(matches!(unavailable, BrokerResponse::Probe(_)));

        let other = BrokerRequest::Probe(BrokerProbeRequest::new(
            LocalId::new("telemetry-read").unwrap(),
            BrokerResource::database_table("log_findings").unwrap(),
        ));
        assert!(matches!(
            broker.handle(&caller, other).await,
            BrokerResponse::Error(ref error) if error.code.as_str() == "resource-not-requested"
        ));
    }

    #[tokio::test]
    async fn knowledge_collection_probe_requires_its_exact_signed_grant() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = BundleBrokerRuntime::open(temp.path(), None).unwrap();
        let contract = contract();
        let caller = BrokerCaller::from_package(&contract);
        let broker = runtime.broker_for(&contract);
        let permission = contract
            .manifest()
            .permissions
            .iter()
            .find(|permission| permission.kind == PermissionKind::KnowledgeCollection)
            .unwrap();
        let request = BrokerRequest::Probe(BrokerProbeRequest::new(
            permission.id.clone(),
            BrokerResource::knowledge_collection().unwrap(),
        ));
        assert!(matches!(
            broker.handle(&caller, request.clone()).await,
            BrokerResponse::Error(ref error) if error.code.as_str() == "permission-not-granted"
        ));
        runtime
            .grants()
            .put(
                BundlePermissionGrant::new(
                    &contract.runtime_identity().id,
                    contract.manifest_sha256(),
                    [GrantedBundlePermission::from(permission)],
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            broker.handle(&caller, request).await,
            BrokerResponse::Probe(ref result)
                if result.readiness == gadgetron_bundle_sdk::BrokerResourceReadiness::Unavailable
        ));
    }

    #[test]
    fn lease_is_package_bound_and_revoked_when_guard_drops() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = BundleBrokerRuntime::open(temp.path(), None).unwrap();
        let contract = contract();
        let caller = BrokerCaller::from_package(&contract);
        let context = InvocationContext::new(Uuid::new_v4().to_string(), "manager-1", "request-1")
            .with_scopes(["Management".into()]);
        let guard = runtime
            .issue_lease(
                contract.runtime_identity().id.to_string(),
                contract.manifest_sha256().to_string(),
                &context,
            )
            .unwrap();
        assert!(runtime.leases.validate(&caller, guard.token()).is_ok());
        let token = guard.token().clone();
        drop(guard);
        assert!(runtime.leases.validate(&caller, &token).is_err());
    }

    #[test]
    fn lease_binds_acting_space_and_delegated_authority() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = BundleBrokerRuntime::open(temp.path(), None).unwrap();
        let contract = contract();
        let caller = BrokerCaller::from_package(&contract);
        let tenant_id = Uuid::new_v4();
        let actor_id = Uuid::new_v4();
        let acting_space_id = Uuid::new_v4();
        let delegated_actor_id = Uuid::new_v4();
        let context = InvocationContext::new(
            tenant_id.to_string(),
            actor_id.to_string(),
            "request-acting-space",
        )
        .with_acting_space_id(acting_space_id.to_string());
        let guard = runtime
            .issue_delegated_lease(
                contract.runtime_identity().id.to_string(),
                contract.manifest_sha256().to_string(),
                &context,
                delegated_actor_id,
            )
            .unwrap();
        let lease = runtime.leases.validate(&caller, guard.token()).unwrap();
        assert_eq!(
            intelligence_binding(&lease).unwrap(),
            IntelligenceActorBinding {
                tenant_id,
                actor_id,
                authority_actor_id: Some(delegated_actor_id),
                acting_space_id: Some(acting_space_id),
            }
        );
    }

    #[test]
    fn expired_lease_is_removed_before_authorization() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = BundleBrokerRuntime::open_with_lease_ttl(
            temp.path(),
            None,
            None,
            Duration::from_millis(1),
        )
        .unwrap();
        let contract = contract();
        let caller = BrokerCaller::from_package(&contract);
        let context = InvocationContext::new(Uuid::new_v4().to_string(), "manager-1", "request-1");
        let guard = runtime
            .issue_lease(
                contract.runtime_identity().id.to_string(),
                contract.manifest_sha256().to_string(),
                &context,
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let error = match runtime.leases.validate(&caller, guard.token()) {
            Err(error) => error,
            Ok(_) => panic!("expired invocation lease was accepted"),
        };
        assert_eq!(error.code.as_str(), "lease-invalid");
    }

    #[tokio::test]
    async fn ssh_requires_both_grants_a_live_lease_and_a_tenant_bound_target() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = BundleBrokerRuntime::open(temp.path(), None).unwrap();
        let contract = contract();
        let caller = BrokerCaller::from_package(&contract);
        let broker = runtime.broker_for(&contract);
        let network = contract
            .manifest()
            .permissions
            .iter()
            .find(|permission| permission.id.as_str() == "ssh-inventory")
            .unwrap();
        runtime
            .grants()
            .put(
                BundlePermissionGrant::new(
                    &contract.runtime_identity().id,
                    contract.manifest_sha256(),
                    [GrantedBundlePermission::from(network)],
                )
                .unwrap(),
            )
            .unwrap();
        let context =
            InvocationContext::new(Uuid::new_v4().to_string(), "manager-1", "request-ssh")
                .with_scopes(["management".into()]);
        let lease = runtime
            .issue_lease(
                contract.runtime_identity().id.to_string(),
                contract.manifest_sha256().to_string(),
                &context,
            )
            .unwrap();
        let request = BrokerRequest::SshExecute(gadgetron_bundle_sdk::SshExecuteRequest::new(
            lease.token().clone(),
            LocalId::new("missing-target").unwrap(),
            LocalId::new("inventory").unwrap(),
        ));
        assert!(matches!(
            broker.handle(&caller, request.clone()).await,
            BrokerResponse::Error(ref error) if error.code.as_str() == "permission-not-granted"
        ));

        runtime
            .grants()
            .put(
                BundlePermissionGrant::new(
                    &contract.runtime_identity().id,
                    contract.manifest_sha256(),
                    contract
                        .manifest()
                        .permissions
                        .iter()
                        .map(GrantedBundlePermission::from),
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            broker.handle(&caller, request.clone()).await,
            BrokerResponse::Error(ref error) if error.code.as_str() == "target-not-found"
        ));
        drop(lease);
        assert!(matches!(
            broker.handle(&caller, request).await,
            BrokerResponse::Error(ref error) if error.code.as_str() == "lease-invalid"
        ));
    }

    #[test]
    fn identifiers_are_quoted_only_after_sdk_grammar_validation() {
        assert_eq!(
            quoted_identifier("host_stats_latest"),
            "\"host_stats_latest\""
        );
    }
}
