use gadgetron_core::agent::{AgentBackend, AgentEffort, ConversationAgentProfile, ModelSource};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{chat_completion_jobs as jobs, conversations};
use uuid::Uuid;

#[tokio::test]
async fn durable_chat_jobs_terminalize_once_and_stay_owner_scoped() {
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let (tenant_a, user_a) = tenant_and_user(pool, "chat-a").await;
    let (tenant_b, user_b) = tenant_and_user(pool, "chat-b").await;
    let conversation_a = Uuid::new_v4();
    conversations::upsert_turn(
        pool,
        conversation_a,
        tenant_a,
        user_a,
        None,
        "Keep this request through restart",
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

    let complete_id = Uuid::new_v4();
    start(
        pool,
        complete_id,
        conversation_a,
        tenant_a,
        user_a,
        &profile,
    )
    .await;
    let duplicate = jobs::start(
        pool,
        jobs::StartChatCompletionJob {
            job_id: Uuid::new_v4(),
            conversation_id: conversation_a,
            tenant_id: tenant_a,
            user_id: user_a,
            model: "penny",
            agent_profile: None,
        },
    )
    .await;
    assert!(matches!(duplicate, Err(sqlx::Error::Database(_))));
    assert!(
        jobs::latest_terminal_for_conversation(pool, tenant_a, user_a, conversation_a)
            .await
            .unwrap()
            .is_none()
    );
    assert!(jobs::finish_with_assistant_message(
        pool,
        complete_id,
        jobs::TerminalStatus::Complete,
        4,
        None,
        Some("Completed answer"),
    )
    .await
    .unwrap());
    assert!(
        !jobs::finish(pool, complete_id, jobs::TerminalStatus::Cancelled, 5, None)
            .await
            .unwrap()
    );
    let completed = jobs::latest_terminal_for_conversation(pool, tenant_a, user_a, conversation_a)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.status, "complete");
    assert_eq!(completed.chunk_count, 4);
    assert_eq!(completed.agent_profile.unwrap()["model"], "gpt-5.6-sol");
    let completed_messages: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = $1 AND content = 'Completed answer'",
    )
    .bind(conversation_a)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(completed_messages, 1);

    let rollback_conversation = Uuid::new_v4();
    conversations::upsert_turn(
        pool,
        rollback_conversation,
        tenant_a,
        user_a,
        None,
        "Keep transcript and terminal state atomic",
    )
    .await
    .unwrap();
    let rollback_id = Uuid::new_v4();
    start(
        pool,
        rollback_id,
        rollback_conversation,
        tenant_a,
        user_a,
        &profile,
    )
    .await;
    assert!(jobs::finish_with_assistant_message(
        pool,
        rollback_id,
        jobs::TerminalStatus::Complete,
        1,
        None,
        Some("invalid\0message"),
    )
    .await
    .is_err());
    let rolled_back_status: String =
        sqlx::query_scalar("SELECT status FROM chat_completion_jobs WHERE job_id = $1")
            .bind(rollback_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(rolled_back_status, "streaming");
    let rolled_back_messages: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = $1 AND role = 'assistant'",
    )
    .bind(rollback_conversation)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(rolled_back_messages, 0);
    assert!(
        jobs::finish(pool, rollback_id, jobs::TerminalStatus::Cancelled, 1, None,)
            .await
            .unwrap()
    );

    let interrupted_id = Uuid::new_v4();
    start(
        pool,
        interrupted_id,
        conversation_a,
        tenant_a,
        user_a,
        &profile,
    )
    .await;
    assert_eq!(jobs::recover_interrupted(pool).await.unwrap(), 1);
    assert_eq!(jobs::recover_interrupted(pool).await.unwrap(), 0);
    let interrupted =
        jobs::latest_terminal_for_conversation(pool, tenant_a, user_a, conversation_a)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(interrupted.job_id, interrupted_id);
    assert_eq!(interrupted.status, "error");
    assert_eq!(
        interrupted.error_message.as_deref(),
        Some(jobs::RESTART_TERMINAL_MESSAGE)
    );
    let restart_messages: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_messages \
         WHERE conversation_id = $1 AND tenant_id = $2 AND user_id = $3 AND content = $4",
    )
    .bind(conversation_a)
    .bind(tenant_a)
    .bind(user_a)
    .bind(jobs::RESTART_TERMINAL_MESSAGE)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(restart_messages, 1);
    assert!(
        jobs::latest_terminal_for_conversation(pool, tenant_b, user_b, conversation_a)
            .await
            .unwrap()
            .is_none()
    );

    let cancelled_conversation = Uuid::new_v4();
    conversations::upsert_turn(
        pool,
        cancelled_conversation,
        tenant_a,
        user_a,
        None,
        "Cancel this request",
    )
    .await
    .unwrap();
    let cancelled_id = Uuid::new_v4();
    start(
        pool,
        cancelled_id,
        cancelled_conversation,
        tenant_a,
        user_a,
        &profile,
    )
    .await;
    assert!(
        jobs::finish(pool, cancelled_id, jobs::TerminalStatus::Cancelled, 2, None,)
            .await
            .unwrap()
    );
    assert_eq!(
        jobs::latest_terminal_for_conversation(pool, tenant_a, user_a, cancelled_conversation,)
            .await
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );

    harness.cleanup().await;
}

async fn start(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    conversation_id: Uuid,
    tenant_id: Uuid,
    user_id: Uuid,
    profile: &ConversationAgentProfile,
) {
    jobs::start(
        pool,
        jobs::StartChatCompletionJob {
            job_id,
            conversation_id,
            tenant_id,
            user_id,
            model: "penny",
            agent_profile: Some(profile),
        },
    )
    .await
    .unwrap();
}

async fn tenant_and_user(pool: &sqlx::PgPool, label: &str) -> (Uuid, Uuid) {
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(tenant_id)
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO users (id, tenant_id, email, display_name, role, password_hash)
           VALUES ($1, $2, $3, $4, 'admin', 'test')"#,
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
