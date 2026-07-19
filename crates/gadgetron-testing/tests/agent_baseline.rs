use std::sync::Arc;

use gadgetron_core::agent::{
    AgentBackend, AgentEffort, ConversationAgentProfile, ModelSource, AUTO_MODEL_ID,
};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{conversations, llm_endpoints};
use serde_json::json;
use tokio::sync::Barrier;
use uuid::Uuid;

async fn insert_user(pool: &sqlx::PgPool, tenant_id: Uuid, label: &str) -> Uuid {
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role) \
         VALUES ($1, $2, $3, $4, 'member')",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(format!("{label}-{user_id}@example.invalid"))
    .bind(label)
    .execute(pool)
    .await
    .expect("insert profile test user");
    user_id
}

fn default_profile(backend: AgentBackend) -> ConversationAgentProfile {
    ConversationAgentProfile {
        backend,
        llm_endpoint_id: None,
        model: AUTO_MODEL_ID.to_string(),
        effort: AgentEffort::Auto,
        model_source: ModelSource::Default,
        local_base_url: String::new(),
        local_api_key_env: String::new(),
    }
}

fn updated_profile(backend: AgentBackend) -> ConversationAgentProfile {
    let model = match backend {
        AgentBackend::ClaudeCode => "claude-sonnet-5",
        AgentBackend::CodexExec => "gpt-5.6-sol",
    };
    ConversationAgentProfile {
        backend,
        llm_endpoint_id: None,
        model: model.to_string(),
        effort: AgentEffort::High,
        model_source: ModelSource::Default,
        local_base_url: String::new(),
        local_api_key_env: String::new(),
    }
}

#[tokio::test]
async fn conversation_profile_first_pin_is_atomic_and_owner_scoped() {
    let pg = PgHarness::new().await;
    let (tenant_id, _) = pg.insert_test_tenant().await;
    let owner_id = insert_user(pg.pool(), tenant_id, "profile-owner").await;
    let other_user_id = insert_user(pg.pool(), tenant_id, "profile-other").await;
    let conversation_id = Uuid::new_v4();

    // Race two different runtimes on an uncreated conversation. Exactly one
    // may establish the durable runtime; the other must observe the row lock
    // and fail with AgentBackendPinned.
    let barrier = Arc::new(Barrier::new(3));
    let claude_task = {
        let pool = pg.pool.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            conversations::upsert_conversation_agent_profile(
                &pool,
                conversation_id,
                tenant_id,
                owner_id,
                &default_profile(AgentBackend::ClaudeCode),
            )
            .await
        })
    };
    let codex_task = {
        let pool = pg.pool.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            conversations::upsert_conversation_agent_profile(
                &pool,
                conversation_id,
                tenant_id,
                owner_id,
                &default_profile(AgentBackend::CodexExec),
            )
            .await
        })
    };
    barrier.wait().await;

    let results = [
        claude_task.await.expect("Claude pin task joins"),
        codex_task.await.expect("Codex pin task joins"),
    ];
    let mut winner = None;
    let mut conflicts = 0;
    for result in results {
        match result {
            Ok(profile) => {
                assert!(winner.replace(profile.backend).is_none(), "one winner only");
            }
            Err(conversations::ConversationError::AgentBackendPinned { .. }) => conflicts += 1,
            Err(error) => panic!("unexpected first-pin result: {error}"),
        }
    }
    let winner = winner.expect("one runtime wins the first pin");
    assert_eq!(conflicts, 1, "the losing runtime must receive one conflict");

    let stored = conversations::get_conversation_agent_profile(
        pg.pool(),
        conversation_id,
        tenant_id,
        owner_id,
    )
    .await
    .expect("load stored profile")
    .expect("profile is pinned");
    assert_eq!(stored.backend, winner);
    assert_eq!(stored.model, AUTO_MODEL_ID);
    assert_eq!(stored.effort, AgentEffort::Auto);

    // Model/effort remain mutable inside the pinned runtime.
    let updated = conversations::upsert_conversation_agent_profile(
        pg.pool(),
        conversation_id,
        tenant_id,
        owner_id,
        &updated_profile(winner),
    )
    .await
    .expect("same-runtime profile update succeeds");
    assert_eq!(updated.backend, winner);
    assert_eq!(updated.effort, AgentEffort::High);

    // A different user neither sees nor mutates the profile. The read path is
    // existence-hiding; the write path reports the internal ownership error.
    let hidden = conversations::get_conversation_agent_profile(
        pg.pool(),
        conversation_id,
        tenant_id,
        other_user_id,
    )
    .await
    .expect("cross-owner lookup executes");
    assert!(hidden.is_none());
    let denied = conversations::upsert_conversation_agent_profile(
        pg.pool(),
        conversation_id,
        tenant_id,
        other_user_id,
        &updated_profile(winner),
    )
    .await;
    assert!(matches!(
        denied,
        Err(conversations::ConversationError::OwnershipMismatch)
    ));

    // The latest Codex catalog adds an explicit Ultra tier for GPT-5.6
    // Sol/Terra. Pin it through the real DB constraint and reload path so a
    // migration/parser drift cannot silently downgrade the chat selection.
    let ultra_conversation_id = Uuid::new_v4();
    let ultra_profile = ConversationAgentProfile {
        backend: AgentBackend::CodexExec,
        llm_endpoint_id: None,
        model: "gpt-5.6-sol".to_string(),
        effort: AgentEffort::Ultra,
        model_source: ModelSource::Default,
        local_base_url: String::new(),
        local_api_key_env: String::new(),
    };
    conversations::upsert_conversation_agent_profile(
        pg.pool(),
        ultra_conversation_id,
        tenant_id,
        owner_id,
        &ultra_profile,
    )
    .await
    .expect("persist Ultra Codex profile");
    let ultra_reloaded = conversations::get_conversation_agent_profile(
        pg.pool(),
        ultra_conversation_id,
        tenant_id,
        owner_id,
    )
    .await
    .expect("reload Ultra Codex profile")
    .expect("Ultra profile exists");
    assert_eq!(ultra_reloaded.effort, AgentEffort::Ultra);

    pg.cleanup().await;
}

#[tokio::test]
async fn endpoint_capability_snapshot_is_durable_and_tenant_scoped() {
    let pg = PgHarness::new().await;
    let (owner_tenant, _) = pg.insert_test_tenant().await;
    let (other_tenant, _) = pg.insert_test_tenant().await;

    let endpoint = llm_endpoints::create_llm_endpoint(
        pg.pool(),
        owner_tenant,
        "responses-probe",
        "openai_compatible",
        "openai_responses",
        "http://127.0.0.1:19112",
        Some("local-tool-model"),
    )
    .await
    .expect("create endpoint");

    let models = vec!["local-tool-model".to_string()];
    let details = json!({
        "models_reachable": true,
        "responses_status": 200,
        "responses_tool_call": true
    });
    let saved = llm_endpoints::update_llm_endpoint_capability(
        pg.pool(),
        owner_tenant,
        endpoint.id,
        llm_endpoints::LlmEndpointCapabilityUpdate {
            protocol: "openai_responses",
            model_id: Some("local-tool-model"),
            discovered_models: &models,
            health_status: "ok",
            last_error: None,
            last_latency_ms: Some(7),
            runtime_compatibility: "codex_exec",
            tool_status: "passed",
            tool_model_id: Some("local-tool-model"),
            last_tool_error: None,
            capability_details: &details,
        },
    )
    .await
    .expect("persist capability snapshot");

    assert_eq!(saved.protocol, "openai_responses");
    assert_eq!(saved.runtime_compatibility, "codex_exec");
    assert_eq!(saved.tool_status, "passed");
    assert_eq!(saved.tool_model_id.as_deref(), Some("local-tool-model"));
    assert_eq!(saved.discovered_models, json!(["local-tool-model"]));
    assert_eq!(saved.capability_details, details);
    assert!(saved.last_tool_probe_at.is_some());

    let reloaded = llm_endpoints::get_llm_endpoint(pg.pool(), owner_tenant, endpoint.id)
        .await
        .expect("owner reloads endpoint");
    assert_eq!(reloaded.tool_status, "passed");
    assert_eq!(reloaded.runtime_compatibility, "codex_exec");

    assert!(matches!(
        llm_endpoints::get_llm_endpoint(pg.pool(), other_tenant, endpoint.id).await,
        Err(llm_endpoints::LlmEndpointError::NotFound)
    ));
    assert!(matches!(
        llm_endpoints::delete_llm_endpoint(pg.pool(), other_tenant, endpoint.id).await,
        Err(llm_endpoints::LlmEndpointError::NotFound)
    ));
    llm_endpoints::get_llm_endpoint(pg.pool(), owner_tenant, endpoint.id)
        .await
        .expect("cross-tenant delete did not remove endpoint");

    pg.cleanup().await;
}

#[tokio::test]
async fn legacy_native_sessions_backfill_latest_runtime_without_row_loss() {
    let pg = PgHarness::new().await;
    let (tenant_id, _) = pg.insert_test_tenant().await;
    let user_id = insert_user(pg.pool(), tenant_id, "migration-owner").await;
    let conversation_id = Uuid::new_v4();

    conversations::create_conversation(
        pg.pool(),
        conversation_id,
        tenant_id,
        user_id,
        "Legacy profile migration",
    )
    .await
    .expect("insert legacy conversation");
    sqlx::query(
        "INSERT INTO conversation_agent_sessions \
             (conversation_id, tenant_id, user_id, backend, backend_session_id, updated_at) \
         VALUES \
             ($1, $2, $3, 'claude_code', 'legacy-claude', now() - interval '1 hour'), \
             ($1, $2, $3, 'codex_exec', 'legacy-codex', now())",
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(pg.pool())
    .await
    .expect("insert both legacy native sessions");

    // PgHarness starts at HEAD. Reverse only the five 20260710 migrations to
    // model the immediately-previous 0.5.x schema, then let the production
    // Migrator apply them again. Keeping this downgrade fixture next to the
    // assertion makes changes to those migrations trip CI deterministically.
    sqlx::raw_sql(
        r#"
        ALTER TABLE conversations DROP CONSTRAINT conversations_agent_effort_check;
        ALTER TABLE conversations ADD CONSTRAINT conversations_agent_effort_check CHECK (
          agent_effort IS NULL OR agent_effort IN ('low', 'medium', 'high', 'xhigh', 'max')
        );
        ALTER TABLE conversations DROP COLUMN agent_endpoint_id;
        ALTER TABLE agent_brain_settings DROP COLUMN llm_endpoint_id;
        ALTER TABLE llm_endpoints
          DROP CONSTRAINT llm_endpoints_discovered_models_array_check,
          DROP CONSTRAINT llm_endpoints_capability_details_object_check,
          DROP CONSTRAINT llm_endpoints_runtime_compatibility_check,
          DROP CONSTRAINT llm_endpoints_tool_status_check,
          DROP COLUMN discovered_models,
          DROP COLUMN runtime_compatibility,
          DROP COLUMN tool_status,
          DROP COLUMN tool_model_id,
          DROP COLUMN last_tool_probe_at,
          DROP COLUMN last_tool_error,
          DROP COLUMN capability_details;
        ALTER TABLE llm_endpoints DROP CONSTRAINT llm_endpoints_protocol_check;
        ALTER TABLE llm_endpoints ADD CONSTRAINT llm_endpoints_protocol_check CHECK (
          protocol IN ('openai_chat', 'anthropic_messages')
        );
        DROP INDEX conversations_agent_backend_idx;
        ALTER TABLE conversations
          DROP COLUMN agent_backend,
          DROP COLUMN agent_model,
          DROP COLUMN agent_effort,
          DROP COLUMN agent_model_source,
          DROP COLUMN agent_local_base_url,
          DROP COLUMN agent_local_api_key_env;
        DELETE FROM _sqlx_migrations
        WHERE version BETWEEN 20260710000001 AND 20260710000005;
        "#,
    )
    .execute(pg.pool())
    .await
    .expect("construct pre-profile migration fixture");

    let migrator = sqlx::migrate!("../../crates/gadgetron-xaas/migrations");
    migrator
        .run(pg.pool())
        .await
        .expect("reapply profile and endpoint migrations");

    let profile: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT agent_backend, agent_effort, agent_model_source \
         FROM conversations WHERE id = $1",
    )
    .bind(conversation_id)
    .fetch_one(pg.pool())
    .await
    .expect("read migrated profile");
    assert_eq!(profile.0.as_deref(), Some("codex_exec"));
    assert_eq!(profile.1.as_deref(), Some("max"));
    assert_eq!(profile.2.as_deref(), Some("default"));

    let session_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_agent_sessions WHERE conversation_id = $1",
    )
    .bind(conversation_id)
    .fetch_one(pg.pool())
    .await
    .expect("count preserved sessions");
    assert_eq!(
        session_count, 2,
        "migration must not discard native sessions"
    );

    let migration: i64 =
        sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations WHERE success")
            .fetch_one(pg.pool())
            .await
            .expect("read migration head");
    let expected_head = migrator
        .iter()
        .map(|migration| migration.version)
        .max()
        .expect("embedded migrator has at least one migration");
    assert_eq!(migration, expected_head);

    pg.cleanup().await;
}
