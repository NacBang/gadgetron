use chrono::{DateTime, Utc};
use gadgetron_bundle_sdk::{BundleId, DomainOntology, DomainSchemaDescriptor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum OntologyRegistryError {
    #[error("ontology package manifest digest must be 64 lowercase hexadecimal characters")]
    InvalidPackageDigest,
    #[error("ontology package version must contain 1-128 non-whitespace characters")]
    InvalidPackageVersion,
    #[error("ontology schema {schema_id:?} version exceeds the registry integer range")]
    SchemaVersionOverflow { schema_id: String },
    #[error(
        "ontology schema {schema_id:?} digest mismatch: descriptor {expected}, bytes {actual}"
    )]
    SchemaDigestMismatch {
        schema_id: String,
        expected: String,
        actual: String,
    },
    #[error(
        "ontology revision {owner_bundle_id}/{schema_id}@{schema_version} already pins {existing_sha256}, not {requested_sha256}"
    )]
    RevisionConflict {
        owner_bundle_id: String,
        schema_id: String,
        schema_version: u32,
        existing_sha256: String,
        requested_sha256: String,
    },
    #[error("ontology activation expected revision must be zero or positive")]
    InvalidExpectedRevision,
    #[error("ontology lifecycle reason must contain 1-2048 characters")]
    InvalidReason,
    #[error("ontology revision {0} is not registered")]
    RevisionNotFound(Uuid),
    #[error(
        "ontology activation changed: expected revision {expected_revision}, current revision {current_revision}"
    )]
    ActivationConflict {
        expected_revision: i64,
        current_revision: i64,
    },
    #[error("ontology is not active for this tenant")]
    ActivationRequired,
    #[error("ontology activation target changed before deactivation")]
    ActivationTargetChanged,
    #[error("knowledge object {0} is unavailable")]
    ObjectNotFound(Uuid),
    #[error(
        "knowledge object revision changed: expected {expected_revision}, current {current_revision}"
    )]
    ObjectRevisionConflict {
        expected_revision: i64,
        current_revision: i64,
    },
    #[error(
        "ontology mapping changed: expected revision {expected_revision}, current revision {current_revision}"
    )]
    MappingConflict {
        expected_revision: i64,
        current_revision: i64,
    },
    #[error("ontology mapping shape does not match its disposition")]
    InvalidMappingShape,
    #[error("ontology mapping confidence must be a finite value between zero and one")]
    InvalidConfidence,
    #[error("ontology mapping evidence must be a JSON object")]
    InvalidEvidence,
    #[error("ontology type {type_id:?} is not declared by revision {ontology_revision_id}")]
    UnknownType {
        ontology_revision_id: Uuid,
        type_id: String,
    },
    #[error("ontology type {type_id:?} is deprecated and cannot receive new mappings")]
    DeprecatedType { type_id: String },
    #[error(transparent)]
    Contract(#[from] gadgetron_bundle_sdk::BundleSdkError),
    #[error("ontology registry database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("ontology contract could not be normalized: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy)]
pub struct OntologySchemaRegistration<'a> {
    pub descriptor: &'a DomainSchemaDescriptor,
    pub bytes: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct OntologyPackageRegistration<'a> {
    pub owner_bundle_id: &'a BundleId,
    pub package_version: &'a str,
    pub package_manifest_sha256: &'a str,
    pub schemas: &'a [OntologySchemaRegistration<'a>],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct OntologyRevision {
    pub id: Uuid,
    pub owner_bundle_id: String,
    pub schema_id: String,
    pub schema_version: i32,
    pub schema_sha256: String,
    pub format_version: i32,
    pub legacy_adapter: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologyRegistrationReceipt {
    pub revision: OntologyRevision,
    pub revision_created: bool,
    pub provenance_created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologyRegistryEntry {
    pub revision: OntologyRevision,
    pub package_count: i64,
    pub type_count: i64,
    pub relation_count: i64,
    pub activation_action: Option<OntologyActivationAction>,
    pub activation_revision: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct OntologyRegistry {
    pool: PgPool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OntologyActivationAction {
    Activate,
    Deactivate,
}

impl OntologyActivationAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Activate => "activate",
            Self::Deactivate => "deactivate",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "activate" => Self::Activate,
            "deactivate" => Self::Deactivate,
            _ => unreachable!("database activation action is constrained"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OntologyActivationCommand<'a> {
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub ontology_revision_id: Uuid,
    pub expected_activation_revision: i64,
    pub reason: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologyActivationEvent {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub ontology_revision_id: Uuid,
    pub owner_bundle_id: String,
    pub schema_id: String,
    pub activation_revision: i64,
    pub action: OntologyActivationAction,
    pub actor_user_id: Uuid,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologyActivationReceipt {
    pub event: OntologyActivationEvent,
    pub created: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OntologyMappingDisposition {
    Proposed,
    Active,
    Unmapped,
}

impl OntologyMappingDisposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Active => "active",
            Self::Unmapped => "unmapped",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "proposed" => Self::Proposed,
            "active" => Self::Active,
            "unmapped" => Self::Unmapped,
            _ => unreachable!("database mapping disposition is constrained"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OntologyMappingCommand<'a> {
    pub tenant_id: Uuid,
    pub recorded_by: Uuid,
    pub object_id: Uuid,
    pub object_revision: i64,
    pub expected_mapping_revision: i64,
    pub disposition: OntologyMappingDisposition,
    pub ontology_revision_id: Option<Uuid>,
    pub type_id: Option<&'a str>,
    pub confidence: Option<f32>,
    pub evidence: serde_json::Value,
    pub reason: &'a str,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OntologyMappingEvent {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub object_id: Uuid,
    pub object_revision: i64,
    pub mapping_revision: i64,
    pub disposition: OntologyMappingDisposition,
    pub ontology_revision_id: Option<Uuid>,
    pub type_id: Option<String>,
    pub confidence: Option<f32>,
    pub evidence: serde_json::Value,
    pub reason: String,
    pub recorded_by: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct OntologyKernel {
    pool: PgPool,
}

impl OntologyRegistry {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn register_package(
        &self,
        registration: OntologyPackageRegistration<'_>,
    ) -> Result<Vec<OntologyRegistrationReceipt>, OntologyRegistryError> {
        validate_package_metadata(
            registration.package_version,
            registration.package_manifest_sha256,
        )?;

        let mut prepared = Vec::with_capacity(registration.schemas.len());
        for schema in registration.schemas {
            let actual = hex::encode(Sha256::digest(schema.bytes));
            if actual != schema.descriptor.sha256 {
                return Err(OntologyRegistryError::SchemaDigestMismatch {
                    schema_id: schema.descriptor.id.as_str().to_string(),
                    expected: schema.descriptor.sha256.clone(),
                    actual,
                });
            }
            let ontology = DomainOntology::parse_json(schema.bytes, schema.descriptor.version)?;
            let normalized = serde_json::to_value(&ontology)?;
            prepared.push((schema, ontology, normalized));
        }

        let mut transaction = self.pool.begin().await?;
        let mut receipts = Vec::with_capacity(prepared.len());
        for (schema, ontology, normalized) in prepared {
            receipts.push(
                register_schema(
                    &mut transaction,
                    registration.owner_bundle_id,
                    registration.package_version,
                    registration.package_manifest_sha256,
                    schema,
                    &ontology,
                    normalized,
                )
                .await?,
            );
        }
        transaction.commit().await?;
        Ok(receipts)
    }
}

#[derive(Debug, FromRow)]
struct ActivationEventRow {
    id: Uuid,
    tenant_id: Uuid,
    ontology_revision_id: Uuid,
    owner_bundle_id: String,
    schema_id: String,
    activation_revision: i64,
    action: String,
    actor_user_id: Uuid,
    reason: String,
    created_at: DateTime<Utc>,
}

impl From<ActivationEventRow> for OntologyActivationEvent {
    fn from(row: ActivationEventRow) -> Self {
        Self {
            id: row.id,
            tenant_id: row.tenant_id,
            ontology_revision_id: row.ontology_revision_id,
            owner_bundle_id: row.owner_bundle_id,
            schema_id: row.schema_id,
            activation_revision: row.activation_revision,
            action: OntologyActivationAction::parse(&row.action),
            actor_user_id: row.actor_user_id,
            reason: row.reason,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, FromRow)]
struct MappingEventRow {
    id: Uuid,
    tenant_id: Uuid,
    object_id: Uuid,
    object_revision: i64,
    mapping_revision: i64,
    disposition: String,
    ontology_revision_id: Option<Uuid>,
    type_id: Option<String>,
    confidence: Option<f32>,
    evidence: serde_json::Value,
    reason: String,
    recorded_by: Uuid,
    created_at: DateTime<Utc>,
}

impl From<MappingEventRow> for OntologyMappingEvent {
    fn from(row: MappingEventRow) -> Self {
        Self {
            id: row.id,
            tenant_id: row.tenant_id,
            object_id: row.object_id,
            object_revision: row.object_revision,
            mapping_revision: row.mapping_revision,
            disposition: OntologyMappingDisposition::parse(&row.disposition),
            ontology_revision_id: row.ontology_revision_id,
            type_id: row.type_id,
            confidence: row.confidence,
            evidence: row.evidence,
            reason: row.reason,
            recorded_by: row.recorded_by,
            created_at: row.created_at,
        }
    }
}

impl OntologyKernel {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn activate(
        &self,
        command: OntologyActivationCommand<'_>,
    ) -> Result<OntologyActivationReceipt, OntologyRegistryError> {
        self.set_activation(command, OntologyActivationAction::Activate)
            .await
    }

    pub async fn deactivate(
        &self,
        command: OntologyActivationCommand<'_>,
    ) -> Result<OntologyActivationReceipt, OntologyRegistryError> {
        self.set_activation(command, OntologyActivationAction::Deactivate)
            .await
    }

    async fn set_activation(
        &self,
        command: OntologyActivationCommand<'_>,
        action: OntologyActivationAction,
    ) -> Result<OntologyActivationReceipt, OntologyRegistryError> {
        validate_expected_revision(command.expected_activation_revision)?;
        validate_reason(command.reason)?;
        let mut transaction = self.pool.begin().await?;
        let identity = sqlx::query_as::<_, (String, String)>(
            "SELECT owner_bundle_id, schema_id FROM knowledge_ontology_revisions WHERE id = $1",
        )
        .bind(command.ontology_revision_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(OntologyRegistryError::RevisionNotFound(
            command.ontology_revision_id,
        ))?;
        lock_key(
            &mut transaction,
            &format!(
                "ontology-activation:{}/{}/{}",
                command.tenant_id, identity.0, identity.1
            ),
        )
        .await?;
        let current = sqlx::query_as::<_, ActivationEventRow>(
            r#"SELECT id, tenant_id, ontology_revision_id, owner_bundle_id, schema_id,
                      activation_revision, action, actor_user_id, reason, created_at
               FROM knowledge_ontology_activation_events
               WHERE tenant_id = $1 AND owner_bundle_id = $2 AND schema_id = $3
               ORDER BY activation_revision DESC
               LIMIT 1
               FOR UPDATE"#,
        )
        .bind(command.tenant_id)
        .bind(&identity.0)
        .bind(&identity.1)
        .fetch_optional(&mut *transaction)
        .await?;
        let current_revision = current
            .as_ref()
            .map(|event| event.activation_revision)
            .unwrap_or(0);
        if current_revision != command.expected_activation_revision {
            return Err(OntologyRegistryError::ActivationConflict {
                expected_revision: command.expected_activation_revision,
                current_revision,
            });
        }
        if let Some(current) = current {
            let current_action = OntologyActivationAction::parse(&current.action);
            if current_action == action
                && current.ontology_revision_id == command.ontology_revision_id
            {
                transaction.commit().await?;
                return Ok(OntologyActivationReceipt {
                    event: current.into(),
                    created: false,
                });
            }
            if action == OntologyActivationAction::Deactivate
                && (current_action != OntologyActivationAction::Activate
                    || current.ontology_revision_id != command.ontology_revision_id)
            {
                return Err(OntologyRegistryError::ActivationTargetChanged);
            }
        } else if action == OntologyActivationAction::Deactivate {
            return Err(OntologyRegistryError::ActivationRequired);
        }

        let next_revision = current_revision
            .checked_add(1)
            .ok_or(OntologyRegistryError::InvalidExpectedRevision)?;
        let event = sqlx::query_as::<_, ActivationEventRow>(
            r#"INSERT INTO knowledge_ontology_activation_events (
                   tenant_id, ontology_revision_id, owner_bundle_id, schema_id,
                   activation_revision, action, actor_user_id, reason
               ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
               RETURNING id, tenant_id, ontology_revision_id, owner_bundle_id, schema_id,
                         activation_revision, action, actor_user_id, reason, created_at"#,
        )
        .bind(command.tenant_id)
        .bind(command.ontology_revision_id)
        .bind(&identity.0)
        .bind(&identity.1)
        .bind(next_revision)
        .bind(action.as_str())
        .bind(command.actor_user_id)
        .bind(command.reason)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(OntologyActivationReceipt {
            event: event.into(),
            created: true,
        })
    }

    pub async fn append_mapping(
        &self,
        command: OntologyMappingCommand<'_>,
    ) -> Result<OntologyMappingEvent, OntologyRegistryError> {
        validate_mapping_command(&command)?;
        let mut transaction = self.pool.begin().await?;
        lock_key(
            &mut transaction,
            &format!(
                "ontology-mapping:{}/{}",
                command.tenant_id, command.object_id
            ),
        )
        .await?;
        let object_revision = sqlx::query_scalar::<_, i64>(
            "SELECT revision FROM knowledge_objects \
             WHERE tenant_id = $1 AND id = $2 AND status = 'active' FOR SHARE",
        )
        .bind(command.tenant_id)
        .bind(command.object_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(OntologyRegistryError::ObjectNotFound(command.object_id))?;
        if object_revision != command.object_revision {
            return Err(OntologyRegistryError::ObjectRevisionConflict {
                expected_revision: command.object_revision,
                current_revision: object_revision,
            });
        }
        let current_mapping_revision = sqlx::query_scalar::<_, i64>(
            r#"SELECT mapping_revision FROM knowledge_ontology_mapping_events
               WHERE tenant_id = $1 AND object_id = $2
               ORDER BY mapping_revision DESC LIMIT 1 FOR UPDATE"#,
        )
        .bind(command.tenant_id)
        .bind(command.object_id)
        .fetch_optional(&mut *transaction)
        .await?
        .unwrap_or(0);
        if current_mapping_revision != command.expected_mapping_revision {
            return Err(OntologyRegistryError::MappingConflict {
                expected_revision: command.expected_mapping_revision,
                current_revision: current_mapping_revision,
            });
        }

        if let (Some(ontology_revision_id), Some(type_id)) =
            (command.ontology_revision_id, command.type_id)
        {
            validate_effective_type(
                &mut transaction,
                command.tenant_id,
                ontology_revision_id,
                type_id,
            )
            .await?;
        }

        let next_revision = current_mapping_revision
            .checked_add(1)
            .ok_or(OntologyRegistryError::InvalidExpectedRevision)?;
        let event = sqlx::query_as::<_, MappingEventRow>(
            r#"INSERT INTO knowledge_ontology_mapping_events (
                   tenant_id, object_id, object_revision, mapping_revision, disposition,
                   ontology_revision_id, type_id, confidence, evidence, reason, recorded_by
               ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
               RETURNING id, tenant_id, object_id, object_revision, mapping_revision,
                         disposition, ontology_revision_id, type_id, confidence, evidence,
                         reason, recorded_by, created_at"#,
        )
        .bind(command.tenant_id)
        .bind(command.object_id)
        .bind(command.object_revision)
        .bind(next_revision)
        .bind(command.disposition.as_str())
        .bind(command.ontology_revision_id)
        .bind(command.type_id)
        .bind(command.confidence)
        .bind(command.evidence)
        .bind(command.reason)
        .bind(command.recorded_by)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(event.into())
    }

    pub async fn list_registry(
        &self,
        tenant_id: Uuid,
    ) -> Result<Vec<OntologyRegistryEntry>, OntologyRegistryError> {
        #[derive(FromRow)]
        struct RegistryRow {
            id: Uuid,
            owner_bundle_id: String,
            schema_id: String,
            schema_version: i32,
            schema_sha256: String,
            format_version: i32,
            legacy_adapter: bool,
            created_at: DateTime<Utc>,
            package_count: i64,
            type_count: i64,
            relation_count: i64,
            activation_action: Option<String>,
            activation_revision: Option<i64>,
        }

        let rows = sqlx::query_as::<_, RegistryRow>(
            r#"SELECT r.id, r.owner_bundle_id, r.schema_id, r.schema_version,
                      r.schema_sha256, r.format_version, r.legacy_adapter, r.created_at,
                      (SELECT COUNT(*) FROM knowledge_ontology_package_provenance p
                       WHERE p.ontology_revision_id = r.id) AS package_count,
                      jsonb_array_length(r.normalized_ontology -> 'types')::BIGINT AS type_count,
                      jsonb_array_length(r.normalized_ontology -> 'relations')::BIGINT AS relation_count,
                      active.action AS activation_action,
                      active.activation_revision
               FROM knowledge_ontology_revisions r
               LEFT JOIN LATERAL (
                   SELECT action, activation_revision
                   FROM knowledge_ontology_activation_events a
                   WHERE a.tenant_id = $1
                     AND a.owner_bundle_id = r.owner_bundle_id
                     AND a.schema_id = r.schema_id
                   ORDER BY activation_revision DESC
                   LIMIT 1
               ) active ON TRUE
               ORDER BY r.owner_bundle_id, r.schema_id, r.schema_version DESC"#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| OntologyRegistryEntry {
                revision: OntologyRevision {
                    id: row.id,
                    owner_bundle_id: row.owner_bundle_id,
                    schema_id: row.schema_id,
                    schema_version: row.schema_version,
                    schema_sha256: row.schema_sha256,
                    format_version: row.format_version,
                    legacy_adapter: row.legacy_adapter,
                    created_at: row.created_at,
                },
                package_count: row.package_count,
                type_count: row.type_count,
                relation_count: row.relation_count,
                activation_action: row
                    .activation_action
                    .as_deref()
                    .map(OntologyActivationAction::parse),
                activation_revision: row.activation_revision,
            })
            .collect())
    }

    pub async fn list_mapping_history(
        &self,
        tenant_id: Uuid,
        object_id: Uuid,
    ) -> Result<Vec<OntologyMappingEvent>, OntologyRegistryError> {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM knowledge_objects \
             WHERE tenant_id = $1 AND id = $2 AND status <> 'tombstone')",
        )
        .bind(tenant_id)
        .bind(object_id)
        .fetch_one(&self.pool)
        .await?;
        if !exists {
            return Err(OntologyRegistryError::ObjectNotFound(object_id));
        }
        let rows = sqlx::query_as::<_, MappingEventRow>(
            r#"SELECT id, tenant_id, object_id, object_revision, mapping_revision,
                      disposition, ontology_revision_id, type_id, confidence, evidence,
                      reason, recorded_by, created_at
               FROM knowledge_ontology_mapping_events
               WHERE tenant_id = $1 AND object_id = $2
               ORDER BY mapping_revision DESC"#,
        )
        .bind(tenant_id)
        .bind(object_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}

fn validate_expected_revision(value: i64) -> Result<(), OntologyRegistryError> {
    if value < 0 {
        Err(OntologyRegistryError::InvalidExpectedRevision)
    } else {
        Ok(())
    }
}

fn validate_reason(value: &str) -> Result<(), OntologyRegistryError> {
    if value.trim().is_empty() || value.len() > 2_048 {
        Err(OntologyRegistryError::InvalidReason)
    } else {
        Ok(())
    }
}

fn validate_mapping_command(
    command: &OntologyMappingCommand<'_>,
) -> Result<(), OntologyRegistryError> {
    validate_expected_revision(command.expected_mapping_revision)?;
    validate_reason(command.reason)?;
    if !command.evidence.is_object() {
        return Err(OntologyRegistryError::InvalidEvidence);
    }
    if command
        .confidence
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(OntologyRegistryError::InvalidConfidence);
    }
    let has_mapping = command.ontology_revision_id.is_some() && command.type_id.is_some();
    let valid_shape = match command.disposition {
        OntologyMappingDisposition::Proposed | OntologyMappingDisposition::Active => has_mapping,
        OntologyMappingDisposition::Unmapped => {
            command.ontology_revision_id.is_none() && command.type_id.is_none()
        }
    };
    if valid_shape {
        Ok(())
    } else {
        Err(OntologyRegistryError::InvalidMappingShape)
    }
}

async fn validate_effective_type(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    ontology_revision_id: Uuid,
    type_id: &str,
) -> Result<(), OntologyRegistryError> {
    let revision = sqlx::query_as::<_, (String, String, serde_json::Value)>(
        "SELECT owner_bundle_id, schema_id, normalized_ontology \
         FROM knowledge_ontology_revisions WHERE id = $1",
    )
    .bind(ontology_revision_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(OntologyRegistryError::RevisionNotFound(
        ontology_revision_id,
    ))?;
    let ontology: DomainOntology = serde_json::from_value(revision.2)?;
    let ontology_type =
        ontology
            .type_by_id(type_id)
            .ok_or_else(|| OntologyRegistryError::UnknownType {
                ontology_revision_id,
                type_id: type_id.to_string(),
            })?;
    if ontology_type.deprecated {
        return Err(OntologyRegistryError::DeprecatedType {
            type_id: type_id.to_string(),
        });
    }
    let active = sqlx::query_as::<_, (Uuid, String)>(
        r#"SELECT ontology_revision_id, action
           FROM knowledge_ontology_activation_events
           WHERE tenant_id = $1 AND owner_bundle_id = $2 AND schema_id = $3
           ORDER BY activation_revision DESC LIMIT 1"#,
    )
    .bind(tenant_id)
    .bind(&revision.0)
    .bind(&revision.1)
    .fetch_optional(&mut **transaction)
    .await?;
    if !matches!(active, Some((id, action)) if id == ontology_revision_id && action == "activate") {
        return Err(OntologyRegistryError::ActivationRequired);
    }
    Ok(())
}

async fn lock_key(
    transaction: &mut Transaction<'_, Postgres>,
    key: &str,
) -> Result<(), OntologyRegistryError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(key)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

fn validate_package_metadata(
    package_version: &str,
    package_manifest_sha256: &str,
) -> Result<(), OntologyRegistryError> {
    if package_version.trim().is_empty() || package_version.len() > 128 {
        return Err(OntologyRegistryError::InvalidPackageVersion);
    }
    let valid_digest = package_manifest_sha256.len() == 64
        && package_manifest_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid_digest {
        return Err(OntologyRegistryError::InvalidPackageDigest);
    }
    Ok(())
}

async fn register_schema(
    transaction: &mut Transaction<'_, Postgres>,
    owner_bundle_id: &BundleId,
    package_version: &str,
    package_manifest_sha256: &str,
    schema: &OntologySchemaRegistration<'_>,
    ontology: &DomainOntology,
    normalized: serde_json::Value,
) -> Result<OntologyRegistrationReceipt, OntologyRegistryError> {
    let inserted = sqlx::query_as::<_, OntologyRevision>(
        r#"INSERT INTO knowledge_ontology_revisions (
               owner_bundle_id, schema_id, schema_version, schema_sha256,
               format_version, legacy_adapter, schema_bytes, normalized_ontology
           ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
           ON CONFLICT (owner_bundle_id, schema_id, schema_version) DO NOTHING
           RETURNING id, owner_bundle_id, schema_id, schema_version,
                     schema_sha256, format_version, legacy_adapter, created_at"#,
    )
    .bind(owner_bundle_id.as_str())
    .bind(schema.descriptor.id.as_str())
    .bind(i32::try_from(schema.descriptor.version).map_err(|_| {
        OntologyRegistryError::SchemaVersionOverflow {
            schema_id: schema.descriptor.id.as_str().to_string(),
        }
    })?)
    .bind(&schema.descriptor.sha256)
    .bind(i32::try_from(ontology.format_version).expect("format version 1 fits i32"))
    .bind(ontology.legacy_adapter)
    .bind(schema.bytes)
    .bind(normalized)
    .fetch_optional(&mut **transaction)
    .await?;
    let revision_created = inserted.is_some();
    let revision = match inserted {
        Some(revision) => revision,
        None => {
            sqlx::query_as::<_, OntologyRevision>(
                r#"SELECT id, owner_bundle_id, schema_id, schema_version,
                      schema_sha256, format_version, legacy_adapter, created_at
               FROM knowledge_ontology_revisions
               WHERE owner_bundle_id = $1 AND schema_id = $2 AND schema_version = $3
               FOR UPDATE"#,
            )
            .bind(owner_bundle_id.as_str())
            .bind(schema.descriptor.id.as_str())
            .bind(i32::try_from(schema.descriptor.version).expect("validated above"))
            .fetch_one(&mut **transaction)
            .await?
        }
    };
    if revision.schema_sha256 != schema.descriptor.sha256 {
        return Err(OntologyRegistryError::RevisionConflict {
            owner_bundle_id: owner_bundle_id.as_str().to_string(),
            schema_id: schema.descriptor.id.as_str().to_string(),
            schema_version: schema.descriptor.version,
            existing_sha256: revision.schema_sha256,
            requested_sha256: schema.descriptor.sha256.clone(),
        });
    }

    let provenance_created = sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO knowledge_ontology_package_provenance (
               ontology_revision_id, package_version, package_manifest_sha256
           ) VALUES ($1, $2, $3)
           ON CONFLICT (ontology_revision_id, package_manifest_sha256) DO NOTHING
           RETURNING id"#,
    )
    .bind(revision.id)
    .bind(package_version)
    .bind(package_manifest_sha256)
    .fetch_optional(&mut **transaction)
    .await?
    .is_some();

    Ok(OntologyRegistrationReceipt {
        revision,
        revision_created,
        provenance_created,
    })
}
