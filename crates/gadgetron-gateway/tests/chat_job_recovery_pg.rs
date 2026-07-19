use gadgetron_core::agent::{AgentBackend, AgentEffort, ConversationAgentProfile, ModelSource};
use gadgetron_gateway::chat_jobs::{JobStatus, JobStore};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{chat_completion_jobs, conversations};
use uuid::Uuid;

#[tokio::test]
async fn restarted_job_store_recovers_interrupted_generation_for_its_owner() {
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let (tenant_id, user_id) = tenant_and_user(pool).await;
    let conversation_id = Uuid::new_v4();
    conversations::upsert_turn(
        pool,
        conversation_id,
        tenant_id,
        user_id,
        None,
        "Preserve this request across a process restart",
    )
    .await
    .unwrap();
    let profile = ConversationAgentProfile {
        backend: AgentBackend::CodexExec,
        llm_endpoint_id: None,
        model: "gpt-5.6-sol".into(),
        effort: AgentEffort::High,
        model_source: ModelSource::Default,
        local_base_url: String::new(),
        local_api_key_env: String::new(),
    };

    let first_process = JobStore::with_postgres(pool.clone());
    let interrupted = first_process
        .create_exclusive(
            conversation_id,
            Some(user_id),
            tenant_id,
            "penny".into(),
            Some(profile.clone()),
        )
        .await
        .unwrap();
    let interrupted_id = interrupted.job_id;
    drop(interrupted);
    drop(first_process);

    let restarted_process = JobStore::with_postgres(pool.clone());
    assert_eq!(restarted_process.recover_interrupted().await.unwrap(), 1);
    assert_eq!(restarted_process.recover_interrupted().await.unwrap(), 0);

    let recovered = restarted_process
        .latest_terminal_for_conversation(tenant_id, user_id, conversation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.job_id, interrupted_id);
    assert!(matches!(recovered.status, JobStatus::Error));
    assert!(recovered.is_finished);
    assert_eq!(
        recovered.error_message.as_deref(),
        Some(chat_completion_jobs::RESTART_TERMINAL_MESSAGE)
    );
    let recovered_profile = recovered.agent_profile.unwrap();
    assert_eq!(recovered_profile.model, profile.model);
    assert_eq!(recovered_profile.effort, profile.effort);
    assert!(restarted_process
        .latest_terminal_for_conversation(tenant_id, Uuid::new_v4(), conversation_id)
        .await
        .unwrap()
        .is_none());

    let retry = restarted_process
        .create_exclusive(
            conversation_id,
            Some(user_id),
            tenant_id,
            "penny".into(),
            Some(profile),
        )
        .await
        .unwrap();
    retry
        .mark_complete_with_assistant_message("Recovered answer")
        .await;
    assert!(matches!(retry.snapshot().await.status, JobStatus::Complete));
    let recovered_answer_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = $1 AND content = 'Recovered answer'",
    )
    .bind(conversation_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(recovered_answer_count, 1);

    harness.cleanup().await;
}

async fn tenant_and_user(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'chat-recovery')")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO users (id, tenant_id, email, display_name, role, password_hash)
           VALUES ($1, $2, 'recovery@example.test', 'Recovery', 'admin', 'test')"#,
    )
    .bind(user_id)
    .bind(tenant_id)
    .execute(pool)
    .await
    .unwrap();
    (tenant_id, user_id)
}
