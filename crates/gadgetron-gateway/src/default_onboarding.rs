//! Canonical guide-note materialization for the default Team Space.

use std::{collections::BTreeMap, io::ErrorKind};

use gadgetron_knowledge::{
    source::serialize_obsidian_note,
    vault::{note_relative_path, TenantVaultLayout, VaultLayoutError},
};
use gadgetron_xaas::{
    default_onboarding::{
        DefaultTeamOnboarding, DEFAULT_TEAM_GUIDE_TITLE, DEFAULT_TEAM_HOME_BUNDLE_ID,
    },
    knowledge_sources,
    knowledge_spaces::{KnowledgeObjectListRow, KnowledgeSpaceError, SpaceActor},
};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultTeamGuide {
    pub tenant_id: uuid::Uuid,
    pub space_id: uuid::Uuid,
    pub vault_id: uuid::Uuid,
    pub object_id: uuid::Uuid,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultTeamGuideDocument {
    pub locale: &'static str,
    pub title: &'static str,
    pub body: &'static str,
}

pub const DEFAULT_TEAM_GUIDE_EN: DefaultTeamGuideDocument = DefaultTeamGuideDocument {
    locale: "en",
    title: DEFAULT_TEAM_GUIDE_TITLE,
    body: "# What this Space is for\n\nRecord the knowledge, decisions, and outcomes your Operations team shares here.\n\n- Operating procedures the team uses repeatedly\n- Decisions the team agreed on and their evidence\n- Verified outcomes and lessons from completed work\n",
};

pub const DEFAULT_TEAM_GUIDE_KO: DefaultTeamGuideDocument = DefaultTeamGuideDocument {
    locale: "ko",
    title: "이 공간에서 하는 일",
    body: "# 이 공간에서 하는 일\n\n운영팀이 함께 사용하는 지식과 결정, 실행 결과를 이곳에 기록하세요.\n\n- 반복해서 참고할 운영 절차\n- 팀이 합의한 결정과 근거\n- 실행 뒤 확인한 결과와 배운 점\n",
};

pub fn default_team_guide_document(locale: &str) -> DefaultTeamGuideDocument {
    if locale == "ko" {
        DEFAULT_TEAM_GUIDE_KO
    } else {
        DEFAULT_TEAM_GUIDE_EN
    }
}

pub fn apply_default_team_guide_titles(objects: &mut [KnowledgeObjectListRow]) {
    for object in objects {
        if object.title.is_some()
            || object.id != object.space_id
            || object.home_bundle_id != DEFAULT_TEAM_HOME_BUNDLE_ID
        {
            continue;
        }
        object.title =
            default_team_guide_title_for_path(&object.path, object.id).map(str::to_owned);
    }
}

fn default_team_guide_title_for_path(path: &str, object_id: uuid::Uuid) -> Option<&'static str> {
    [DEFAULT_TEAM_GUIDE_EN, DEFAULT_TEAM_GUIDE_KO]
        .into_iter()
        .find(|document| note_relative_path(document.title, object_id) == path)
        .map(|document| document.title)
}

#[derive(Debug, thiserror::Error)]
pub enum DefaultOnboardingError {
    #[error("default Team guide registry failed: {0}")]
    Registry(#[from] KnowledgeSpaceError),
    #[error("default Team guide Vault reconciliation failed: {0}")]
    Vault(#[from] VaultLayoutError),
    #[error("default Team guide serialization failed: {0}")]
    Format(#[from] gadgetron_knowledge::source::NoteFormatError),
    #[error("default Team guide topology is inconsistent: {0}")]
    Inconsistent(String),
}

pub async fn ensure_default_team_guides(
    pool: &sqlx::PgPool,
    layout: &TenantVaultLayout,
    topologies: &[DefaultTeamOnboarding],
) -> Result<Vec<DefaultTeamGuide>, DefaultOnboardingError> {
    let mut guides = Vec::with_capacity(topologies.len());
    for topology in topologies {
        guides.push(ensure_default_team_guide(pool, layout, topology).await?);
    }
    Ok(guides)
}

pub async fn ensure_default_team_guide(
    pool: &sqlx::PgPool,
    layout: &TenantVaultLayout,
    topology: &DefaultTeamOnboarding,
) -> Result<DefaultTeamGuide, DefaultOnboardingError> {
    let actor = SpaceActor {
        tenant_id: topology.tenant_id,
        user_id: topology.service_actor_id,
    };
    let object_id = topology.space_id;
    let path = note_relative_path(DEFAULT_TEAM_GUIDE_TITLE, object_id);
    let legacy_korean_path = note_relative_path(DEFAULT_TEAM_GUIDE_KO.title, object_id);
    let mut materialized_path = path.clone();
    let repository = layout.open_or_init(topology.tenant_id)?;
    repository.ensure_domain(topology.space_id, DEFAULT_TEAM_HOME_BUNDLE_ID)?;

    match knowledge_sources::note_location(
        pool,
        actor,
        object_id,
        gadgetron_xaas::knowledge_spaces::SpaceRole::Contributor,
        true,
    )
    .await
    {
        Ok(location) => {
            if location.vault_id != topology.vault_id
                || location.space_id != topology.space_id
                || location.home_bundle_id != DEFAULT_TEAM_HOME_BUNDLE_ID
                || (location.path != path && location.path != legacy_korean_path)
            {
                return Err(DefaultOnboardingError::Inconsistent(format!(
                    "guide object {object_id} is registered outside the default Team Vault"
                )));
            }
            materialized_path = location.path.clone();
            let document_locale = if materialized_path == legacy_korean_path {
                "ko"
            } else {
                "en"
            };
            match repository.read_note_reconciled(
                topology.space_id,
                DEFAULT_TEAM_HOME_BUNDLE_ID,
                &materialized_path,
                location.content_hash.as_deref(),
            ) {
                Ok(note) => {
                    if note.externally_changed {
                        knowledge_sources::update_note_hash_system(
                            pool,
                            topology.tenant_id,
                            object_id,
                            location.revision,
                            &note.content_hash,
                        )
                        .await?;
                    }
                }
                Err(VaultLayoutError::Io(error)) if error.kind() == ErrorKind::NotFound => {
                    let raw = guide_note(topology, object_id, document_locale)?;
                    let note = repository.write_note(
                        topology.space_id,
                        DEFAULT_TEAM_HOME_BUNDLE_ID,
                        &materialized_path,
                        raw.as_bytes(),
                        "vault: restore default Team guide",
                    )?;
                    knowledge_sources::update_note_hash_system(
                        pool,
                        topology.tenant_id,
                        object_id,
                        location.revision,
                        &note.content_hash,
                    )
                    .await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(KnowledgeSpaceError::NotFound) => {
            let raw = guide_note(topology, object_id, "en")?;
            let content_hash = hex::encode(Sha256::digest(raw.as_bytes()));
            knowledge_sources::register_manual_note(
                pool,
                actor,
                topology.vault_id,
                object_id,
                &path,
                Some(&content_hash),
            )
            .await?;
            // Keep the registry row active on a filesystem failure. Startup
            // remains failed and the next idempotent run restores the bytes.
            repository.write_note(
                topology.space_id,
                DEFAULT_TEAM_HOME_BUNDLE_ID,
                &path,
                raw.as_bytes(),
                "vault: create default Team guide",
            )?;
        }
        Err(error) => return Err(error.into()),
    }

    Ok(DefaultTeamGuide {
        tenant_id: topology.tenant_id,
        space_id: topology.space_id,
        vault_id: topology.vault_id,
        object_id,
        path: materialized_path,
    })
}

fn guide_note(
    topology: &DefaultTeamOnboarding,
    object_id: uuid::Uuid,
    locale: &str,
) -> Result<String, gadgetron_knowledge::source::NoteFormatError> {
    let document = default_team_guide_document(locale);
    let now = chrono::Utc::now().to_rfc3339();
    let mut properties = BTreeMap::new();
    properties.insert("id".to_string(), serde_json::json!(object_id));
    properties.insert("title".to_string(), serde_json::json!(document.title));
    properties.insert("kind".to_string(), serde_json::json!("note"));
    properties.insert("locale".to_string(), serde_json::json!(document.locale));
    properties.insert("status".to_string(), serde_json::json!("draft"));
    properties.insert("space_id".to_string(), serde_json::json!(topology.space_id));
    properties.insert(
        "home_bundle_id".to_string(),
        serde_json::json!(DEFAULT_TEAM_HOME_BUNDLE_ID),
    );
    properties.insert("source_ids".to_string(), serde_json::json!([]));
    properties.insert("source_hashes".to_string(), serde_json::json!([]));
    properties.insert("created".to_string(), serde_json::json!(now.clone()));
    properties.insert("updated".to_string(), serde_json::json!(now));
    serialize_obsidian_note(&properties, document.body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_documents_are_locale_specific_and_english_default() {
        assert_eq!(default_team_guide_document("en"), DEFAULT_TEAM_GUIDE_EN);
        assert_eq!(
            default_team_guide_document("unsupported"),
            DEFAULT_TEAM_GUIDE_EN
        );
        assert_eq!(default_team_guide_document("ko"), DEFAULT_TEAM_GUIDE_KO);
        assert_ne!(DEFAULT_TEAM_GUIDE_EN.title, DEFAULT_TEAM_GUIDE_KO.title);
        assert!(DEFAULT_TEAM_GUIDE_EN
            .body
            .starts_with("# What this Space is for"));
        assert!(DEFAULT_TEAM_GUIDE_KO
            .body
            .starts_with("# 이 공간에서 하는 일"));
    }

    #[test]
    fn guide_titles_are_recovered_from_canonical_and_legacy_paths() {
        let object_id = uuid::Uuid::parse_str("8f06b574-1234-5678-9012-345678901234").unwrap();
        for document in [DEFAULT_TEAM_GUIDE_EN, DEFAULT_TEAM_GUIDE_KO] {
            let path = note_relative_path(document.title, object_id);
            assert_eq!(
                default_team_guide_title_for_path(&path, object_id),
                Some(document.title)
            );
        }
        assert_eq!(
            default_team_guide_title_for_path("notes/unrelated--8f06b574.md", object_id),
            None
        );
    }
}
