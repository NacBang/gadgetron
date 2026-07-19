//! Tenant-owned AI role overrides for the Awakening Engine.

use chrono::{DateTime, Utc};
use gadgetron_core::agent::config::{
    AgentBackend, AgentEffort, ConversationAgentProfile, ModelSource,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeRoleProfileScope {
    Core,
    Bundle,
}

impl KnowledgeRoleProfileScope {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Bundle => "bundle",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeRoleProfileSource {
    Global,
    Core,
    Bundle,
}

impl KnowledgeRoleProfileSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Core => "core",
            Self::Bundle => "bundle",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeRoleSelection {
    pub backend: AgentBackend,
    #[serde(default)]
    pub model: String,
    pub effort: AgentEffort,
    #[serde(default)]
    pub model_source: ModelSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_endpoint_id: Option<Uuid>,
}

impl KnowledgeRoleSelection {
    pub fn from_profile(profile: &ConversationAgentProfile) -> Self {
        Self {
            backend: profile.backend,
            model: profile.model.clone(),
            effort: profile.effort,
            model_source: profile.model_source,
            llm_endpoint_id: profile.llm_endpoint_id,
        }
    }

    pub fn into_profile(self) -> ConversationAgentProfile {
        ConversationAgentProfile {
            backend: self.backend,
            llm_endpoint_id: self.llm_endpoint_id,
            model: self.model,
            effort: self.effort,
            model_source: self.model_source,
            local_base_url: String::new(),
            local_api_key_env: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeRoleProfileOverride {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub scope: KnowledgeRoleProfileScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    pub role_id: String,
    pub selection: KnowledgeRoleSelection,
    pub revision: i64,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveKnowledgeRoleSelection {
    pub selection: KnowledgeRoleSelection,
    pub source: KnowledgeRoleProfileSource,
    pub core_revision: Option<i64>,
    pub bundle_revision: Option<i64>,
    pub profile_ref: String,
}

pub struct UpsertKnowledgeRoleProfile<'a> {
    pub scope: KnowledgeRoleProfileScope,
    pub bundle_id: Option<&'a str>,
    pub role_id: &'a str,
    pub expected_revision: Option<i64>,
    pub selection: &'a KnowledgeRoleSelection,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RoleProfileDbRow {
    id: Uuid,
    tenant_id: Uuid,
    scope_kind: String,
    bundle_id: String,
    role_id: String,
    runtime_backend: String,
    runtime_model: String,
    runtime_effort: String,
    runtime_model_source: String,
    runtime_endpoint_id: Option<Uuid>,
    revision: i64,
    updated_by_user_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum KnowledgeRoleProfileError {
    #[error("invalid Knowledge AI role profile: {0}")]
    InvalidInput(String),
    #[error("persisted Knowledge AI role profile is invalid: {0}")]
    InvalidPersisted(String),
    #[error("Knowledge AI role profile changed; refresh before saving")]
    Conflict,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

const ROLE_PROFILE_COLUMNS: &str = r#"id, tenant_id, scope_kind, bundle_id, role_id,
    runtime_backend, runtime_model, runtime_effort, runtime_model_source,
    runtime_endpoint_id, revision, updated_by_user_id, created_at, updated_at"#;

pub async fn get_role_profile_override(
    pool: &PgPool,
    tenant_id: Uuid,
    scope: KnowledgeRoleProfileScope,
    bundle_id: Option<&str>,
    role_id: &str,
) -> Result<Option<KnowledgeRoleProfileOverride>, KnowledgeRoleProfileError> {
    let bundle_id = normalized_bundle_id(scope, bundle_id)?;
    validate_role_id(role_id)?;
    let query = format!(
        "SELECT {ROLE_PROFILE_COLUMNS} FROM knowledge_agent_role_profiles \
         WHERE tenant_id = $1 AND scope_kind = $2 AND bundle_id = $3 AND role_id = $4"
    );
    sqlx::query_as::<_, RoleProfileDbRow>(&query)
        .bind(tenant_id)
        .bind(scope.as_str())
        .bind(bundle_id)
        .bind(role_id)
        .fetch_optional(pool)
        .await?
        .map(row_to_override)
        .transpose()
}

pub async fn upsert_role_profile_override(
    pool: &PgPool,
    tenant_id: Uuid,
    actor_user_id: Uuid,
    request: UpsertKnowledgeRoleProfile<'_>,
) -> Result<KnowledgeRoleProfileOverride, KnowledgeRoleProfileError> {
    let bundle_id = normalized_bundle_id(request.scope, request.bundle_id)?;
    validate_role_id(request.role_id)?;
    validate_selection(request.selection)?;
    let query = format!(
        r#"INSERT INTO knowledge_agent_role_profiles
           (tenant_id, scope_kind, bundle_id, role_id, runtime_backend, runtime_model,
            runtime_effort, runtime_model_source, runtime_endpoint_id, updated_by_user_id)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
           ON CONFLICT (tenant_id, scope_kind, bundle_id, role_id) DO UPDATE SET
             runtime_backend = EXCLUDED.runtime_backend,
             runtime_model = EXCLUDED.runtime_model,
             runtime_effort = EXCLUDED.runtime_effort,
             runtime_model_source = EXCLUDED.runtime_model_source,
             runtime_endpoint_id = EXCLUDED.runtime_endpoint_id,
             updated_by_user_id = EXCLUDED.updated_by_user_id,
             revision = knowledge_agent_role_profiles.revision + 1,
             updated_at = NOW()
           WHERE $11::BIGINT IS NOT NULL
             AND knowledge_agent_role_profiles.revision = $11
           RETURNING {ROLE_PROFILE_COLUMNS}"#
    );
    let row = sqlx::query_as::<_, RoleProfileDbRow>(&query)
        .bind(tenant_id)
        .bind(request.scope.as_str())
        .bind(bundle_id)
        .bind(request.role_id)
        .bind(request.selection.backend.as_str())
        .bind(&request.selection.model)
        .bind(request.selection.effort.as_str())
        .bind(model_source_str(request.selection.model_source))
        .bind(request.selection.llm_endpoint_id)
        .bind(actor_user_id)
        .bind(request.expected_revision)
        .fetch_optional(pool)
        .await?
        .ok_or(KnowledgeRoleProfileError::Conflict)?;
    row_to_override(row)
}

pub async fn delete_role_profile_override(
    pool: &PgPool,
    tenant_id: Uuid,
    scope: KnowledgeRoleProfileScope,
    bundle_id: Option<&str>,
    role_id: &str,
    expected_revision: i64,
) -> Result<bool, KnowledgeRoleProfileError> {
    let bundle_id = normalized_bundle_id(scope, bundle_id)?;
    validate_role_id(role_id)?;
    let result = sqlx::query(
        r#"DELETE FROM knowledge_agent_role_profiles
           WHERE tenant_id = $1 AND scope_kind = $2 AND bundle_id = $3
             AND role_id = $4 AND revision = $5"#,
    )
    .bind(tenant_id)
    .bind(scope.as_str())
    .bind(bundle_id)
    .bind(role_id)
    .bind(expected_revision)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(KnowledgeRoleProfileError::Conflict);
    }
    Ok(true)
}

pub async fn resolve_role_profile(
    pool: &PgPool,
    tenant_id: Uuid,
    global: &ConversationAgentProfile,
    core_role_id: &str,
    bundle_role: Option<(&str, &str)>,
) -> Result<EffectiveKnowledgeRoleSelection, KnowledgeRoleProfileError> {
    let core = get_role_profile_override(
        pool,
        tenant_id,
        KnowledgeRoleProfileScope::Core,
        None,
        core_role_id,
    )
    .await?;
    let bundle = if let Some((bundle_id, role_id)) = bundle_role {
        get_role_profile_override(
            pool,
            tenant_id,
            KnowledgeRoleProfileScope::Bundle,
            Some(bundle_id),
            role_id,
        )
        .await?
    } else {
        None
    };
    let (selection, source) = if let Some(profile) = bundle.as_ref() {
        (
            profile.selection.clone(),
            KnowledgeRoleProfileSource::Bundle,
        )
    } else if let Some(profile) = core.as_ref() {
        (profile.selection.clone(), KnowledgeRoleProfileSource::Core)
    } else {
        (
            KnowledgeRoleSelection::from_profile(global),
            KnowledgeRoleProfileSource::Global,
        )
    };
    let core_revision = core.as_ref().map(|profile| profile.revision);
    let bundle_revision = bundle.as_ref().map(|profile| profile.revision);
    let profile_ref = profile_ref(
        tenant_id,
        core_role_id,
        bundle_role,
        &selection,
        source,
        core_revision,
        bundle_revision,
    );
    Ok(EffectiveKnowledgeRoleSelection {
        selection,
        source,
        core_revision,
        bundle_revision,
        profile_ref,
    })
}

fn profile_ref(
    tenant_id: Uuid,
    core_role_id: &str,
    bundle_role: Option<(&str, &str)>,
    selection: &KnowledgeRoleSelection,
    source: KnowledgeRoleProfileSource,
    core_revision: Option<i64>,
    bundle_revision: Option<i64>,
) -> String {
    let payload = serde_json::json!({
        "tenant_id": tenant_id,
        "core_role_id": core_role_id,
        "bundle_id": bundle_role.map(|value| value.0),
        "bundle_role_id": bundle_role.map(|value| value.1),
        "selection": selection,
        "source": source.as_str(),
        "core_revision": core_revision,
        "bundle_revision": bundle_revision,
    });
    hex::encode(Sha256::digest(
        serde_json::to_vec(&payload).expect("role profile snapshot is serializable"),
    ))
}

fn validate_selection(selection: &KnowledgeRoleSelection) -> Result<(), KnowledgeRoleProfileError> {
    if selection.model.len() > 200 || selection.model.chars().any(char::is_control) {
        return Err(KnowledgeRoleProfileError::InvalidInput(
            "model must contain at most 200 characters and no control characters".to_string(),
        ));
    }
    if matches!(selection.model_source, ModelSource::Local) != selection.llm_endpoint_id.is_some() {
        return Err(KnowledgeRoleProfileError::InvalidInput(
            "local model selection requires one registered endpoint and default selection forbids it"
                .to_string(),
        ));
    }
    Ok(())
}

fn normalized_bundle_id(
    scope: KnowledgeRoleProfileScope,
    bundle_id: Option<&str>,
) -> Result<&str, KnowledgeRoleProfileError> {
    match (scope, bundle_id) {
        (KnowledgeRoleProfileScope::Core, None | Some("")) => Ok(""),
        (KnowledgeRoleProfileScope::Bundle, Some(value))
            if !value.is_empty()
                && value.len() <= 128
                && value.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'.')
                }) =>
        {
            Ok(value)
        }
        (KnowledgeRoleProfileScope::Core, Some(_)) => Err(KnowledgeRoleProfileError::InvalidInput(
            "Core role profiles cannot name a Bundle".to_string(),
        )),
        (KnowledgeRoleProfileScope::Bundle, _) => Err(KnowledgeRoleProfileError::InvalidInput(
            "Bundle role profiles require a valid Bundle id".to_string(),
        )),
    }
}

fn validate_role_id(role_id: &str) -> Result<(), KnowledgeRoleProfileError> {
    if role_id.is_empty()
        || role_id.len() > 128
        || !role_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(KnowledgeRoleProfileError::InvalidInput(
            "role id must use 1..128 lowercase letters, digits, hyphens or underscores".to_string(),
        ));
    }
    Ok(())
}

fn row_to_override(
    row: RoleProfileDbRow,
) -> Result<KnowledgeRoleProfileOverride, KnowledgeRoleProfileError> {
    let scope = match row.scope_kind.as_str() {
        "core" => KnowledgeRoleProfileScope::Core,
        "bundle" => KnowledgeRoleProfileScope::Bundle,
        value => {
            return Err(KnowledgeRoleProfileError::InvalidPersisted(format!(
                "unknown scope {value:?}"
            )))
        }
    };
    let backend = AgentBackend::parse(&row.runtime_backend).ok_or_else(|| {
        KnowledgeRoleProfileError::InvalidPersisted(format!(
            "unknown backend {:?}",
            row.runtime_backend
        ))
    })?;
    let effort = AgentEffort::parse(&row.runtime_effort).ok_or_else(|| {
        KnowledgeRoleProfileError::InvalidPersisted(format!(
            "unknown effort {:?}",
            row.runtime_effort
        ))
    })?;
    let model_source = match row.runtime_model_source.as_str() {
        "default" => ModelSource::Default,
        "local" => ModelSource::Local,
        value => {
            return Err(KnowledgeRoleProfileError::InvalidPersisted(format!(
                "unknown model source {value:?}"
            )))
        }
    };
    Ok(KnowledgeRoleProfileOverride {
        id: row.id,
        tenant_id: row.tenant_id,
        scope,
        bundle_id: (!row.bundle_id.is_empty()).then_some(row.bundle_id),
        role_id: row.role_id,
        selection: KnowledgeRoleSelection {
            backend,
            model: row.runtime_model,
            effort,
            model_source,
            llm_endpoint_id: row.runtime_endpoint_id,
        },
        revision: row.revision,
        updated_by_user_id: row.updated_by_user_id,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

const fn model_source_str(source: ModelSource) -> &'static str {
    match source {
        ModelSource::Default => "default",
        ModelSource::Local => "local",
    }
}
