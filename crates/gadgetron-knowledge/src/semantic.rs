use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use gadgetron_core::agent::config::EnvResolver;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

use crate::config::{EmbeddingConfig, EmbeddingWriteMode};
use crate::embedding::{EmbeddingError, EmbeddingProvider, OpenAiCompatEmbedding};
use crate::error::WikiError;
use crate::wiki::{chunk_page, parse_page, serialize_page};

const RRF_K: f32 = 60.0;
const SEMANTIC_WEIGHT: f32 = 0.6;
const KEYWORD_WEIGHT: f32 = 0.4;
const SEARCH_CANDIDATE_LIMIT: i64 = 20;
const SNIPPET_CHARS: usize = 200;

#[derive(Debug, Clone)]
pub(crate) struct SemanticSearchHit {
    pub page_name: String,
    pub score: f32,
    pub section: Option<String>,
    pub snippet: Option<String>,
}

/// Pgvector-backed semantic index engine.
///
/// Originally `pub(crate)` (P2A). Promoted to `pub` in W3-KL-1 so
/// `SemanticPgVectorIndex` (which holds an `Arc<SemanticBackend>`) can
/// expose a `pub fn new(...)` surface without leaking visibility. All
/// associated methods stay `pub(crate)` — external callers still must
/// route through `KnowledgeService` / `SemanticPgVectorIndex` for the
/// knowledge plane surface.
#[derive(Clone)]
pub struct SemanticBackend {
    pool: PgPool,
    embedding: Arc<dyn EmbeddingProvider>,
    write_mode: EmbeddingWriteMode,
}

// Manual `Debug` — `PgPool` + `dyn EmbeddingProvider` are not Debug. The
// knowledge plug architecture (`KnowledgeIndex: Debug`) requires
// `Arc<SemanticBackend>` to satisfy Debug through `SemanticPgVectorIndex`.
// The impl deliberately omits the pool / secret fields and only emits
// shape-only fingerprints.
impl std::fmt::Debug for SemanticBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticBackend")
            .field("write_mode", &self.write_mode)
            .field("embedding_model", &self.embedding.model_name())
            .finish_non_exhaustive()
    }
}

impl SemanticBackend {
    pub(crate) fn from_config(
        pool: Option<PgPool>,
        config: Option<&EmbeddingConfig>,
        env: &dyn EnvResolver,
    ) -> Result<Option<Self>, WikiError> {
        let Some(config) = config else {
            return Ok(None);
        };

        let Some(pool) = pool else {
            tracing::warn!(
                target: "knowledge_semantic",
                "embedding configured but GADGETRON_DATABASE_URL is unavailable; semantic indexing/search disabled"
            );
            return Ok(None);
        };

        let provider = OpenAiCompatEmbedding::new(config, env).map_err(|e| {
            WikiError::Frontmatter(format!("failed to initialize embedding provider: {e}"))
        })?;

        Ok(Some(Self::new(
            pool,
            Arc::new(provider) as Arc<dyn EmbeddingProvider>,
            config.write_mode,
        )))
    }

    pub(crate) fn new(
        pool: PgPool,
        embedding: Arc<dyn EmbeddingProvider>,
        write_mode: EmbeddingWriteMode,
    ) -> Self {
        Self {
            pool,
            embedding,
            write_mode,
        }
    }

    // Exposed for maintenance tooling; `KnowledgeGadgetProvider` stopped
    // calling this directly after the W3-KL-1 cutover (fanout is always
    // async via `FuturesUnordered`), but `maintenance::run_reindex` may
    // consult the mode for dry-run vs sync decisions.
    #[allow(dead_code)]
    pub(crate) fn write_mode(&self) -> EmbeddingWriteMode {
        self.write_mode
    }

    pub(crate) async fn index_page(
        &self,
        page_name: &str,
        raw_content: &str,
    ) -> Result<(), SemanticError> {
        let parsed = parse_page(raw_content).map_err(SemanticError::Wiki)?;
        let chunks = chunk_page(page_name, &parsed.body);
        let content_hash = sha256_hex(raw_content);
        let frontmatter_json =
            serde_json::to_string(&parsed.frontmatter).map_err(SemanticError::Serialize)?;

        let texts: Vec<&str> = chunks.iter().map(|chunk| chunk.content.as_str()).collect();
        let embeddings = self
            .embedding
            .embed(&texts)
            .await
            .map_err(SemanticError::Embedding)?;

        if embeddings.len() != chunks.len() {
            return Err(SemanticError::Embedding(EmbeddingError::Parse));
        }

        let mut tx = self.pool.begin().await.map_err(SemanticError::Db)?;
        sqlx::query("DELETE FROM wiki_chunks WHERE page_name = $1")
            .bind(page_name)
            .execute(&mut *tx)
            .await
            .map_err(SemanticError::Db)?;

        for (chunk, embedding) in chunks.iter().zip(embeddings.iter()) {
            sqlx::query(
                "INSERT INTO wiki_chunks (page_name, chunk_index, section, content, embedding) \
                 VALUES ($1, $2, $3, $4, $5::vector)",
            )
            .bind(page_name)
            .bind(chunk.chunk_index as i32)
            .bind(chunk.section.as_deref())
            .bind(&chunk.content)
            .bind(encode_vector(embedding))
            .execute(&mut *tx)
            .await
            .map_err(SemanticError::Db)?;
        }

        sqlx::query(
            "INSERT INTO wiki_pages (page_name, frontmatter, content_hash, indexed_at) \
             VALUES ($1, $2::jsonb, $3, NOW()) \
             ON CONFLICT (page_name) DO UPDATE SET \
                 frontmatter = EXCLUDED.frontmatter, \
                 content_hash = EXCLUDED.content_hash, \
                 indexed_at = NOW()",
        )
        .bind(page_name)
        .bind(frontmatter_json)
        .bind(content_hash)
        .execute(&mut *tx)
        .await
        .map_err(SemanticError::Db)?;

        tx.commit().await.map_err(SemanticError::Db)?;
        Ok(())
    }

    pub(crate) async fn page_hashes(&self) -> Result<HashMap<String, String>, SemanticError> {
        let rows = sqlx::query("SELECT page_name, content_hash FROM wiki_pages")
            .fetch_all(&self.pool)
            .await
            .map_err(SemanticError::Db)?;

        let mut out = HashMap::with_capacity(rows.len());
        for row in rows {
            let page_name: String = row.try_get("page_name").map_err(SemanticError::Db)?;
            let content_hash: String = row.try_get("content_hash").map_err(SemanticError::Db)?;
            out.insert(page_name, content_hash);
        }
        Ok(out)
    }

    pub(crate) async fn delete_page(&self, page_name: &str) -> Result<(), SemanticError> {
        let mut tx = self.pool.begin().await.map_err(SemanticError::Db)?;
        sqlx::query("DELETE FROM wiki_chunks WHERE page_name = $1")
            .bind(page_name)
            .execute(&mut *tx)
            .await
            .map_err(SemanticError::Db)?;
        sqlx::query("DELETE FROM wiki_pages WHERE page_name = $1")
            .bind(page_name)
            .execute(&mut *tx)
            .await
            .map_err(SemanticError::Db)?;
        tx.commit().await.map_err(SemanticError::Db)?;
        Ok(())
    }

    pub(crate) async fn truncate_all(&self) -> Result<(), SemanticError> {
        let mut tx = self.pool.begin().await.map_err(SemanticError::Db)?;
        sqlx::query("TRUNCATE wiki_chunks, wiki_pages")
            .execute(&mut *tx)
            .await
            .map_err(SemanticError::Db)?;
        tx.commit().await.map_err(SemanticError::Db)?;
        Ok(())
    }

    pub(crate) async fn rename_page(&self, from: &str, to: &str) -> Result<(), SemanticError> {
        let mut tx = self.pool.begin().await.map_err(SemanticError::Db)?;
        sqlx::query(
            "UPDATE wiki_pages SET page_name = $2, indexed_at = NOW() WHERE page_name = $1",
        )
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await
        .map_err(SemanticError::Db)?;
        sqlx::query(
            "UPDATE wiki_chunks SET page_name = $2, updated_at = NOW() WHERE page_name = $1",
        )
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await
        .map_err(SemanticError::Db)?;
        tx.commit().await.map_err(SemanticError::Db)?;
        Ok(())
    }

    pub(crate) async fn hybrid_search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SemanticSearchHit>, SemanticError> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let query_embedding = self
            .embedding
            .embed(&[query])
            .await
            .map_err(SemanticError::Embedding)?
            .into_iter()
            .next()
            .ok_or(SemanticError::Embedding(EmbeddingError::Parse))?;
        let vector = encode_vector(&query_embedding);

        let semantic_rows = sqlx::query(
            "SELECT page_name, section, content \
             FROM wiki_chunks \
             ORDER BY embedding <=> $1::vector \
             LIMIT $2",
        )
        .bind(vector)
        .bind(SEARCH_CANDIDATE_LIMIT)
        .fetch_all(&self.pool)
        .await
        .map_err(SemanticError::Db)?;

        let keyword_rows = sqlx::query(
            "SELECT page_name, section, content \
             FROM wiki_chunks \
             WHERE content_tsv @@ plainto_tsquery('simple', $1) \
             ORDER BY ts_rank(content_tsv, plainto_tsquery('simple', $1)) DESC \
             LIMIT $2",
        )
        .bind(query)
        .bind(SEARCH_CANDIDATE_LIMIT)
        .fetch_all(&self.pool)
        .await
        .map_err(SemanticError::Db)?;

        let semantic_candidates = dedup_candidates(semantic_rows)?;
        let keyword_candidates = dedup_candidates(keyword_rows)?;
        let mut fused: HashMap<String, FusedHit> = HashMap::new();

        apply_rrf(&mut fused, semantic_candidates, SEMANTIC_WEIGHT);
        apply_rrf(&mut fused, keyword_candidates, KEYWORD_WEIGHT);

        let mut hits: Vec<SemanticSearchHit> = fused
            .into_iter()
            .map(|(page_name, hit)| SemanticSearchHit {
                page_name,
                score: hit.score,
                section: hit.section,
                snippet: hit.snippet,
            })
            .collect();

        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.page_name.cmp(&b.page_name))
        });
        hits.truncate(limit);
        Ok(hits)
    }
}

pub(crate) fn normalize_page_content(raw_content: &str) -> Result<String, WikiError> {
    let parsed = parse_page(raw_content)?;
    let mut frontmatter = parsed.frontmatter;
    let now = Utc::now();

    if frontmatter.created.is_none() {
        frontmatter.created = Some(now);
    }
    frontmatter.updated = Some(now);
    if frontmatter.source.is_none() {
        frontmatter.source = Some("conversation".to_string());
    }
    if frontmatter.confidence.is_none() {
        frontmatter.confidence = Some("medium".to_string());
    }

    serialize_page(&frontmatter, &parsed.body)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SemanticError {
    #[error("semantic db error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("semantic embedding error: {0}")]
    Embedding(#[from] EmbeddingError),
    #[error("semantic wiki parse error: {0}")]
    Wiki(#[from] WikiError),
    #[error("semantic serialize error: {0}")]
    Serialize(serde_json::Error),
}

#[derive(Debug)]
struct Candidate {
    page_name: String,
    section: Option<String>,
    content: String,
}

#[derive(Debug, Default)]
struct FusedHit {
    score: f32,
    best_component: f32,
    section: Option<String>,
    snippet: Option<String>,
}

fn dedup_candidates(rows: Vec<sqlx::postgres::PgRow>) -> Result<Vec<Candidate>, SemanticError> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for row in rows {
        let page_name: String = row.try_get("page_name").map_err(SemanticError::Db)?;
        if !seen.insert(page_name.clone()) {
            continue;
        }
        let section: Option<String> = row.try_get("section").map_err(SemanticError::Db)?;
        let content: String = row.try_get("content").map_err(SemanticError::Db)?;
        out.push(Candidate {
            page_name,
            section,
            content,
        });
    }
    Ok(out)
}

fn apply_rrf(fused: &mut HashMap<String, FusedHit>, candidates: Vec<Candidate>, weight: f32) {
    for (idx, candidate) in candidates.into_iter().enumerate() {
        let contribution = weight / (RRF_K + idx as f32 + 1.0);
        let hit = fused.entry(candidate.page_name).or_default();
        hit.score += contribution;
        if contribution > hit.best_component {
            hit.best_component = contribution;
            hit.section = candidate.section;
            hit.snippet = Some(snippet(&candidate.content));
        }
    }
}

fn snippet(content: &str) -> String {
    let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(SNIPPET_CHARS).collect()
}

fn encode_vector(values: &[f32]) -> String {
    let mut out = String::from("[");
    for (idx, value) in values.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&value.to_string());
    }
    out.push(']');
    out
}

fn sha256_hex(raw: &str) -> String {
    hex::encode(Sha256::digest(raw.as_bytes()))
}

impl SemanticBackend {
    pub(crate) fn content_hash(raw: &str) -> String {
        sha256_hex(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_page_content_adds_frontmatter_defaults() {
        let normalized = normalize_page_content("# Title\n\nBody").expect("normalize");
        assert!(normalized.starts_with("---\n"));
        assert!(normalized.contains("source = \"conversation\""));
        assert!(normalized.contains("confidence = \"medium\""));
        assert!(normalized.contains("# Title"));
    }

    #[test]
    fn encode_vector_uses_pgvector_literal_shape() {
        assert_eq!(encode_vector(&[1.0, 2.5, 3.0]), "[1,2.5,3]");
    }

    #[test]
    fn snippet_collapses_whitespace_and_truncates() {
        let input = "alpha\n\n beta\tgamma";
        assert_eq!(snippet(input), "alpha beta gamma");
    }

    #[test]
    fn normalized_content_preserves_existing_source() {
        let raw = "---\nsource = \"seed\"\n---\nBody";
        let normalized = normalize_page_content(raw).expect("normalize");
        assert!(normalized.contains("source = \"seed\""));
        assert!(!normalized.contains("source = \"conversation\""));
    }

    #[test]
    fn tokenizer_basis_is_shared_with_keyword_fallback() {
        let tokens = crate::wiki::tokenize("GPU fan boot");
        assert_eq!(tokens, vec!["gpu", "fan", "boot"]);
    }
}
