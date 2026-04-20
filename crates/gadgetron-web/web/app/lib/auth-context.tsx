"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";

// Single source of truth for the browser-local API key. Before ISSUE 29
// each /web page (`/`, `/wiki`, `/dashboard`) owned its own copy of
// `useApiKey()` → three independent localStorage reads + three
// independent sign-in forms, so signing out on one tab didn't
// invalidate the others until a reload. The shared-layout refactor
// hoists the state one level up; pages read via `useAuth()`.
const STORAGE_KEY = "gadgetron_api_key";

export interface AuthState {
  apiKey: string | null;
  saveKey: (k: string) => void;
  clearKey: () => void;
  /** True after the initial `localStorage` read, so callers can avoid
   * rendering a flash of the login form on authed page loads. */
  hydrated: boolean;
}

const AuthCtx = createContext<AuthState | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [apiKey, setApiKey] = useState<string | null>(null);
  const [hydrated, setHydrated] = useState(false);

  useEffect(() => {
    const stored =
      typeof localStorage !== "undefined"
        ? localStorage.getItem(STORAGE_KEY)
        : null;
    if (stored) setApiKey(stored);
    setHydrated(true);
  }, []);

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

  return (
    <AuthCtx.Provider value={{ apiKey, saveKey, clearKey, hydrated }}>
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
