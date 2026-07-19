use std::collections::HashSet;

use gadgetron_core::agent::tools::GadgetDispatchContext;
use sqlx::PgPool;
use uuid::Uuid;

use crate::vault::{TenantVaultLayout, VaultLayoutError};

const MAX_VISIBLE_PAGES: i64 = 200;

#[derive(Clone)]
pub(crate) struct ReviewedVaultReader {
    pool: PgPool,
    layout: TenantVaultLayout,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewedVaultPage {
    pub page_name: String,
    pub title: String,
    pub markdown: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewedVaultHit {
    pub page_name: String,
    pub title: String,
    pub score: f32,
    pub snippet: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReviewedVaultError {
    #[error("invalid authenticated identity")]
    InvalidIdentity,
    #[error("reviewed knowledge index is unavailable")]
    Database(#[from] sqlx::Error),
    #[error("reviewed knowledge Vault is unavailable")]
    Vault(#[from] VaultLayoutError),
    #[error("reviewed knowledge changed after its indexed revision")]
    RevisionChanged,
    #[error("reviewed knowledge is not valid UTF-8")]
    InvalidUtf8,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ReviewedObjectRow {
    id: Uuid,
    space_id: Uuid,
    home_bundle_id: String,
    path: String,
    content_hash: Option<String>,
    title: String,
}

impl ReviewedVaultReader {
    pub(crate) fn new(pool: PgPool, layout: TenantVaultLayout) -> Self {
        Self { pool, layout }
    }

    pub(crate) async fn list(
        &self,
        context: &GadgetDispatchContext,
    ) -> Result<Vec<String>, ReviewedVaultError> {
        let (tenant_id, actor_id) = parse_identity(context)?;
        let rows = self.visible_rows(tenant_id, actor_id, None).await?;
        Ok(rows.iter().map(page_name).collect())
    }

    pub(crate) async fn get(
        &self,
        context: &GadgetDispatchContext,
        requested_page: &str,
    ) -> Result<Option<ReviewedVaultPage>, ReviewedVaultError> {
        let Some(object_id) = object_id_from_page_name(requested_page) else {
            return Ok(None);
        };
        let (tenant_id, actor_id) = parse_identity(context)?;
        let Some(row) = self
            .visible_rows(tenant_id, actor_id, Some(object_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let layout = self.layout.clone();
        let page = tokio::task::spawn_blocking(move || read_page(layout, tenant_id, row))
            .await
            .map_err(|_| ReviewedVaultError::RevisionChanged)??;
        Ok(Some(page))
    }

    pub(crate) async fn search(
        &self,
        context: &GadgetDispatchContext,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ReviewedVaultHit>, ReviewedVaultError> {
        let (tenant_id, actor_id) = parse_identity(context)?;
        let rows = self.visible_rows(tenant_id, actor_id, None).await?;
        let layout = self.layout.clone();
        let pages = tokio::task::spawn_blocking(move || {
            let mut pages = Vec::with_capacity(rows.len());
            for row in rows {
                match read_page(layout.clone(), tenant_id, row) {
                    Ok(page) => pages.push(page),
                    Err(ReviewedVaultError::RevisionChanged) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok::<_, ReviewedVaultError>(pages)
        })
        .await
        .map_err(|_| ReviewedVaultError::RevisionChanged)??;
        Ok(rank_pages(pages, query, limit))
    }

    async fn visible_rows(
        &self,
        tenant_id: Uuid,
        actor_id: Uuid,
        object_id: Option<Uuid>,
    ) -> Result<Vec<ReviewedObjectRow>, sqlx::Error> {
        sqlx::query_as::<_, ReviewedObjectRow>(
            r#"WITH active_actor AS (
                   SELECT role FROM users
                    WHERE tenant_id = $1 AND id = $2 AND is_active = TRUE
               )
               SELECT o.id, v.space_id, v.home_bundle_id, o.path, o.content_hash,
                      COALESCE(NULLIF(n.title, ''), o.path) AS title
                 FROM knowledge_objects o
                 JOIN knowledge_vaults v
                   ON v.tenant_id = o.tenant_id AND v.id = o.vault_id
                 JOIN knowledge_spaces s
                   ON s.tenant_id = v.tenant_id AND s.id = v.space_id
                 JOIN knowledge_graph_generations generation
                   ON generation.tenant_id = o.tenant_id AND generation.state = 'active'
                 JOIN knowledge_graph_nodes n
                   ON n.tenant_id = o.tenant_id AND n.generation_id = generation.id
                  AND n.stable_node_id = 'note:' || o.id::TEXT
                 JOIN active_actor actor ON TRUE
                WHERE o.tenant_id = $1
                  AND ($3::UUID IS NULL OR o.id = $3)
                  AND o.status = 'active'
                  AND v.owner_state = 'enabled'
                  AND s.status = 'active'
                  AND n.status = 'active'
                  AND n.node_kind IN ('lesson', 'insight')
                  AND n.metadata->>'review_state' IN ('reviewed', 'verified')
                  AND (
                       actor.role = 'admin'
                       OR s.owner_user_id = $2
                       OR EXISTS (
                           SELECT 1 FROM projects project
                            WHERE project.tenant_id = $1 AND project.id = s.owner_project_id
                              AND project.owner_user_id = $2)
                       OR EXISTS (
                           SELECT 1 FROM team_members member
                           JOIN teams team ON team.id = member.team_id
                            WHERE team.tenant_id = $1 AND member.user_id = $2
                              AND member.team_id = s.owner_team_id)
                       OR s.kind = 'tenant_shared'
                       OR EXISTS (
                           SELECT 1 FROM knowledge_space_grants grant_row
                            WHERE grant_row.tenant_id = $1 AND grant_row.space_id = s.id
                              AND grant_row.revoked_at IS NULL
                              AND (grant_row.expires_at IS NULL OR grant_row.expires_at > NOW())
                              AND (
                                  (grant_row.principal_kind = 'user'
                                   AND grant_row.principal_id = $2::TEXT)
                                  OR (grant_row.principal_kind = 'team' AND EXISTS (
                                      SELECT 1 FROM team_members member
                                      JOIN teams team ON team.id = member.team_id
                                       WHERE team.tenant_id = $1 AND member.user_id = $2
                                         AND member.team_id = grant_row.principal_id))
                                  OR (grant_row.principal_kind = 'group' AND EXISTS (
                                      SELECT 1 FROM user_groups membership
                                      JOIN groups group_row ON group_row.id = membership.group_id
                                       WHERE group_row.tenant_id = $1 AND membership.user_id = $2
                                         AND membership.group_id = grant_row.principal_id))
                              ))
                  )
                ORDER BY o.updated_at DESC, o.id
                LIMIT $4"#,
        )
        .bind(tenant_id)
        .bind(actor_id)
        .bind(object_id)
        .bind(if object_id.is_some() {
            1
        } else {
            MAX_VISIBLE_PAGES
        })
        .fetch_all(&self.pool)
        .await
    }
}

fn parse_identity(context: &GadgetDispatchContext) -> Result<(Uuid, Uuid), ReviewedVaultError> {
    let tenant_id =
        Uuid::parse_str(&context.tenant_id).map_err(|_| ReviewedVaultError::InvalidIdentity)?;
    let actor_id =
        Uuid::parse_str(&context.actor_id).map_err(|_| ReviewedVaultError::InvalidIdentity)?;
    Ok((tenant_id, actor_id))
}

fn read_page(
    layout: TenantVaultLayout,
    tenant_id: Uuid,
    row: ReviewedObjectRow,
) -> Result<ReviewedVaultPage, ReviewedVaultError> {
    let repository = layout.open_existing(tenant_id)?;
    let note = repository.read_note_exact(
        row.space_id,
        &row.home_bundle_id,
        &row.path,
        row.content_hash.as_deref(),
    )?;
    if note.externally_changed {
        return Err(ReviewedVaultError::RevisionChanged);
    }
    let markdown = String::from_utf8(note.bytes).map_err(|_| ReviewedVaultError::InvalidUtf8)?;
    Ok(ReviewedVaultPage {
        page_name: page_name(&row),
        title: row.title,
        markdown,
    })
}

fn page_name(row: &ReviewedObjectRow) -> String {
    format!(
        "vault/{}/{}--{}",
        row.home_bundle_id,
        page_segment(&row.title),
        row.id
    )
}

fn object_id_from_page_name(page_name: &str) -> Option<Uuid> {
    let page_name = page_name.strip_prefix("vault/")?;
    let (_, object_id) = page_name.rsplit_once("--")?;
    Uuid::parse_str(object_id).ok()
}

fn page_segment(title: &str) -> String {
    let mut result = String::new();
    let mut pending_separator = false;
    for character in title.chars() {
        if character.is_alphanumeric() {
            if pending_separator && !result.is_empty() {
                result.push('-');
            }
            for lowered in character.to_lowercase() {
                result.push(lowered);
            }
            pending_separator = false;
        } else {
            pending_separator = true;
        }
        if result.chars().count() >= 72 {
            break;
        }
    }
    let result = result.trim_matches('-');
    if result.is_empty() {
        "knowledge".to_string()
    } else {
        result.to_string()
    }
}

fn rank_pages(pages: Vec<ReviewedVaultPage>, query: &str, limit: usize) -> Vec<ReviewedVaultHit> {
    let query = query.to_lowercase();
    let tokens: HashSet<_> = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut hits = pages
        .into_iter()
        .filter_map(|page| {
            let title = page.title.to_lowercase();
            let searchable_markdown = crate::source::parse_obsidian_note(&page.markdown)
                .map(|note| note.body)
                .unwrap_or_else(|_| page.markdown.clone());
            let body = searchable_markdown.to_lowercase();
            let title_matches = tokens
                .iter()
                .filter(|token| title.contains(**token))
                .count();
            let body_matches = tokens.iter().filter(|token| body.contains(**token)).count();
            if title_matches + body_matches == 0 {
                return None;
            }
            let phrase_bonus = if body.contains(&query) || title.contains(&query) {
                4.0
            } else {
                0.0
            };
            let score = phrase_bonus + (title_matches as f32 * 3.0) + body_matches as f32;
            Some(ReviewedVaultHit {
                page_name: page.page_name,
                title: page.title,
                score,
                snippet: matching_snippet(&searchable_markdown, &tokens),
            })
        })
        .collect::<Vec<_>>();
    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.page_name.cmp(&right.page_name))
    });
    hits.truncate(limit);
    hits
}

fn matching_snippet(markdown: &str, tokens: &HashSet<&str>) -> String {
    let paragraph = markdown
        .split("\n\n")
        .find(|paragraph| {
            let lowered = paragraph.to_lowercase();
            tokens.iter().any(|token| lowered.contains(*token))
        })
        .unwrap_or(markdown);
    let mut snippet = paragraph
        .lines()
        .filter(|line| !line.starts_with("---"))
        .collect::<Vec<_>>()
        .join(" ");
    snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
    if snippet.chars().count() > 400 {
        snippet = snippet.chars().take(397).collect::<String>() + "...";
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_testing::harness::pg::PgHarness;

    async fn pg_available() -> bool {
        let url = std::env::var("DATABASE_URL")
            .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
            .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
        let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
        else {
            return false;
        };
        let vector: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
            "SELECT default_version FROM pg_available_extensions WHERE name = 'vector'",
        )
        .fetch_optional(&pool)
        .await;
        pool.close().await;
        matches!(vector, Ok(Some(_)))
    }

    async fn insert_user(pool: &PgPool, tenant_id: Uuid, label: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
             VALUES ($1, $2, $3, $3, 'member', 'test')",
        )
        .bind(id)
        .bind(tenant_id)
        .bind(format!("{label}-{id}@example.test"))
        .execute(pool)
        .await
        .unwrap();
        id
    }

    async fn seed_page(
        pool: &PgPool,
        tenant_id: Uuid,
        generation_id: Uuid,
        space_id: Uuid,
        created_by: Uuid,
        title: &str,
        review_state: &str,
    ) -> Uuid {
        let vault_id: Uuid = sqlx::query_scalar(
            r#"INSERT INTO knowledge_vaults
               (tenant_id, space_id, home_bundle_id, knowledge_schema_id)
               VALUES ($1, $2, 'core', 'core.knowledge')
               ON CONFLICT (tenant_id, space_id, home_bundle_id)
               DO UPDATE SET updated_at = knowledge_vaults.updated_at
               RETURNING id"#,
        )
        .bind(tenant_id)
        .bind(space_id)
        .fetch_one(pool)
        .await
        .unwrap();
        let object_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO knowledge_objects
               (id, tenant_id, vault_id, canonical_kind, path, content_hash, created_by)
               VALUES ($1, $2, $3, 'note', $4, $5, $6)"#,
        )
        .bind(object_id)
        .bind(tenant_id)
        .bind(vault_id)
        .bind(format!("notes/{object_id}.md"))
        .bind("a".repeat(64))
        .bind(created_by)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO knowledge_graph_nodes
               (generation_id, tenant_id, stable_node_id, space_id, vault_id,
                node_kind, canonical_id, canonical_revision, home_bundle_id,
                title, status, freshness, content_hash, metadata)
               VALUES ($1, $2, $3, $4, $5, 'lesson', $6, 1, 'core',
                       $7, 'active', 'current', $8,
                       jsonb_build_object('review_state', $9::TEXT))"#,
        )
        .bind(generation_id)
        .bind(tenant_id)
        .bind(format!("note:{object_id}"))
        .bind(space_id)
        .bind(vault_id)
        .bind(object_id)
        .bind(title)
        .bind("a".repeat(64))
        .bind(review_state)
        .execute(pool)
        .await
        .unwrap();
        object_id
    }

    fn context(tenant_id: Uuid, actor_id: Uuid) -> GadgetDispatchContext {
        GadgetDispatchContext::new(tenant_id.to_string(), actor_id.to_string(), "acl-check")
    }

    async fn visible_ids(
        reader: &ReviewedVaultReader,
        context: &GadgetDispatchContext,
    ) -> HashSet<Uuid> {
        reader
            .list(context)
            .await
            .unwrap()
            .iter()
            .filter_map(|page| object_id_from_page_name(page))
            .collect()
    }

    async fn materialize_page(
        pool: &PgPool,
        layout: &TenantVaultLayout,
        tenant_id: Uuid,
        space_id: Uuid,
        object_id: Uuid,
        title: &str,
    ) {
        let repository = layout.open_or_init(tenant_id).unwrap();
        repository.ensure_domain(space_id, "core").unwrap();
        let path = format!("notes/{object_id}.md");
        let note = format!("# {title}\n\n{title} trust boundary knowledge.\n");
        let state = repository
            .write_note(
                space_id,
                "core",
                &path,
                note.as_bytes(),
                "test: materialize reviewed page",
            )
            .unwrap();
        sqlx::query("UPDATE knowledge_objects SET content_hash = $1 WHERE id = $2")
            .bind(state.content_hash)
            .bind(object_id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[test]
    fn page_locator_stays_human_readable_and_recovers_object_id() {
        let id = Uuid::parse_str("78bb9b8c-194d-4183-b8cc-aec8afa4c945").unwrap();
        let row = ReviewedObjectRow {
            id,
            space_id: Uuid::nil(),
            home_bundle_id: "restaurant-research".to_string(),
            path: format!("notes/{id}.md"),
            content_hash: None,
            title: "서울 Restaurant Insight".to_string(),
        };
        let locator = page_name(&row);
        assert!(locator.starts_with("vault/restaurant-research/서울-restaurant-insight--"));
        assert_eq!(object_id_from_page_name(&locator), Some(id));
    }

    #[tokio::test]
    async fn reviewed_retrieval_applies_space_revocation_and_tenant_boundaries() {
        if !pg_available().await {
            eprintln!("skipping reviewed retrieval PostgreSQL fixture: PostgreSQL unavailable");
            return;
        }
        let harness = PgHarness::new().await;
        let pool = harness.pool();
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();
        for (tenant_id, name) in [(tenant_a, "reviewed-a"), (tenant_b, "reviewed-b")] {
            sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
                .bind(tenant_id)
                .bind(name)
                .execute(pool)
                .await
                .unwrap();
        }
        let owner_a = insert_user(pool, tenant_a, "owner-a").await;
        let member_a = insert_user(pool, tenant_a, "member-a").await;
        let owner_b = insert_user(pool, tenant_b, "owner-b").await;
        let team_id = format!("ops-{}", &Uuid::new_v4().simple().to_string()[..8]);
        sqlx::query("INSERT INTO teams (id, tenant_id, display_name) VALUES ($1, $2, 'Ops')")
            .bind(&team_id)
            .bind(tenant_a)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO team_members (team_id, user_id) VALUES ($1, $2)")
            .bind(&team_id)
            .bind(member_a)
            .execute(pool)
            .await
            .unwrap();

        let project_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO projects (id, tenant_id, slug, title, owner_user_id) \
             VALUES ($1, $2, $3, 'R3.4a', $4)",
        )
        .bind(project_id)
        .bind(tenant_a)
        .bind(format!("trust-{}", &project_id.simple().to_string()[..8]))
        .bind(owner_a)
        .execute(pool)
        .await
        .unwrap();
        let personal = Uuid::new_v4();
        let project = Uuid::new_v4();
        let team = Uuid::new_v4();
        let shared = Uuid::new_v4();
        let foreign = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO knowledge_spaces
               (id, tenant_id, kind, title, owner_user_id)
               VALUES ($1, $2, 'personal', 'Private', $3)"#,
        )
        .bind(personal)
        .bind(tenant_a)
        .bind(owner_a)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO knowledge_spaces
               (id, tenant_id, kind, title, owner_project_id)
               VALUES ($1, $2, 'project', 'Project', $3)"#,
        )
        .bind(project)
        .bind(tenant_a)
        .bind(project_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO knowledge_spaces
               (id, tenant_id, kind, title, owner_team_id)
               VALUES ($1, $2, 'team', 'Team', $3)"#,
        )
        .bind(team)
        .bind(tenant_a)
        .bind(&team_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO knowledge_spaces (id, tenant_id, kind, title) \
             VALUES ($1, $2, 'tenant_shared', 'Shared')",
        )
        .bind(shared)
        .bind(tenant_a)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO knowledge_spaces
               (id, tenant_id, kind, title, owner_user_id)
               VALUES ($1, $2, 'personal', 'Foreign', $3)"#,
        )
        .bind(foreign)
        .bind(tenant_b)
        .bind(owner_b)
        .execute(pool)
        .await
        .unwrap();
        let grant_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO knowledge_space_grants
               (id, tenant_id, space_id, principal_kind, principal_id, role, created_by)
               VALUES ($1, $2, $3, 'user', $4, 'viewer', $5)"#,
        )
        .bind(grant_id)
        .bind(tenant_a)
        .bind(project)
        .bind(member_a.to_string())
        .bind(owner_a)
        .execute(pool)
        .await
        .unwrap();

        let generation_a = Uuid::new_v4();
        let generation_b = Uuid::new_v4();
        for (generation, tenant, user) in [
            (generation_a, tenant_a, owner_a),
            (generation_b, tenant_b, owner_b),
        ] {
            sqlx::query(
                r#"INSERT INTO knowledge_graph_generations
                   (id, tenant_id, state, input_digest, built_by, activated_at)
                   VALUES ($1, $2, 'active', $3, $4, NOW())"#,
            )
            .bind(generation)
            .bind(tenant)
            .bind(format!("sha256:{}", "a".repeat(64)))
            .bind(user)
            .execute(pool)
            .await
            .unwrap();
        }
        let personal_page = seed_page(
            pool,
            tenant_a,
            generation_a,
            personal,
            owner_a,
            "Personal",
            "reviewed",
        )
        .await;
        let project_page = seed_page(
            pool,
            tenant_a,
            generation_a,
            project,
            owner_a,
            "Project",
            "verified",
        )
        .await;
        let team_page = seed_page(
            pool,
            tenant_a,
            generation_a,
            team,
            owner_a,
            "Team",
            "reviewed",
        )
        .await;
        let shared_page = seed_page(
            pool,
            tenant_a,
            generation_a,
            shared,
            owner_a,
            "Shared",
            "reviewed",
        )
        .await;
        let unreviewed_page = seed_page(
            pool,
            tenant_a,
            generation_a,
            shared,
            owner_a,
            "Unreviewed",
            "pending",
        )
        .await;
        let foreign_page = seed_page(
            pool,
            tenant_b,
            generation_b,
            foreign,
            owner_b,
            "Foreign",
            "reviewed",
        )
        .await;
        let share_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO knowledge_shares
               (id, tenant_id, source_space_id, source_object_id, source_revision,
                target_space_id, mode, follow_latest, created_by)
               VALUES ($1, $2, $3, $4, 1, $5, 'reference', TRUE, $6)"#,
        )
        .bind(share_id)
        .bind(tenant_a)
        .bind(project)
        .bind(project_page)
        .bind(team)
        .bind(owner_a)
        .execute(pool)
        .await
        .unwrap();

        let vault_root = tempfile::tempdir().unwrap();
        let layout = TenantVaultLayout::new(vault_root.path());
        for (space_id, object_id, title) in [
            (project, project_page, "Project"),
            (team, team_page, "Team"),
            (shared, shared_page, "Shared"),
        ] {
            materialize_page(pool, &layout, tenant_a, space_id, object_id, title).await;
        }
        let reader = ReviewedVaultReader::new(pool.clone(), layout);
        let owner_visible = visible_ids(&reader, &context(tenant_a, owner_a)).await;
        assert_eq!(
            owner_visible,
            HashSet::from([personal_page, project_page, shared_page])
        );
        let member_context = context(tenant_a, member_a);
        let member_visible = visible_ids(&reader, &member_context).await;
        assert_eq!(
            member_visible,
            HashSet::from([project_page, team_page, shared_page])
        );
        assert!(!member_visible.contains(&personal_page));
        assert!(!member_visible.contains(&unreviewed_page));
        let project_hits = reader.search(&member_context, "Project", 10).await.unwrap();
        assert_eq!(project_hits.len(), 1);
        assert_eq!(
            object_id_from_page_name(&project_hits[0].page_name),
            Some(project_page)
        );
        assert_eq!(
            visible_ids(&reader, &context(tenant_b, owner_b)).await,
            HashSet::from([foreign_page])
        );
        assert!(visible_ids(&reader, &context(tenant_a, owner_b))
            .await
            .is_empty());
        assert!(visible_ids(&reader, &context(tenant_b, owner_a))
            .await
            .is_empty());

        sqlx::query("DELETE FROM team_members WHERE team_id = $1 AND user_id = $2")
            .bind(&team_id)
            .bind(member_a)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("UPDATE knowledge_space_grants SET revoked_at = NOW() WHERE id = $1")
            .bind(grant_id)
            .execute(pool)
            .await
            .unwrap();
        assert_eq!(
            visible_ids(&reader, &member_context).await,
            HashSet::from([shared_page])
        );
        assert!(reader
            .search(&member_context, "Project", 10)
            .await
            .unwrap()
            .is_empty());
        let hidden_locator = format!("vault/core/project--{project_page}");
        assert!(reader
            .get(&member_context, &hidden_locator)
            .await
            .unwrap()
            .is_none());
        sqlx::query("UPDATE knowledge_shares SET revoked_at = NOW() WHERE id = $1")
            .bind(share_id)
            .execute(pool)
            .await
            .unwrap();
        assert_eq!(
            visible_ids(&reader, &member_context).await,
            HashSet::from([shared_page])
        );
        sqlx::query("UPDATE users SET is_active = FALSE WHERE id = $1")
            .bind(member_a)
            .execute(pool)
            .await
            .unwrap();
        assert!(visible_ids(&reader, &member_context).await.is_empty());

        harness.cleanup().await;
    }
}
