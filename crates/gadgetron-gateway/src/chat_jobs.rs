//! Background chat-completion jobs.
//!
//! A streaming chat completion (`POST /v1/chat/completions` with
//! `stream: true`) is split into:
//!
//!   * **Foreground SSE response** — the HTTP response the requesting
//!     client receives. It's just a view onto the job's chunk buffer.
//!   * **Background producer** — a `tokio::spawn`'d task that pulls
//!     from the LLM provider and pushes each chunk into the job's
//!     buffer. It runs to completion regardless of whether the
//!     foreground client is still connected.
//!
//! That separation is what lets the user navigate away from a chat
//! mid-stream and come back to find the response intact (the
//! producer kept going, the buffer kept growing). Returning clients
//! re-attach by querying `/conversations/{conv_id}/active-job`,
//! reading the `job_id`, then opening
//! `/jobs/{job_id}/sync?since=N` — they get a replay of every chunk
//! the producer has already buffered plus a live tail until the job
//! completes.
//!
//! `JobStore` and `JobState` own the live chunk buffer. PostgreSQL-backed
//! deployments also record job start and terminal metadata so startup can
//! turn an interrupted generation into an explicit retryable error. Chunk
//! replay remains process-local. The HTTP wiring lives in `handlers.rs` and
//! the resume endpoints live in `web/workbench.rs`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use gadgetron_core::agent::ConversationAgentProfile;
use gadgetron_xaas::chat_completion_jobs;
use sqlx::PgPool;
use tokio::sync::{Mutex, Notify, RwLock};
use uuid::Uuid;

/// How long a completed job stays in memory before being eligible
/// for cleanup. Keep large enough that operators who navigate away
/// for a coffee and come back can still resume; small enough that
/// stale jobs don't pin RAM forever.
pub const COMPLETED_JOB_TTL: Duration = Duration::from_secs(10 * 60);

/// How often the background cleanup task scans for expired jobs.
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Streaming,
    Complete,
    Error,
    /// Operator pressed stop: `POST /jobs/{id}/cancel` asked the
    /// producer to abandon the upstream stream. Whatever was buffered
    /// stays replayable until the TTL reaps the job.
    Cancelled,
}

#[derive(Debug)]
struct JobInner {
    status: JobStatus,
    /// Each entry is a single OpenAI-compatible SSE event body
    /// (already serialized including the trailing `\n\n`). Stored as
    /// `Bytes` so cloning into a per-subscriber stream is a pointer
    /// increment, not a copy.
    chunks: Vec<Bytes>,
    /// Captured when the job transitions to `Error`. Surfaced in
    /// `/conversations/{id}/active-job` and replayed as the final
    /// SSE error event on `sync`.
    error_message: Option<String>,
    /// Set when the job finishes (Complete, Error, or Cancelled). `sync` waiters
    /// check this before parking on `notify` so they don't sleep
    /// past the producer's last push.
    is_finished: bool,
}

/// Live state for one chat-completion job. Mutable bits are behind
/// a single `Mutex` so chunk-push + status-flip + watcher-wake stay
/// atomic. `Notify` wakes every subscriber currently blocked in
/// `tail_after`.
#[derive(Debug)]
pub struct JobState {
    pub job_id: Uuid,
    pub conversation_id: Uuid,
    pub user_id: Option<Uuid>,
    pub tenant_id: Uuid,
    pub model: String,
    /// Effective per-conversation runtime/model/effort snapshot used for this
    /// generation. Immutable even if the user edits the next turn's profile.
    pub agent_profile: Option<ConversationAgentProfile>,
    pub created_at: Instant,
    /// Last write timestamp — used by cleanup to bias toward
    /// killing the truly idle jobs first.
    completed_at: Mutex<Option<Instant>>,
    inner: Mutex<JobInner>,
    watchers: Notify,
    /// Cancellation signal. The cancel handler flips the flag and
    /// notifies; the producer's select loop observes it between
    /// chunks (`cancelled_signal`). Separate from `watchers`, which
    /// wakes buffer SUBSCRIBERS — mixing the two would wake every
    /// sync client on each cancel request.
    cancel_requested: std::sync::atomic::AtomicBool,
    cancel_notify: Notify,
    persistence: Option<PgPool>,
}

impl JobState {
    fn new(
        job_id: Uuid,
        conversation_id: Uuid,
        user_id: Option<Uuid>,
        tenant_id: Uuid,
        model: String,
        agent_profile: Option<ConversationAgentProfile>,
        persistence: Option<PgPool>,
    ) -> Self {
        Self {
            job_id,
            conversation_id,
            user_id,
            tenant_id,
            model,
            agent_profile,
            created_at: Instant::now(),
            completed_at: Mutex::new(None),
            inner: Mutex::new(JobInner {
                status: JobStatus::Streaming,
                chunks: Vec::new(),
                error_message: None,
                is_finished: false,
            }),
            watchers: Notify::new(),
            cancel_requested: std::sync::atomic::AtomicBool::new(false),
            cancel_notify: Notify::new(),
            persistence,
        }
    }

    /// Append one SSE-formatted chunk. Wakes every subscriber that
    /// was waiting for the buffer to grow.
    pub async fn push_chunk(&self, chunk: Bytes) {
        let mut inner = self.inner.lock().await;
        inner.chunks.push(chunk);
        drop(inner);
        self.watchers.notify_waiters();
    }

    /// Mark the job as completed normally. Final wake so any
    /// `tail_after` that was blocked gets a chance to drain the
    /// last chunks and observe `is_finished == true`.
    pub async fn mark_complete(&self) {
        self.mark_complete_with_assistant(None).await;
    }

    pub async fn mark_complete_with_assistant_message(&self, message: &str) {
        self.mark_complete_with_assistant(Some(message)).await;
    }

    async fn mark_complete_with_assistant(&self, assistant_message: Option<&str>) {
        let mut inner = self.inner.lock().await;
        inner.status = JobStatus::Complete;
        inner.is_finished = true;
        let chunk_count = inner.chunks.len();
        drop(inner);
        *self.completed_at.lock().await = Some(Instant::now());
        self.watchers.notify_waiters();
        self.persist_terminal(
            chat_completion_jobs::TerminalStatus::Complete,
            chunk_count,
            None,
            assistant_message,
        )
        .await;
    }

    /// Mark the job as failed. `error_message` is surfaced through
    /// the metadata endpoint and the SSE tail.
    pub async fn mark_error(&self, message: impl Into<String>) {
        let message = message.into();
        let mut inner = self.inner.lock().await;
        inner.status = JobStatus::Error;
        inner.error_message = Some(message.clone());
        inner.is_finished = true;
        let chunk_count = inner.chunks.len();
        drop(inner);
        *self.completed_at.lock().await = Some(Instant::now());
        self.watchers.notify_waiters();
        self.persist_terminal(
            chat_completion_jobs::TerminalStatus::Error,
            chunk_count,
            Some(&message),
            None,
        )
        .await;
    }

    /// Mark the job as cancelled by the operator. Terminal like
    /// `mark_complete` — subscribers drain the buffer and stop.
    pub async fn mark_cancelled(&self) {
        self.mark_cancelled_with_assistant(None).await;
    }

    pub async fn mark_cancelled_with_assistant_message(&self, message: &str) {
        self.mark_cancelled_with_assistant(Some(message)).await;
    }

    async fn mark_cancelled_with_assistant(&self, assistant_message: Option<&str>) {
        let mut inner = self.inner.lock().await;
        inner.status = JobStatus::Cancelled;
        inner.is_finished = true;
        let chunk_count = inner.chunks.len();
        drop(inner);
        *self.completed_at.lock().await = Some(Instant::now());
        self.watchers.notify_waiters();
        self.persist_terminal(
            chat_completion_jobs::TerminalStatus::Cancelled,
            chunk_count,
            None,
            assistant_message,
        )
        .await;
    }

    /// Ask the producer to stop pulling from the upstream stream. The
    /// actual transition to `Cancelled` happens in the producer once
    /// it observes the flag — callers read the snapshot for the
    /// eventual state. Idempotent; a no-op on finished jobs.
    pub fn request_cancel(&self) {
        self.cancel_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.cancel_notify.notify_waiters();
    }

    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_requested
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Resolves once `request_cancel` has been called. Same
    /// enable-before-check pattern as `wait_for_chunk_after` so a
    /// notify between the flag check and the park is not lost.
    pub async fn cancelled_signal(&self) {
        loop {
            let notified = self.cancel_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancel_requested() {
                return;
            }
            notified.await;
        }
    }

    /// Snapshot for the `/active-job` endpoint.
    pub async fn snapshot(&self) -> JobSnapshot {
        let inner = self.inner.lock().await;
        JobSnapshot {
            job_id: self.job_id,
            conversation_id: self.conversation_id,
            status: inner.status.clone(),
            chunk_count: inner.chunks.len(),
            is_finished: inner.is_finished,
            error_message: inner.error_message.clone(),
            agent_profile: self.agent_profile.clone(),
        }
    }

    /// Read every buffered chunk from `since` (inclusive index)
    /// onward, in order. Used by `tail_after` for the initial
    /// replay slice.
    pub async fn replay_from(&self, since: usize) -> (Vec<Bytes>, bool) {
        let inner = self.inner.lock().await;
        let from = since.min(inner.chunks.len());
        let slice: Vec<Bytes> = inner.chunks[from..].to_vec();
        (slice, inner.is_finished)
    }

    /// Subscriber-facing tail: produce chunks at index `since` and
    /// beyond, blocking on the watcher when caught up to the live
    /// edge. Resolves once the job is finished AND the caller has
    /// consumed every chunk.
    ///
    /// The returned future yields one batch per producer wake — the
    /// caller drains each batch and loops.
    pub async fn wait_for_chunk_after(&self, current: usize) -> WaitResult {
        loop {
            let notified = self.watchers.notified();
            tokio::pin!(notified);
            // Register with the Notify BEFORE checking the buffer.
            // `notify_waiters` stores no permit, so a producer push that
            // lands between the condition check and the first poll of
            // `notified` would otherwise be lost — fatal when it was the
            // job's final wake (mark_complete), leaving this subscriber
            // parked forever. With `enable()` first, a push after the
            // check wakes us; a push before it is seen by the check.
            notified.as_mut().enable();
            {
                let inner = self.inner.lock().await;
                if inner.chunks.len() > current {
                    let slice: Vec<Bytes> = inner.chunks[current..].to_vec();
                    let finished = inner.is_finished;
                    return WaitResult::Chunks {
                        chunks: slice,
                        finished,
                    };
                }
                if inner.is_finished {
                    return WaitResult::Finished;
                }
            }
            notified.await;
        }
    }

    async fn persist_terminal(
        &self,
        status: chat_completion_jobs::TerminalStatus,
        chunk_count: usize,
        error_message: Option<&str>,
        assistant_message: Option<&str>,
    ) {
        let Some(pool) = self.persistence.as_ref() else {
            return;
        };
        if let Err(error) = chat_completion_jobs::finish_with_assistant_message(
            pool,
            self.job_id,
            status,
            chunk_count,
            error_message,
            assistant_message,
        )
        .await
        {
            tracing::error!(
                job_id = %self.job_id,
                status = status.as_str(),
                error = %error,
                "chat job terminal state persistence failed"
            );
        }
    }
}

#[derive(Debug, Clone)]
pub enum WaitResult {
    Chunks {
        chunks: Vec<Bytes>,
        /// Whether the job finished while we were preparing this
        /// batch. The caller can skip another round trip when true.
        finished: bool,
    },
    Finished,
}

/// Public snapshot returned by the `/active-job` endpoint.
#[derive(Debug, Clone, serde::Serialize)]
pub struct JobSnapshot {
    pub job_id: Uuid,
    pub conversation_id: Uuid,
    pub status: JobStatus,
    pub chunk_count: usize,
    pub is_finished: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_profile: Option<ConversationAgentProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// Concurrent registry of in-flight + recently-completed jobs.
///
/// Two indexes:
///   * `by_id` — primary key (job_id → JobState). Source of truth.
///   * `by_conv` — conversation_id → currently-active job_id. The
///     `/conversations/{id}/active-job` endpoint uses this; a new
///     job for the same conversation overwrites the slot.
///
/// `RwLock` because reads (resume polling) vastly outnumber writes
/// (job creation / cleanup). `Arc<JobState>` so consumers can keep
/// a handle without holding the outer lock.
#[derive(Debug, Default)]
pub struct JobStore {
    by_id: RwLock<HashMap<Uuid, Arc<JobState>>>,
    by_conv: RwLock<HashMap<Uuid, Uuid>>,
    /// Serializes the check-and-register sequence for active-job exclusivity.
    create_guard: Mutex<()>,
    persistence: Option<PgPool>,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateJobError {
    #[error("conversation already has an active generation")]
    Active(Arc<JobState>),
    #[error("chat job persistence failed: {0}")]
    Persistence(#[from] sqlx::Error),
}

impl JobStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_postgres(pool: PgPool) -> Self {
        Self {
            persistence: Some(pool),
            ..Self::default()
        }
    }

    /// Register a fresh job and return the `Arc<JobState>` so the
    /// caller can push chunks into it.
    pub async fn create(
        &self,
        conversation_id: Uuid,
        user_id: Option<Uuid>,
        tenant_id: Uuid,
        model: String,
    ) -> Arc<JobState> {
        self.create_with_profile(conversation_id, user_id, tenant_id, model, None)
            .await
    }

    pub async fn create_with_profile(
        &self,
        conversation_id: Uuid,
        user_id: Option<Uuid>,
        tenant_id: Uuid,
        model: String,
        agent_profile: Option<ConversationAgentProfile>,
    ) -> Arc<JobState> {
        let job_id = Uuid::new_v4();
        let job = Arc::new(JobState::new(
            job_id,
            conversation_id,
            user_id,
            tenant_id,
            model,
            agent_profile,
            self.persistence.clone(),
        ));
        self.by_id.write().await.insert(job_id, Arc::clone(&job));
        self.by_conv.write().await.insert(conversation_id, job_id);
        job
    }

    /// Register a job only when the conversation has no unfinished producer.
    /// The guard closes the race between active lookup and insertion; callers
    /// receive the existing job so they can return a deterministic conflict.
    pub async fn create_exclusive(
        &self,
        conversation_id: Uuid,
        user_id: Option<Uuid>,
        tenant_id: Uuid,
        model: String,
        agent_profile: Option<ConversationAgentProfile>,
    ) -> Result<Arc<JobState>, CreateJobError> {
        let _guard = self.create_guard.lock().await;
        if let Some(existing) = self.active_for_conversation(conversation_id).await {
            if !existing.snapshot().await.is_finished {
                return Err(CreateJobError::Active(existing));
            }
        }
        let job_id = Uuid::new_v4();
        if let (Some(pool), Some(user_id)) = (self.persistence.as_ref(), user_id) {
            if !conversation_id.is_nil() {
                chat_completion_jobs::start(
                    pool,
                    chat_completion_jobs::StartChatCompletionJob {
                        job_id,
                        conversation_id,
                        tenant_id,
                        user_id,
                        model: &model,
                        agent_profile: agent_profile.as_ref(),
                    },
                )
                .await?;
            }
        }
        let job = Arc::new(JobState::new(
            job_id,
            conversation_id,
            user_id,
            tenant_id,
            model,
            agent_profile,
            self.persistence.clone(),
        ));
        self.by_id.write().await.insert(job_id, Arc::clone(&job));
        self.by_conv.write().await.insert(conversation_id, job_id);
        Ok(job)
    }

    pub async fn recover_interrupted(&self) -> Result<u64, sqlx::Error> {
        let Some(pool) = self.persistence.as_ref() else {
            return Ok(0);
        };
        chat_completion_jobs::recover_interrupted(pool).await
    }

    pub async fn latest_terminal_for_conversation(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        conversation_id: Uuid,
    ) -> Result<Option<JobSnapshot>, sqlx::Error> {
        let Some(pool) = self.persistence.as_ref() else {
            return Ok(None);
        };
        let Some(row) = chat_completion_jobs::latest_terminal_for_conversation(
            pool,
            tenant_id,
            user_id,
            conversation_id,
        )
        .await?
        else {
            return Ok(None);
        };
        let status = match row.status.as_str() {
            "complete" => JobStatus::Complete,
            "error" => JobStatus::Error,
            "cancelled" => JobStatus::Cancelled,
            _ => return Ok(None),
        };
        let agent_profile = row
            .agent_profile
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        Ok(Some(JobSnapshot {
            job_id: row.job_id,
            conversation_id: row.conversation_id,
            status,
            chunk_count: usize::try_from(row.chunk_count).unwrap_or_default(),
            is_finished: true,
            agent_profile,
            error_message: row.error_message,
        }))
    }

    /// Look up a job by id. None when expired or never registered.
    pub async fn get(&self, job_id: Uuid) -> Option<Arc<JobState>> {
        self.by_id.read().await.get(&job_id).cloned()
    }

    /// Look up the active (or most-recent) job for a conversation.
    /// `None` if the conversation has never run a streaming
    /// completion in this process, or the prior job has been
    /// reaped.
    pub async fn active_for_conversation(&self, conv_id: Uuid) -> Option<Arc<JobState>> {
        let job_id = *self.by_conv.read().await.get(&conv_id)?;
        self.get(job_id).await
    }

    /// Every registered job (streaming + recently completed). The
    /// batched `/jobs/active` endpoint filters this for caller
    /// visibility and liveness — one sidebar poll per interval instead
    /// of one per conversation row.
    pub async fn all_jobs(&self) -> Vec<Arc<JobState>> {
        self.by_id.read().await.values().cloned().collect()
    }

    /// Drop jobs that finished long enough ago to be reaped.
    /// Intended to be invoked from a periodic background task.
    pub async fn cleanup_expired(&self, now: Instant, ttl: Duration) {
        let mut by_id = self.by_id.write().await;
        let mut by_conv = self.by_conv.write().await;
        let mut to_remove: Vec<Uuid> = Vec::new();
        for (job_id, job) in by_id.iter() {
            if let Some(done_at) = *job.completed_at.lock().await {
                if now.duration_since(done_at) >= ttl {
                    to_remove.push(*job_id);
                }
            }
        }
        for job_id in &to_remove {
            if let Some(job) = by_id.remove(job_id) {
                // Only clear the conv index if this is still the
                // active job for that conv — a newer job may have
                // overwritten the slot.
                if let Some(active_job_id) = by_conv.get(&job.conversation_id) {
                    if active_job_id == job_id {
                        by_conv.remove(&job.conversation_id);
                    }
                }
            }
        }
    }

    /// Spawn a background loop that periodically reaps expired
    /// jobs. The returned `JoinHandle` is detached intentionally —
    /// the loop lives for the duration of the process.
    pub fn spawn_cleanup_task(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let store = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(CLEANUP_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                store
                    .cleanup_expired(Instant::now(), COMPLETED_JOB_TTL)
                    .await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::agent::{AgentBackend, AgentEffort, ModelSource};

    fn fresh_store() -> Arc<JobStore> {
        Arc::new(JobStore::new())
    }

    #[tokio::test]
    async fn create_then_get() {
        let store = fresh_store();
        let conv = Uuid::new_v4();
        let job = store.create(conv, None, Uuid::nil(), "penny".into()).await;
        let by_id = store.get(job.job_id).await.expect("by_id");
        assert_eq!(by_id.job_id, job.job_id);
        let by_conv = store.active_for_conversation(conv).await.expect("by_conv");
        assert_eq!(by_conv.job_id, job.job_id);
    }

    #[tokio::test]
    async fn replay_from_walks_chunks_in_order() {
        let store = fresh_store();
        let conv = Uuid::new_v4();
        let job = store.create(conv, None, Uuid::nil(), "penny".into()).await;
        job.push_chunk(Bytes::from_static(b"a")).await;
        job.push_chunk(Bytes::from_static(b"b")).await;
        job.push_chunk(Bytes::from_static(b"c")).await;
        let (all, finished) = job.replay_from(0).await;
        assert!(!finished);
        assert_eq!(
            all.iter().map(|b| b.as_ref()).collect::<Vec<_>>(),
            vec![b"a".as_ref(), b"b".as_ref(), b"c".as_ref()]
        );
        let (tail, _) = job.replay_from(2).await;
        assert_eq!(
            tail.iter().map(|b| b.as_ref()).collect::<Vec<_>>(),
            vec![b"c".as_ref()]
        );
    }

    #[tokio::test]
    async fn create_exclusive_rejects_a_second_live_job_and_snapshots_profile() {
        let store = fresh_store();
        let conv = Uuid::new_v4();
        let profile = ConversationAgentProfile {
            backend: AgentBackend::CodexExec,
            llm_endpoint_id: None,
            model: "gpt-5.5".into(),
            effort: AgentEffort::High,
            model_source: ModelSource::Default,
            local_base_url: String::new(),
            local_api_key_env: String::new(),
        };
        let first = store
            .create_exclusive(
                conv,
                None,
                Uuid::nil(),
                "penny".into(),
                Some(profile.clone()),
            )
            .await
            .unwrap();
        let snapshot = first.snapshot().await;
        assert_eq!(snapshot.agent_profile, Some(profile));

        let existing = store
            .create_exclusive(conv, None, Uuid::nil(), "penny".into(), None)
            .await
            .expect_err("second unfinished job must be rejected");
        let CreateJobError::Active(existing) = existing else {
            panic!("expected active-job conflict");
        };
        assert_eq!(existing.job_id, first.job_id);

        first.mark_complete().await;
        assert!(store
            .create_exclusive(conv, None, Uuid::nil(), "penny".into(), None)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn wait_for_chunk_unblocks_on_push() {
        let store = fresh_store();
        let conv = Uuid::new_v4();
        let job = store.create(conv, None, Uuid::nil(), "penny".into()).await;

        let waiter = {
            let job = Arc::clone(&job);
            tokio::spawn(async move { job.wait_for_chunk_after(0).await })
        };

        // Tiny delay to ensure the waiter has registered with notify.
        tokio::time::sleep(Duration::from_millis(20)).await;
        job.push_chunk(Bytes::from_static(b"hello")).await;

        let result = waiter.await.expect("waiter joined");
        match result {
            WaitResult::Chunks { chunks, finished } => {
                assert!(!finished);
                assert_eq!(chunks.len(), 1);
                assert_eq!(&chunks[0][..], b"hello");
            }
            WaitResult::Finished => panic!("expected Chunks"),
        }
    }

    #[tokio::test]
    async fn wait_returns_finished_after_complete() {
        let store = fresh_store();
        let conv = Uuid::new_v4();
        let job = store.create(conv, None, Uuid::nil(), "penny".into()).await;
        job.push_chunk(Bytes::from_static(b"x")).await;
        job.mark_complete().await;
        let result = job.wait_for_chunk_after(1).await;
        match result {
            WaitResult::Finished => {}
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mark_error_records_message() {
        let store = fresh_store();
        let job = store
            .create(Uuid::new_v4(), None, Uuid::nil(), "penny".into())
            .await;
        job.mark_error("upstream 500").await;
        let snap = job.snapshot().await;
        assert!(matches!(snap.status, JobStatus::Error));
        assert_eq!(snap.error_message.as_deref(), Some("upstream 500"));
    }

    #[tokio::test]
    async fn cancelled_signal_resolves_after_request_cancel() {
        let store = fresh_store();
        let job = store
            .create(Uuid::new_v4(), None, Uuid::nil(), "penny".into())
            .await;

        let waiter = {
            let job = Arc::clone(&job);
            tokio::spawn(async move { job.cancelled_signal().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "must wait until cancel requested");

        job.request_cancel();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("cancelled_signal must resolve")
            .expect("waiter joined");
        assert!(job.is_cancel_requested());

        // Signal is level-triggered: a second await resolves instantly.
        tokio::time::timeout(Duration::from_secs(1), job.cancelled_signal())
            .await
            .expect("already-cancelled signal resolves immediately");
    }

    #[tokio::test]
    async fn mark_cancelled_is_terminal_for_subscribers() {
        let store = fresh_store();
        let job = store
            .create(Uuid::new_v4(), None, Uuid::nil(), "penny".into())
            .await;
        job.push_chunk(Bytes::from_static(b"partial")).await;
        job.mark_cancelled().await;

        let snap = job.snapshot().await;
        assert!(matches!(snap.status, JobStatus::Cancelled));
        assert!(snap.is_finished);
        // Buffer stays replayable; the tail terminates.
        let (chunks, finished) = job.replay_from(0).await;
        assert_eq!(chunks.len(), 1);
        assert!(finished);
        match job.wait_for_chunk_after(1).await {
            WaitResult::Finished => {}
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    #[test]
    fn cancelled_status_serializes_snake_case() {
        let s = serde_json::to_string(&JobStatus::Cancelled).expect("serialize");
        assert_eq!(s, "\"cancelled\"");
    }

    #[tokio::test]
    async fn cleanup_reaps_completed_jobs_past_ttl() {
        let store = fresh_store();
        let conv = Uuid::new_v4();
        let job = store.create(conv, None, Uuid::nil(), "penny".into()).await;
        job.mark_complete().await;
        // Default TTL is 10 min; force it to 0 for the test so the
        // freshly-completed job is immediately reapable.
        store
            .cleanup_expired(Instant::now(), Duration::from_secs(0))
            .await;
        assert!(store.get(job.job_id).await.is_none());
        assert!(store.active_for_conversation(conv).await.is_none());
    }

    #[tokio::test]
    async fn cleanup_does_not_clear_conv_index_for_replaced_job() {
        let store = fresh_store();
        let conv = Uuid::new_v4();
        let old = store.create(conv, None, Uuid::nil(), "penny".into()).await;
        let new = store.create(conv, None, Uuid::nil(), "penny".into()).await;
        old.mark_complete().await;
        store
            .cleanup_expired(Instant::now(), Duration::from_secs(0))
            .await;
        // `new` is still pending, so it should remain in both indexes.
        assert!(store.get(new.job_id).await.is_some());
        assert_eq!(
            store.active_for_conversation(conv).await.map(|j| j.job_id),
            Some(new.job_id),
        );
    }
}
