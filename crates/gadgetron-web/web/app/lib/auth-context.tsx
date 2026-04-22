"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";

// Two coexisting auth paths:
//   1. API key in localStorage (pre-OAuth, CLI-minted keys).
//   2. Session cookie (Google OAuth + email/password login).
// The shell works when EITHER is present. When we have a session
// cookie, `identity` carries the user's email/name/role for UI gating.
const STORAGE_KEY = "gadgetron_api_key";
const VIEW_MODE_KEY = "gadgetron_view_mode"; // "admin" | "user"

export interface Identity {
  email?: string | null;
  display_name?: string | null;
  role?: "admin" | "member" | "service" | null;
  avatar_url?: string | null;
  user_id?: string | null;
}

export type ViewMode = "admin" | "user";

export interface AuthState {
  apiKey: string | null;
  saveKey: (k: string) => void;
  clearKey: () => void;
  /** True after the initial `localStorage` read, so callers can avoid
   * rendering a flash of the login form on authed page loads. */
  hydrated: boolean;
  /** Session-backed user identity. Null when only an API key is used
   * (whoami returns 401) OR when the session probe hasn't landed yet. */
  identity: Identity | null;
  refreshIdentity: () => Promise<void>;
  /** For admin users, whether the shell is currently in admin or user
   * view. Defaults to "admin" (i.e. show admin-only surfaces) the first
   * time an admin lands; persisted in localStorage thereafter. For
   * non-admin users this value is always "user". */
  viewMode: ViewMode;
  setViewMode: (m: ViewMode) => void;
}

const AuthCtx = createContext<AuthState | null>(null);

function getServerRoot(): string {
  if (typeof document === "undefined") return "";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const base = meta?.content ?? "/v1";
  return base.replace(/\/v\d+$/, "");
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [apiKey, setApiKey] = useState<string | null>(null);
  const [hydrated, setHydrated] = useState(false);
  const [identity, setIdentity] = useState<Identity | null>(null);
  const [viewMode, setViewModeState] = useState<ViewMode>("user");

  useEffect(() => {
    const stored =
      typeof localStorage !== "undefined"
        ? localStorage.getItem(STORAGE_KEY)
        : null;
    if (stored) setApiKey(stored);
    const vm =
      typeof localStorage !== "undefined"
        ? (localStorage.getItem(VIEW_MODE_KEY) as ViewMode | null)
        : null;
    if (vm === "admin" || vm === "user") setViewModeState(vm);
    setHydrated(true);
  }, []);

  const refreshIdentity = useCallback(async () => {
    try {
      const res = await fetch(`${getServerRoot()}/api/v1/auth/whoami`, {
        credentials: "include",
      });
      if (!res.ok) {
        setIdentity(null);
        return;
      }
      const body = (await res.json()) as Identity & {
        role?: string | null;
        user_id?: string | null;
        avatar_url?: string | null;
      };
      setIdentity({
        email: body.email ?? null,
        display_name: body.display_name ?? null,
        role: (body.role as Identity["role"]) ?? null,
        avatar_url: body.avatar_url ?? null,
        user_id: body.user_id ?? null,
      });
    } catch {
      setIdentity(null);
    }
  }, []);

  useEffect(() => {
    void refreshIdentity();
  }, [refreshIdentity]);

  // Default view: admins land in admin view on first visit. Non-admins
  // are pinned to user view regardless of stored preference.
  useEffect(() => {
    if (!hydrated || !identity) return;
    if (identity.role !== "admin" && viewMode === "admin") {
      setViewModeState("user");
      if (typeof localStorage !== "undefined") {
        localStorage.setItem(VIEW_MODE_KEY, "user");
      }
    } else if (
      identity.role === "admin" &&
      !localStorage.getItem(VIEW_MODE_KEY)
    ) {
      setViewModeState("admin");
      localStorage.setItem(VIEW_MODE_KEY, "admin");
    }
  }, [hydrated, identity, viewMode]);

  const saveKey = useCallback((k: string) => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(STORAGE_KEY, k);
    }
    setApiKey(k);
  }, []);

  const clearKey = useCallback(() => {
    if (typeof localStorage !== "undefined") {
      localStorage.removeItem(STORAGE_KEY);
    }
    setApiKey(null);
  }, []);

  const setViewMode = useCallback(
    (m: ViewMode) => {
      if (m === "admin" && identity?.role !== "admin") return;
      setViewModeState(m);
      if (typeof localStorage !== "undefined") {
        localStorage.setItem(VIEW_MODE_KEY, m);
      }
    },
    [identity],
  );

  return (
    <AuthCtx.Provider
      value={{
        apiKey,
        saveKey,
        clearKey,
        hydrated,
        identity,
        refreshIdentity,
        viewMode,
        setViewMode,
      }}
    >
      {children}
    </AuthCtx.Provider>
  );
}

export function useAuth(): AuthState {
  const v = useContext(AuthCtx);
  if (!v) {
    throw new Error("useAuth must be used inside <AuthProvider>");
  }
  return v;
}

/// Returns true when the caller is authenticated by EITHER an API key
/// or a live session cookie. Pages use this to gate fetches that went
/// through `Authorization: Bearer …` in the API-key-only era; the
/// backend middleware (see `gadgetron_gateway::middleware::auth`)
/// accepts the session cookie when Bearer is absent, so callers don't
/// need the key value — just the fact that auth is wired.
export function useHasAuth(): boolean {
  const { apiKey, identity } = useAuth();
  return !!apiKey || !!identity;
}

/// Build an `Authorization: Bearer …` header when an API key is
/// available; empty object otherwise. Combine with `credentials:
/// "include"` on the fetch so the browser sends the session cookie.
export function authHeaders(apiKey: string | null): Record<string, string> {
  return apiKey ? { Authorization: `Bearer ${apiKey}` } : {};
}
