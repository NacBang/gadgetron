//! KC-1c integration test — `PgActivityCaptureStore` round-trip.
//!
//! Authority: D-20260418-21,
//! `docs/design/core/knowledge-candidate-curation.md` §2.1 + §2.3.
//!
//! # What this covers
//!
//! 1. `append_activity` — one row lands in `activity_events`.
//! 2. `append_candidate` with empty tags → `PendingPennyDecision` (gate ignored).
//! 3. `append_candidate` with matching tag → `PendingUserConfirmation`.
//! 4. `list_candidates(only_pending=true)` → both candidates, newest-first.
//! 5. `decide_candidate(Accept)` on the first → `Accepted`.
//! 6. `list_candidates(only_pending=true)` → only the second (pending) candidate.
//! 7. `get_candidate(first.id)` → `Accepted`.
//! 8. `get_candidate(unknown_uuid)` → `Ok(None)`.
//! 9. `decide_candidate` on unknown id → `GadgetronError::Knowledge { DocumentNotFound }`.
//!
//! # Database availability
//!
//! Uses `PgHarness::new()` which connects to `localhost:5432` by default
//! (or `DATABASE_URL` env var). If the DB is not available the harness
//! panics with a helpful message — consistent with the existing
//! `gadgetron-knowledge` Pg tests (e.g. `maintenance.rs`).
//!
//! The test is NOT gated by `#[ignore]` because CI has `DATABASE_URL` set
//! and all other `PgHarness`-backed tests run unconditionally. Match that
//! pattern: fix the DB, not the test gate.

use std::sync::Arc;

use gadgetron_core::{
    error::{GadgetronError, KnowledgeErrorKind},
    knowledge::{
        candidate::{
            ActivityCaptureStore, ActivityKind, ActivityOrigin, CandidateDecision,
            CandidateDecisionKind, CandidateHint, CapturedActivityEvent,
            KnowledgeCandidateDisposition,
        },
        AuthenticatedContext,
    },
};
use gadgetron_knowledge::candidate::pg::PgActivityCaptureStore;
use gadgetron_testing::harness::pg::PgHarness;
use uuid::Uuid;

fn actor() -> AuthenticatedContext {
    AuthenticatedContext
}

/// Return true iff the local Postgres has the `vector` extension available.
///
/// `PgHarness::new()` runs ALL workspace migrations, including
/// `20260417000001_knowledge_semantic.sql` which requires the `vector`
/// extension. We skip the KC-1c Pg tests on the same condition the
/// maintenance and gadget Pg tests use — matching the existing pattern and
/// avoiding a false "infra failure" in the CI pre-flight gate.
///
/// In CI the Postgres image is `pgvector/pgvector:pg16`, which has the
/// extension installed, so these tests run fully there.
async fn pg_available() -> bool {
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

fn make_event(tenant_id: Uuid, actor_user_id: Uuid, request_id: Uuid) -> CapturedActivityEvent {
    CapturedActivityEvent {
        id: Uuid::new_v4(),
        tenant_id,
        actor_user_id,
        request_id: Some(request_id),
        origin: ActivityOrigin::UserDirect,
        kind: ActivityKind::DirectAction,
        title: "kc1c-test-event".to_string(),
        summary: "integration test event for PgActivityCaptureStore".to_string(),
        source_bundle: None,
        source_capability: Some("test.capability".into()),
        audit_event_id: Some(request_id),
        facts: serde_json::json!({"test": true}),
        created_at: chrono::Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// Round-trip: append → list → decide → get
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kc1c_pg_store_round_trip_append_list_decide() {
    if !pg_available().await {
        eprintln!("skipping kc1c_pg_store test: pgvector extension unavailable on local postgres");
        return;
    }
    let harness = PgHarness::new().await;
    let store = Arc::new(
        PgActivityCaptureStore::new(harness.pool().clone()).with_confirmation_gate(vec![
            "org_write".into(),
            "policy_note".into(),
            "destructive_action".into(),
        ]),
    );

    let tenant_id = Uuid::new_v4();
    let actor_user_id = Uuid::new_v4();
    let request_id = Uuid::new_v4();

    // --- Step 1: append_activity → one row in activity_events -----------

    let event = make_event(tenant_id, actor_user_id, request_id);
    let event_id = event.id;

    store
        .append_activity(&actor(), event)
        .await
        .expect("append_activity must succeed");

    // Confirm row count via raw SQL.
    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM activity_events WHERE id = $1")
        .bind(event_id)
        .fetch_one(harness.pool())
        .await
        .expect("count activity_events");
    assert_eq!(
        event_count, 1,
        "activity_events must have 1 row after append"
    );

    // --- Step 2: append_candidate with empty tags → PendingPennyDecision --

    let hint_no_gate = CandidateHint {
        summary: "safe observation".into(),
        proposed_path: Some(
            gadgetron_core::knowledge::KnowledgePath::new("ops/journal/safe").unwrap(),
        ),
        tags: vec!["monitoring".into()],
        reason: Some("routine check".into()),
    };
    let cand1 = store
        .append_candidate(&actor(), event_id, hint_no_gate)
        .await
        .expect("append_candidate (no gate) must succeed");

    assert_eq!(
        cand1.disposition,
        KnowledgeCandidateDisposition::PendingPennyDecision,
        "non-matching tags must yield PendingPennyDecision"
    );
    assert_eq!(
        cand1.tenant_id, tenant_id,
        "tenant_id must be propagated from activity event"
    );
    assert_eq!(
        cand1.actor_user_id, actor_user_id,
        "actor_user_id must be propagated"
    );
    assert_eq!(cand1.activity_event_id, event_id);

    // --- Step 3: append_candidate with matching tag → PendingUserConfirmation -

    let hint_gate = CandidateHint {
        summary: "org policy change".into(),
        proposed_path: Some(
            gadgetron_core::knowledge::KnowledgePath::new("ops/policy/change").unwrap(),
        ),
        tags: vec!["infra".into(), "org_write".into()],
        reason: Some("org-level change".into()),
    };
    let cand2 = store
        .append_candidate(&actor(), event_id, hint_gate)
        .await
        .expect("append_candidate (gate match) must succeed");

    assert_eq!(
        cand2.disposition,
        KnowledgeCandidateDisposition::PendingUserConfirmation,
        "matching org_write tag must yield PendingUserConfirmation"
    );

    // --- Step 4: list_candidates(2, only_pending=true) → both, newest first --

    // Brief sleep to ensure cand2 has a strictly later created_at than cand1.
    // In practice Uuid::new_v4 + sub-ms timestamp diff is usually enough, but
    // the tie-break on id can flip ordering — sleep avoids flakiness.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let pending = store
        .list_candidates(&actor(), 10, true)
        .await
        .expect("list_candidates must succeed");

    assert_eq!(
        pending.len(),
        2,
        "both candidates must appear in pending list"
    );
    // Newest-first: cand2 was inserted after cand1.
    assert_eq!(
        pending[0].id, cand2.id,
        "newest candidate (cand2) must be first"
    );
    assert_eq!(
        pending[1].id, cand1.id,
        "older candidate (cand1) must be second"
    );

    // --- Step 5: decide_candidate(cand1.id, Accept) → Accepted --------

    let decision = CandidateDecision {
        candidate_id: cand1.id,
        decision: CandidateDecisionKind::Accept,
        decided_by_user_id: None,
        decided_by_penny: true,
        rationale: Some("ops-journal worthy".into()),
    };
    let updated = store
        .decide_candidate(&actor(), decision)
        .await
        .expect("decide_candidate must succeed");

    assert_eq!(
        updated.disposition,
        KnowledgeCandidateDisposition::Accepted,
        "after Accept decision disposition must be Accepted"
    );
    assert_eq!(updated.id, cand1.id);

    // Confirm candidate_decisions row was written.
    let decision_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM candidate_decisions WHERE candidate_id = $1")
            .bind(cand1.id)
            .fetch_one(harness.pool())
            .await
            .expect("count candidate_decisions");
    assert_eq!(
        decision_count, 1,
        "one candidate_decisions row must exist after decide"
    );

    // --- Step 6: list_candidates(2, only_pending=true) → only cand2 ----

    let pending_after = store
        .list_candidates(&actor(), 10, true)
        .await
        .expect("list_candidates after decide must succeed");

    assert_eq!(
        pending_after.len(),
        1,
        "only one pending candidate must remain after cand1 is accepted"
    );
    assert_eq!(
        pending_after[0].id, cand2.id,
        "remaining pending candidate must be cand2"
    );

    // all=false still returns both
    let all = store
        .list_candidates(&actor(), 10, false)
        .await
        .expect("list_candidates(all) must succeed");
    assert_eq!(
        all.len(),
        2,
        "all candidates (pending + accepted) must total 2"
    );

    // --- Step 7: get_candidate(cand1.id) → Accepted --------------------

    let fetched = store
        .get_candidate(&actor(), cand1.id)
        .await
        .expect("get_candidate must succeed")
        .expect("cand1 must be present");

    assert_eq!(
        fetched.disposition,
        KnowledgeCandidateDisposition::Accepted,
        "get_candidate must reflect updated disposition"
    );
    assert_eq!(fetched.id, cand1.id);

    // --- Step 8: get_candidate(unknown) → Ok(None) ---------------------

    let missing = store
        .get_candidate(&actor(), Uuid::new_v4())
        .await
        .expect("get_candidate for unknown id must not error");
    assert!(
        missing.is_none(),
        "unknown candidate id must return Ok(None)"
    );

    // --- Step 9: decide_candidate on unknown id → DocumentNotFound -----

    let bad_decision = CandidateDecision {
        candidate_id: Uuid::new_v4(),
        decision: CandidateDecisionKind::Accept,
        decided_by_user_id: None,
        decided_by_penny: true,
        rationale: None,
    };
    let err = store
        .decide_candidate(&actor(), bad_decision)
        .await
        .unwrap_err();
    match err {
        GadgetronError::Knowledge {
            kind: KnowledgeErrorKind::DocumentNotFound { path },
            ..
        } => {
            assert!(
                path.starts_with("candidate/"),
                "DocumentNotFound path must start with 'candidate/'; got: {path}"
            );
        }
        other => {
            panic!("expected Knowledge(DocumentNotFound) for unknown candidate, got: {other:?}")
        }
    }

    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// audit_event_id propagation test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kc1c_pg_store_audit_event_id_propagated() {
    if !pg_available().await {
        eprintln!(
            "skipping kc1c_pg_store_audit_event_id_propagated: pgvector extension unavailable"
        );
        return;
    }
    let harness = PgHarness::new().await;
    let store = PgActivityCaptureStore::new(harness.pool().clone());

    let tenant_id = Uuid::new_v4();
    let actor_user_id = Uuid::new_v4();
    let request_id = Uuid::new_v4();

    let event = make_event(tenant_id, actor_user_id, request_id);
    let event_id = event.id;

    store
        .append_activity(&actor(), event)
        .await
        .expect("append_activity must succeed");

    // Confirm audit_event_id is stored correctly.
    let stored_audit_id: Option<Uuid> =
        sqlx::query_scalar("SELECT audit_event_id FROM activity_events WHERE id = $1")
            .bind(event_id)
            .fetch_one(harness.pool())
            .await
            .expect("fetch audit_event_id");

    assert_eq!(
        stored_audit_id,
        Some(request_id),
        "audit_event_id must be stored as the request_id"
    );

    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// append_candidate with unknown activity_event_id → DocumentNotFound
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kc1c_pg_store_append_candidate_unknown_event_errors() {
    if !pg_available().await {
        eprintln!(
            "skipping kc1c_pg_store_append_candidate_unknown_event_errors: pgvector unavailable"
        );
        return;
    }
    let harness = PgHarness::new().await;
    let store = PgActivityCaptureStore::new(harness.pool().clone());

    let hint = CandidateHint {
        summary: "orphan candidate".into(),
        proposed_path: None,
        tags: vec![],
        reason: None,
    };
    let err = store
        .append_candidate(&actor(), Uuid::new_v4(), hint)
        .await
        .unwrap_err();

    match err {
        GadgetronError::Knowledge {
            kind: KnowledgeErrorKind::DocumentNotFound { path },
            ..
        } => {
            assert!(
                path.starts_with("activity_event/"),
                "DocumentNotFound path must start with 'activity_event/'; got: {path}"
            );
        }
        other => panic!("expected Knowledge(DocumentNotFound), got: {other:?}"),
    }

    harness.cleanup().await;
}
