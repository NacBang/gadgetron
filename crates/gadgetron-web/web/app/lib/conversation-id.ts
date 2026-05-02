// Per-tab active-conversation storage.
//
// Background: pre-fix, the active conversation id lived in
// `localStorage`, which is shared across every tab/window of the same
// origin. Two browser windows chatting with Penny in parallel would
// stomp on each other — tab B switching to a different conversation
// silently rewrote the storage key, so tab A's NEXT chat turn picked
// up B's id and Penny appended into the wrong jsonl. The operator
// observed: "대화 내용이 서로 섞인다."
//
// Fix: store the active id in `sessionStorage`, which is per-tab. A
// one-time migration copies any existing `localStorage` value into the
// new tab's `sessionStorage` on first read so users don't lose their
// previously-active chat after deploy. Subsequent writes go ONLY to
// `sessionStorage` so each tab stays isolated.
//
// All chat-transport / history-fetch / draft-restore / deep-link
// callers MUST go through this module. Direct `localStorage`
// access for `gadgetron_conversation_id` is now a bug.

const KEY = "gadgetron_conversation_id";

// Cheap per-call check: if this tab's sessionStorage already holds a
// value, skip immediately. Otherwise, adopt the cross-tab `localStorage`
// seed (set by any prior `setActiveConversationId` write — see below)
// so a single-tab user reloading after deploy still resumes their last
// conversation. Idempotent and cheap, so no need for a module-level
// "already-migrated" flag — that flag would have made this function
// blind to seed updates from sibling tabs after the first call.
function maybeMigrate(): void {
  if (typeof window === "undefined") return;
  if (window.sessionStorage.getItem(KEY)) return;
  const legacy = window.localStorage.getItem(KEY);
  if (legacy) {
    window.sessionStorage.setItem(KEY, legacy);
  }
}

/// Read the active conversation id for THIS tab. Returns null if the
/// tab has no active conversation yet (e.g. fresh tab open).
export function getActiveConversationId(): string | null {
  if (typeof window === "undefined") return null;
  maybeMigrate();
  return window.sessionStorage.getItem(KEY);
}

/// Set this tab's active conversation. The new value is visible only
/// to this tab on subsequent reads; sibling tabs keep their own.
export function setActiveConversationId(id: string): void {
  if (typeof window === "undefined") return;
  maybeMigrate();
  window.sessionStorage.setItem(KEY, id);
  // Mirror to localStorage so a brand-new tab opened later still has
  // a "last seen" seed. Each new tab promotes this seed into its own
  // sessionStorage on first read; subsequent writes diverge.
  window.localStorage.setItem(KEY, id);
}

/// Clear this tab's active conversation. Sibling tabs are unaffected.
/// Also clears the cross-tab seed, so the next new tab starts blank.
export function clearActiveConversationId(): void {
  if (typeof window === "undefined") return;
  maybeMigrate();
  window.sessionStorage.removeItem(KEY);
  window.localStorage.removeItem(KEY);
}
