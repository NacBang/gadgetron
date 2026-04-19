//! In-memory replay cache for `client_invocation_id` deduplication.
//!
//! Authority: `docs/design/gateway/workbench-projection-and-actions.md` §2.1.1
//!
//! Drift-fix PR 8 (Nominal N5): replaces the v1 `HashMap<ReplayKey, (Instant,
//! response)>` behind a `tokio::sync::Mutex` with
//! [`moka::future::Cache`]. Moka brings:
//!
//! - **Concurrent sharded TTL LRU** — lock-free reads, sharded writes; no
//!   single mutex point of contention under workbench-action load.
//! - **Automatic expiry** — TTL eviction runs on a housekeeper task; no
//!   opportunistic "evict on every put" scan needed.
//! - **Bounded memory** — `max_capacity` caps the working set so a
//!   misbehaving client cannot blow the heap with fresh
//!   `client_invocation_id`s.
//!
//! Public API (`InMemoryReplayCache::{new,get,put}`) is unchanged — the
//! cutover is a pure implementation swap for call sites.

use std::{sync::Arc, time::Duration};

use gadgetron_core::workbench::InvokeWorkbenchActionResponse;
use moka::future::Cache;
use uuid::Uuid;

/// Default TTL for replay-cache entries: 5 minutes.
pub const DEFAULT_REPLAY_TTL: Duration = Duration::from_secs(300);

/// Soft upper bound on cached entries. Workbench action invocations arrive at
/// O(N_tenants × N_actions × recent_client_invocations); 10k leaves generous
/// headroom for the P2B fleet while preventing unbounded growth from a
/// replay-key storm.
const DEFAULT_REPLAY_MAX_ENTRIES: u64 = 10_000;

/// Composite key for a replay-cache entry.
///
/// Keyed on `(tenant_id, action_id, client_invocation_id)` so replay
/// protection is scoped per-tenant and cannot cross tenant boundaries.
///
/// The key wraps an `Arc` under the hood in moka, so `ReplayKey` stays
/// cheap to clone across cache operations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReplayKey {
    pub tenant_id: Uuid,
    pub action_id: String,
    pub client_invocation_id: Uuid,
}

/// Thread-safe in-memory TTL cache for action invocation responses.
///
/// Backed by [`moka::future::Cache`]. Reads are lock-free; writes use moka's
/// sharded locking. Expired entries are reaped automatically by moka's
/// housekeeper — callers do not need to poke the cache to force eviction.
#[derive(Debug, Clone)]
pub struct InMemoryReplayCache {
    inner: Cache<Arc<ReplayKey>, InvokeWorkbenchActionResponse>,
}

impl InMemoryReplayCache {
    /// Create a new cache with the given TTL.
    ///
    /// The max-capacity bound ([`DEFAULT_REPLAY_MAX_ENTRIES`]) is applied
    /// implicitly — a misbehaving tenant cannot cause unbounded growth.
    pub fn new(ttl: Duration) -> Self {
        Self::with_capacity(ttl, DEFAULT_REPLAY_MAX_ENTRIES)
    }

    /// Variant that lets callers (tests, tuning probes) pick an explicit
    /// entry cap. Production code should use [`InMemoryReplayCache::new`].
    pub fn with_capacity(ttl: Duration, max_entries: u64) -> Self {
        Self {
            inner: Cache::builder()
                .time_to_live(ttl)
                .max_capacity(max_entries)
                .build(),
        }
    }

    /// Look up an entry. Returns `Some` if the key exists and has not
    /// exceeded the TTL; returns `None` otherwise.
    pub async fn get(&self, key: &ReplayKey) -> Option<InvokeWorkbenchActionResponse> {
        // moka's key type is `Arc<ReplayKey>`; we Arc-wrap transparently so
        // the public API continues to take `&ReplayKey` as before.
        let arc_key = Arc::new(key.clone());
        self.inner.get(&arc_key).await
    }

    /// Insert a fresh entry.
    pub async fn put(&self, key: ReplayKey, response: InvokeWorkbenchActionResponse) {
        self.inner.insert(Arc::new(key), response).await;
    }

    /// Run any outstanding housekeeping (primarily a test hook). Under normal
    /// workloads, moka's background reaper handles this; the explicit call
    /// is useful for deterministic-eviction tests.
    #[cfg(test)]
    pub async fn run_pending_tasks(&self) {
        self.inner.run_pending_tasks().await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::workbench::WorkbenchActionResult;

    fn make_response(status: &str) -> InvokeWorkbenchActionResponse {
        InvokeWorkbenchActionResponse {
            result: WorkbenchActionResult {
                status: status.to_string(),
                approval_id: None,
                activity_event_id: None,
                audit_event_id: None,
                refresh_view_ids: vec![],
                knowledge_candidates: vec![],
                payload: None,
            },
        }
    }

    fn make_key() -> ReplayKey {
        ReplayKey {
            tenant_id: Uuid::new_v4(),
            action_id: "knowledge-search".into(),
            client_invocation_id: Uuid::new_v4(),
        }
    }

    // -----------------------------------------------------------------------
    // Miss
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_miss_on_empty_cache() {
        let cache = InMemoryReplayCache::new(DEFAULT_REPLAY_TTL);
        let key = make_key();
        assert!(cache.get(&key).await.is_none(), "empty cache must miss");
    }

    // -----------------------------------------------------------------------
    // Hit
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_hit_after_put() {
        let cache = InMemoryReplayCache::new(DEFAULT_REPLAY_TTL);
        let key = make_key();
        let resp = make_response("ok");
        cache.put(key.clone(), resp.clone()).await;
        let found = cache.get(&key).await;
        assert!(found.is_some(), "must hit after put");
        assert_eq!(found.unwrap().result.status, "ok");
    }

    // -----------------------------------------------------------------------
    // Expired
    // -----------------------------------------------------------------------

    // moka uses a real wall clock (Quanta) — `tokio::time::pause` does not
    // affect it. Use a short real TTL + real sleep + `run_pending_tasks`
    // to flush the expiry queue deterministically.
    #[tokio::test(flavor = "multi_thread")]
    async fn get_miss_after_ttl_elapsed() {
        let cache = InMemoryReplayCache::new(Duration::from_millis(20));
        let key = make_key();
        cache.put(key.clone(), make_response("ok")).await;
        assert!(cache.get(&key).await.is_some(), "pre-expiry hit");

        // Sleep past the TTL, then flush moka's eviction queue.
        tokio::time::sleep(Duration::from_millis(60)).await;
        cache.run_pending_tasks().await;

        assert!(
            cache.get(&key).await.is_none(),
            "entry must be treated as absent after TTL elapsed"
        );
    }

    // -----------------------------------------------------------------------
    // Different keys do not collide
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn different_keys_do_not_collide() {
        let cache = InMemoryReplayCache::new(DEFAULT_REPLAY_TTL);
        let key1 = make_key();
        let key2 = make_key(); // new Uuid → different client_invocation_id
        cache.put(key1.clone(), make_response("first")).await;
        cache.put(key2.clone(), make_response("second")).await;

        assert_eq!(cache.get(&key1).await.unwrap().result.status, "first");
        assert_eq!(cache.get(&key2).await.unwrap().result.status, "second");
    }

    // -----------------------------------------------------------------------
    // Max-capacity eviction (moka LRU bound)
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn max_capacity_bounds_working_set() {
        // Cap at 5 entries, insert 12 — the working set must stay bounded.
        let cache = InMemoryReplayCache::with_capacity(DEFAULT_REPLAY_TTL, 5);
        for _ in 0..12u32 {
            cache.put(make_key(), make_response("x")).await;
        }
        cache.run_pending_tasks().await;

        // moka's policy guarantees `entry_count` does not exceed
        // `max_capacity` once housekeeping has run. Exact surviving set
        // depends on moka's TinyLFU admission rules; the bound is the
        // invariant we care about.
        assert!(
            cache.inner.entry_count() <= 5,
            "cap=5 must not be exceeded after run_pending_tasks, got {}",
            cache.inner.entry_count()
        );
    }
}
