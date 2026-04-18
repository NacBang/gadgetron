//! In-memory replay cache for `client_invocation_id` deduplication.
//!
//! Authority: `docs/design/gateway/workbench-projection-and-actions.md` §2.1.1
//!
//! Entries expire after a configurable TTL (default 5 minutes). Expired entries
//! are evicted opportunistically on each `put` call — no background task is
//! spawned in this PR (P2B scope).
//!
//! Clock is sourced from `tokio::time::Instant` so tests can use
//! `tokio::time::pause()` for deterministic TTL expiry.

use std::{collections::HashMap, time::Duration};

use gadgetron_core::workbench::InvokeWorkbenchActionResponse;
use tokio::time::Instant;
use uuid::Uuid;

/// Default TTL for replay-cache entries: 5 minutes.
pub const DEFAULT_REPLAY_TTL: Duration = Duration::from_secs(300);

/// Composite key for a replay-cache entry.
///
/// Keyed on `(tenant_id, action_id, client_invocation_id)` so replay
/// protection is scoped per-tenant and cannot cross tenant boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReplayKey {
    pub tenant_id: Uuid,
    pub action_id: String,
    pub client_invocation_id: Uuid,
}

/// Thread-safe in-memory TTL cache for action invocation responses.
///
/// Uses `tokio::sync::Mutex` over a `HashMap<ReplayKey, (Instant, response)>`.
/// The lock is held only for the duration of a hash lookup or insert, not
/// across any I/O, so contention should be negligible.
#[derive(Debug)]
pub struct InMemoryReplayCache {
    inner: tokio::sync::Mutex<HashMap<ReplayKey, (Instant, InvokeWorkbenchActionResponse)>>,
    ttl: Duration,
}

impl InMemoryReplayCache {
    /// Create a new cache with the given TTL.
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Look up an entry. Returns `Some` if the key exists **and** has not
    /// exceeded the TTL; returns `None` otherwise (expired entries are treated
    /// as absent).
    pub async fn get(&self, key: &ReplayKey) -> Option<InvokeWorkbenchActionResponse> {
        let guard = self.inner.lock().await;
        guard.get(key).and_then(|(stored_at, resp)| {
            if stored_at.elapsed() < self.ttl {
                Some(resp.clone())
            } else {
                None
            }
        })
    }

    /// Insert a fresh entry. Opportunistically evicts all expired entries on
    /// each call (no background task required for P2B).
    pub async fn put(&self, key: ReplayKey, response: InvokeWorkbenchActionResponse) {
        let mut guard = self.inner.lock().await;
        // Opportunistic eviction: remove expired entries before inserting.
        let ttl = self.ttl;
        guard.retain(|_, (stored_at, _)| stored_at.elapsed() < ttl);
        guard.insert(key, (Instant::now(), response));
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

    #[tokio::test(start_paused = true)]
    async fn get_miss_after_ttl_elapsed() {
        // Use a very short TTL (1 ms) so we can advance time past it.
        let cache = InMemoryReplayCache::new(Duration::from_millis(1));
        let key = make_key();
        cache.put(key.clone(), make_response("ok")).await;

        // Advance time past the TTL.
        tokio::time::advance(Duration::from_millis(10)).await;

        assert!(
            cache.get(&key).await.is_none(),
            "entry must be treated as absent after TTL elapsed"
        );
    }

    // -----------------------------------------------------------------------
    // Opportunistic eviction
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn put_evicts_expired_entries() {
        let ttl = Duration::from_millis(5);
        let cache = InMemoryReplayCache::new(ttl);

        let key_a = make_key();
        let key_b = make_key();

        cache.put(key_a.clone(), make_response("a")).await;

        // Advance time so key_a is expired.
        tokio::time::advance(Duration::from_millis(10)).await;

        // Insert key_b — this triggers opportunistic eviction of key_a.
        cache.put(key_b.clone(), make_response("b")).await;

        // key_a must be gone.
        assert!(cache.get(&key_a).await.is_none(), "key_a must be evicted");
        // key_b must still be there.
        assert!(cache.get(&key_b).await.is_some(), "key_b must survive");
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
}
