use std::collections::HashSet;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use gadgetron_core::agent::config::StdEnv;
use sqlx::PgPool;

use crate::config::KnowledgeConfig;
use crate::error::WikiError;
use crate::semantic::{SemanticBackend, SemanticError};
use crate::wiki::{parse_page, Wiki};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReindexMode {
    Incremental,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReindexOptions {
    pub mode: ReindexMode,
    pub dry_run: bool,
    pub verbose: bool,
}

impl Default for ReindexOptions {
    fn default() -> Self {
        Self {
            mode: ReindexMode::Incremental,
            dry_run: false,
            verbose: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReindexActionKind {
    Reembedded,
    Deleted,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReindexAction {
    pub page_name: String,
    pub kind: ReindexActionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReindexReport {
    pub mode: ReindexMode,
    pub dry_run: bool,
    pub scanned: usize,
    pub reembedded: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub actions: Vec<ReindexAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StalePage {
    pub name: String,
    pub updated: DateTime<Utc>,
    pub days_stale: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiAuditReport {
    pub generated_at: DateTime<Utc>,
    pub wiki_path: PathBuf,
    pub total_pages: usize,
    pub stale_threshold_days: u16,
    pub stale_pages: Vec<StalePage>,
    pub pages_without_frontmatter: Vec<String>,
}

impl WikiAuditReport {
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Wiki audit report - {}\n",
            self.generated_at.to_rfc3339()
        ));
        out.push_str(&format!("Wiki path: {}\n", self.wiki_path.display()));
        out.push_str(&format!("Total pages: {}\n\n", self.total_pages));

        out.push_str(&format!(
            "## Stale pages (updated more than {} days ago)\n",
            self.stale_threshold_days
        ));
        if self.stale_pages.is_empty() {
            out.push_str("- none\n\n");
        } else {
            for page in &self.stale_pages {
                out.push_str(&format!("- {}\n", page.name));
                out.push_str(&format!(
                    "  updated: {} ({} days ago)\n",
                    page.updated.format("%Y-%m-%d"),
                    page.days_stale
                ));
                out.push_str("  suggestion: review for current relevance\n\n");
            }
        }

        out.push_str("## Pages without frontmatter\n");
        if self.pages_without_frontmatter.is_empty() {
            out.push_str("- none\n");
        } else {
            for page in &self.pages_without_frontmatter {
                out.push_str(&format!("- {}\n", page));
                out.push_str("  suggestion: add frontmatter (tags, type, created)\n\n");
            }
        }

        out.trim_end().to_string()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MaintenanceError {
    #[error("knowledge config error: {0}")]
    Config(String),
    #[error(transparent)]
    Wiki(#[from] WikiError),
    #[error("semantic maintenance error: {0}")]
    Semantic(String),
}

impl From<SemanticError> for MaintenanceError {
    fn from(value: SemanticError) -> Self {
        Self::Semantic(value.to_string())
    }
}

pub async fn run_reindex(
    config: &KnowledgeConfig,
    pool: PgPool,
    options: ReindexOptions,
) -> Result<ReindexReport, MaintenanceError> {
    let wiki = Wiki::open(config.to_wiki_config().map_err(MaintenanceError::Config)?)?;
    let semantic = SemanticBackend::from_config(Some(pool), config.embedding.as_ref(), &StdEnv)?
        .ok_or_else(|| {
            MaintenanceError::Config(
                "reindex requires [knowledge.embedding] and a working PostgreSQL connection"
                    .to_string(),
            )
        })?;

    run_reindex_with_components(&wiki, &semantic, options).await
}

pub fn audit_wiki(
    config: &KnowledgeConfig,
    now: DateTime<Utc>,
) -> Result<WikiAuditReport, MaintenanceError> {
    let wiki = Wiki::open(config.to_wiki_config().map_err(MaintenanceError::Config)?)?;

    let mut stale_pages = Vec::new();
    let mut pages_without_frontmatter = Vec::new();
    let entries = wiki.list()?;

    for entry in &entries {
        let raw = wiki.read(&entry.name)?;
        if !raw_has_frontmatter(&raw) {
            pages_without_frontmatter.push(entry.name.clone());
        }

        let parsed = parse_page(&raw)?;
        if let Some(updated) = parsed.frontmatter.updated {
            let days_stale = now.signed_duration_since(updated).num_days();
            if days_stale >= i64::from(config.reindex.stale_threshold_days) {
                stale_pages.push(StalePage {
                    name: entry.name.clone(),
                    updated,
                    days_stale,
                });
            }
        }
    }

    stale_pages.sort_by(|a, b| a.updated.cmp(&b.updated).then_with(|| a.name.cmp(&b.name)));
    pages_without_frontmatter.sort();

    Ok(WikiAuditReport {
        generated_at: now,
        wiki_path: config.wiki_path.clone(),
        total_pages: entries.len(),
        stale_threshold_days: config.reindex.stale_threshold_days,
        stale_pages,
        pages_without_frontmatter,
    })
}

async fn run_reindex_with_components(
    wiki: &Wiki,
    semantic: &SemanticBackend,
    options: ReindexOptions,
) -> Result<ReindexReport, MaintenanceError> {
    let mut report = ReindexReport {
        mode: options.mode,
        dry_run: options.dry_run,
        scanned: 0,
        reembedded: 0,
        deleted: 0,
        skipped: 0,
        actions: Vec::new(),
    };

    let entries = wiki.list()?;
    let indexed_pages = semantic.page_hashes().await?;

    match options.mode {
        ReindexMode::Full => {
            if !options.dry_run {
                semantic.truncate_all().await?;
            }

            for entry in &entries {
                report.scanned += 1;
                let raw = wiki.read(&entry.name)?;
                if !options.dry_run {
                    semantic.index_page(&entry.name, &raw).await?;
                }
                report.reembedded += 1;
                report.actions.push(ReindexAction {
                    page_name: entry.name.clone(),
                    kind: ReindexActionKind::Reembedded,
                });
            }
        }
        ReindexMode::Incremental => {
            let mut fs_pages = HashSet::new();
            for entry in &entries {
                report.scanned += 1;
                fs_pages.insert(entry.name.clone());

                let raw = wiki.read(&entry.name)?;
                let hash = SemanticBackend::content_hash(&raw);
                let needs_reindex = match indexed_pages.get(&entry.name) {
                    Some(existing) => existing != &hash,
                    None => true,
                };

                if needs_reindex {
                    if !options.dry_run {
                        semantic.index_page(&entry.name, &raw).await?;
                    }
                    report.reembedded += 1;
                    report.actions.push(ReindexAction {
                        page_name: entry.name.clone(),
                        kind: ReindexActionKind::Reembedded,
                    });
                } else {
                    report.skipped += 1;
                    report.actions.push(ReindexAction {
                        page_name: entry.name.clone(),
                        kind: ReindexActionKind::Skipped,
                    });
                }
            }

            let mut deleted_pages: Vec<String> = indexed_pages
                .keys()
                .filter(|page_name| !fs_pages.contains(*page_name))
                .cloned()
                .collect();
            deleted_pages.sort();

            for page_name in deleted_pages {
                if !options.dry_run {
                    semantic.delete_page(&page_name).await?;
                }
                report.deleted += 1;
                report.actions.push(ReindexAction {
                    page_name,
                    kind: ReindexActionKind::Deleted,
                });
            }
        }
    }

    Ok(report)
}

fn raw_has_frontmatter(raw: &str) -> bool {
    raw.starts_with("---\n") || raw.starts_with("---\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::TimeZone;
    use gadgetron_testing::harness::pg::PgHarness;
    use tempfile::TempDir;

    use crate::config::{
        EmbeddingConfig, EmbeddingWriteMode, KnowledgeCurationConfig, ReindexConfig,
    };
    use crate::embedding::{EmbeddingError, EmbeddingProvider};

    #[derive(Clone)]
    struct FakeEmbeddingProvider {
        dimension: usize,
    }

    impl FakeEmbeddingProvider {
        fn new(dimension: usize) -> Self {
            Self { dimension }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for FakeEmbeddingProvider {
        async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
            Ok(texts
                .iter()
                .map(|text| {
                    let mut vec = vec![0.0; self.dimension];
                    let idx = text.len() % self.dimension.max(1);
                    vec[idx] = 1.0;
                    vec
                })
                .collect())
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        fn model_name(&self) -> &str {
            "fake"
        }
    }

    fn test_config(root: PathBuf) -> KnowledgeConfig {
        KnowledgeConfig {
            wiki_path: root,
            wiki_autocommit: true,
            wiki_git_author: Some("Test <test@example.local>".to_string()),
            wiki_max_page_bytes: 1024 * 1024,
            search: None,
            embedding: Some(EmbeddingConfig {
                write_mode: EmbeddingWriteMode::Sync,
                ..EmbeddingConfig::default()
            }),
            reindex: ReindexConfig::default(),
            curation: KnowledgeCurationConfig::default(),
        }
    }

    fn file_path(root: &std::path::Path, page_name: &str) -> PathBuf {
        let mut path = root.join(page_name);
        path.set_extension("md");
        path
    }

    fn seeded_wiki() -> (TempDir, Wiki) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("wiki");
        fs::create_dir_all(root.join("notes")).expect("mkdir");
        fs::write(root.join("notes/existing.md"), "# Existing\n").expect("seed file");
        let wiki = Wiki::open(test_config(root).to_wiki_config().expect("wiki config"))
            .expect("open wiki");
        (dir, wiki)
    }

    async fn semantic_pg_available() -> bool {
        let admin_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
        let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
        else {
            return false;
        };

        let available: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
            "SELECT default_version FROM pg_available_extensions WHERE name = 'vector'",
        )
        .fetch_optional(&pool)
        .await;
        pool.close().await;
        matches!(available, Ok(Some(_)))
    }

    #[tokio::test]
    async fn reindex_incremental_picks_up_manually_edited_file() {
        if !semantic_pg_available().await {
            eprintln!("skipping semantic pg test: vector extension is unavailable");
            return;
        }
        let harness = PgHarness::new().await;
        let (_dir, wiki) = seeded_wiki();
        let semantic = SemanticBackend::new(
            harness.pool.clone(),
            Arc::new(FakeEmbeddingProvider::new(1536)) as Arc<dyn EmbeddingProvider>,
            EmbeddingWriteMode::Sync,
        );

        let root = wiki.config().root.clone();
        let manual = file_path(&root, "notes/manual-edit");
        fs::create_dir_all(manual.parent().expect("parent")).expect("mkdir parent");
        fs::write(
            &manual,
            "---\ntags = [\"manual\"]\n---\n\nGPU fan cable reseat fixed the boot loop.\n",
        )
        .expect("write manual file");

        let report = run_reindex_with_components(&wiki, &semantic, ReindexOptions::default())
            .await
            .expect("reindex");

        assert_eq!(report.reembedded, 2, "existing + manual pages reindexed");
        assert!(report
            .actions
            .iter()
            .any(|action| action.page_name == "notes/manual-edit"
                && action.kind == ReindexActionKind::Reembedded));

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wiki_pages WHERE page_name = $1")
            .bind("notes/manual-edit")
            .fetch_one(&harness.pool)
            .await
            .expect("count");
        assert_eq!(count, 1);

        harness.cleanup().await;
    }

    #[tokio::test]
    async fn reindex_incremental_removes_deleted_file_from_db() {
        if !semantic_pg_available().await {
            eprintln!("skipping semantic pg test: vector extension is unavailable");
            return;
        }
        let harness = PgHarness::new().await;
        let (_dir, wiki) = seeded_wiki();
        let semantic = SemanticBackend::new(
            harness.pool.clone(),
            Arc::new(FakeEmbeddingProvider::new(1536)) as Arc<dyn EmbeddingProvider>,
            EmbeddingWriteMode::Sync,
        );

        run_reindex_with_components(&wiki, &semantic, ReindexOptions::default())
            .await
            .expect("initial reindex");

        fs::remove_file(file_path(&wiki.config().root, "notes/existing")).expect("delete file");

        let report = run_reindex_with_components(&wiki, &semantic, ReindexOptions::default())
            .await
            .expect("second reindex");

        assert_eq!(report.deleted, 1);
        assert!(report
            .actions
            .iter()
            .any(|action| action.page_name == "notes/existing"
                && action.kind == ReindexActionKind::Deleted));

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wiki_pages WHERE page_name = $1")
            .bind("notes/existing")
            .fetch_one(&harness.pool)
            .await
            .expect("count");
        assert_eq!(count, 0);

        harness.cleanup().await;
    }

    #[tokio::test]
    async fn reindex_full_truncates_and_rebuilds() {
        if !semantic_pg_available().await {
            eprintln!("skipping semantic pg test: vector extension is unavailable");
            return;
        }
        let harness = PgHarness::new().await;
        let (_dir, wiki) = seeded_wiki();
        let semantic = SemanticBackend::new(
            harness.pool.clone(),
            Arc::new(FakeEmbeddingProvider::new(1536)) as Arc<dyn EmbeddingProvider>,
            EmbeddingWriteMode::Sync,
        );

        run_reindex_with_components(&wiki, &semantic, ReindexOptions::default())
            .await
            .expect("initial reindex");

        sqlx::query(
            "INSERT INTO wiki_pages (page_name, frontmatter, content_hash, indexed_at) \
             VALUES ('ghost/page', '{}'::jsonb, 'ghost', NOW())",
        )
        .execute(&harness.pool)
        .await
        .expect("insert ghost page");

        let report = run_reindex_with_components(
            &wiki,
            &semantic,
            ReindexOptions {
                mode: ReindexMode::Full,
                ..ReindexOptions::default()
            },
        )
        .await
        .expect("full reindex");

        assert_eq!(report.mode, ReindexMode::Full);
        assert_eq!(report.reembedded, 1);

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM wiki_pages WHERE page_name = 'ghost/page'")
                .fetch_one(&harness.pool)
                .await
                .expect("count");
        assert_eq!(count, 0);

        harness.cleanup().await;
    }

    #[tokio::test]
    async fn reindex_dry_run_makes_no_db_changes() {
        if !semantic_pg_available().await {
            eprintln!("skipping semantic pg test: vector extension is unavailable");
            return;
        }
        let harness = PgHarness::new().await;
        let (_dir, wiki) = seeded_wiki();
        let semantic = SemanticBackend::new(
            harness.pool.clone(),
            Arc::new(FakeEmbeddingProvider::new(1536)) as Arc<dyn EmbeddingProvider>,
            EmbeddingWriteMode::Sync,
        );

        let report = run_reindex_with_components(
            &wiki,
            &semantic,
            ReindexOptions {
                dry_run: true,
                ..ReindexOptions::default()
            },
        )
        .await
        .expect("dry run");

        assert_eq!(report.reembedded, 1);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wiki_pages")
            .fetch_one(&harness.pool)
            .await
            .expect("count");
        assert_eq!(count, 0);

        harness.cleanup().await;
    }

    #[test]
    fn audit_reports_stale_pages_and_missing_frontmatter() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("wiki");
        fs::create_dir_all(root.join("notes")).expect("mkdir");
        fs::write(
            root.join("notes/stale.md"),
            format!(
                "---\nupdated = {}\n---\n\nOld note.\n",
                Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                    .single()
                    .expect("date")
                    .to_rfc3339()
            ),
        )
        .expect("write stale");
        fs::write(root.join("notes/plain.md"), "# Plain\n").expect("write plain");

        let cfg = test_config(root);
        let report = audit_wiki(
            &cfg,
            Utc.with_ymd_and_hms(2026, 4, 18, 0, 0, 0)
                .single()
                .expect("date"),
        )
        .expect("audit");

        assert_eq!(report.total_pages, 2);
        assert_eq!(report.stale_pages.len(), 1);
        assert_eq!(report.stale_pages[0].name, "notes/stale");
        assert_eq!(report.stale_pages[0].days_stale, 107);
        assert_eq!(report.pages_without_frontmatter, vec!["notes/plain"]);
    }

    #[test]
    fn audit_render_includes_expected_sections() {
        let report = WikiAuditReport {
            generated_at: Utc
                .with_ymd_and_hms(2026, 4, 18, 12, 0, 0)
                .single()
                .expect("date"),
            wiki_path: PathBuf::from("/tmp/wiki"),
            total_pages: 2,
            stale_threshold_days: 90,
            stale_pages: vec![StalePage {
                name: "notes/stale".to_string(),
                updated: Utc
                    .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                    .single()
                    .expect("date"),
                days_stale: 107,
            }],
            pages_without_frontmatter: vec!["notes/plain".to_string()],
        };

        let rendered = report.render();
        assert!(rendered.contains("Wiki audit report"));
        assert!(rendered.contains("## Stale pages"));
        assert!(rendered.contains("notes/stale"));
        assert!(rendered.contains("## Pages without frontmatter"));
        assert!(rendered.contains("notes/plain"));
    }
}
