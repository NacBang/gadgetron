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

let migrated = false;

function maybeMigrate(): void {
  if (migrated) return;
  migrated = true;
  if (typeof window === "undefined") return;
  // If this tab already has its own active id, do nothing — the
  // operator has already established a session in this tab.
  if (window.sessionStorage.getItem(KEY)) return;
  // First read in this tab. If the legacy `localStorage` slot is set,
  // adopt it as the seed value so a single-tab user reloading the
  // page after deploy still resumes their last conversation. If
  // multiple tabs hit this branch concurrently, each one gets the
  // same seed but writes diverge from there because each tab uses
  // its own sessionStorage from this point on.
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
