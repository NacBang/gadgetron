//! KC-1b canonical writeback E2E test (D-20260418-18).
//!
//! Authority: `docs/design/core/knowledge-candidate-curation.md` §2.1 + §2.3.
//!
//! # What this covers
//!
//! 1. Capture a `DirectAction` activity with one hint that leaves
//!    `proposed_path` unset — the `InProcessCandidateCoordinator`
//!    expands the `path_rules` template keyed on the `ActivityKind`
//!    snake_case label (`"direct_action"`).
//! 2. Accept the candidate through the real coordinator
//!    (`store.decide_candidate`).
//! 3. `materialize_accepted_candidate` — coordinator routes the payload
//!    through `KnowledgeService::write`, which hits the canonical
//!    `LlmWikiStore` and fans out to the `WikiKeywordIndex`.
//! 4. `knowledge_service.search("kafka")` — the just-written page must
//!    surface as a hit at the materialized path.
//!
//! # Skipping
//!
//! Uses the keyword index only (no pgvector / Postgres), but still
//! respects `GADGETRON_SKIP_POSTGRES_TESTS` to keep the skip surface
//! uniform with the other knowledge-layer E2E tests.

use std::collections::BTreeMap;
use std::sync::Arc;

use gadgetron_core::knowledge::candidate::{
    ActivityCaptureStore, ActivityKind, ActivityOrigin, CandidateDecision, CandidateDecisionKind,
    CandidateHint, CapturedActivityEvent, KnowledgeCandidateCoordinator, KnowledgeDocumentWrite,
};
use gadgetron_core::knowledge::{AuthenticatedContext, KnowledgeQuery, KnowledgeQueryMode};
use gadgetron_knowledge::candidate::{InMemoryActivityCaptureStore, InProcessCandidateCoordinator};
use gadgetron_knowledge::wiki::Wiki;
use gadgetron_knowledge::WikiKeywordIndex;
use gadgetron_knowledge::{KnowledgeService, KnowledgeServiceBuilder, LlmWikiStore};
use tempfile::TempDir;
use uuid::Uuid;

fn should_skip() -> bool {
    std::env::var("GADGETRON_SKIP_POSTGRES_TESTS").is_ok()
}

fn actor() -> AuthenticatedContext {
    AuthenticatedContext
}

fn build_wiki(dir: &TempDir) -> Arc<Wiki> {
    use gadgetron_knowledge::config::WikiConfig;
    let cfg = WikiConfig {
        root: dir.path().join("wiki"),
        autocommit: true,
        git_author_name: "KC-1b E2E".into(),
        git_author_email: "kc1b-e2e@test.local".into(),
        max_page_bytes: 1024 * 1024,
    };
    Arc::new(Wiki::open(cfg).expect("wiki open"))
}

fn build_service(wiki: Arc<Wiki>) -> Arc<KnowledgeService> {
    let store = Arc::new(LlmWikiStore::new(wiki).expect("llm-wiki store"));
    let keyword = Arc::new(WikiKeywordIndex::new().expect("keyword index"));
    KnowledgeServiceBuilder::new()
        .canonical_store(store)
        .add_index(keyword)
        .build()
        .expect("service build")
}

/// End-to-end: capture → accept → materialize → search.
///
/// Validates three KC-1b invariants in one flow:
///
/// 1. `capture_action` expands `path_rules` so a hint with
///    `proposed_path = None` lands at a deterministic template-derived
///    path rather than the `ops/journal/<uuid>` KC-1a fallback.
/// 2. `materialize_accepted_candidate` delegates to the wired
///    `KnowledgeService::write` (canonical + fanout) instead of the
///    synthetic-path stub.
/// 3. The canonical + keyword plugs agree — a subsequent search finds
///    the materialized page at the same path the coordinator returned.
#[tokio::test]
async fn kc1b_capture_accept_materialize_then_wiki_search_finds_it() {
    if should_skip() {
        eprintln!(
            "GADGETRON_SKIP_POSTGRES_TESTS set — skipping kc1b_canonical_write_e2e \
             (even though this case needs no PG)"
        );
        return;
    }

    // -------- Step 1 — build knowledge service + coordinator --------
    let dir = tempfile::tempdir().expect("tempdir");
    let wiki = build_wiki(&dir);
    let service = build_service(wiki);

    let store: Arc<dyn ActivityCaptureStore> = Arc::new(InMemoryActivityCaptureStore::new());
    // Scope the rule to `direct_action` so the coordinator can expand
    // `{date}` / `{topic}` against a `DirectAction` event. `{topic}` ==
    // `direct_action` here because ActivityKind serializes that way.
    let mut path_rules = BTreeMap::new();
    path_rules.insert(
        "direct_action".to_string(),
        "ops/journal/{date}/{topic}".to_string(),
    );
    let coord = InProcessCandidateCoordinator::new(store.clone(), /*max=*/ 8)
        .with_knowledge_service(service.clone())
        .with_path_rules(path_rules);

    // -------- Step 2 — capture with a no-path hint --------
    let event = CapturedActivityEvent {
        id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(),
        actor_user_id: Uuid::new_v4(),
        request_id: None,
        origin: ActivityOrigin::UserDirect,
        kind: ActivityKind::DirectAction,
        title: "kafka-restart".into(),
        summary: "restart kafka broker leader-07".into(),
        source_bundle: Some("bundle.ops".into()),
        source_capability: Some("service.restart".into()),
        audit_event_id: None,
        facts: serde_json::json!({ "broker": "leader-07" }),
        created_at: chrono::Utc::now(),
    };
    let event_date = event.created_at.format("%Y-%m-%d").to_string();

    let hints = vec![CandidateHint {
        summary: "restart kafka broker leader-07".into(),
        proposed_path: None, // triggers path_rules expansion
        tags: vec!["ops".into(), "kafka".into()],
        reason: Some("direct_action".into()),
    }];
    let created = coord
        .capture_action(&actor(), event, hints)
        .await
        .expect("capture_action must succeed");
    assert_eq!(
        created.len(),
        1,
        "single hint must produce a single candidate"
    );
    let candidate = &created[0];

    // Template-derived path: `ops/journal/<YYYY-MM-DD>/direct_action`.
    let expected_path = format!("ops/journal/{event_date}/direct_action");
    assert_eq!(
        candidate.proposed_path.as_deref(),
        Some(expected_path.as_str()),
        "path_rules expansion must land at the template-derived path; \
         got {:?}, expected {expected_path}",
        candidate.proposed_path,
    );
    let candidate_id = candidate.id;

    // -------- Step 3 — accept the candidate --------
    store
        .decide_candidate(
            &actor(),
            CandidateDecision {
                candidate_id,
                decision: CandidateDecisionKind::Accept,
                decided_by_user_id: None,
                decided_by_penny: true,
                rationale: Some("kc1b e2e".into()),
            },
        )
        .await
        .expect("decide_candidate(Accept) must succeed");

    // -------- Step 4 — materialize --------
    let mut provenance = BTreeMap::new();
    provenance.insert("source".into(), "kc1b-e2e".into());
    let write = KnowledgeDocumentWrite {
        path: expected_path.clone(),
        content: "# Restart kafka broker leader-07\n\nRestarted successfully.\n".into(),
        provenance,
    };
    let resolved_path = coord
        .materialize_accepted_candidate(&actor(), candidate_id, write)
        .await
        .expect("materialize must succeed when service is wired");
    assert!(
        resolved_path.starts_with("ops/journal/"),
        "materialized path must live under ops/journal/; got {resolved_path}"
    );
    assert_eq!(
        resolved_path, expected_path,
        "KnowledgeService::write must return the canonical path the \
         coordinator fed it"
    );

    // -------- Step 5 — keyword search finds the page --------
    let query = KnowledgeQuery {
        text: "kafka".into(),
        limit: 5,
        mode: KnowledgeQueryMode::Auto,
        include_relations: false,
    };
    let hits = service
        .search(&actor(), &query)
        .await
        .expect("knowledge.search must succeed");
    assert!(
        !hits.is_empty(),
        "search('kafka') must return at least one hit after materialization; got {hits:?}"
    );
    let match_hit = hits
        .iter()
        .find(|h| h.path == resolved_path)
        .unwrap_or_else(|| {
            panic!(
                "expected a hit with path == {resolved_path}; got paths={:?}",
                hits.iter().map(|h| &h.path).collect::<Vec<_>>()
            )
        });
    assert_eq!(
        match_hit.path, resolved_path,
        "search hit path must match the materialized path verbatim"
    );
}
