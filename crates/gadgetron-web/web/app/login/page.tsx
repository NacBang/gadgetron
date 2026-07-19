"use client";

import { useEffect, useState } from "react";
import { LocaleProvider, useI18n } from "../lib/i18n";

// ---------------------------------------------------------------------------
// /web/login — sign-in screen.
//
// Currently a minimal page with "Sign in with Google" (OAuth configured
// on the server) + email/password form that hits `/api/v1/auth/login`.
// The email/password form stays as fallback for operators who don't want
// to wire OAuth in the demo.
// ---------------------------------------------------------------------------

const LEGACY_API_KEY_STORAGE_KEY = "gadgetron_api_key";

function getServerRoot(): string {
  if (typeof document === "undefined") return "";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const base = meta?.content ?? "/v1";
  return base.replace(/\/v\d+$/, "");
}

async function whoami(): Promise<{ user_id: string } | null> {
  try {
    const res = await fetch(`${getServerRoot()}/api/v1/auth/whoami`, {
      credentials: "include",
    });
    if (!res.ok) return null;
    return (await res.json()) as { user_id: string };
  } catch {
    return null;
  }
}

function LoginContent() {
  const { locale, labels, setLocale } = useI18n();
  const copy = labels.login;
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    void whoami().then((me) => {
      if (me && typeof window !== "undefined") {
        window.location.assign("/web/");
      }
    });
  }, []);

  const googleHref = `${getServerRoot()}/auth/google/login`;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setErr(null);
    try {
      const res = await fetch(`${getServerRoot()}/api/v1/auth/login`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "include",
        body: JSON.stringify({ email, password }),
      });
      if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        throw new Error(
          (body as { error?: { message?: string } }).error?.message ??
            `HTTP ${res.status}`,
        );
      }
      localStorage.removeItem(LEGACY_API_KEY_STORAGE_KEY);
      window.location.assign("/web/");
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="relative flex min-h-screen items-center justify-center bg-zinc-950 p-6">
      <div
        className="absolute right-4 top-4 flex rounded border border-zinc-800 bg-zinc-900 p-0.5 text-[11px]"
        role="group"
        aria-label={labels.locale.selector}
      >
        <button
          type="button"
          aria-pressed={locale === "en"}
          className={`rounded px-2 py-1 ${locale === "en" ? "bg-zinc-700 text-zinc-100" : "text-zinc-500 hover:text-zinc-200"}`}
          onClick={() => setLocale("en")}
        >
          {labels.locale.english}
        </button>
        <button
          type="button"
          aria-pressed={locale === "ko"}
          className={`rounded px-2 py-1 ${locale === "ko" ? "bg-zinc-700 text-zinc-100" : "text-zinc-500 hover:text-zinc-200"}`}
          onClick={() => setLocale("ko")}
        >
          {labels.locale.korean}
        </button>
      </div>
      <div className="w-full max-w-sm space-y-6 rounded-lg border border-zinc-800 bg-zinc-900 p-6">
        <div className="flex items-center gap-3">
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img
            src="/web/brand/manycoresoft.png"
            alt="ManyCoreSoft"
            className="h-5 w-auto"
          />
          <span className="text-lg font-semibold text-zinc-100">Gadgetron</span>
        </div>
        <div>
          <h1 className="text-xl font-semibold text-zinc-100">{copy.title}</h1>
          <p className="mt-1 text-[11px] text-zinc-500">
            {copy.description}
          </p>
        </div>

        <a
          href={googleHref}
          className="flex w-full items-center justify-center gap-2 rounded border border-zinc-700 bg-zinc-950 px-3 py-2 text-sm text-zinc-200 hover:bg-zinc-800"
        >
          <span
            aria-hidden
            className="inline-block size-4 rounded-sm bg-white text-[10px] font-bold text-black"
            style={{
              lineHeight: "1rem",
              textAlign: "center",
              fontFamily: "Arial, sans-serif",
            }}
          >
            G
          </span>
          {copy.google}
        </a>

        <div className="flex items-center gap-3 text-[10px] uppercase text-zinc-600">
          <span className="h-px flex-1 bg-zinc-800" />
          {copy.or}
          <span className="h-px flex-1 bg-zinc-800" />
        </div>

        <form onSubmit={submit} className="space-y-3">
          <div>
            <label htmlFor="login-email" className="mb-1 block text-[11px] text-zinc-500">
              {copy.email}
            </label>
            <input
              id="login-email"
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="you@example.com"
              className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
              autoComplete="email"
              required
            />
          </div>
          <div>
            <label htmlFor="login-password" className="mb-1 block text-[11px] text-zinc-500">
              {copy.password}
            </label>
            <input
              id="login-password"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
              autoComplete="current-password"
              required
            />
          </div>
          {err && (
            <div className="rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
              {err}
            </div>
          )}
          <button
            type="submit"
            disabled={busy}
            className="flex h-9 w-full items-center justify-center rounded bg-[var(--copper)] text-sm font-medium text-[var(--bg)] hover:bg-[var(--copper-hi)] disabled:opacity-50"
          >
            {busy ? copy.signingIn : copy.submit}
          </button>
        </form>
      </div>
    </div>
  );
}

export default function LoginPage() {
  return (
    <LocaleProvider>
      <LoginContent />
    </LocaleProvider>
  );
}
