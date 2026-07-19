use gadgetron_core::agent::tools::GadgetDispatchContext;
use gadgetron_core::ingest::{BlobError, BlobId, BlobStore};
use gadgetron_xaas::knowledge_sources;
use gadgetron_xaas::knowledge_spaces::{KnowledgeSpaceError, SpaceActor};
use sqlx::PgPool;
use uuid::Uuid;

use crate::source::{extract_source, FilesystemBlobStore, SourceExtractError};
use crate::vault::TenantVaultLayout;

#[derive(Clone)]
pub(crate) struct ConversationSourceReader {
    pool: PgPool,
    layout: TenantVaultLayout,
}

#[derive(Debug, Clone)]
pub(crate) struct ConversationSourcePage {
    pub source_id: Uuid,
    pub source_revision: i64,
    pub object_id: Uuid,
    pub object_revision: i64,
    pub locator: String,
    pub requested_uri: Option<String>,
    pub content_hash: String,
    pub source_metadata: serde_json::Value,
    pub markdown: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConversationSourceError {
    #[error("invalid authenticated identity")]
    InvalidIdentity,
    #[error("the pinned conversation source is unavailable")]
    NotFound,
    #[error("the pinned conversation source changed after it became citation-ready")]
    RevisionChanged,
    #[error("conversation source ledger is unavailable")]
    Ledger(#[from] KnowledgeSpaceError),
    #[error("conversation source blob is unavailable")]
    Blob(#[from] BlobError),
    #[error("conversation source extraction failed")]
    Extraction(#[from] SourceExtractError),
}

impl ConversationSourceReader {
    pub(crate) fn new(pool: PgPool, layout: TenantVaultLayout) -> Self {
        Self { pool, layout }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn get(
        &self,
        context: &GadgetDispatchContext,
        conversation_id: Uuid,
        source_id: Uuid,
        source_revision: i64,
        object_id: Uuid,
        object_revision: i64,
        locator: &str,
    ) -> Result<ConversationSourcePage, ConversationSourceError> {
        let actor = parse_actor(context)?;
        let ready =
            knowledge_sources::list_ready_conversation_sources(&self.pool, actor, conversation_id)
                .await?;
        let (source, object) = ready
            .into_iter()
            .find(|(source, object)| {
                source.id == source_id
                    && source.revision == source_revision
                    && object.id == object_id
                    && object.revision == object_revision
                    && object.path == locator
            })
            .ok_or(ConversationSourceError::NotFound)?;
        let blob_id = source
            .blob_id
            .map(BlobId)
            .ok_or(ConversationSourceError::RevisionChanged)?;
        let content_type = source
            .content_type
            .as_deref()
            .ok_or(ConversationSourceError::RevisionChanged)?;
        let content_hash = source
            .content_hash
            .clone()
            .ok_or(ConversationSourceError::RevisionChanged)?;
        let store = FilesystemBlobStore::new(self.pool.clone(), self.layout.root());
        let bytes = store.get(&blob_id).await?;
        let extracted = extract_source(&bytes, content_type).await?;
        Ok(ConversationSourcePage {
            source_id: source.id,
            source_revision: source.revision,
            object_id: object.id,
            object_revision: object.revision,
            locator: object.path,
            requested_uri: source.requested_uri,
            content_hash,
            source_metadata: extracted.metadata,
            markdown: extracted.markdown,
        })
    }
}

fn parse_actor(context: &GadgetDispatchContext) -> Result<SpaceActor, ConversationSourceError> {
    let tenant_id = Uuid::parse_str(&context.tenant_id)
        .map_err(|_| ConversationSourceError::InvalidIdentity)?;
    let user_id =
        Uuid::parse_str(&context.actor_id).map_err(|_| ConversationSourceError::InvalidIdentity)?;
    Ok(SpaceActor { tenant_id, user_id })
}
