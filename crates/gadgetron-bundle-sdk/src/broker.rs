use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::{
    BundleRuntimeIdentity, BundleSdkError, IntelligenceQueryDraft, KnowledgeContextPack, LocalId,
    OutcomeFeedbackDraft, OutcomeFeedbackReceipt, Result, BUNDLE_BROKER_PROTOCOL_VERSION,
};

const LEASE_TOKEN_LEN: usize = 43;
const MAX_RESOURCE_ID: usize = 256;
const MAX_MESSAGE_ID: usize = 128;
const MAX_TEXT: usize = 2_048;
const MAX_FIELD_NAME: usize = 63;
const MAX_COLUMNS: usize = 64;
const MAX_FILTERS: usize = 32;
const MAX_ORDER_FIELDS: usize = 8;
const MAX_DATABASE_ROWS: usize = 500;
const MAX_DATABASE_MUTATION_ROWS: u32 = 100;
const MAX_KNOWLEDGE_COLLECTIONS: u32 = 200;
const MAX_COLLECTION_LOCATORS: usize = 64;
const MAX_COLLECTION_QUERY_TAGS: usize = 8;
const MAX_SSH_OUTPUT_BYTES: u32 = 262_144;
const MAX_SSH_STDERR_BYTES: u32 = 65_536;
const MAX_JSON_BYTES: usize = 1_048_576;

/// A 256-bit base64url, no-padding bearer token issued and revoked by Core.
///
/// Debug output is deliberately redacted. The token is still serialized on the
/// private broker channel because presenting it is how an invocation proves
/// its Core-authenticated lease.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InvocationLeaseToken(String);

impl InvocationLeaseToken {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_lease_token(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for InvocationLeaseToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InvocationLeaseToken([REDACTED])")
    }
}

impl Serialize for InvocationLeaseToken {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for InvocationLeaseToken {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Signed, typed resource identifier shared by manifest requests, operator
/// grants and broker calls. It is data, not a Core-owned domain alias.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BrokerResource(String);

impl BrokerResource {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_broker_resource(&value)?;
        Ok(Self(value))
    }

    pub fn database_table(table: impl AsRef<str>) -> Result<Self> {
        let table = table.as_ref();
        if !is_database_identifier(table) {
            return Err(BundleSdkError::protocol(
                "broker.resource",
                "database table must be a 1-63 character lowercase identifier",
            ));
        }
        Self::new(format!("postgres:table:{table}"))
    }

    pub fn database_table_name(&self) -> Option<&str> {
        let table = self.0.strip_prefix("postgres:table:")?;
        is_database_identifier(table).then_some(table)
    }

    pub fn ssh_operation(operation: impl AsRef<str>) -> Result<Self> {
        let operation = operation.as_ref();
        if !is_local_resource_segment(operation) {
            return Err(BundleSdkError::protocol(
                "broker.resource",
                "SSH operation must be a 1-64 character lowercase kebab-case identifier",
            ));
        }
        Self::new(format!("ssh:operation:{operation}"))
    }

    pub fn ssh_operation_name(&self) -> Option<&str> {
        let operation = self.0.strip_prefix("ssh:operation:")?;
        is_local_resource_segment(operation).then_some(operation)
    }

    pub fn secret_use(purpose: impl AsRef<str>) -> Result<Self> {
        let purpose = purpose.as_ref();
        if !is_local_resource_segment(purpose) {
            return Err(BundleSdkError::protocol(
                "broker.resource",
                "secret purpose must be a 1-64 character lowercase kebab-case identifier",
            ));
        }
        Self::new(format!("secret:use:{purpose}"))
    }

    pub fn secret_use_name(&self) -> Option<&str> {
        let purpose = self.0.strip_prefix("secret:use:")?;
        is_local_resource_segment(purpose).then_some(purpose)
    }

    pub fn knowledge_context() -> Result<Self> {
        Self::new("knowledge:context")
    }

    pub fn is_knowledge_context(&self) -> bool {
        self.0 == "knowledge:context"
    }

    pub fn knowledge_feedback() -> Result<Self> {
        Self::new("knowledge:feedback")
    }

    pub fn is_knowledge_feedback(&self) -> bool {
        self.0 == "knowledge:feedback"
    }

    pub fn knowledge_collection() -> Result<Self> {
        Self::new("knowledge:collection")
    }

    pub fn is_knowledge_collection(&self) -> bool {
        self.0 == "knowledge:collection"
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for BrokerResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for BrokerResource {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BrokerResource {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Versioned identity-bound envelope carried only on the Bundle broker FD.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BrokerEnvelope<T> {
    pub protocol_version: u32,
    pub message_id: String,
    pub bundle: BundleRuntimeIdentity,
    pub payload: T,
}

impl<T> BrokerEnvelope<T> {
    pub fn new(message_id: impl Into<String>, bundle: BundleRuntimeIdentity, payload: T) -> Self {
        Self {
            protocol_version: BUNDLE_BROKER_PROTOCOL_VERSION,
            message_id: message_id.into(),
            bundle,
            payload,
        }
    }

    pub fn validate_routing(&self, expected_bundle: &BundleRuntimeIdentity) -> Result<()> {
        if self.protocol_version != BUNDLE_BROKER_PROTOCOL_VERSION {
            return Err(BundleSdkError::protocol(
                "broker.protocol_version",
                format!(
                    "expected {BUNDLE_BROKER_PROTOCOL_VERSION}, received {}",
                    self.protocol_version
                ),
            ));
        }
        validate_message_id(&self.message_id)?;
        if &self.bundle != expected_bundle {
            return Err(BundleSdkError::protocol(
                "broker.bundle",
                format!(
                    "expected {}@{}, received {}@{}",
                    expected_bundle.id,
                    expected_bundle.version,
                    self.bundle.id,
                    self.bundle.version
                ),
            ));
        }
        Ok(())
    }
}

/// Domain-neutral operations an external Bundle may request from Core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "params", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BrokerRequest {
    /// Check channel, grant and dependency readiness without accessing tenant data.
    Probe(BrokerProbeRequest),
    /// Execute a bounded, Core-compiled, tenant-forced read. Raw SQL is absent.
    DatabaseSelect(DatabaseSelectRequest),
    /// Insert or upsert one row through a Core-compiled tenant-forced mutation.
    DatabaseInsert(DatabaseInsertRequest),
    /// Update a bounded equality-selected row set. Raw predicates are absent.
    DatabaseUpdate(DatabaseUpdateRequest),
    /// Delete a bounded equality-selected row set. Raw predicates are absent.
    DatabaseDelete(DatabaseDeleteRequest),
    /// Run one signed, operator-granted SSH operation against a Core-owned target.
    SshExecute(SshExecuteRequest),
    /// Resolve a Core-authorized, revision-pinned cited context pack.
    IntelligenceContext(IntelligenceContextRequest),
    /// Return a verified domain outcome to the Core experience lifecycle.
    OutcomeFeedback(OutcomeFeedbackRequest),
    /// Manage only this signed package's Core-owned Knowledge collections.
    KnowledgeCollection(KnowledgeCollectionRequest),
}

impl BrokerRequest {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Probe(request) => request.validate(),
            Self::DatabaseSelect(request) => request.validate(),
            Self::DatabaseInsert(request) => request.validate(),
            Self::DatabaseUpdate(request) => request.validate(),
            Self::DatabaseDelete(request) => request.validate(),
            Self::SshExecute(request) => request.validate(),
            Self::IntelligenceContext(request) => request.validate(),
            Self::OutcomeFeedback(request) => request.validate(),
            Self::KnowledgeCollection(request) => request.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct KnowledgeCollectionRequest {
    pub lease: InvocationLeaseToken,
    pub permission_id: LocalId,
    pub action: KnowledgeCollectionAction,
}

impl KnowledgeCollectionRequest {
    pub fn new(
        lease: InvocationLeaseToken,
        permission_id: LocalId,
        action: KnowledgeCollectionAction,
    ) -> Self {
        Self {
            lease,
            permission_id,
            action,
        }
    }

    fn validate(&self) -> Result<()> {
        self.action.validate()?;
        validate_serialized_size("broker.knowledge_collection", self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
#[non_exhaustive]
pub enum KnowledgeCollectionAction {
    List {
        #[serde(default = "default_knowledge_collection_limit")]
        limit: u32,
    },
    Create {
        space_id: String,
        output_vault_id: String,
        profile_id: LocalId,
        topic: String,
        #[serde(default)]
        schedule_enabled: bool,
        #[serde(default)]
        locators: Vec<KnowledgeCollectionLocator>,
        #[serde(default)]
        queries: Vec<KnowledgeCollectionQuery>,
    },
    Update {
        collection_id: String,
        expected_revision: i64,
        topic: String,
        status: KnowledgeCollectionStatus,
        #[serde(default)]
        schedule_enabled: bool,
        #[serde(default)]
        locators: Vec<KnowledgeCollectionLocator>,
        #[serde(default)]
        queries: Vec<KnowledgeCollectionQuery>,
    },
    Archive {
        collection_id: String,
        expected_revision: i64,
    },
    Enqueue {
        collection_id: String,
        expected_revision: i64,
    },
}

impl KnowledgeCollectionAction {
    fn validate(&self) -> Result<()> {
        match self {
            Self::List { limit } => {
                if !(1..=MAX_KNOWLEDGE_COLLECTIONS).contains(limit) {
                    return Err(BundleSdkError::protocol(
                        "broker.knowledge_collection.list.limit",
                        format!("must be between 1 and {MAX_KNOWLEDGE_COLLECTIONS}"),
                    ));
                }
            }
            Self::Create {
                space_id,
                output_vault_id,
                topic,
                locators,
                queries,
                ..
            } => {
                validate_uuid("broker.knowledge_collection.create.space_id", space_id)?;
                validate_uuid(
                    "broker.knowledge_collection.create.output_vault_id",
                    output_vault_id,
                )?;
                bounded_nonempty("broker.knowledge_collection.create.topic", topic, MAX_TEXT)?;
                validate_collection_inputs("create", locators, queries)?;
            }
            Self::Update {
                collection_id,
                expected_revision,
                topic,
                locators,
                queries,
                ..
            } => {
                validate_uuid(
                    "broker.knowledge_collection.update.collection_id",
                    collection_id,
                )?;
                validate_positive_revision("update", *expected_revision)?;
                bounded_nonempty("broker.knowledge_collection.update.topic", topic, MAX_TEXT)?;
                validate_collection_inputs("update", locators, queries)?;
            }
            Self::Archive {
                collection_id,
                expected_revision,
            }
            | Self::Enqueue {
                collection_id,
                expected_revision,
            } => {
                let action = if matches!(self, Self::Archive { .. }) {
                    "archive"
                } else {
                    "enqueue"
                };
                validate_uuid(
                    &format!("broker.knowledge_collection.{action}.collection_id"),
                    collection_id,
                )?;
                validate_positive_revision(action, *expected_revision)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum KnowledgeCollectionStatus {
    Active,
    Paused,
}

impl KnowledgeCollectionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct KnowledgeCollectionLocator {
    pub url: String,
    #[serde(default)]
    pub title: String,
    pub source_class: LocalId,
}

impl KnowledgeCollectionLocator {
    pub fn new(url: impl Into<String>, title: impl Into<String>, source_class: LocalId) -> Self {
        Self {
            url: url.into(),
            title: title.into(),
            source_class,
        }
    }
}

/// User-owned, endpoint-free query executed only by a signed Core connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct KnowledgeCollectionQuery {
    pub provider: LocalId,
    pub query: String,
    pub scope: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub window_days: u32,
}

impl KnowledgeCollectionQuery {
    pub fn new(
        provider: LocalId,
        query: impl Into<String>,
        scope: impl Into<String>,
        window_days: u32,
    ) -> Self {
        Self {
            provider,
            query: query.into(),
            scope: scope.into(),
            tags: Vec::new(),
            language: None,
            window_days,
        }
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn with_language(mut self, language: Option<String>) -> Self {
        self.language = language;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IntelligenceContextRequest {
    pub lease: InvocationLeaseToken,
    pub permission_id: LocalId,
    pub draft: IntelligenceQueryDraft,
}

impl IntelligenceContextRequest {
    pub fn new(
        lease: InvocationLeaseToken,
        permission_id: LocalId,
        draft: IntelligenceQueryDraft,
    ) -> Self {
        Self {
            lease,
            permission_id,
            draft,
        }
    }

    fn validate(&self) -> Result<()> {
        self.draft.validate()?;
        validate_serialized_size("broker.intelligence_context", self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OutcomeFeedbackRequest {
    pub lease: InvocationLeaseToken,
    pub permission_id: LocalId,
    pub draft: OutcomeFeedbackDraft,
}

impl OutcomeFeedbackRequest {
    pub fn new(
        lease: InvocationLeaseToken,
        permission_id: LocalId,
        draft: OutcomeFeedbackDraft,
    ) -> Self {
        Self {
            lease,
            permission_id,
            draft,
        }
    }

    fn validate(&self) -> Result<()> {
        validate_serialized_size("broker.outcome_feedback", self)
    }
}

/// A Bundle selects only opaque Core-owned target and signed operation ids.
/// Hostname, username, command, private-key path and secret value are absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SshExecuteRequest {
    pub lease: InvocationLeaseToken,
    pub target_id: LocalId,
    pub operation_id: LocalId,
}

impl SshExecuteRequest {
    pub fn new(lease: InvocationLeaseToken, target_id: LocalId, operation_id: LocalId) -> Self {
        Self {
            lease,
            target_id,
            operation_id,
        }
    }

    fn validate(&self) -> Result<()> {
        validate_serialized_size("broker.ssh_execute", self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BrokerProbeRequest {
    pub permission_id: LocalId,
    pub resource: BrokerResource,
}

impl BrokerProbeRequest {
    pub fn new(permission_id: LocalId, resource: BrokerResource) -> Self {
        Self {
            permission_id,
            resource,
        }
    }

    fn validate(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DatabaseSelectRequest {
    pub lease: InvocationLeaseToken,
    pub permission_id: LocalId,
    pub resource: BrokerResource,
    pub columns: Vec<String>,
    #[serde(default)]
    pub filters: BTreeMap<String, Value>,
    #[serde(default)]
    pub order_by: Vec<DatabaseOrder>,
    #[serde(default = "default_database_limit")]
    pub limit: u32,
}

impl DatabaseSelectRequest {
    pub fn new(
        lease: InvocationLeaseToken,
        permission_id: LocalId,
        resource: BrokerResource,
        columns: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            lease,
            permission_id,
            resource,
            columns: columns.into_iter().collect(),
            filters: BTreeMap::new(),
            order_by: Vec::new(),
            limit: default_database_limit(),
        }
    }

    pub fn with_filter(mut self, field: impl Into<String>, value: Value) -> Self {
        self.filters.insert(field.into(), value);
        self
    }

    pub fn with_order(
        mut self,
        field: impl Into<String>,
        direction: DatabaseOrderDirection,
    ) -> Self {
        self.order_by.push(DatabaseOrder::new(field, direction));
        self
    }

    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    fn validate(&self) -> Result<()> {
        if self.resource.database_table_name().is_none() {
            return Err(BundleSdkError::protocol(
                "broker.database_select.resource",
                "must identify an exact postgres:table:<lowercase_identifier> resource",
            ));
        }
        if self.columns.is_empty() || self.columns.len() > MAX_COLUMNS {
            return Err(BundleSdkError::protocol(
                "broker.database_select.columns",
                format!("must contain 1-{MAX_COLUMNS} fields"),
            ));
        }
        let mut seen = BTreeSet::new();
        for (index, column) in self.columns.iter().enumerate() {
            validate_database_field(&format!("broker.database_select.columns[{index}]"), column)?;
            if !seen.insert(column) {
                return Err(BundleSdkError::protocol(
                    "broker.database_select.columns",
                    format!("duplicate field {column:?}"),
                ));
            }
        }
        if self.filters.len() > MAX_FILTERS {
            return Err(BundleSdkError::protocol(
                "broker.database_select.filters",
                format!("at most {MAX_FILTERS} equality filters may be supplied"),
            ));
        }
        for (field, value) in &self.filters {
            validate_database_field("broker.database_select.filters", field)?;
            validate_filter_value("database_select.filters", field, value)?;
        }
        if self.order_by.len() > MAX_ORDER_FIELDS {
            return Err(BundleSdkError::protocol(
                "broker.database_select.order_by",
                format!("at most {MAX_ORDER_FIELDS} order fields may be supplied"),
            ));
        }
        for (index, order) in self.order_by.iter().enumerate() {
            validate_database_field(
                &format!("broker.database_select.order_by[{index}].field"),
                &order.field,
            )?;
        }
        if !(1..=MAX_DATABASE_ROWS as u32).contains(&self.limit) {
            return Err(BundleSdkError::protocol(
                "broker.database_select.limit",
                format!("must be between 1 and {MAX_DATABASE_ROWS}"),
            ));
        }
        validate_serialized_size("broker.database_select", self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DatabaseInsertRequest {
    pub lease: InvocationLeaseToken,
    pub permission_id: LocalId,
    pub resource: BrokerResource,
    pub values: BTreeMap<String, Value>,
    /// Empty means insert-only. Non-empty keys compile to
    /// `ON CONFLICT (tenant_id, ...) DO UPDATE` for the supplied value fields.
    #[serde(default)]
    pub conflict_keys: Vec<String>,
    /// Optional signed-domain event metadata. Core matches this only against
    /// an enabled `EventJobDescriptor` whose subject owner is this caller and
    /// enqueues it in the same transaction as the database insert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<DatabaseMutationEvent>,
}

impl DatabaseInsertRequest {
    pub fn new(
        lease: InvocationLeaseToken,
        permission_id: LocalId,
        resource: BrokerResource,
        values: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            lease,
            permission_id,
            resource,
            values,
            conflict_keys: Vec::new(),
            event: None,
        }
    }

    pub fn with_conflict_keys(mut self, keys: impl IntoIterator<Item = String>) -> Self {
        self.conflict_keys = keys.into_iter().collect();
        self
    }

    pub fn with_event(mut self, event: DatabaseMutationEvent) -> Self {
        self.event = Some(event);
        self
    }

    fn validate(&self) -> Result<()> {
        validate_database_mutation_resource("database_insert", &self.resource)?;
        validate_database_values("database_insert.values", &self.values)?;
        let mut seen = BTreeSet::new();
        for (index, field) in self.conflict_keys.iter().enumerate() {
            validate_database_field(
                &format!("broker.database_insert.conflict_keys[{index}]"),
                field,
            )?;
            if !seen.insert(field) || !self.values.contains_key(field) {
                return Err(BundleSdkError::protocol(
                    "broker.database_insert.conflict_keys",
                    "keys must be unique and present in values",
                ));
            }
        }
        if !self.conflict_keys.is_empty()
            && self
                .values
                .keys()
                .all(|field| self.conflict_keys.contains(field))
        {
            return Err(BundleSdkError::protocol(
                "broker.database_insert.values",
                "upsert must include at least one non-key value",
            ));
        }
        if let Some(event) = &self.event {
            event.validate()?;
            if event.post_mutation_snapshot.is_some() {
                return Err(BundleSdkError::protocol(
                    "broker.database_insert.event",
                    "insert enrichment events cannot use a post-mutation snapshot",
                ));
            }
        }
        validate_serialized_size("broker.database_insert", self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DatabaseMutationEvent {
    pub event_kind: LocalId,
    pub subject_kind: LocalId,
    pub subject_id: String,
    /// Publisher-computed canonical SHA-256 for enrichment events. Knowledge
    /// events leave this empty and Core fills the opaque revision from the
    /// signed post-mutation projection.
    pub subject_revision: String,
    /// Bounded event input validated against the enabled signed descriptor.
    pub input: Value,
    /// Present only for update/delete Knowledge events. The signed manifest
    /// owns the permission, resource and returned fields; the request may
    /// supply only bounded equality filters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_mutation_snapshot: Option<DatabasePostMutationSnapshotRef>,
}

impl DatabaseMutationEvent {
    pub fn new(
        event_kind: LocalId,
        subject_kind: LocalId,
        subject_id: impl Into<String>,
        subject_revision: impl Into<String>,
        input: Value,
    ) -> Self {
        Self {
            event_kind,
            subject_kind,
            subject_id: subject_id.into(),
            subject_revision: subject_revision.into(),
            input,
            post_mutation_snapshot: None,
        }
    }

    pub fn post_mutation(
        event_kind: LocalId,
        subject_kind: LocalId,
        filters: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            event_kind,
            subject_kind,
            subject_id: String::new(),
            subject_revision: String::new(),
            input: Value::Object(Default::default()),
            post_mutation_snapshot: Some(DatabasePostMutationSnapshotRef { filters }),
        }
    }

    fn validate(&self) -> Result<()> {
        if let Some(snapshot) = &self.post_mutation_snapshot {
            if !self.subject_id.is_empty() || !self.subject_revision.is_empty() {
                return Err(BundleSdkError::protocol(
                    "broker.database_mutation.event.subject",
                    "post-mutation Knowledge events derive subject id and revision from the signed snapshot",
                ));
            }
            snapshot.validate()?;
        } else {
            if self.subject_id.is_empty()
                || self.subject_id.len() > MAX_RESOURCE_ID
                || self.subject_id.chars().any(char::is_control)
            {
                return Err(BundleSdkError::protocol(
                    "broker.database_mutation.event.subject_id",
                    "must contain 1-256 characters and no control characters",
                ));
            }
            if self.subject_revision.len() != 64
                || !self
                    .subject_revision
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            {
                return Err(BundleSdkError::protocol(
                    "broker.database_mutation.event.subject_revision",
                    "enrichment events require a lowercase hexadecimal SHA-256 digest",
                ));
            }
        }
        if !self.input.is_object() {
            return Err(BundleSdkError::protocol(
                "broker.database_mutation.event.input",
                "must be a JSON object",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DatabasePostMutationSnapshotRef {
    pub filters: BTreeMap<String, Value>,
}

impl DatabasePostMutationSnapshotRef {
    fn validate(&self) -> Result<()> {
        validate_database_filters(
            "database_mutation.event.snapshot.filters",
            &self.filters,
            true,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DatabaseUpdateRequest {
    pub lease: InvocationLeaseToken,
    pub permission_id: LocalId,
    pub resource: BrokerResource,
    pub values: BTreeMap<String, Value>,
    pub filters: BTreeMap<String, Value>,
    #[serde(default = "default_database_mutation_limit")]
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<DatabaseMutationEvent>,
}

impl DatabaseUpdateRequest {
    pub fn new(
        lease: InvocationLeaseToken,
        permission_id: LocalId,
        resource: BrokerResource,
        values: BTreeMap<String, Value>,
        filters: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            lease,
            permission_id,
            resource,
            values,
            filters,
            limit: 1,
            event: None,
        }
    }

    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    pub fn with_event(mut self, event: DatabaseMutationEvent) -> Self {
        self.event = Some(event);
        self
    }

    fn validate(&self) -> Result<()> {
        validate_database_mutation_resource("database_update", &self.resource)?;
        validate_database_values("database_update.values", &self.values)?;
        validate_database_filters("database_update.filters", &self.filters, true)?;
        validate_database_mutation_limit("database_update.limit", self.limit)?;
        if let Some(event) = &self.event {
            event.validate()?;
            if event.post_mutation_snapshot.is_none() {
                return Err(BundleSdkError::protocol(
                    "broker.database_update.event",
                    "update events require a post-mutation snapshot",
                ));
            }
        }
        validate_serialized_size("broker.database_update", self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DatabaseDeleteRequest {
    pub lease: InvocationLeaseToken,
    pub permission_id: LocalId,
    pub resource: BrokerResource,
    pub filters: BTreeMap<String, Value>,
    #[serde(default = "default_database_mutation_limit")]
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<DatabaseMutationEvent>,
}

impl DatabaseDeleteRequest {
    pub fn new(
        lease: InvocationLeaseToken,
        permission_id: LocalId,
        resource: BrokerResource,
        filters: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            lease,
            permission_id,
            resource,
            filters,
            limit: 1,
            event: None,
        }
    }

    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    pub fn with_event(mut self, event: DatabaseMutationEvent) -> Self {
        self.event = Some(event);
        self
    }

    fn validate(&self) -> Result<()> {
        validate_database_mutation_resource("database_delete", &self.resource)?;
        validate_database_filters("database_delete.filters", &self.filters, true)?;
        validate_database_mutation_limit("database_delete.limit", self.limit)?;
        if let Some(event) = &self.event {
            event.validate()?;
            if event.post_mutation_snapshot.is_none() {
                return Err(BundleSdkError::protocol(
                    "broker.database_delete.event",
                    "delete events require a post-mutation snapshot",
                ));
            }
        }
        validate_serialized_size("broker.database_delete", self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DatabaseOrder {
    pub field: String,
    pub direction: DatabaseOrderDirection,
}

impl DatabaseOrder {
    pub fn new(field: impl Into<String>, direction: DatabaseOrderDirection) -> Self {
        Self {
            field: field.into(),
            direction,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DatabaseOrderDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BrokerResponse {
    Probe(BrokerProbeResult),
    DatabaseRows(DatabaseRows),
    DatabaseMutation(DatabaseMutationResult),
    SshExecution(SshExecutionResult),
    KnowledgeContext(KnowledgeContextPack),
    OutcomeFeedbackAccepted(OutcomeFeedbackReceipt),
    KnowledgeCollection(KnowledgeCollectionResult),
    Error(BrokerError),
}

impl BrokerResponse {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Probe(result) => result.validate(),
            Self::DatabaseRows(result) => result.validate(),
            Self::DatabaseMutation(result) => result.validate(),
            Self::SshExecution(result) => result.validate(),
            Self::KnowledgeContext(result) => {
                validate_serialized_size("broker.knowledge_context", result)
            }
            Self::OutcomeFeedbackAccepted(result) => result.validate(),
            Self::KnowledgeCollection(result) => result.validate(),
            Self::Error(error) => error.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[non_exhaustive]
pub enum KnowledgeCollectionResult {
    Listed {
        collections: Vec<KnowledgeCollectionRecord>,
        truncated: bool,
    },
    Saved {
        collection: Box<KnowledgeCollectionRecord>,
    },
    Enqueued {
        collection_id: String,
        run_id: String,
        status: String,
        created: bool,
    },
}

impl KnowledgeCollectionResult {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Listed { collections, .. } => {
                if collections.len() > MAX_KNOWLEDGE_COLLECTIONS as usize {
                    return Err(BundleSdkError::protocol(
                        "broker.knowledge_collection_result.collections",
                        format!("must not exceed {MAX_KNOWLEDGE_COLLECTIONS} records"),
                    ));
                }
            }
            Self::Saved { collection } => collection.validate()?,
            Self::Enqueued {
                collection_id,
                run_id,
                status,
                ..
            } => {
                validate_uuid(
                    "broker.knowledge_collection_result.collection_id",
                    collection_id,
                )?;
                validate_uuid("broker.knowledge_collection_result.run_id", run_id)?;
                bounded_nonempty(
                    "broker.knowledge_collection_result.status",
                    status,
                    MAX_FIELD_NAME,
                )?;
            }
        }
        validate_serialized_size("broker.knowledge_collection_result", self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct KnowledgeCollectionRecord {
    pub collection_id: String,
    pub space_id: String,
    pub output_vault_id: String,
    pub profile_id: LocalId,
    pub label: String,
    pub topic: String,
    pub status: String,
    pub source_classes: Vec<LocalId>,
    pub schedule_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_enqueued_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    pub locators: Vec<KnowledgeCollectionLocator>,
    #[serde(default)]
    pub queries: Vec<KnowledgeCollectionQuery>,
    pub revision: i64,
    pub updated_at: String,
}

impl KnowledgeCollectionRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        collection_id: String,
        space_id: String,
        output_vault_id: String,
        profile_id: LocalId,
        label: String,
        topic: String,
        status: String,
        source_classes: Vec<LocalId>,
        schedule_enabled: bool,
        next_run_at: Option<String>,
        last_enqueued_at: Option<String>,
        last_run_at: Option<String>,
        locators: Vec<KnowledgeCollectionLocator>,
        queries: Vec<KnowledgeCollectionQuery>,
        revision: i64,
        updated_at: String,
    ) -> Self {
        Self {
            collection_id,
            space_id,
            output_vault_id,
            profile_id,
            label,
            topic,
            status,
            source_classes,
            schedule_enabled,
            next_run_at,
            last_enqueued_at,
            last_run_at,
            locators,
            queries,
            revision,
            updated_at,
        }
    }

    fn validate(&self) -> Result<()> {
        validate_uuid(
            "broker.knowledge_collection_record.collection_id",
            &self.collection_id,
        )?;
        validate_uuid(
            "broker.knowledge_collection_record.space_id",
            &self.space_id,
        )?;
        validate_uuid(
            "broker.knowledge_collection_record.output_vault_id",
            &self.output_vault_id,
        )?;
        bounded_nonempty(
            "broker.knowledge_collection_record.label",
            &self.label,
            MAX_TEXT,
        )?;
        bounded_nonempty(
            "broker.knowledge_collection_record.topic",
            &self.topic,
            MAX_TEXT,
        )?;
        bounded_nonempty(
            "broker.knowledge_collection_record.status",
            &self.status,
            MAX_FIELD_NAME,
        )?;
        validate_positive_revision("record", self.revision)?;
        validate_collection_locators("record", &self.locators)?;
        validate_collection_queries("record", &self.queries)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DatabaseMutationResult {
    pub affected_rows: u32,
}

impl DatabaseMutationResult {
    pub fn new(affected_rows: u32) -> Self {
        Self { affected_rows }
    }

    fn validate(&self) -> Result<()> {
        if self.affected_rows > MAX_DATABASE_MUTATION_ROWS {
            return Err(BundleSdkError::protocol(
                "broker.database_mutation.affected_rows",
                format!("must not exceed {MAX_DATABASE_MUTATION_ROWS}"),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SshExecutionResult {
    pub target_id: LocalId,
    pub operation_id: LocalId,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

impl SshExecutionResult {
    pub fn new(
        target_id: LocalId,
        operation_id: LocalId,
        exit_code: i32,
        stdout: String,
        stderr: String,
        duration_ms: u64,
    ) -> Self {
        Self {
            target_id,
            operation_id,
            exit_code,
            stdout,
            stderr,
            duration_ms,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.exit_code < -1 || self.exit_code > 255 {
            return Err(BundleSdkError::protocol(
                "broker.ssh_execution.exit_code",
                "must be -1 or an unsigned process exit status",
            ));
        }
        if self.stdout.len() > MAX_SSH_OUTPUT_BYTES as usize {
            return Err(BundleSdkError::protocol(
                "broker.ssh_execution.stdout",
                format!("must not exceed {MAX_SSH_OUTPUT_BYTES} UTF-8 bytes"),
            ));
        }
        if self.stderr.len() > MAX_SSH_STDERR_BYTES as usize {
            return Err(BundleSdkError::protocol(
                "broker.ssh_execution.stderr",
                format!("must not exceed {MAX_SSH_STDERR_BYTES} UTF-8 bytes"),
            ));
        }
        validate_serialized_size("broker.ssh_execution", self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BrokerProbeResult {
    pub permission_id: LocalId,
    pub resource: BrokerResource,
    pub readiness: BrokerResourceReadiness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl BrokerProbeResult {
    pub fn ready(permission_id: LocalId, resource: BrokerResource) -> Self {
        Self {
            permission_id,
            resource,
            readiness: BrokerResourceReadiness::Ready,
            message: None,
        }
    }

    pub fn unavailable(
        permission_id: LocalId,
        resource: BrokerResource,
        message: impl Into<String>,
    ) -> Self {
        Self {
            permission_id,
            resource,
            readiness: BrokerResourceReadiness::Unavailable,
            message: Some(message.into()),
        }
    }

    fn validate(&self) -> Result<()> {
        if let Some(message) = &self.message {
            bounded_nonempty("broker.probe.message", message, MAX_TEXT)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BrokerResourceReadiness {
    Ready,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DatabaseRows {
    pub rows: Vec<BTreeMap<String, Value>>,
    pub truncated: bool,
}

impl DatabaseRows {
    pub fn new(rows: Vec<BTreeMap<String, Value>>, truncated: bool) -> Self {
        Self { rows, truncated }
    }

    fn validate(&self) -> Result<()> {
        if self.rows.len() > MAX_DATABASE_ROWS {
            return Err(BundleSdkError::protocol(
                "broker.database_rows.rows",
                format!("at most {MAX_DATABASE_ROWS} rows may be returned"),
            ));
        }
        for (row_index, row) in self.rows.iter().enumerate() {
            if row.len() > MAX_COLUMNS {
                return Err(BundleSdkError::protocol(
                    format!("broker.database_rows.rows[{row_index}]"),
                    format!("at most {MAX_COLUMNS} fields may be returned per row"),
                ));
            }
            for field in row.keys() {
                validate_database_field(&format!("broker.database_rows.rows[{row_index}]"), field)?;
            }
        }
        validate_serialized_size("broker.database_rows", self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BrokerError {
    pub code: LocalId,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl BrokerError {
    pub fn new(code: LocalId, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    fn validate(&self) -> Result<()> {
        bounded_nonempty("broker.error.message", &self.message, MAX_TEXT)?;
        if let Some(details) = &self.details {
            validate_serialized_size("broker.error.details", details)?;
        }
        Ok(())
    }
}

fn default_database_limit() -> u32 {
    100
}

fn default_knowledge_collection_limit() -> u32 {
    100
}

fn default_database_mutation_limit() -> u32 {
    1
}

fn validate_collection_inputs(
    action: &str,
    locators: &[KnowledgeCollectionLocator],
    queries: &[KnowledgeCollectionQuery],
) -> Result<()> {
    match (locators.is_empty(), queries.is_empty()) {
        (false, true) => validate_collection_locators(action, locators),
        (true, false) => validate_collection_queries(action, queries),
        _ => Err(BundleSdkError::protocol(
            format!("broker.knowledge_collection.{action}.inputs"),
            "must contain either approved URLs or provider queries, but not both",
        )),
    }
}

fn validate_collection_locators(
    action: &str,
    locators: &[KnowledgeCollectionLocator],
) -> Result<()> {
    if locators.is_empty() || locators.len() > MAX_COLLECTION_LOCATORS {
        return Err(BundleSdkError::protocol(
            format!("broker.knowledge_collection.{action}.locators"),
            format!("must contain 1-{MAX_COLLECTION_LOCATORS} locators"),
        ));
    }
    for (index, locator) in locators.iter().enumerate() {
        bounded_nonempty(
            &format!("broker.knowledge_collection.{action}.locators[{index}].url"),
            &locator.url,
            MAX_RESOURCE_ID,
        )?;
        if locator.title.len() > MAX_TEXT {
            return Err(BundleSdkError::protocol(
                format!("broker.knowledge_collection.{action}.locators[{index}].title"),
                format!("must not exceed {MAX_TEXT} bytes"),
            ));
        }
    }
    Ok(())
}

fn validate_collection_queries(action: &str, queries: &[KnowledgeCollectionQuery]) -> Result<()> {
    if queries.len() > MAX_COLLECTION_LOCATORS {
        return Err(BundleSdkError::protocol(
            format!("broker.knowledge_collection.{action}.queries"),
            format!("must not exceed {MAX_COLLECTION_LOCATORS} provider queries"),
        ));
    }
    let mut providers = BTreeSet::new();
    for (index, query) in queries.iter().enumerate() {
        let path = format!("broker.knowledge_collection.{action}.queries[{index}]");
        if !providers.insert(query.provider.as_str()) {
            return Err(BundleSdkError::protocol(
                format!("{path}.provider"),
                "provider may appear only once",
            ));
        }
        bounded_nonempty(&format!("{path}.query"), &query.query, MAX_TEXT)?;
        bounded_nonempty(&format!("{path}.scope"), &query.scope, MAX_RESOURCE_ID)?;
        if !(1..=3_650).contains(&query.window_days) {
            return Err(BundleSdkError::protocol(
                format!("{path}.window_days"),
                "must be between 1 and 3650",
            ));
        }
        if query.tags.len() > MAX_COLLECTION_QUERY_TAGS {
            return Err(BundleSdkError::protocol(
                format!("{path}.tags"),
                format!("must not exceed {MAX_COLLECTION_QUERY_TAGS} tags"),
            ));
        }
        for (tag_index, tag) in query.tags.iter().enumerate() {
            bounded_nonempty(&format!("{path}.tags[{tag_index}]"), tag, MAX_FIELD_NAME)?;
        }
        if let Some(language) = &query.language {
            bounded_nonempty(&format!("{path}.language"), language, MAX_FIELD_NAME)?;
        }
    }
    Ok(())
}

fn validate_positive_revision(action: &str, revision: i64) -> Result<()> {
    if revision < 1 {
        return Err(BundleSdkError::protocol(
            format!("broker.knowledge_collection.{action}.expected_revision"),
            "must be positive",
        ));
    }
    Ok(())
}

fn validate_uuid(path: &str, value: &str) -> Result<()> {
    let valid = value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    if !valid {
        return Err(BundleSdkError::protocol(path, "must be a canonical UUID"));
    }
    Ok(())
}

fn validate_database_mutation_resource(operation: &str, resource: &BrokerResource) -> Result<()> {
    if resource.database_table_name().is_none() {
        return Err(BundleSdkError::protocol(
            format!("broker.{operation}.resource"),
            "must identify an exact postgres:table:<lowercase_identifier> resource",
        ));
    }
    Ok(())
}

fn validate_database_values(path: &str, values: &BTreeMap<String, Value>) -> Result<()> {
    if values.is_empty() || values.len() > MAX_COLUMNS {
        return Err(BundleSdkError::protocol(
            format!("broker.{path}"),
            format!("must contain 1-{MAX_COLUMNS} fields"),
        ));
    }
    for field in values.keys() {
        validate_database_field(&format!("broker.{path}.{field}"), field)?;
    }
    Ok(())
}

fn validate_database_filters(
    path: &str,
    filters: &BTreeMap<String, Value>,
    require_one: bool,
) -> Result<()> {
    if (require_one && filters.is_empty()) || filters.len() > MAX_FILTERS {
        return Err(BundleSdkError::protocol(
            format!("broker.{path}"),
            format!("must contain 1-{MAX_FILTERS} equality filters"),
        ));
    }
    for (field, value) in filters {
        validate_database_field(&format!("broker.{path}.{field}"), field)?;
        validate_filter_value(path, field, value)?;
    }
    Ok(())
}

fn validate_database_mutation_limit(path: &str, limit: u32) -> Result<()> {
    if !(1..=MAX_DATABASE_MUTATION_ROWS).contains(&limit) {
        return Err(BundleSdkError::protocol(
            format!("broker.{path}"),
            format!("must be between 1 and {MAX_DATABASE_MUTATION_ROWS}"),
        ));
    }
    Ok(())
}

fn validate_lease_token(value: &str) -> Result<()> {
    if value.len() != LEASE_TOKEN_LEN
        || !value.is_ascii()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(BundleSdkError::protocol(
            "broker.lease",
            "must be a 43-character base64url token encoding 256 bits",
        ));
    }
    Ok(())
}

fn validate_broker_resource(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_RESOURCE_ID
        || !value.is_ascii()
        || !value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
    {
        return Err(BundleSdkError::protocol(
            "broker.resource",
            "must contain 1-256 lowercase characters from [a-z0-9._:-] and begin with a letter",
        ));
    }
    Ok(())
}

fn is_database_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FIELD_NAME
        && value.is_ascii()
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn is_local_resource_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.is_ascii()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn validate_message_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_MESSAGE_ID
        || !value.is_ascii()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(BundleSdkError::protocol(
            "broker.message_id",
            "must contain 1-128 characters from [A-Za-z0-9_.:-]",
        ));
    }
    Ok(())
}

pub(crate) fn validate_database_field(path: &str, field: &str) -> Result<()> {
    if !is_database_identifier(field) {
        return Err(BundleSdkError::protocol(
            path,
            "must be a 1-63 character lowercase database field identifier",
        ));
    }
    if field == "tenant_id" {
        return Err(BundleSdkError::protocol(
            path,
            "tenant_id is Core-owned and cannot be supplied or returned by a Bundle",
        ));
    }
    Ok(())
}

fn validate_filter_value(path: &str, field: &str, value: &Value) -> Result<()> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::String(value) => {
            bounded_nonempty(&format!("broker.{path}.{field}"), value, MAX_TEXT)
        }
        Value::Object(_) => {
            let encoded = serde_json::to_vec(value).map_err(|error| {
                BundleSdkError::protocol(
                    format!("broker.{path}.{field}"),
                    format!("JSON equality value cannot be serialized: {error}"),
                )
            })?;
            if encoded.len() > MAX_TEXT {
                return Err(BundleSdkError::protocol(
                    format!("broker.{path}.{field}"),
                    format!("JSON equality value exceeds {MAX_TEXT} serialized bytes"),
                ));
            }
            Ok(())
        }
        Value::Array(_) => Err(BundleSdkError::protocol(
            format!("broker.{path}.{field}"),
            "equality filters do not accept array values",
        )),
    }
}

fn bounded_nonempty(field: &str, value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(BundleSdkError::protocol(
            field,
            format!("must contain 1-{max} characters and no control characters"),
        ));
    }
    Ok(())
}

fn validate_serialized_size<T>(field: &str, value: &T) -> Result<()>
where
    T: Serialize,
{
    let encoded = serde_json::to_vec(value).map_err(|error| {
        BundleSdkError::protocol(field, format!("JSON cannot be serialized: {error}"))
    })?;
    if encoded.len() > MAX_JSON_BYTES {
        return Err(BundleSdkError::protocol(
            field,
            format!("JSON exceeds {MAX_JSON_BYTES} serialized bytes"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BundleId, PermissionKind};
    use semver::Version;

    const LEASE: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    fn identity() -> BundleRuntimeIdentity {
        BundleRuntimeIdentity::new(
            BundleId::new("server-administrator").unwrap(),
            Version::new(0, 1, 0),
        )
    }

    fn select_request() -> DatabaseSelectRequest {
        DatabaseSelectRequest::new(
            InvocationLeaseToken::new(LEASE).unwrap(),
            LocalId::new("telemetry-read").unwrap(),
            BrokerResource::database_table("host_stats_latest").unwrap(),
            ["host_id".to_string(), "cpu_percent".to_string()],
        )
        .with_filter("host_id", Value::String("edge-1".into()))
        .with_order("host_id", DatabaseOrderDirection::Ascending)
        .with_limit(25)
    }

    #[test]
    fn broker_envelope_round_trips_on_an_independent_versioned_contract() {
        let request = BrokerEnvelope::new(
            "bundle:1",
            identity(),
            BrokerRequest::DatabaseSelect(select_request()),
        );
        request.validate_routing(&identity()).unwrap();
        request.payload.validate().unwrap();

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"operation\":\"database_select\""));
        assert!(!json.contains("tenant_id"));
        let decoded: BrokerEnvelope<BrokerRequest> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn knowledge_collection_request_is_typed_bounded_and_tenant_free() {
        let request = BrokerRequest::KnowledgeCollection(KnowledgeCollectionRequest::new(
            InvocationLeaseToken::new(LEASE).unwrap(),
            LocalId::new("news-collections").unwrap(),
            KnowledgeCollectionAction::Create {
                space_id: "11111111-1111-1111-1111-111111111111".into(),
                output_vault_id: "22222222-2222-2222-2222-222222222222".into(),
                profile_id: LocalId::new("news-public-sources").unwrap(),
                topic: "A developing event".into(),
                schedule_enabled: true,
                locators: vec![KnowledgeCollectionLocator::new(
                    "https://www.nasa.gov/news/",
                    "NASA News",
                    LocalId::new("official").unwrap(),
                )],
                queries: vec![],
            },
        ));
        request.validate().unwrap();
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(encoded.contains("\"operation\":\"knowledge_collection\""));
        assert!(!encoded.contains("tenant_id"));
        assert_eq!(
            serde_json::from_str::<BrokerRequest>(&encoded).unwrap(),
            request
        );

        let invalid = BrokerRequest::KnowledgeCollection(KnowledgeCollectionRequest::new(
            InvocationLeaseToken::new(LEASE).unwrap(),
            LocalId::new("news-collections").unwrap(),
            KnowledgeCollectionAction::Enqueue {
                collection_id: "not-a-uuid".into(),
                expected_revision: 0,
            },
        ));
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn ssh_request_contains_only_lease_and_opaque_ids() {
        let request = BrokerRequest::SshExecute(SshExecuteRequest::new(
            InvocationLeaseToken::new(LEASE).unwrap(),
            LocalId::new("edge-one").unwrap(),
            LocalId::new("inventory").unwrap(),
        ));
        request.validate().unwrap();
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"operation\":\"ssh_execute\""));
        for forbidden in ["hostname", "username", "command", "private_key", "key_path"] {
            assert!(!json.contains(forbidden));
        }

        let result = BrokerResponse::SshExecution(SshExecutionResult::new(
            LocalId::new("edge-one").unwrap(),
            LocalId::new("inventory").unwrap(),
            0,
            "hostname=edge-one\n".into(),
            String::new(),
            12,
        ));
        result.validate().unwrap();
    }

    #[test]
    fn invocation_lease_is_canonical_and_redacted_from_debug_output() {
        let lease = InvocationLeaseToken::new(LEASE).unwrap();
        assert_eq!(lease.as_str(), LEASE);
        assert!(!format!("{lease:?}").contains(LEASE));
        assert!(InvocationLeaseToken::new("short").is_err());
        assert!(InvocationLeaseToken::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").is_err());
    }

    #[test]
    fn database_select_rejects_tenant_sql_shapes_and_unbounded_results() {
        let mut request = select_request().with_filter("tenant_id", Value::String("other".into()));
        assert!(request.validate().is_err());

        request.filters.clear();
        request.columns = vec!["host_id;drop_table".into()];
        assert!(request.validate().is_err());

        request.columns = vec!["host_id".into()];
        request
            .filters
            .insert("host_id".into(), serde_json::json!(["edge-1"]));
        assert!(request.validate().is_err());

        request.filters.clear();
        request.limit = 501;
        assert!(request.validate().is_err());

        let unknown = r#"{
            "operation":"database_select",
            "params":{
                "lease":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "permission_id":"telemetry-read",
                "resource":"postgres:table:host_stats_latest",
                "columns":["host_id"],
                "tenant_id":"other",
                "limit":1
            }
        }"#;
        assert!(serde_json::from_str::<BrokerRequest>(unknown).is_err());
    }

    #[test]
    fn database_select_accepts_bounded_exact_json_object_filters() {
        let request = select_request().with_filter(
            "labels",
            serde_json::json!({"mount":"/", "source":"node-exporter"}),
        );
        request.validate().unwrap();

        let oversized = select_request()
            .with_filter("labels", serde_json::json!({"source":"x".repeat(MAX_TEXT)}));
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn database_mutations_are_bounded_and_never_accept_tenant_or_sql() {
        let lease = InvocationLeaseToken::new(LEASE).unwrap();
        let permission = LocalId::new("operations-write").unwrap();
        let resource = BrokerResource::database_table("host_stats_latest").unwrap();
        let values = BTreeMap::from([
            (
                "host_id".into(),
                Value::String("11111111-1111-1111-1111-111111111111".into()),
            ),
            ("stats".into(), serde_json::json!({"load1": 0.5})),
        ]);
        let insert =
            DatabaseInsertRequest::new(lease.clone(), permission.clone(), resource.clone(), values)
                .with_conflict_keys(["host_id".into()]);
        BrokerRequest::DatabaseInsert(insert).validate().unwrap();

        let update = DatabaseUpdateRequest::new(
            lease.clone(),
            permission.clone(),
            resource.clone(),
            BTreeMap::from([("stats".into(), serde_json::json!({"load1": 0.2}))]),
            BTreeMap::from([("host_id".into(), Value::String("host".into()))]),
        );
        BrokerRequest::DatabaseUpdate(update).validate().unwrap();

        let delete = DatabaseDeleteRequest::new(
            lease,
            permission,
            resource,
            BTreeMap::from([("host_id".into(), Value::String("host".into()))]),
        );
        BrokerRequest::DatabaseDelete(delete).validate().unwrap();

        let forbidden = r#"{
            "operation":"database_insert",
            "params":{
                "lease":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "permission_id":"operations-write",
                "resource":"postgres:table:host_stats_latest",
                "values":{"tenant_id":"other","host_id":"host"},
                "conflict_keys":["tenant_id","host_id"]
            }
        }"#;
        let request: BrokerRequest = serde_json::from_str(forbidden).unwrap();
        assert!(request.validate().is_err());

        let empty_filters = DatabaseDeleteRequest::new(
            InvocationLeaseToken::new(LEASE).unwrap(),
            LocalId::new("operations-write").unwrap(),
            BrokerResource::database_table("host_stats_latest").unwrap(),
            BTreeMap::new(),
        );
        assert!(BrokerRequest::DatabaseDelete(empty_filters)
            .validate()
            .is_err());
    }

    #[test]
    fn database_insert_event_carries_only_bounded_signed_subject_metadata() {
        let request = DatabaseInsertRequest::new(
            InvocationLeaseToken::new(LEASE).unwrap(),
            LocalId::new("operations-write").unwrap(),
            BrokerResource::database_table("log_findings").unwrap(),
            BTreeMap::from([(
                "id".into(),
                serde_json::json!("11111111-1111-4111-8111-111111111111"),
            )]),
        )
        .with_event(DatabaseMutationEvent::new(
            LocalId::new("server-log-finding-created").unwrap(),
            LocalId::new("log-finding").unwrap(),
            "11111111-1111-4111-8111-111111111111",
            "a".repeat(64),
            serde_json::json!({"subject":{"summary":"rule evidence"}}),
        ));
        BrokerRequest::DatabaseInsert(request.clone())
            .validate()
            .unwrap();
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(encoded.contains("server-log-finding-created"));
        assert!(!encoded.contains("tenant_id"));

        let mut invalid = request;
        invalid.event.as_mut().unwrap().subject_revision = "not-a-revision".into();
        assert!(BrokerRequest::DatabaseInsert(invalid).validate().is_err());
    }

    #[test]
    fn database_delete_knowledge_event_derives_subject_from_bounded_snapshot() {
        let event = DatabaseMutationEvent::post_mutation(
            LocalId::new("server-incident-closed").unwrap(),
            LocalId::new("server-incident").unwrap(),
            BTreeMap::from([(
                "incident_id".into(),
                serde_json::json!("11111111-1111-4111-8111-111111111111"),
            )]),
        );
        let request = DatabaseDeleteRequest::new(
            InvocationLeaseToken::new(LEASE).unwrap(),
            LocalId::new("operations-write").unwrap(),
            BrokerResource::database_table("server_alert_state").unwrap(),
            BTreeMap::from([("fingerprint".into(), serde_json::json!("disk-full"))]),
        )
        .with_event(event);
        BrokerRequest::DatabaseDelete(request.clone())
            .validate()
            .unwrap();

        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(encoded["event"]["subject_id"], "");
        assert_eq!(encoded["event"]["subject_revision"], "");
        assert_eq!(
            encoded["event"]["post_mutation_snapshot"]["filters"]["incident_id"],
            "11111111-1111-4111-8111-111111111111"
        );
        assert!(encoded.get("tenant_id").is_none());

        let mut invalid = request;
        invalid.event.as_mut().unwrap().subject_id = "publisher-chosen".into();
        assert!(BrokerRequest::DatabaseDelete(invalid).validate().is_err());
    }

    #[test]
    fn broker_response_caps_rows_fields_and_bytes() {
        let mut row = BTreeMap::new();
        row.insert("host_id".into(), Value::String("edge-1".into()));
        BrokerResponse::DatabaseRows(DatabaseRows::new(vec![row.clone()], false))
            .validate()
            .unwrap();

        row.insert("tenant_id".into(), Value::String("tenant-1".into()));
        assert!(
            BrokerResponse::DatabaseRows(DatabaseRows::new(vec![row], false))
                .validate()
                .is_err()
        );

        let oversized = Value::String("x".repeat(MAX_JSON_BYTES));
        let error = BrokerError::new(
            LocalId::new("dependency-unavailable").unwrap(),
            "unavailable",
            true,
        )
        .with_details(oversized);
        assert!(BrokerResponse::Error(error).validate().is_err());
    }

    #[test]
    fn probe_has_no_lease_and_routing_rejects_wrong_identity_or_version() {
        let mut envelope = BrokerEnvelope::new(
            "bundle:2",
            identity(),
            BrokerRequest::Probe(BrokerProbeRequest::new(
                LocalId::new("telemetry-read").unwrap(),
                BrokerResource::database_table("host_stats_latest").unwrap(),
            )),
        );
        envelope.payload.validate().unwrap();
        assert!(!serde_json::to_string(&envelope).unwrap().contains("lease"));

        let other = BundleRuntimeIdentity::new(
            BundleId::new("restaurant-research").unwrap(),
            Version::new(1, 0, 0),
        );
        assert!(envelope.validate_routing(&other).is_err());
        envelope.protocol_version += 1;
        assert!(envelope.validate_routing(&identity()).is_err());
    }

    #[test]
    fn database_permission_has_a_stable_wire_name() {
        assert_eq!(
            serde_json::to_string(&PermissionKind::Database).unwrap(),
            "\"database\""
        );
        assert_eq!(
            BrokerResource::ssh_operation("inventory").unwrap().as_str(),
            "ssh:operation:inventory"
        );
        assert_eq!(
            BrokerResource::secret_use("ssh-identity").unwrap().as_str(),
            "secret:use:ssh-identity"
        );
    }
}
