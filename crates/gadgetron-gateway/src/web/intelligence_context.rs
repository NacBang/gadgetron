use std::{collections::BTreeSet, sync::Arc};

use chrono::{DateTime, Utc};
use gadgetron_bundle_sdk::{
    BundleId, CitationRole, CitationUseRef, ContextCoverage, IntelligenceAuthority,
    IntelligenceQuery, IntelligenceQueryDraft, KnowledgeCitation, KnowledgeContextPack,
    OutcomeFeedbackDraft, OutcomeFeedbackReceipt,
};
use gadgetron_knowledge::vault::TenantVaultLayout;
use gadgetron_xaas::{
    knowledge_sources,
    knowledge_spaces::{self, SpaceActor},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

const MAX_CONTEXT_CANDIDATES: i64 = 200;
const MAX_EXPERIENCE_CANDIDATES: i64 = 200;
const MAX_EXPERIENCE_CITATIONS: usize = 4;
const MAX_PASSAGE_BYTES: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntelligenceActorBinding {
    pub tenant_id: Uuid,
    pub actor_id: Uuid,
    pub authority_actor_id: Option<Uuid>,
    pub acting_space_id: Option<Uuid>,
}

#[derive(Debug, thiserror::Error)]
pub enum IntelligenceContextError {
    #[error("Intelligence context input is invalid: {0}")]
    Invalid(String),
    #[error("Intelligence context is not visible to this actor")]
    Forbidden,
    #[error("Intelligence context dependencies are unavailable")]
    Unavailable,
    #[error("Intelligence context id or revision conflicts with an existing exchange")]
    Conflict,
    #[error("Intelligence context persistence failed")]
    Persistence,
}

#[derive(Debug, Clone)]
pub struct IntelligenceContextService {
    pool: PgPool,
    vault_layout: Arc<TenantVaultLayout>,
}

#[derive(Debug, sqlx::FromRow)]
struct ContextCandidateRow {
    object_id: Uuid,
    object_revision: i64,
    canonical_kind: String,
    path: String,
    content_hash: Option<String>,
    updated_at: DateTime<Utc>,
    space_id: Uuid,
    home_bundle_id: String,
    owner_state: String,
    source_id: Option<Uuid>,
    source_revision: Option<i64>,
    source_observed_at: Option<DateTime<Utc>>,
    title: String,
    exact_subject: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct ExperienceCandidateRow {
    feedback_id: String,
    experience_revision: String,
    subject_revision: String,
    verification_summary: String,
    used_citations: serde_json::Value,
    created_at: DateTime<Utc>,
}

struct AnchoredExperience {
    source_citation_id: String,
    source_revision: String,
    citation: KnowledgeCitation,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ContextExchangeSummary {
    pub id: Uuid,
    pub consumer_bundle_id: String,
    pub query_id: String,
    pub subject_owner_bundle: String,
    pub subject_kind: String,
    pub subject_stable_id: String,
    pub subject_revision: String,
    pub question: String,
    pub context_revision: String,
    pub coverage: String,
    pub citation_count: i32,
    pub gap_count: i32,
    pub pack_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OutcomeFeedbackSummary {
    pub id: Uuid,
    pub consumer_bundle_id: String,
    pub feedback_id: String,
    pub experience_revision: String,
    pub subject_owner_bundle: String,
    pub subject_kind: String,
    pub subject_stable_id: String,
    pub subject_revision: String,
    pub operation_id: String,
    pub context_query_id: Option<String>,
    pub context_revision: Option<String>,
    pub predicate_result: String,
    pub verification_summary: String,
    pub before_state: serde_json::Value,
    pub after_state: serde_json::Value,
    pub used_citations: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl IntelligenceContextService {
    pub fn new(pool: PgPool, vault_layout: Arc<TenantVaultLayout>) -> Self {
        Self { pool, vault_layout }
    }

    pub async fn resolve(
        &self,
        binding: IntelligenceActorBinding,
        consumer_bundle: &BundleId,
        draft: IntelligenceQueryDraft,
    ) -> Result<KnowledgeContextPack, IntelligenceContextError> {
        draft
            .validate()
            .map_err(|error| IntelligenceContextError::Invalid(error.to_string()))?;
        let authority = self.authority(binding).await?;
        let query = draft
            .bind(authority)
            .map_err(|error| IntelligenceContextError::Invalid(error.to_string()))?;
        let query_json =
            serde_json::to_value(&query).map_err(|_| IntelligenceContextError::Persistence)?;
        if let Some((existing_query, existing_pack)) =
            sqlx::query_as::<_, (serde_json::Value, serde_json::Value)>(
                r#"SELECT query_json, pack_json FROM knowledge_context_exchanges
               WHERE tenant_id = $1 AND consumer_bundle_id = $2 AND query_id = $3"#,
            )
            .bind(binding.tenant_id)
            .bind(consumer_bundle.as_str())
            .bind(&query.query_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| IntelligenceContextError::Persistence)?
        {
            if existing_query != query_json {
                return Err(IntelligenceContextError::Conflict);
            }
            let pack: KnowledgeContextPack = serde_json::from_value(existing_pack)
                .map_err(|_| IntelligenceContextError::Persistence)?;
            pack.validate_for_query(&query)
                .map_err(|_| IntelligenceContextError::Persistence)?;
            return Ok(pack);
        }

        let candidates = self.candidates(&query, binding.acting_space_id).await?;
        let tokens = query_tokens(&query.question);
        let repository = self
            .vault_layout
            .open_or_init(binding.tenant_id)
            .map_err(|_| IntelligenceContextError::Unavailable)?;
        let now = Utc::now();
        let mut ranked = Vec::new();
        let mut stale_matches = 0_usize;
        let mut unreadable_candidates = 0_usize;
        for candidate in candidates {
            let note = match repository.read_note_reconciled(
                candidate.space_id,
                &candidate.home_bundle_id,
                &candidate.path,
                candidate.content_hash.as_deref(),
            ) {
                Ok(note) => note,
                Err(_) => {
                    unreadable_candidates += 1;
                    continue;
                }
            };
            let object_revision = if note.externally_changed {
                knowledge_sources::update_note_hash_system(
                    &self.pool,
                    binding.tenant_id,
                    candidate.object_id,
                    candidate.object_revision,
                    &note.content_hash,
                )
                .await
                .map_err(|_| IntelligenceContextError::Conflict)?
                .revision
            } else {
                candidate.object_revision
            };
            let text = String::from_utf8(note.bytes)
                .map_err(|_| IntelligenceContextError::Invalid("Vault note is not UTF-8".into()))?;
            let passage = bounded_passage(note_body(&text));
            let score = if candidate.exact_subject {
                usize::MAX / 2
            } else {
                match_score(&tokens, &candidate.title, &passage)
            };
            if score == 0 {
                continue;
            }
            let observed_at = candidate.source_observed_at.unwrap_or(candidate.updated_at);
            let freshness_seconds =
                now.signed_duration_since(observed_at).num_seconds().max(0) as u64;
            if !candidate.exact_subject && freshness_seconds > query.max_freshness_seconds {
                stale_matches += 1;
                continue;
            }
            let role = citation_role(&passage);
            if passage.is_empty() {
                continue;
            }
            ranked.push((
                score,
                candidate.updated_at,
                KnowledgeCitation::new(
                    format!("{}:{object_revision}", candidate.object_id),
                    candidate.space_id.to_string(),
                    BundleId::new(candidate.home_bundle_id.clone())
                        .map_err(|error| IntelligenceContextError::Invalid(error.to_string()))?,
                    candidate
                        .source_id
                        .unwrap_or(candidate.object_id)
                        .to_string(),
                    candidate
                        .source_revision
                        .unwrap_or(object_revision)
                        .to_string(),
                    passage,
                    role,
                    format!(
                        "{} · {} · {}",
                        candidate.title, candidate.canonical_kind, candidate.owner_state
                    ),
                    freshness_seconds,
                    Some(note.content_hash),
                ),
            ));
        }
        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.cmp(&left.1))
                .then_with(|| left.2.citation_id.cmp(&right.2.citation_id))
        });
        retain_relevant_candidates(&mut ranked);
        let citation_limit = usize::try_from(query.budget.max_sources)
            .unwrap_or(usize::MAX)
            .min(256);
        let (experiences, stale_experiences, invalid_experiences) = self
            .verified_experiences(&query, consumer_bundle, &ranked, now)
            .await?;
        let citations = interleave_experiences(ranked, experiences, citation_limit);
        let (coverage, gaps) = if citations.is_empty() {
            let gap = if stale_matches > 0 {
                "Matching knowledge is older than the requested freshness window"
            } else if unreadable_candidates > 0 {
                "No readable cited knowledge matched; one or more Vault objects need reconciliation"
            } else {
                "No cited knowledge matched the question in the actor's visible Spaces"
            };
            (ContextCoverage::Unavailable, vec![gap.to_string()])
        } else {
            let mut gaps = vec![
                "Context is limited to keyword-matched, revision-pinned Vault knowledge"
                    .to_string(),
            ];
            if stale_matches > 0 {
                gaps.push(format!(
                    "{stale_matches} matching item(s) were excluded by the freshness window"
                ));
            }
            if unreadable_candidates > 0 {
                gaps.push(format!(
                    "{unreadable_candidates} Vault object(s) were unavailable and excluded"
                ));
            }
            if stale_experiences > 0 {
                gaps.push(format!(
                    "{stale_experiences} verified outcome(s) were excluded by the freshness window"
                ));
            }
            if invalid_experiences > 0 {
                gaps.push(format!(
                    "{invalid_experiences} outcome feedback row(s) failed integrity or citation validation"
                ));
            }
            (ContextCoverage::Partial, gaps)
        };
        let pack = fit_pack_to_budget(&query, coverage, citations, gaps)?;
        let pack_json =
            serde_json::to_value(&pack).map_err(|_| IntelligenceContextError::Persistence)?;
        sqlx::query(
            r#"INSERT INTO knowledge_context_exchanges
               (tenant_id, actor_user_id, consumer_bundle_id, query_id,
                subject_owner_bundle, subject_kind, subject_stable_id, subject_revision,
                question, context_revision, coverage, citation_count, gap_count,
                query_json, pack_json)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)"#,
        )
        .bind(binding.tenant_id)
        .bind(binding.actor_id)
        .bind(consumer_bundle.as_str())
        .bind(&query.query_id)
        .bind(query.subject.owner_bundle.as_str())
        .bind(query.subject.kind.as_str())
        .bind(&query.subject.stable_id)
        .bind(&query.subject.revision)
        .bind(&query.question)
        .bind(&pack.context_revision)
        .bind(coverage_name(pack.coverage))
        .bind(pack.citations.len() as i32)
        .bind(pack.gaps.len() as i32)
        .bind(query_json)
        .bind(pack_json)
        .execute(&self.pool)
        .await
        .map_err(|_| IntelligenceContextError::Persistence)?;
        Ok(pack)
    }

    pub async fn record_feedback(
        &self,
        binding: IntelligenceActorBinding,
        consumer_bundle: &BundleId,
        draft: OutcomeFeedbackDraft,
    ) -> Result<OutcomeFeedbackReceipt, IntelligenceContextError> {
        let authority = if let Some(context) = &draft.context {
            let (query_json, pack_json): (serde_json::Value, serde_json::Value) = sqlx::query_as(
                r#"SELECT query_json, pack_json FROM knowledge_context_exchanges
                   WHERE tenant_id = $1 AND actor_user_id = $2 AND consumer_bundle_id = $3
                     AND query_id = $4 AND context_revision = $5"#,
            )
            .bind(binding.tenant_id)
            .bind(binding.actor_id)
            .bind(consumer_bundle.as_str())
            .bind(&context.query_id)
            .bind(&context.context_revision)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| IntelligenceContextError::Persistence)?
            .ok_or(IntelligenceContextError::Forbidden)?;
            let query: IntelligenceQuery = serde_json::from_value(query_json)
                .map_err(|_| IntelligenceContextError::Persistence)?;
            let pack: KnowledgeContextPack = serde_json::from_value(pack_json)
                .map_err(|_| IntelligenceContextError::Persistence)?;
            if query.subject != draft.subject {
                return Err(IntelligenceContextError::Conflict);
            }
            for used in &draft.used_citations {
                if !pack.citations.iter().any(|citation| {
                    citation.citation_id == used.citation_id
                        && citation.source_revision == used.source_revision
                }) {
                    return Err(IntelligenceContextError::Conflict);
                }
            }
            query.authority
        } else {
            self.authority(binding).await?
        };
        let feedback = draft
            .bind(authority)
            .map_err(|error| IntelligenceContextError::Invalid(error.to_string()))?;
        let feedback_json =
            serde_json::to_value(&feedback).map_err(|_| IntelligenceContextError::Persistence)?;
        let experience_revision = format!(
            "sha256:{}",
            hex::encode(Sha256::digest(
                serde_json::to_vec(&feedback_json)
                    .map_err(|_| IntelligenceContextError::Persistence)?
            ))
        );
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            r#"SELECT experience_revision FROM knowledge_outcome_feedback
               WHERE tenant_id = $1 AND consumer_bundle_id = $2 AND feedback_id = $3"#,
        )
        .bind(binding.tenant_id)
        .bind(consumer_bundle.as_str())
        .bind(&feedback.feedback_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| IntelligenceContextError::Persistence)?
        {
            if existing != experience_revision {
                return Err(IntelligenceContextError::Conflict);
            }
            return OutcomeFeedbackReceipt::new(feedback.feedback_id, experience_revision, true)
                .map_err(|_| IntelligenceContextError::Persistence);
        }
        sqlx::query(
            r#"INSERT INTO knowledge_outcome_feedback
               (tenant_id, actor_user_id, consumer_bundle_id, feedback_id,
                experience_revision, subject_owner_bundle, subject_kind,
                subject_stable_id, subject_revision, operation_id,
                context_query_id, context_revision, predicate_result,
                verification_summary, before_state, after_state, used_citations,
                feedback_json)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)"#,
        )
        .bind(binding.tenant_id)
        .bind(binding.actor_id)
        .bind(consumer_bundle.as_str())
        .bind(&feedback.feedback_id)
        .bind(&experience_revision)
        .bind(feedback.subject.owner_bundle.as_str())
        .bind(feedback.subject.kind.as_str())
        .bind(&feedback.subject.stable_id)
        .bind(&feedback.subject.revision)
        .bind(&feedback.operation_id)
        .bind(feedback.context.as_ref().map(|context| &context.query_id))
        .bind(
            feedback
                .context
                .as_ref()
                .map(|context| &context.context_revision),
        )
        .bind(predicate_name(feedback.predicate_result))
        .bind(&feedback.verification_summary)
        .bind(&feedback.before_state)
        .bind(&feedback.after_state)
        .bind(
            serde_json::to_value(&feedback.used_citations)
                .map_err(|_| IntelligenceContextError::Persistence)?,
        )
        .bind(feedback_json)
        .execute(&self.pool)
        .await
        .map_err(|_| IntelligenceContextError::Persistence)?;
        OutcomeFeedbackReceipt::new(feedback.feedback_id, experience_revision, false)
            .map_err(|_| IntelligenceContextError::Persistence)
    }

    async fn authority(
        &self,
        binding: IntelligenceActorBinding,
    ) -> Result<IntelligenceAuthority, IntelligenceContextError> {
        if binding.authority_actor_id.is_some() && binding.acting_space_id.is_none() {
            return Err(IntelligenceContextError::Forbidden);
        }
        let execution_actor = SpaceActor {
            tenant_id: binding.tenant_id,
            user_id: binding.actor_id,
        };
        let execution_space_ids: BTreeSet<_> =
            knowledge_spaces::effective_spaces(&self.pool, execution_actor)
                .await
                .map_err(map_space_error)?
                .into_iter()
                .filter(|space| space.space.status == "active")
                .map(|space| space.space.id)
                .collect();
        let authority_actor_id = binding.authority_actor_id.unwrap_or(binding.actor_id);
        let authority_space_ids = if authority_actor_id == binding.actor_id {
            execution_space_ids.clone()
        } else {
            knowledge_spaces::effective_spaces(
                &self.pool,
                SpaceActor {
                    tenant_id: binding.tenant_id,
                    user_id: authority_actor_id,
                },
            )
            .await
            .map_err(map_space_error)?
            .into_iter()
            .filter(|space| space.space.status == "active")
            .map(|space| space.space.id)
            .collect()
        };
        let mut allowed_space_ids = authority_space_ids.clone();
        if let Some(acting_space_id) = binding.acting_space_id {
            if !execution_space_ids.contains(&acting_space_id)
                || !authority_space_ids.contains(&acting_space_id)
            {
                return Err(IntelligenceContextError::Forbidden);
            }
            let visible: Vec<_> = authority_space_ids.iter().copied().collect();
            let referenced_source_spaces: Vec<Uuid> = sqlx::query_scalar(
                r#"SELECT DISTINCT sh.source_space_id
                   FROM knowledge_shares sh
                   JOIN knowledge_objects o
                     ON o.tenant_id = sh.tenant_id AND o.id = sh.source_object_id
                   WHERE sh.tenant_id = $1 AND sh.target_space_id = $2
                     AND sh.mode = 'reference' AND sh.revoked_at IS NULL
                     AND sh.source_space_id = ANY($3) AND o.status = 'active'
                     AND sh.policy_disposition IN ('allowed', 'reviewed')
                   ORDER BY sh.source_space_id"#,
            )
            .bind(binding.tenant_id)
            .bind(acting_space_id)
            .bind(&visible)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| IntelligenceContextError::Persistence)?;
            allowed_space_ids.clear();
            allowed_space_ids.insert(acting_space_id);
            allowed_space_ids.extend(referenced_source_spaces);
        }
        let mut allowed_space_ids: Vec<_> = allowed_space_ids
            .into_iter()
            .map(|space_id| space_id.to_string())
            .collect();
        allowed_space_ids.sort();
        if allowed_space_ids.is_empty() {
            return Err(IntelligenceContextError::Forbidden);
        }
        IntelligenceAuthority::new(
            binding.tenant_id.to_string(),
            binding.actor_id.to_string(),
            allowed_space_ids,
        )
        .map_err(|error| IntelligenceContextError::Invalid(error.to_string()))
    }

    async fn candidates(
        &self,
        query: &IntelligenceQuery,
        acting_space_id: Option<Uuid>,
    ) -> Result<Vec<ContextCandidateRow>, IntelligenceContextError> {
        let spaces: Vec<Uuid> = query
            .authority
            .allowed_space_ids
            .iter()
            .map(|value| {
                Uuid::parse_str(value)
                    .map_err(|_| IntelligenceContextError::Invalid("Space id is not a UUID".into()))
            })
            .collect::<Result<_, _>>()?;
        let limit = i64::from(query.budget.max_items.min(MAX_CONTEXT_CANDIDATES as u32));
        if let Some(acting_space_id) = acting_space_id {
            return sqlx::query_as::<_, ContextCandidateRow>(
                r#"SELECT o.id AS object_id, o.revision AS object_revision,
                          o.canonical_kind, o.path, o.content_hash, o.updated_at,
                          v.space_id, v.home_bundle_id, v.owner_state,
                          s.id AS source_id, s.revision AS source_revision,
                          COALESCE(s.fetched_at, s.updated_at) AS source_observed_at,
                          COALESCE(NULLIF(s.title, ''), n.title, o.path) AS title,
                          COALESCE((o.originating_owner_bundle = $4
                           AND o.originating_subject_kind = $5
                           AND o.originating_subject_id = $6
                           AND o.originating_subject_revision = $7), FALSE) AS exact_subject
                   FROM knowledge_objects o
                   JOIN knowledge_vaults v
                     ON v.tenant_id = o.tenant_id AND v.id = o.vault_id
                   LEFT JOIN knowledge_sources s
                     ON s.tenant_id = o.tenant_id AND s.id = o.source_id
                   LEFT JOIN knowledge_graph_generations g
                     ON g.tenant_id = o.tenant_id AND g.state = 'active'
                   LEFT JOIN knowledge_graph_nodes n
                     ON n.tenant_id = o.tenant_id AND n.generation_id = g.id
                    AND n.stable_node_id = 'note:' || o.id::TEXT
                   WHERE o.tenant_id = $1 AND o.status = 'active'
                     AND o.canonical_kind IN ('note', 'lesson', 'insight')
                     AND (
                       v.space_id = $2 OR EXISTS (
                         SELECT 1 FROM knowledge_shares sh
                         WHERE sh.tenant_id = o.tenant_id
                           AND sh.source_object_id = o.id
                           AND sh.source_space_id = v.space_id
                           AND sh.target_space_id = $2
                           AND sh.mode = 'reference' AND sh.revoked_at IS NULL
                           AND sh.policy_disposition IN ('allowed', 'reviewed')
                           AND sh.source_space_id = ANY($3)
                       )
                     )
                   ORDER BY exact_subject DESC, o.updated_at DESC, o.id
                   LIMIT $8"#,
            )
            .bind(
                Uuid::parse_str(&query.authority.tenant_id).map_err(|_| {
                    IntelligenceContextError::Invalid("tenant id is not a UUID".into())
                })?,
            )
            .bind(acting_space_id)
            .bind(&spaces)
            .bind(query.subject.owner_bundle.as_str())
            .bind(query.subject.kind.as_str())
            .bind(&query.subject.stable_id)
            .bind(&query.subject.revision)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| IntelligenceContextError::Persistence);
        }
        sqlx::query_as::<_, ContextCandidateRow>(
            r#"SELECT o.id AS object_id, o.revision AS object_revision,
                      o.canonical_kind, o.path, o.content_hash, o.updated_at,
                      v.space_id, v.home_bundle_id, v.owner_state,
                      s.id AS source_id, s.revision AS source_revision,
                      COALESCE(s.fetched_at, s.updated_at) AS source_observed_at,
                      COALESCE(NULLIF(s.title, ''), n.title, o.path) AS title,
                      COALESCE((o.originating_owner_bundle = $3
                       AND o.originating_subject_kind = $4
                       AND o.originating_subject_id = $5
                       AND o.originating_subject_revision = $6), FALSE) AS exact_subject
               FROM knowledge_objects o
               JOIN knowledge_vaults v
                 ON v.tenant_id = o.tenant_id AND v.id = o.vault_id
               LEFT JOIN knowledge_sources s
                 ON s.tenant_id = o.tenant_id AND s.id = o.source_id
               LEFT JOIN knowledge_graph_generations g
                 ON g.tenant_id = o.tenant_id AND g.state = 'active'
               LEFT JOIN knowledge_graph_nodes n
                 ON n.tenant_id = o.tenant_id AND n.generation_id = g.id
                AND n.stable_node_id = 'note:' || o.id::TEXT
               WHERE o.tenant_id = $1 AND v.space_id = ANY($2)
                 AND o.status = 'active'
                 AND o.canonical_kind IN ('note', 'lesson', 'insight')
               ORDER BY exact_subject DESC, o.updated_at DESC, o.id
               LIMIT $7"#,
        )
        .bind(
            Uuid::parse_str(&query.authority.tenant_id)
                .map_err(|_| IntelligenceContextError::Invalid("tenant id is not a UUID".into()))?,
        )
        .bind(&spaces)
        .bind(query.subject.owner_bundle.as_str())
        .bind(query.subject.kind.as_str())
        .bind(&query.subject.stable_id)
        .bind(&query.subject.revision)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| IntelligenceContextError::Persistence)
    }

    async fn verified_experiences(
        &self,
        query: &IntelligenceQuery,
        consumer_bundle: &BundleId,
        ranked_sources: &[(usize, DateTime<Utc>, KnowledgeCitation)],
        now: DateTime<Utc>,
    ) -> Result<(Vec<AnchoredExperience>, usize, usize), IntelligenceContextError> {
        if ranked_sources.is_empty() {
            return Ok((Vec::new(), 0, 0));
        }
        let tenant_id = Uuid::parse_str(&query.authority.tenant_id)
            .map_err(|_| IntelligenceContextError::Invalid("tenant id is not a UUID".into()))?;
        let rows = sqlx::query_as::<_, ExperienceCandidateRow>(
            r#"SELECT feedback_id, experience_revision, subject_revision,
                      verification_summary, used_citations, created_at
               FROM knowledge_outcome_feedback
               WHERE tenant_id = $1 AND consumer_bundle_id = $2
                 AND subject_owner_bundle = $3 AND subject_kind = $4
                 AND subject_stable_id = $5 AND predicate_result = 'satisfied'
                 AND jsonb_typeof(used_citations) = 'array'
                 AND jsonb_array_length(used_citations) > 0
               ORDER BY created_at DESC, id DESC
               LIMIT $6"#,
        )
        .bind(tenant_id)
        .bind(consumer_bundle.as_str())
        .bind(query.subject.owner_bundle.as_str())
        .bind(query.subject.kind.as_str())
        .bind(&query.subject.stable_id)
        .bind(MAX_EXPERIENCE_CANDIDATES)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| IntelligenceContextError::Persistence)?;

        let mut anchored = Vec::new();
        let mut used_sources = BTreeSet::new();
        let mut stale = 0_usize;
        let mut invalid = 0_usize;
        for row in rows {
            if anchored.len() >= MAX_EXPERIENCE_CITATIONS {
                break;
            }
            let Ok(used_citations) =
                serde_json::from_value::<Vec<CitationUseRef>>(row.used_citations)
            else {
                continue;
            };
            let mut matching_sources: Vec<_> = ranked_sources
                .iter()
                .map(|(_, _, citation)| citation)
                .filter(|citation| {
                    used_citations.iter().any(|used| {
                        used.citation_id == citation.citation_id
                            && used.source_revision == citation.source_revision
                    })
                })
                .collect();
            matching_sources.sort_by(|left, right| {
                left.citation_id
                    .cmp(&right.citation_id)
                    .then_with(|| left.source_revision.cmp(&right.source_revision))
            });
            let Some(source) = matching_sources.into_iter().find(|citation| {
                !used_sources.contains(&(
                    citation.citation_id.clone(),
                    citation.source_revision.clone(),
                ))
            }) else {
                continue;
            };
            let freshness_seconds = now
                .signed_duration_since(row.created_at)
                .num_seconds()
                .max(0) as u64;
            if freshness_seconds > query.max_freshness_seconds {
                stale += 1;
                continue;
            }
            let Some(digest) = experience_digest(&row.experience_revision).map(str::to_owned)
            else {
                invalid += 1;
                continue;
            };
            used_sources.insert((source.citation_id.clone(), source.source_revision.clone()));
            anchored.push(AnchoredExperience {
                source_citation_id: source.citation_id.clone(),
                source_revision: source.source_revision.clone(),
                citation: KnowledgeCitation::new(
                    format!("experience:{digest}"),
                    source.space_id.clone(),
                    consumer_bundle.clone(),
                    row.feedback_id,
                    row.experience_revision,
                    format!(
                        "Verified operational outcome: {}",
                        row.verification_summary.trim()
                    ),
                    CitationRole::Context,
                    format!(
                        "Verified operational experience · {}.{} {} · subject revision {} · exact source {}",
                        query.subject.owner_bundle.as_str(),
                        query.subject.kind.as_str(),
                        query.subject.stable_id,
                        row.subject_revision,
                        source.citation_id
                    ),
                    freshness_seconds,
                    Some(digest),
                ),
            });
        }
        Ok((anchored, stale, invalid))
    }

    pub async fn list_exchanges(
        &self,
        binding: IntelligenceActorBinding,
        space_id: Uuid,
        limit: i64,
    ) -> Result<Vec<ContextExchangeSummary>, IntelligenceContextError> {
        self.require_visible_space(binding, space_id).await?;
        sqlx::query_as::<_, ContextExchangeSummary>(
            r#"SELECT id, consumer_bundle_id, query_id, subject_owner_bundle,
                      subject_kind, subject_stable_id, subject_revision, question,
                      context_revision, coverage, citation_count, gap_count,
                      pack_json, created_at
               FROM knowledge_context_exchanges
               WHERE tenant_id = $1 AND actor_user_id = $2
                 AND query_json->'authority'->'allowed_space_ids' ? $3
               ORDER BY created_at DESC, id DESC LIMIT $4"#,
        )
        .bind(binding.tenant_id)
        .bind(binding.actor_id)
        .bind(space_id.to_string())
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| IntelligenceContextError::Persistence)
    }

    pub async fn list_feedback(
        &self,
        binding: IntelligenceActorBinding,
        space_id: Uuid,
        limit: i64,
    ) -> Result<Vec<OutcomeFeedbackSummary>, IntelligenceContextError> {
        self.require_visible_space(binding, space_id).await?;
        sqlx::query_as::<_, OutcomeFeedbackSummary>(
            r#"SELECT f.id, f.consumer_bundle_id, f.feedback_id,
                      f.experience_revision, f.subject_owner_bundle, f.subject_kind,
                      f.subject_stable_id, f.subject_revision, f.operation_id,
                      f.context_query_id, f.context_revision, f.predicate_result,
                      f.verification_summary, f.before_state, f.after_state,
                      f.used_citations, f.created_at
               FROM knowledge_outcome_feedback f
               WHERE f.tenant_id = $1 AND f.actor_user_id = $2
                 AND f.feedback_json->'authority'->'allowed_space_ids' ? $3
               ORDER BY f.created_at DESC, f.id DESC LIMIT $4"#,
        )
        .bind(binding.tenant_id)
        .bind(binding.actor_id)
        .bind(space_id.to_string())
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| IntelligenceContextError::Persistence)
    }

    async fn require_visible_space(
        &self,
        binding: IntelligenceActorBinding,
        space_id: Uuid,
    ) -> Result<(), IntelligenceContextError> {
        let authority = self.authority(binding).await?;
        if authority
            .allowed_space_ids
            .binary_search(&space_id.to_string())
            .is_err()
        {
            return Err(IntelligenceContextError::Forbidden);
        }
        Ok(())
    }
}

fn map_space_error(error: knowledge_spaces::KnowledgeSpaceError) -> IntelligenceContextError {
    match error {
        knowledge_spaces::KnowledgeSpaceError::Forbidden
        | knowledge_spaces::KnowledgeSpaceError::NotFound => IntelligenceContextError::Forbidden,
        knowledge_spaces::KnowledgeSpaceError::InvalidInput(detail) => {
            IntelligenceContextError::Invalid(detail)
        }
        _ => IntelligenceContextError::Persistence,
    }
}

fn query_tokens(question: &str) -> BTreeSet<String> {
    const STOP: &[&str] = &[
        "about",
        "after",
        "all",
        "and",
        "any",
        "are",
        "as",
        "at",
        "be",
        "before",
        "by",
        "can",
        "could",
        "current",
        "did",
        "do",
        "does",
        "for",
        "from",
        "had",
        "has",
        "have",
        "how",
        "in",
        "into",
        "is",
        "it",
        "its",
        "knowledge",
        "must",
        "of",
        "on",
        "only",
        "or",
        "our",
        "should",
        "team",
        "that",
        "the",
        "their",
        "then",
        "there",
        "this",
        "to",
        "use",
        "using",
        "was",
        "were",
        "what",
        "when",
        "where",
        "which",
        "who",
        "why",
        "with",
        "would",
        "you",
        "your",
    ];
    question
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() >= 2 && !STOP.contains(token))
        .map(str::to_string)
        .collect()
}

fn match_score(tokens: &BTreeSet<String>, title: &str, body: &str) -> usize {
    if tokens.is_empty() {
        return 0;
    }
    let title = title.to_lowercase();
    let body = body.to_lowercase();
    let mut matched_tokens = 0_usize;
    let score = tokens
        .iter()
        .map(|token| {
            let title_match = title.contains(token);
            let body_match = body.contains(token);
            matched_tokens += usize::from(title_match || body_match);
            usize::from(title_match) * 3 + usize::from(body_match)
        })
        .sum();
    if matched_tokens >= tokens.len().min(2) {
        score
    } else {
        0
    }
}

fn retain_relevant_candidates(ranked: &mut Vec<(usize, DateTime<Utc>, KnowledgeCitation)>) {
    let Some((best_score, _, _)) = ranked.first() else {
        return;
    };
    let minimum_score = (*best_score).div_ceil(2).max(2).min(*best_score);
    ranked.retain(|(score, _, _)| *score >= minimum_score);
}

fn citation_role(body: &str) -> CitationRole {
    let body = body.to_lowercase();
    if ["contradict", "conflict", "반박", "상충"]
        .iter()
        .any(|marker| body.contains(marker))
    {
        CitationRole::Contradicting
    } else {
        CitationRole::Supporting
    }
}

fn note_body(note: &str) -> &str {
    let Some(rest) = note.strip_prefix("---\n") else {
        return note.trim();
    };
    rest.find("\n---\n")
        .map(|end| rest[end + 5..].trim())
        .unwrap_or_else(|| note.trim())
}

fn bounded_passage(value: &str) -> String {
    if value.len() <= MAX_PASSAGE_BYTES {
        return value.to_string();
    }
    let mut end = MAX_PASSAGE_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim_end().to_string()
}

fn experience_digest(revision: &str) -> Option<&str> {
    let digest = revision.strip_prefix("sha256:")?;
    (digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then_some(digest)
}

fn interleave_experiences(
    ranked_sources: Vec<(usize, DateTime<Utc>, KnowledgeCitation)>,
    mut experiences: Vec<AnchoredExperience>,
    citation_limit: usize,
) -> Vec<KnowledgeCitation> {
    let mut citations = Vec::with_capacity(citation_limit);
    for (_, _, source) in ranked_sources {
        if citations.len() >= citation_limit {
            break;
        }
        let source_citation_id = source.citation_id.clone();
        let source_revision = source.source_revision.clone();
        citations.push(source);
        if citations.len() >= citation_limit {
            break;
        }
        if let Some(index) = experiences.iter().position(|experience| {
            experience.source_citation_id == source_citation_id
                && experience.source_revision == source_revision
        }) {
            citations.push(experiences.remove(index).citation);
        }
    }
    citations
}

fn context_revision(
    query: &IntelligenceQuery,
    citations: &[KnowledgeCitation],
    gaps: &[String],
) -> Result<String, IntelligenceContextError> {
    let bytes = serde_json::to_vec(&(query, citations, gaps))
        .map_err(|_| IntelligenceContextError::Persistence)?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

fn fit_pack_to_budget(
    query: &IntelligenceQuery,
    mut coverage: ContextCoverage,
    mut citations: Vec<KnowledgeCitation>,
    mut gaps: Vec<String>,
) -> Result<KnowledgeContextPack, IntelligenceContextError> {
    loop {
        if citations.is_empty() && coverage != ContextCoverage::Unavailable {
            coverage = ContextCoverage::Unavailable;
            gaps = vec!["Knowledge matched, but no citation fit within the response budget".into()];
        }
        let revision = context_revision(query, &citations, &gaps)?;
        match KnowledgeContextPack::new(query, revision, coverage, citations.clone(), gaps.clone())
        {
            Ok(pack) => return Ok(pack),
            Err(error)
                if !citations.is_empty() && error.to_string().contains("query response budget") =>
            {
                citations.pop();
            }
            Err(error) => {
                tracing::warn!(
                    target: "intelligence_context",
                    error = %error,
                    "Core rejected a generated knowledge context pack"
                );
                return Err(IntelligenceContextError::Invalid(error.to_string()));
            }
        }
    }
}

fn coverage_name(coverage: ContextCoverage) -> &'static str {
    match coverage {
        ContextCoverage::Complete => "complete",
        ContextCoverage::Partial => "partial",
        ContextCoverage::Unavailable => "unavailable",
        _ => "unavailable",
    }
}

fn predicate_name(result: gadgetron_bundle_sdk::OutcomePredicateResult) -> &'static str {
    match result {
        gadgetron_bundle_sdk::OutcomePredicateResult::Satisfied => "satisfied",
        gadgetron_bundle_sdk::OutcomePredicateResult::Failed => "failed",
        gadgetron_bundle_sdk::OutcomePredicateResult::Indeterminate => "indeterminate",
        _ => "indeterminate",
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use gadgetron_bundle_sdk::{BundleId, CitationRole, KnowledgeCitation};

    use super::{match_score, query_tokens, retain_relevant_candidates};

    #[test]
    fn lexical_shortlist_rejects_single_boilerplate_cross_domain_match() {
        let tokens = query_tokens(
            "Use current team knowledge to verify safe monitoring recovery before collecting server state",
        );
        for ignored in ["use", "current", "team", "knowledge", "to", "before"] {
            assert!(!tokens.contains(ignored));
        }
        assert!(tokens.contains("monitoring"));
        assert!(tokens.contains("recovery"));
        assert!(tokens.contains("server"));

        let restaurant = "Independent signed restaurant runtime. Use the package staging script.";
        assert_eq!(
            match_score(&tokens, "R2.6 cited restaurant fixture", restaurant),
            0
        );
        let runbook =
            "Verify the monitoring state, recover the signed marker, and reread server state.";
        assert!(match_score(&tokens, "Monitoring recovery runbook", runbook) > 0);
    }

    #[test]
    fn single_meaningful_token_question_still_matches() {
        let tokens = query_tokens("monitoring");
        assert_eq!(tokens.len(), 1);
        assert!(match_score(&tokens, "Monitoring guide", "") > 0);
    }

    #[test]
    fn lexical_shortlist_keeps_candidates_near_the_best_match() {
        let citation = |id: &str| {
            KnowledgeCitation::new(
                id,
                "00000000-0000-0000-0000-000000000001",
                BundleId::new("server-operations-intelligence").unwrap(),
                id,
                "1",
                "passage",
                CitationRole::Supporting,
                "applicability",
                0,
                None,
            )
        };
        let mut ranked = vec![
            (19, Utc::now(), citation("runbook")),
            (9, Utc::now(), citation("weak")),
            (10, Utc::now(), citation("related")),
        ];

        retain_relevant_candidates(&mut ranked);

        assert_eq!(
            ranked
                .into_iter()
                .map(|(_, _, citation)| citation.citation_id)
                .collect::<Vec<_>>(),
            vec!["runbook", "related"]
        );

        let mut single_match = vec![(1, Utc::now(), citation("single"))];
        retain_relevant_candidates(&mut single_match);
        assert_eq!(single_match.len(), 1);
    }
}
