use std::collections::BTreeSet;

use gadgetron_core::{
    agent::{GadgetMode, GadgetsConfig},
    policy::{
        EnforcementPath, EvidenceAssessment, EvidenceState, OutcomeAssessment, OutcomeState,
        PolicyAuthorization, PolicyDocument, PolicyEffect, PolicyEvaluationRequest,
        PolicyEvaluator, PolicyInput, PolicyReviewState, PolicyRisk, RollbackAssessment,
        RollbackState,
    },
};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::policy::{self, PolicyRevisionSource, PolicyStoreError};
use uuid::Uuid;

async fn pg_available() -> bool {
    let admin_url = std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
        .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .is_ok()
}

async fn tenant_and_admin(pool: &sqlx::PgPool, label: &str) -> (Uuid, Uuid) {
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(tenant_id)
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1,$2,$3,$4,'admin','test')",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(format!("{label}@example.test"))
    .bind(label)
    .execute(pool)
    .await
    .unwrap();
    (tenant_id, user_id)
}

fn input(namespace: &str) -> PolicyInput {
    PolicyInput {
        action_id: format!("{namespace}.write"),
        gadget_name: Some(format!("{namespace}.write")),
        parameters_hash: None,
        namespace: namespace.to_string(),
        effect: PolicyEffect::Write,
        risk: PolicyRisk::Low,
        requested_scopes: BTreeSet::from(["management".to_string()]),
        actor_scopes: BTreeSet::from(["management".to_string()]),
        evidence: EvidenceAssessment {
            state: EvidenceState::Sufficient,
            references: BTreeSet::from(["evidence:1".to_string()]),
        },
        outcome: OutcomeAssessment {
            state: OutcomeState::Verifiable,
            predicate_ref: Some("outcome:v1".to_string()),
        },
        rollback: RollbackAssessment {
            state: RollbackState::Available,
            compensating_action: Some(format!("{namespace}.rollback")),
        },
    }
}

#[tokio::test]
async fn r3_2a_policy_revision_migration_isolation_conflict_and_decision_roundtrip() {
    if !pg_available().await {
        eprintln!("skipping R3.2a PostgreSQL fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let (tenant_a, admin_a) = tenant_and_admin(pool, "policy-a").await;
    let (tenant_b, admin_b) = tenant_and_admin(pool, "policy-b").await;

    let legacy = GadgetsConfig::default();
    let first = policy::ensure_legacy_policy(pool, tenant_a, Some(admin_a), &legacy)
        .await
        .unwrap();
    let repeated = policy::ensure_legacy_policy(pool, tenant_a, Some(admin_a), &legacy)
        .await
        .unwrap();
    assert_eq!(first.identity.policy_id, repeated.identity.policy_id);
    assert_eq!(first.identity.revision, 1);
    assert_eq!(
        serde_json::to_value(first.legacy_modes.as_ref().unwrap()).unwrap(),
        serde_json::to_value(&legacy).unwrap()
    );

    let other = policy::ensure_legacy_policy(pool, tenant_b, Some(admin_b), &legacy)
        .await
        .unwrap();
    assert_ne!(first.identity.policy_id, other.identity.policy_id);

    let mut changed = legacy.clone();
    changed.write.wiki_write = GadgetMode::Never;
    let changed_document = PolicyDocument::from_legacy_gadget_modes(&changed).unwrap();
    let mismatch = policy::create_revision(
        pool,
        tenant_a,
        Some(admin_a),
        1,
        PolicyRevisionSource::Manager,
        &changed_document,
        Some(&legacy),
    )
    .await
    .unwrap_err();
    assert!(matches!(mismatch, PolicyStoreError::InvalidPersisted(_)));

    let second = policy::create_revision(
        pool,
        tenant_a,
        Some(admin_a),
        1,
        PolicyRevisionSource::Manager,
        &changed_document,
        Some(&changed),
    )
    .await
    .unwrap();
    assert_eq!(second.identity.revision, 2);
    assert_eq!(second.identity.policy_id, first.identity.policy_id);

    let conflict = policy::create_revision(
        pool,
        tenant_a,
        Some(admin_a),
        1,
        PolicyRevisionSource::Manager,
        &changed_document,
        Some(&changed),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        conflict,
        PolicyStoreError::RevisionConflict {
            current_revision: 2
        }
    ));
    assert!(
        policy::policy_revision(pool, tenant_a, first.identity.policy_id, 1)
            .await
            .unwrap()
            .superseded_at
            .is_some()
    );
    assert_eq!(
        policy::active_policy(pool, tenant_a)
            .await
            .unwrap()
            .unwrap()
            .identity
            .revision,
        2
    );

    let request = input("wiki");
    let trace = second
        .document
        .evaluate(second.identity.clone(), &request)
        .unwrap();
    let event = policy::record_decision(
        pool,
        policy::PolicyDecisionRecord {
            tenant_id: tenant_a,
            event_id: Uuid::new_v4(),
            enforcement_path: EnforcementPath::WorkbenchAction,
            authorization: PolicyAuthorization::Denied,
            approval_id: None,
            input: &request,
            trace: &trace,
        },
    )
    .await
    .unwrap();
    assert_eq!(event.trace, trace);
    assert_eq!(
        policy::recent_decisions(pool, tenant_a, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(policy::recent_decisions(pool, tenant_b, 10)
        .await
        .unwrap()
        .is_empty());

    let mut forged = trace;
    forged.reason = "forged".to_string();
    let error = policy::record_decision(
        pool,
        policy::PolicyDecisionRecord {
            tenant_id: tenant_a,
            event_id: Uuid::new_v4(),
            enforcement_path: EnforcementPath::WorkbenchAction,
            authorization: PolicyAuthorization::Denied,
            approval_id: None,
            input: &request,
            trace: &forged,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, PolicyStoreError::TraceMismatch));

    harness.cleanup().await;
}

#[tokio::test]
async fn r3_2b_evaluator_pins_revision_and_records_review_authorization() {
    if !pg_available().await {
        eprintln!("skipping R3.2b PostgreSQL fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let (tenant_id, admin_id) = tenant_and_admin(pool, "policy-enforcement").await;
    let mut auto_modes = GadgetsConfig::default();
    auto_modes.write.wiki_write = GadgetMode::Auto;
    let evaluator = policy::PgPolicyEvaluator::new(pool.clone(), auto_modes.clone());
    let auto_identity = evaluator.active_identity(tenant_id).await.unwrap();

    let auto = evaluator
        .evaluate(PolicyEvaluationRequest {
            tenant_id,
            path: EnforcementPath::Tool,
            input: input("wiki"),
            pinned_policy: None,
            approval_id: None,
            review_state: PolicyReviewState::Pending,
        })
        .await
        .unwrap();
    assert_eq!(auto.authorization, PolicyAuthorization::Auto);

    let mut review_modes = auto_modes.clone();
    review_modes.write.wiki_write = GadgetMode::Ask;
    let review_revision = policy::create_revision(
        pool,
        tenant_id,
        Some(admin_id),
        1,
        PolicyRevisionSource::Manager,
        &PolicyDocument::from_legacy_gadget_modes(&review_modes).unwrap(),
        Some(&review_modes),
    )
    .await
    .unwrap();
    let pending = evaluator
        .evaluate(PolicyEvaluationRequest {
            tenant_id,
            path: EnforcementPath::BundleBackground,
            input: input("wiki"),
            pinned_policy: None,
            approval_id: None,
            review_state: PolicyReviewState::Pending,
        })
        .await
        .unwrap();
    assert_eq!(pending.authorization, PolicyAuthorization::PendingReview);
    assert_eq!(pending.trace.policy, review_revision.identity);

    let approval_id = Uuid::new_v4();
    let approved = evaluator
        .evaluate(PolicyEvaluationRequest {
            tenant_id,
            path: EnforcementPath::ReviewResume,
            input: input("wiki"),
            pinned_policy: Some(review_revision.identity.clone()),
            approval_id: Some(approval_id),
            review_state: PolicyReviewState::Approved,
        })
        .await
        .unwrap();
    assert_eq!(approved.authorization, PolicyAuthorization::ApprovedReview);

    let mut deny_modes = review_modes.clone();
    deny_modes.write.wiki_write = GadgetMode::Never;
    policy::create_revision(
        pool,
        tenant_id,
        Some(admin_id),
        2,
        PolicyRevisionSource::Manager,
        &PolicyDocument::from_legacy_gadget_modes(&deny_modes).unwrap(),
        Some(&deny_modes),
    )
    .await
    .unwrap();
    let denied = evaluator
        .evaluate(PolicyEvaluationRequest {
            tenant_id,
            path: EnforcementPath::WorkbenchAction,
            input: input("wiki"),
            pinned_policy: None,
            approval_id: None,
            review_state: PolicyReviewState::Pending,
        })
        .await
        .unwrap();
    assert_eq!(denied.authorization, PolicyAuthorization::Denied);

    let pinned = evaluator
        .evaluate(PolicyEvaluationRequest {
            tenant_id,
            path: EnforcementPath::KnowledgeBackground,
            input: input("wiki"),
            pinned_policy: Some(review_revision.identity),
            approval_id: Some(approval_id),
            review_state: PolicyReviewState::Approved,
        })
        .await
        .unwrap();
    assert_eq!(pinned.authorization, PolicyAuthorization::ApprovedReview);
    assert_ne!(pinned.trace.policy, auto_identity);

    let ledger = policy::recent_decisions(pool, tenant_id, 10).await.unwrap();
    assert!(ledger.iter().any(|event| {
        event.enforcement_path == "review_resume"
            && event.authorization == "approved_review"
            && event.approval_id == Some(approval_id)
    }));
    assert!(ledger
        .iter()
        .any(|event| event.enforcement_path == "knowledge_background"));

    harness.cleanup().await;
}
