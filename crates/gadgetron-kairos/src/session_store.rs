//! `SessionStore` — per-conversation Claude Code native session tracking.
//!
//! Spec: `docs/design/phase2/02-kairos-agent.md §5.2.4`.
//!
//! The store maps gadgetron-side `conversation_id` → `SessionEntry`,
//! which holds the UUID passed to Claude Code's `--session-id` /
//! `--resume` flags, bookkeeping (last_used, turn_count), and a
//! per-entry `tokio::sync::Mutex<()>` that serializes concurrent
//! resumes of the same conversation.
//!
//! # Why `get_or_create` and NOT `get` + `insert_new`
//!
//! A naive two-step `get` → `insert_new` flow races two concurrent
//! first-turn calls for the same id into two separate Claude Code
//! sessions (ADR-P2A-06 addendum item 7, Codex review
//! `a957d8d6cebf4ee5a` finding 7). `get_or_create` uses
//! `DashMap::entry(...).or_insert_with(...)` so exactly one caller
//! observes `first_turn == true` per `conversation_id`, even under
//! arbitrary concurrency.
//!
//! # TTL + eviction
//!
//! - **TTL**: entries older than `ttl` (default 24h) are removed by
//!   piggyback sweep on every `get_or_create` call. The sweep is
//!   bounded to `min(max_entries / 10, 256)` scans per request so a
//!   misconfigured `max_entries = 1_000_000` cannot pin a core on
//!   hot paths.
//! - **LRU eviction**: when `entries.len() > max_entries`, the entry
//!   with the oldest `last_used` is removed. The Claude Code jsonl
//!   file on disk is NOT deleted — that is Claude Code's own
//!   responsibility.
//!
//! P2A does not delete the on-disk jsonl. A P2A-post patch may add an
//! explicit `tokio::fs::remove_file` for evicted/stale sessions.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Hard cap on the number of entries inspected per `sweep_expired`
/// call. Load-bearing: at `max_entries = 1_000_000` (V16 upper bound)
/// a proportional-to-capacity bound would let one request pin a core
/// for hundreds of milliseconds. Chosen in ADR-P2A-06 addendum item 7
/// to keep worst-case per-request sweep cost O(256) regardless of
/// configured store size.
const SWEEP_SCAN_HARD_CAP: usize = 256;

/// Opaque conversation identifier coming from the gateway
/// (`X-Gadgetron-Conversation-Id` header or `metadata.conversation_id`
/// body field). Validated for length + forbidden chars at the gateway
/// boundary — the store never re-validates.
///
/// # Namespace convention (Type 1 Decision #2 — office-hours 2026-04-16)
///
/// The string MUST be of the form `"{owner_id}:{local_name}"` where
/// `owner_id` identifies the principal whose credentials drove the
/// conversation, and `local_name` is a caller-chosen short name
/// (uuid, slug, incident id, whatever).
///
/// P2A single-user default: `owner_id = "_self"`. Every P2A
/// conversation id therefore looks like `"_self:<something>"` or any
/// string with a leading `"_self:"` prefix. Callers that pass a bare
/// string without the colon separator are tolerated in P2A and
/// interpreted as local names under the implicit `"_self"` owner —
/// see [`parse_conversation_id`].
///
/// When multi-tenant arrives (P2B or P2C), the gateway authentication
/// layer sets `owner_id` to the authenticated principal, and this
/// namespace prefix makes cross-principal collisions impossible.
///
/// This is a convention documented in a helper function, not a new
/// type, because PR A7.5 intentionally avoids refactoring every
/// existing `String` signature. If/when the system moves to multi-
/// tenant in earnest, upgrading `ConversationId` to a struct is a
/// straightforward rename.
pub type ConversationId = String;

/// Default `owner_id` used by P2A single-user mode when no principal
/// is attached to the request. Exported so tests and the driver can
/// name it explicitly rather than sprinkling magic strings.
pub const DEFAULT_OWNER_ID: &str = "_self";

/// Parse a conversation id into its `(owner_id, local_name)` pair.
///
/// Rules (Type 1 Decision #2, PR A7.5):
///
/// - `"alice:incident-42"` → `("alice", "incident-42")`
/// - `"_self:nightly-boot"` → `("_self", "nightly-boot")`
/// - `"bare-local-name"` → `("_self", "bare-local-name")`  (P2A default)
/// - `""` → `("_self", "")` (edge case — caller probably shouldn't
///   pass empty, but we don't panic)
/// - Multiple colons: first `:` is the separator. `"a:b:c"` →
///   `("a", "b:c")`. This matters because URL-ish local names may
///   contain their own colons.
///
/// Note this is a pure function with no allocation beyond the
/// returned `&str` slices. Use `.to_string()` on the slices if
/// ownership is required.
pub fn parse_conversation_id(raw: &str) -> (&str, &str) {
    match raw.split_once(':') {
        Some((owner, local)) if !owner.is_empty() => (owner, local),
        _ => (DEFAULT_OWNER_ID, raw),
    }
}

/// Per-conversation bookkeeping. Cloning is `Arc`-cheap.
#[derive(Debug)]
pub struct SessionEntry {
    /// UUID passed to Claude Code via `--session-id` (first turn) or
    /// `--resume` (subsequent turns). Generated with `Uuid::new_v4()`
    /// inside `SessionStore::get_or_create` — v4 (random) is chosen
    /// because v1 leaks spawn time and v3/v5 require a stable
    /// namespace we do not have.
    pub claude_session_uuid: Uuid,
    pub created_at: Instant,
    pub last_used: std::sync::Mutex<Instant>,
    pub turn_count: std::sync::atomic::AtomicU32,
    /// Held for the duration of one spawn-and-drive cycle. Prevents
    /// two concurrent resume requests from corrupting the jsonl file
    /// (or generating nondeterministic audit entries). RAII: when the
    /// session driver future is dropped the guard is released.
    pub mutex: Arc<Mutex<()>>,
}

impl SessionEntry {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            claude_session_uuid: Uuid::new_v4(),
            created_at: now,
            last_used: std::sync::Mutex::new(now),
            turn_count: std::sync::atomic::AtomicU32::new(0),
            mutex: Arc::new(Mutex::new(())),
        }
    }

    /// Return the most recent `last_used` instant.
    pub fn last_used(&self) -> Instant {
        *self.last_used.lock().unwrap()
    }

    /// Increment `turn_count` and bump `last_used` to `now`. Called
    /// by the session driver after a successful turn completes.
    fn bump(&self) {
        self.turn_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *self.last_used.lock().unwrap() = Instant::now();
    }

    /// Read the current turn_count (atomic, Relaxed).
    pub fn turn_count(&self) -> u32 {
        self.turn_count.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// `SessionStore` maintains an `id → Arc<SessionEntry>` mapping with
/// LRU eviction + TTL purge.
#[derive(Debug)]
pub struct SessionStore {
    entries: DashMap<ConversationId, Arc<SessionEntry>>,
    ttl: Duration,
    max_entries: usize,
}

impl SessionStore {
    /// Construct a store with the given TTL (seconds) and max-entries
    /// cap. Production defaults are TTL = 24h, max = 10_000; tests
    /// pass much smaller numbers.
    pub fn new(ttl_secs: u64, max_entries: usize) -> Self {
        Self {
            entries: DashMap::new(),
            ttl: Duration::from_secs(ttl_secs),
            max_entries,
        }
    }

    /// Read-only lookup. Used by tests + metrics. **MUST NOT** be
    /// used on the session-driver path — callers there go through
    /// `get_or_create` to avoid the two-concurrent-first-turns race.
    pub fn get(&self, id: &str) -> Option<Arc<SessionEntry>> {
        self.entries.get(id).map(|e| e.value().clone())
    }

    /// Number of entries currently held. Used by tests + metrics.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Atomic get-or-insert. Returns `(entry, first_turn)` where
    /// `first_turn == true` iff this call inserted a new entry. The
    /// first-turn discriminant is the ONLY safe way to decide
    /// `--session-id` vs `--resume` in the session driver; an
    /// external `get` → `insert_new` split races.
    ///
    /// Side effects:
    /// - Runs a bounded `sweep_expired` to purge stale TTL entries.
    /// - If `entries.len() > max_entries` AFTER insertion, evicts the
    ///   LRU entry (oldest `last_used`).
    pub fn get_or_create(&self, id: ConversationId) -> (Arc<SessionEntry>, bool) {
        self.sweep_expired();

        let mut first_turn = false;
        let entry = self
            .entries
            .entry(id.clone())
            .or_insert_with(|| {
                first_turn = true;
                Arc::new(SessionEntry::new())
            })
            .value()
            .clone();

        if first_turn && self.entries.len() > self.max_entries {
            self.evict_lru_excluding(&id);
        }

        (entry, first_turn)
    }

    /// Bump `last_used` + `turn_count` for the entry keyed by `id`.
    /// Called by the session driver after a successful turn.
    pub fn touch(&self, id: &str) {
        if let Some(entry) = self.entries.get(id) {
            entry.value().bump();
        }
    }

    /// Remove up to `SWEEP_SCAN_HARD_CAP` entries older than `self.ttl`.
    /// The cap is load-bearing: see `SWEEP_SCAN_HARD_CAP` doc comment.
    /// Entries beyond the cap carry over to subsequent calls.
    pub fn sweep_expired(&self) {
        let budget = (self.max_entries / 10).clamp(1, SWEEP_SCAN_HARD_CAP);
        let now = Instant::now();
        let mut expired: Vec<ConversationId> = Vec::new();
        for item in self.entries.iter() {
            if expired.len() >= budget {
                break;
            }
            let last = *item.value().last_used.lock().unwrap();
            if now.saturating_duration_since(last) > self.ttl {
                expired.push(item.key().clone());
            }
        }
        for id in expired {
            self.entries.remove(&id);
        }
    }

    /// Remove the LRU entry (oldest `last_used`), skipping the
    /// conversation id we just inserted so the freshly-inserted turn
    /// cannot evict itself. Called from `get_or_create` after a new
    /// insert that pushed the store over `max_entries`.
    fn evict_lru_excluding(&self, protect: &str) {
        let mut oldest: Option<(ConversationId, Instant)> = None;
        for item in self.entries.iter() {
            let key = item.key();
            if key == protect {
                continue;
            }
            let last = *item.value().last_used.lock().unwrap();
            match &oldest {
                None => oldest = Some((key.clone(), last)),
                Some((_, t)) if last < *t => oldest = Some((key.clone(), last)),
                _ => {}
            }
        }
        if let Some((id, _)) = oldest {
            self.entries.remove(&id);
        }
    }

    /// Materialize an owned snapshot of the currently-held session
    /// UUIDs. Used only by tests + metrics; callers that need to
    /// iterate live entries must use `entries()` directly.
    #[cfg(test)]
    fn snapshot_ids(&self) -> std::collections::HashMap<ConversationId, Uuid> {
        let mut out = std::collections::HashMap::new();
        for item in self.entries.iter() {
            out.insert(item.key().clone(), item.value().claude_session_uuid);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_store_new_is_empty() {
        let store = SessionStore::new(60, 10);
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn get_or_create_first_call_reports_first_turn_true() {
        let store = SessionStore::new(60, 10);
        let (entry, first) = store.get_or_create("c1".to_string());
        assert!(first);
        assert_eq!(entry.turn_count(), 0);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn get_or_create_second_call_reports_first_turn_false_and_same_uuid() {
        let store = SessionStore::new(60, 10);
        let (entry_a, first_a) = store.get_or_create("c1".to_string());
        let (entry_b, first_b) = store.get_or_create("c1".to_string());
        assert!(first_a);
        assert!(!first_b);
        assert_eq!(entry_a.claude_session_uuid, entry_b.claude_session_uuid);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn touch_increments_turn_count() {
        let store = SessionStore::new(60, 10);
        let (_entry, _) = store.get_or_create("c1".to_string());
        store.touch("c1");
        store.touch("c1");
        let entry = store.get("c1").unwrap();
        assert_eq!(entry.turn_count(), 2);
    }

    #[test]
    fn session_store_eviction_respects_lru() {
        // Per §5.2.10 item 7 — regression lock on LRU ordering.
        let store = SessionStore::new(60, 3);
        for id in ["c1", "c2", "c3"] {
            store.get_or_create(id.to_string());
        }
        // Touch c1 so c2 becomes the oldest.
        std::thread::sleep(Duration::from_millis(5));
        store.touch("c1");
        // Insert c4 — pushes us to 4 > 3, LRU (c2) must be evicted.
        let (_, _) = store.get_or_create("c4".to_string());
        let keys = store.snapshot_ids();
        assert!(keys.contains_key("c1"));
        assert!(keys.contains_key("c3"));
        assert!(keys.contains_key("c4"));
        assert!(!keys.contains_key("c2"));
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn session_store_ttl_cleanup_purges_stale_entries() {
        // Per §5.2.10 item 8 — piggyback sweep triggers on
        // `get_or_create`, NOT on `get` (which is read-only).
        //
        // Use a sub-second TTL so `std::thread::sleep` drives the
        // clock forward deterministically without needing tokio
        // time mocking.
        let store = SessionStore::new(1, 10);
        // Construct via raw insert so we can push the last_used
        // into the past without waiting.
        let (_, _) = store.get_or_create("c1".to_string());
        // Force c1 past TTL by rewinding `last_used` by 5s.
        {
            let e = store.get("c1").unwrap();
            *e.last_used.lock().unwrap() = Instant::now() - Duration::from_secs(5);
        }
        // `get_or_create("c2")` is the API that runs sweep_expired.
        let (_, _) = store.get_or_create("c2".to_string());
        assert!(store.get("c1").is_none(), "c1 must be purged by sweep");
        assert!(store.get("c2").is_some());
    }

    // ---- Type 1 Decision #2 (PR A7.5, office-hours 2026-04-16) ----

    #[test]
    fn parse_conversation_id_splits_on_first_colon() {
        let (owner, local) = parse_conversation_id("alice:incident-42");
        assert_eq!(owner, "alice");
        assert_eq!(local, "incident-42");
    }

    #[test]
    fn parse_conversation_id_uses_default_owner_for_bare_local_name() {
        let (owner, local) = parse_conversation_id("bare-local-name");
        assert_eq!(owner, DEFAULT_OWNER_ID);
        assert_eq!(owner, "_self");
        assert_eq!(local, "bare-local-name");
    }

    #[test]
    fn parse_conversation_id_preserves_colons_in_local_name() {
        // URL-ish local names can contain their own colons — only the
        // first `:` is the namespace separator.
        let (owner, local) = parse_conversation_id("alice:https://foo.bar:8080/x");
        assert_eq!(owner, "alice");
        assert_eq!(local, "https://foo.bar:8080/x");
    }

    #[test]
    fn parse_conversation_id_leading_colon_falls_back_to_default_owner() {
        // `":local"` has an empty owner, which is NOT a valid principal.
        // Treat as bare local with default owner.
        let (owner, local) = parse_conversation_id(":local");
        assert_eq!(owner, DEFAULT_OWNER_ID);
        assert_eq!(local, ":local");
    }

    #[test]
    fn parse_conversation_id_empty_string() {
        let (owner, local) = parse_conversation_id("");
        assert_eq!(owner, DEFAULT_OWNER_ID);
        assert_eq!(local, "");
    }

    #[tokio::test]
    async fn session_store_get_or_create_is_atomic_under_concurrent_first_turns() {
        // Per §5.2.10 item 16 — regression lock on the atomicity
        // contract: N concurrent first-turn calls for the same id
        // must produce exactly ONE entry and exactly ONE first_turn
        // = true observation.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let store = Arc::new(SessionStore::new(60, 10));
        let first_count = Arc::new(AtomicUsize::new(0));
        let not_first_count = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..20 {
            let store = store.clone();
            let fc = first_count.clone();
            let nfc = not_first_count.clone();
            handles.push(tokio::spawn(async move {
                let (_, first) = store.get_or_create("c1".to_string());
                if first {
                    fc.fetch_add(1, Ordering::Relaxed);
                } else {
                    nfc.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(store.len(), 1, "exactly one entry for c1");
        assert_eq!(
            first_count.load(Ordering::Relaxed),
            1,
            "exactly one observer saw first_turn=true"
        );
        assert_eq!(
            not_first_count.load(Ordering::Relaxed),
            19,
            "the other 19 must see first_turn=false"
        );
    }
}
