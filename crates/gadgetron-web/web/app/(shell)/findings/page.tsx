"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { Toaster, toast } from "sonner";
import { useAuth } from "../../lib/auth-context";
import { Button } from "../../components/ui/button";
import { safeRandomUUID } from "../../lib/uuid";

// /web/findings — log analyzer triage view.
//
// Server emits findings via the log-analyzer bundle's background
// scanner. This page lists open (non-dismissed) ones, grouped by host
// + severity, with a one-click dismiss. Deep link via `?host=<uuid>`
// pre-filters to a single host (used by card badges + drawer).

interface Host {
  id: string;
  host: string;
  alias?: string | null;
}

interface Remediation {
  tool: "server.systemctl" | "server.apt";
  args: Record<string, unknown>;
  label?: string;
}

interface Finding {
  id: string;
  host_id: string;
  source: string;
  severity: "critical" | "high" | "medium" | "info";
  category: string;
  summary: string;
  excerpt: string;
  ts_first: string;
  ts_last: string;
  count: number;
  classified_by: string;
  cause: string | null;
  solution: string | null;
  remediation: Remediation | null;
  comment_count?: number;
}

interface Comment {
  id: string;
  finding_id: string;
  author_kind: "user" | "penny";
  author_user_id: string | null;
  body: string;
  created_at: string;
}

const REMEDIATION_TOOL_TO_ACTION: Record<string, string> = {
  "server.systemctl": "server-systemctl",
  "server.apt": "server-apt",
};

interface ScanStatus {
  host_id: string;
  last_scanned_at: string | null;
  interval_secs: number;
  enabled: boolean;
}

const SEVERITY_TONES: Record<Finding["severity"], string> = {
  critical: "border-red-800 bg-red-950/40 text-red-200",
  high: "border-amber-800 bg-amber-950/40 text-amber-200",
  medium: "border-yellow-800 bg-yellow-950/30 text-yellow-200",
  info: "border-zinc-700 bg-zinc-900 text-zinc-300",
};

const SEVERITY_ORDER: Finding["severity"][] = [
  "critical",
  "high",
  "medium",
  "info",
];

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

async function invokeAction(
  apiKey: string | null,
  actionId: string,
  args: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  const res = await fetch(`${getApiBase()}/workbench/actions/${actionId}`, {
    method: "POST",
    credentials: "include",
    headers: {
      ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ args, client_invocation_id: safeRandomUUID() }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`${actionId}: ${res.status} ${text.slice(0, 200)}`);
  }
  return res.json();
}

function unwrapPayload(resp: Record<string, unknown>): unknown {
  const result = resp.result as { payload?: Array<{ text?: string }> } | undefined;
  const text = result?.payload?.[0]?.text;
  if (typeof text !== "string") return undefined;
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function relativeTime(iso: string): string {
  const d = new Date(iso);
  const diff = Math.max(0, (Date.now() - d.getTime()) / 1000);
  if (diff < 60) return `${Math.floor(diff)}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

/// Mint a fresh conversation and seed the composer with a finding
/// summary so Penny picks it up as the opening message. Writes the
/// new conversation id into localStorage (consumed by the chat
/// transport) and the draft under a per-conversation key (consumed
/// by the composer on mount), then navigates to /web.
function openChatAboutFinding(f: Finding, hostLabel: string): void {
  if (typeof window === "undefined") return;
  const convId = safeRandomUUID();
  const lines: string[] = [];
  lines.push(
    `이 Log를 같이 봐줘요. 어떻게 할지 같이 정하고 싶어요.`,
  );
  lines.push("");
  lines.push(`host: ${hostLabel} (${f.host_id.slice(0, 8)})`);
  lines.push(`source: ${f.source} · severity: ${f.severity}`);
  lines.push(`category: ${f.category}`);
  lines.push(`summary: ${f.summary}`);
  if (f.cause) lines.push(`cause: ${f.cause}`);
  if (f.solution) lines.push(`solution hint: ${f.solution}`);
  lines.push("");
  lines.push("excerpt:");
  lines.push("```");
  lines.push(
    f.excerpt.length > 1200 ? `${f.excerpt.slice(0, 1200)}…` : f.excerpt,
  );
  lines.push("```");
  const draft = lines.join("\n");
  window.localStorage.setItem("gadgetron_conversation_id", convId);
  window.localStorage.setItem(`gadgetron_draft_${convId}`, draft);
  window.localStorage.setItem(`gadgetron_pending_submit_${convId}`, "1");
  window.location.assign("/web");
}

// Expandable thread per finding. Collapsed by default so a long incident
// list isn't drowned by older comments; count badge surfaces activity
// without opening. Members can write, Penny can write via gadget calls
// from her chat turns; delete is self + admin.
function CommentsSection({
  finding,
  apiKey,
  identity,
  onCountChange,
}: {
  finding: Finding;
  apiKey: string | null;
  identity: {
    user_id?: string | null;
    display_name?: string | null;
    email?: string | null;
    role?: string | null;
  } | null;
  onCountChange: (findingId: string, newCount: number) => void;
}) {
  const [open, setOpen] = useState(false);
  const [comments, setComments] = useState<Comment[]>([]);
  const [loading, setLoading] = useState(false);
  const [draft, setDraft] = useState("");
  const [posting, setPosting] = useState(false);
  const count = finding.comment_count ?? 0;
  const isAdmin = identity?.role === "admin";

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const resp = await invokeAction(apiKey, "loganalysis-comment-list", {
        finding_id: finding.id,
      });
      const payload = unwrapPayload(resp) as { comments?: Comment[] } | undefined;
      setComments(payload?.comments ?? []);
    } catch (e) {
      toast.error(`댓글 로드 실패: ${(e as Error).message}`);
    } finally {
      setLoading(false);
    }
  }, [apiKey, finding.id]);

  const toggle = useCallback(() => {
    const next = !open;
    setOpen(next);
    if (next && comments.length === 0) void load();
  }, [open, comments.length, load]);

  const submit = useCallback(async () => {
    const body = draft.trim();
    if (!body || posting) return;
    setPosting(true);
    try {
      const args: Record<string, unknown> = {
        finding_id: finding.id,
        body,
      };
      if (identity?.user_id) args.actor_user_id = identity.user_id;
      const resp = await invokeAction(apiKey, "loganalysis-comment-add", args);
      const payload = unwrapPayload(resp) as { comment?: Comment } | undefined;
      if (payload?.comment) {
        setComments((prev) => [...prev, payload.comment!]);
        onCountChange(finding.id, count + 1);
      }
      setDraft("");
    } catch (e) {
      toast.error(`댓글 작성 실패: ${(e as Error).message}`);
    } finally {
      setPosting(false);
    }
  }, [apiKey, draft, finding.id, identity?.user_id, posting, count, onCountChange]);

  const remove = useCallback(
    async (c: Comment) => {
      if (!identity?.user_id) return;
      if (!window.confirm("이 댓글을 삭제할까요?")) return;
      try {
        await invokeAction(apiKey, "loganalysis-comment-delete", {
          comment_id: c.id,
          actor_user_id: identity.user_id,
          actor_is_admin: isAdmin,
        });
        setComments((prev) => prev.filter((x) => x.id !== c.id));
        onCountChange(finding.id, Math.max(0, count - 1));
      } catch (e) {
        toast.error(`댓글 삭제 실패: ${(e as Error).message}`);
      }
    },
    [apiKey, identity?.user_id, isAdmin, finding.id, count, onCountChange],
  );

  return (
    <div className="mt-2 border-t border-zinc-800/80 pt-2">
      <button
        type="button"
        onClick={toggle}
        className="flex items-center gap-1.5 text-[11px] text-zinc-400 hover:text-zinc-200"
      >
        <span>{open ? "▾" : "▸"}</span>
        <span>💬 댓글</span>
        {count > 0 && (
          <span className="rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-200">
            {count}
          </span>
        )}
      </button>
      {open && (
        <div className="mt-2 space-y-2">
          {loading && (
            <div className="text-[11px] text-zinc-500">불러오는 중…</div>
          )}
          {!loading && comments.length === 0 && (
            <div className="text-[11px] text-zinc-600">
              아직 댓글이 없습니다. 해결책이나 감상을 남겨보세요.
            </div>
          )}
          {comments.map((c) => {
            const isPenny = c.author_kind === "penny";
            const canDelete =
              isAdmin || (c.author_user_id && c.author_user_id === identity?.user_id);
            return (
              <div
                key={c.id}
                className="rounded border border-zinc-800 bg-zinc-950/40 px-2.5 py-1.5"
              >
                <div className="mb-1 flex items-center gap-2 text-[10px]">
                  {isPenny ? (
                    <span className="rounded bg-purple-950/40 px-1.5 py-0.5 font-semibold uppercase text-purple-300">
                      Penny
                    </span>
                  ) : (
                    <span className="font-semibold text-zinc-400">
                      {c.author_user_id === identity?.user_id
                        ? identity?.display_name || identity?.email || "나"
                        : (c.author_user_id ?? "").slice(0, 8)}
                    </span>
                  )}
                  <span className="text-zinc-600">{relativeTime(c.created_at)}</span>
                  {canDelete && (
                    <button
                      type="button"
                      onClick={() => void remove(c)}
                      className="ml-auto text-zinc-600 hover:text-red-400"
                      title="삭제"
                    >
                      ✕
                    </button>
                  )}
                </div>
                <div className="whitespace-pre-wrap text-[12px] text-zinc-200">
                  {c.body}
                </div>
              </div>
            );
          })}
          {identity?.user_id && (
            <div className="flex items-end gap-2">
              <textarea
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
                    e.preventDefault();
                    void submit();
                  }
                }}
                placeholder="댓글 달기… (Ctrl+Enter 전송)"
                className="min-h-[48px] flex-1 resize-y rounded border border-zinc-800 bg-zinc-950 px-2 py-1 text-[12px] text-zinc-200 placeholder:text-zinc-600 focus:border-zinc-600 focus:outline-none"
              />
              <Button
                type="button"
                size="sm"
                onClick={() => void submit()}
                disabled={posting || draft.trim().length === 0}
              >
                {posting ? "…" : "올리기"}
              </Button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export default function FindingsPage() {
  const { apiKey, identity } = useAuth();
  const [findings, setFindings] = useState<Finding[]>([]);
  const [hosts, setHosts] = useState<Host[]>([]);
  const [statuses, setStatuses] = useState<ScanStatus[]>([]);
  const [hostFilter, setHostFilter] = useState<string | null>(null);
  const [sevFilter, setSevFilter] = useState<Finding["severity"] | null>(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  // Read ?host= from URL once on mount.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const u = new URLSearchParams(window.location.search);
    const h = u.get("host");
    if (h) setHostFilter(h);
  }, []);

  const refresh = useCallback(async () => {
    setLoading(true);
    setErr(null);
    try {
      const args: Record<string, unknown> = { limit: 500 };
      if (hostFilter) args.host_id = hostFilter;
      if (sevFilter) args.severity = sevFilter;
      const [findingsResp, hostsResp, statusResp] = await Promise.all([
        invokeAction(apiKey, "loganalysis-list", args),
        invokeAction(apiKey, "server-list", {}),
        invokeAction(apiKey, "loganalysis-status", {}),
      ]);
      const fp = unwrapPayload(findingsResp) as
        | { findings?: Finding[] }
        | undefined;
      const hp = unwrapPayload(hostsResp) as { hosts?: Host[] } | undefined;
      const sp = unwrapPayload(statusResp) as { hosts?: ScanStatus[] } | undefined;
      setFindings(fp?.findings ?? []);
      setHosts(hp?.hosts ?? []);
      setStatuses(sp?.hosts ?? []);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [apiKey, hostFilter, sevFilter]);

  useEffect(() => {
    void refresh();
    const t = window.setInterval(refresh, 15_000);
    return () => window.clearInterval(t);
  }, [refresh]);

  const setInterval_ = useCallback(
    async (host_id: string, interval_secs: number, enabled: boolean) => {
      try {
        await invokeAction(apiKey, "loganalysis-set-interval", {
          host_id,
          interval_secs,
          enabled,
        });
        // Optimistic local update so the slider doesn't snap back.
        setStatuses((prev) => {
          const idx = prev.findIndex((s) => s.host_id === host_id);
          if (idx >= 0) {
            const next = [...prev];
            next[idx] = { ...next[idx], interval_secs, enabled };
            return next;
          }
          return [
            ...prev,
            { host_id, last_scanned_at: null, interval_secs, enabled },
          ];
        });
      } catch (e) {
        toast.error((e as Error).message);
      }
    },
    [apiKey],
  );

  const scanNow = useCallback(
    async (host_id: string) => {
      try {
        await invokeAction(apiKey, "loganalysis-scan-now", { host_id });
        toast.success("스캔 큐잉됨 (≤ 30초 안에 실행)");
      } catch (e) {
        toast.error((e as Error).message);
      }
    },
    [apiKey],
  );

  const dismiss = useCallback(
    async (id: string) => {
      try {
        await invokeAction(apiKey, "loganalysis-dismiss", {
          id,
          ...(identity?.user_id ? { actor_user_id: identity.user_id } : {}),
        });
        setFindings((prev) => prev.filter((f) => f.id !== id));
      } catch (e) {
        toast.error((e as Error).message);
      }
    },
    [apiKey, identity],
  );

  /// Run the remediation embedded in a finding. Whitelist already
  /// enforced server-side (rules + LLM validate_remediation), so we
  /// trust `tool` to be one of `server.systemctl` / `server.apt`.
  /// On success, auto-dismiss the finding so it doesn't reappear.
  const applyRemediation = useCallback(
    async (f: Finding) => {
      if (!f.remediation) return;
      const action = REMEDIATION_TOOL_TO_ACTION[f.remediation.tool];
      if (!action) {
        toast.error(`unsupported tool: ${f.remediation.tool}`);
        return;
      }
      const label = f.remediation.label ?? f.remediation.tool;
      if (!window.confirm(`실행: ${label} (${f.host_id.slice(0, 8)})?`)) {
        return;
      }
      try {
        const resp = await invokeAction(apiKey, action, {
          id: f.host_id,
          ...f.remediation.args,
        });
        const result = resp.result as
          | { payload?: Array<{ text?: string }> }
          | undefined;
        const out = result?.payload?.[0]?.text ?? "";
        toast.success(`${label} 실행 완료`, {
          description: out.slice(0, 200),
        });
        // Auto-dismiss; the next scan tick will surface a fresh
        // finding if the issue persists.
        await invokeAction(apiKey, "loganalysis-dismiss", {
          id: f.id,
          ...(identity?.user_id ? { actor_user_id: identity.user_id } : {}),
        });
        setFindings((prev) => prev.filter((x) => x.id !== f.id));
      } catch (e) {
        toast.error((e as Error).message);
      }
    },
    [apiKey, identity],
  );

  const hostsById = useMemo(() => {
    const m = new Map<string, Host>();
    for (const h of hosts) m.set(h.id, h);
    return m;
  }, [hosts]);

  // Group by severity then host.
  const grouped = useMemo(() => {
    const out: Record<Finding["severity"], Finding[]> = {
      critical: [],
      high: [],
      medium: [],
      info: [],
    };
    for (const f of findings) {
      out[f.severity].push(f);
    }
    return out;
  }, [findings]);

  const severityCounts = useMemo(() => {
    const c: Record<Finding["severity"], number> = {
      critical: 0,
      high: 0,
      medium: 0,
      info: 0,
    };
    for (const f of findings) c[f.severity]++;
    return c;
  }, [findings]);

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-4 p-6">
        <header className="flex items-center justify-between">
          <div>
            <h1 className="text-lg font-semibold text-zinc-100">Logs</h1>
          </div>
          <div className="flex items-center gap-2">
            {hostFilter && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => {
                  setHostFilter(null);
                  if (typeof window !== "undefined")
                    window.history.replaceState(null, "", "/web/findings");
                }}
                className="h-7 px-2 text-[11px]"
              >
                전체 호스트 ✕
              </Button>
            )}
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void refresh()}
              disabled={loading}
              className="h-7 px-2 text-[11px]"
            >
              {loading ? "…" : "Refresh"}
            </Button>
          </div>
        </header>

        {/* Per-host scan status + interval slider */}
        <details className="rounded border border-zinc-800 bg-zinc-900/50">
          <summary className="cursor-pointer select-none px-3 py-2 text-[11px] uppercase tracking-wider text-zinc-500">
            스캔 상태 ({statuses.length} hosts)
          </summary>
          <div className="space-y-2 border-t border-zinc-800 p-3">
            {statuses.length === 0 ? (
              <div className="text-[11px] text-zinc-600">
                아직 스캔된 호스트가 없습니다 (백그라운드 워커가 첫 tick을
                기다리는 중).
              </div>
            ) : (
              statuses.map((s) => {
                const host = hostsById.get(s.host_id);
                const label = host?.alias ?? host?.host ?? s.host_id.slice(0, 8);
                return (
                  <div
                    key={s.host_id}
                    className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px]"
                  >
                    <span className="min-w-[120px] font-mono font-semibold text-zinc-200">
                      {label}
                    </span>
                    <span className="text-zinc-500">
                      마지막 스캔:{" "}
                      <span className="text-zinc-300">
                        {s.last_scanned_at
                          ? relativeTime(s.last_scanned_at)
                          : "—"}
                      </span>
                    </span>
                    <label className="flex items-center gap-2 text-zinc-500">
                      주기
                      <input
                        type="range"
                        min={30}
                        max={1800}
                        step={30}
                        value={s.interval_secs}
                        onChange={(e) =>
                          void setInterval_(
                            s.host_id,
                            Number(e.target.value),
                            s.enabled,
                          )
                        }
                        className="h-1 w-32 accent-blue-500"
                      />
                      <span className="w-12 text-right font-mono text-zinc-300">
                        {s.interval_secs}s
                      </span>
                    </label>
                    <label className="flex items-center gap-1 text-zinc-500">
                      <input
                        type="checkbox"
                        checked={s.enabled}
                        onChange={(e) =>
                          void setInterval_(
                            s.host_id,
                            s.interval_secs,
                            e.target.checked,
                          )
                        }
                        className="accent-blue-500"
                      />
                      enabled
                    </label>
                    <button
                      type="button"
                      onClick={() => void scanNow(s.host_id)}
                      className="rounded border border-zinc-700 px-1.5 py-0.5 text-[10px] text-zinc-400 hover:border-blue-700 hover:text-blue-300"
                    >
                      지금 스캔
                    </button>
                  </div>
                );
              })
            )}
          </div>
        </details>

        {/* Severity filter pills */}
        <div className="flex gap-1.5">
          <button
            type="button"
            onClick={() => setSevFilter(null)}
            className={`rounded border px-2 py-0.5 text-[11px] ${
              sevFilter == null
                ? "border-zinc-500 bg-zinc-800 text-zinc-100"
                : "border-zinc-800 bg-zinc-900 text-zinc-500"
            }`}
          >
            All ({findings.length})
          </button>
          {SEVERITY_ORDER.map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => setSevFilter(s === sevFilter ? null : s)}
              className={`rounded border px-2 py-0.5 text-[11px] ${
                sevFilter === s
                  ? SEVERITY_TONES[s]
                  : "border-zinc-800 bg-zinc-900 text-zinc-500 hover:text-zinc-300"
              }`}
            >
              {s} ({severityCounts[s]})
            </button>
          ))}
        </div>

        {hostFilter && (
          <div className="rounded border border-blue-900/40 bg-blue-950/20 px-3 py-2 text-[11px] text-blue-300">
            호스트 필터:{" "}
            <span className="font-mono">
              {hostsById.get(hostFilter)?.alias ??
                hostsById.get(hostFilter)?.host ??
                hostFilter.slice(0, 8)}
            </span>
          </div>
        )}

        {err && (
          <div className="rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
            {err}
          </div>
        )}

        {!loading && findings.length === 0 && !err && (
          <div className="rounded border border-zinc-800 bg-zinc-900/50 px-4 py-8 text-center text-[12px] text-zinc-500">
            🟢 처리할 이상 징후가 없습니다.
          </div>
        )}

        {SEVERITY_ORDER.map((sev) =>
          grouped[sev].length === 0 ? null : (
            <section key={sev}>
              <h2 className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
                {sev} ({grouped[sev].length})
              </h2>
              <div className="space-y-2">
                {grouped[sev].map((f) => {
                  const host = hostsById.get(f.host_id);
                  const hostLabel = host?.alias ?? host?.host ?? f.host_id.slice(0, 8);
                  return (
                    <article
                      key={f.id}
                      className={`rounded border ${SEVERITY_TONES[sev]} p-3`}
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0 flex-1">
                          <div className="mb-1 flex flex-wrap items-baseline gap-x-3 gap-y-1 text-[11px]">
                            <span className="font-mono font-semibold">
                              {hostLabel}
                            </span>
                            <span className="text-zinc-500">·</span>
                            <span className="text-zinc-400">{f.source}</span>
                            <span className="text-zinc-500">·</span>
                            <span className="font-mono text-zinc-400">
                              {f.category}
                            </span>
                            {f.classified_by === "penny" && (
                              <span className="rounded bg-purple-950/40 px-1 text-[9px] uppercase text-purple-300">
                                penny
                              </span>
                            )}
                            {f.count > 1 && (
                              <span className="rounded bg-zinc-800 px-1 text-[10px] text-zinc-300">
                                ×{f.count}
                              </span>
                            )}
                            <span className="ml-auto text-[10px] text-zinc-500">
                              {relativeTime(f.ts_last)}
                            </span>
                          </div>
                          <div className="text-[13px]">{f.summary}</div>
                          {f.cause && (
                            <div className="mt-2 text-[11px] text-zinc-300">
                              <span className="text-zinc-500">원인 · </span>
                              {f.cause}
                            </div>
                          )}
                          {f.solution && (
                            <div className="mt-1 flex items-start gap-2 text-[11px] text-zinc-300">
                              <span className="shrink-0 text-zinc-500">
                                해결 ·{" "}
                              </span>
                              <span className="flex-1 whitespace-pre-wrap">
                                {f.solution}
                              </span>
                              {f.remediation && (
                                <button
                                  type="button"
                                  onClick={() => void applyRemediation(f)}
                                  title={`${f.remediation.tool} ${JSON.stringify(f.remediation.args)}`}
                                  className="shrink-0 rounded border border-blue-700 bg-blue-950/40 px-2 py-0.5 text-[11px] font-bold text-blue-200 hover:border-blue-500 hover:bg-blue-900/60"
                                >
                                  ⚡ {f.remediation.label ?? "승인 실행"}
                                </button>
                              )}
                            </div>
                          )}
                          <pre className="mt-2 max-h-32 overflow-auto rounded bg-zinc-950/50 p-2 text-[11px] text-zinc-400">
                            {f.excerpt}
                          </pre>
                        </div>
                        <div className="flex shrink-0 flex-col gap-1">
                          <button
                            type="button"
                            onClick={() => void dismiss(f.id)}
                            title="목록에서 감춥니다 (실제로 고치지는 않음). 같은 이슈가 다시 발생하면 새 finding으로 다시 올라옵니다."
                            className="rounded border border-zinc-700 px-2 py-1 text-[11px] text-zinc-300 hover:border-zinc-500 hover:bg-zinc-800 hover:text-zinc-100"
                          >
                            🙈 감추기
                          </button>
                          <button
                            type="button"
                            onClick={() => openChatAboutFinding(f, hostLabel)}
                            title="이 이슈를 주제로 Penny와 새 대화를 시작합니다."
                            className="rounded border border-purple-800 bg-purple-950/30 px-2 py-1 text-[11px] text-purple-200 hover:border-purple-500 hover:bg-purple-900/40"
                          >
                            💬 Penny와 상의
                          </button>
                        </div>
                      </div>
                      <CommentsSection
                        finding={f}
                        apiKey={apiKey}
                        identity={identity}
                        onCountChange={(id, n) =>
                          setFindings((prev) =>
                            prev.map((x) =>
                              x.id === id ? { ...x, comment_count: n } : x,
                            ),
                          )
                        }
                      />
                    </article>
                  );
                })}
              </div>
            </section>
          ),
        )}
      </div>
      <Toaster theme="dark" position="top-right" richColors />
    </div>
  );
}
