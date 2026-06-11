"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  PanelRight,
  X,
  Zap,
  BookOpen,
  Activity,
  MessageSquareText,
  Settings as SettingsIcon,
} from "lucide-react";
import { useEvidence, type EvidenceItem } from "../../lib/evidence-context";
import { useAuth } from "../../lib/auth-context";
import { ContextTab } from "./side-panel-context";
import { getApiBase } from "../../lib/workbench-client";

// ---------------------------------------------------------------------------
// Side panel (ex-Evidence)
//
// Three tabs:
//   Actions  (default) — pending operator decisions. Today sourced
//                         from `loganalysis-list`: every finding with
//                         a `remediation` field surfaces as a one-click
//                         card. Future home of Penny's inline action
//                         proposals once the Ask approval flow (Task
//                         #52) lands.
//   Sources           — wiki / web calls consumed in the current
//                         conversation. Filtered from the Evidence WS
//                         feed; lets the operator audit Penny's
//                         citations without scrolling chat history.
//   Activity          — full raw tool/action log (previous default).
//                         Useful as backstage debugging, not primary.
// ---------------------------------------------------------------------------

interface EvidencePaneProps {
  open: boolean;
  onToggle: (open: boolean) => void;
  width?: number;
}

type TabId = "context" | "actions" | "sources" | "activity" | "settings";

function formatRelative(at: number, now: number): string {
  const s = Math.max(0, Math.floor((now - at) / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return `${h}h`;
}

function isKnowledgeKind(name: string): boolean {
  return (
    name.startsWith("wiki.") ||
    name === "web.search" ||
    name === "knowledge-search" ||
    name === "wiki-read" ||
    name === "wiki-list" ||
    name === "wiki-search"
  );
}

// ---------------------------------------------------------------------------
// Actions tab — pending remediations
// ---------------------------------------------------------------------------

interface PendingAction {
  findingId: string;
  hostId: string;
  severity: "critical" | "high" | "medium" | "info";
  category: string;
  summary: string;
  remediation: {
    tool: string;
    args: Record<string, unknown>;
    label?: string;
  };
}

function useActionsFeed(apiKey: string | null): PendingAction[] {
  const [actions, setActions] = useState<PendingAction[]>([]);
  useEffect(() => {
    let cancel = false;
    let timer: ReturnType<typeof setInterval> | null = null;
    const fetchOnce = async () => {
      try {
        const res = await fetch(
          `${getApiBase()}/workbench/actions/loganalysis-list`,
          {
            method: "POST",
            credentials: "include",
            headers: {
              ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
              "Content-Type": "application/json",
            },
            body: JSON.stringify({ args: {} }),
          },
        );
        if (!res.ok) return;
        const body = await res.json();
        const payload = body?.result?.payload;
        const raw =
          Array.isArray(payload) && payload[0]?.text
            ? JSON.parse(payload[0].text)
            : null;
        const findings: Array<Record<string, unknown>> = raw?.findings ?? [];
        const next: PendingAction[] = [];
        for (const f of findings) {
          const rem = f.remediation as PendingAction["remediation"] | null;
          if (!rem || typeof rem !== "object" || !rem.tool) continue;
          next.push({
            findingId: String(f.id),
            hostId: String(f.host_id),
            severity: (f.severity as PendingAction["severity"]) ?? "info",
            category: String(f.category ?? ""),
            summary: String(f.summary ?? ""),
            remediation: rem,
          });
        }
        if (!cancel) setActions(next);
      } catch {
        // Ignore — keep whatever we had on transient fail.
      }
    };
    void fetchOnce();
    timer = setInterval(fetchOnce, 15_000);
    return () => {
      cancel = true;
      if (timer) clearInterval(timer);
    };
  }, [apiKey]);
  return actions;
}

// -----------------------------------------------------------------
// Pending approvals (distinct from finding remediations — these are
// Penny / workbench calls sitting at the `pending_approval` stage of
// the action lifecycle, waiting for an operator click).
// -----------------------------------------------------------------

interface PendingApproval {
  id: string;
  actionId: string;
  gadgetName: string | null;
  args: unknown;
  createdAt: string;
}

// `host_id` → display alias, used to humanize approval cards (the
// operator sees `dg4R-4090-4` instead of a UUID slug). Refreshed
// every 60 s — far slower than the 5 s approvals poll because the
// fleet rarely churns and `server.list` is an SSH-touching call we
// don't want to over-pull.
function useHostAliasMap(apiKey: string | null): Record<string, string> {
  const [map, setMap] = useState<Record<string, string>>({});
  useEffect(() => {
    let cancel = false;
    const fetchOnce = async () => {
      try {
        const res = await fetch(
          `${getApiBase()}/workbench/actions/server-list`,
          {
            method: "POST",
            credentials: "include",
            headers: {
              ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
              "Content-Type": "application/json",
            },
            body: JSON.stringify({ args: {} }),
          },
        );
        if (!res.ok) return;
        const body = (await res.json()) as {
          result?: { payload?: Array<{ text?: string }> };
        };
        const text = body.result?.payload?.[0]?.text;
        if (typeof text !== "string") return;
        const data = JSON.parse(text) as {
          hosts?: Array<{ id: string; alias?: string | null; host: string }>;
        };
        const next: Record<string, string> = {};
        for (const h of data.hosts ?? []) {
          next[h.id] = h.alias ?? h.host;
        }
        if (!cancel) setMap(next);
      } catch {
        // keep existing on transient fail
      }
    };
    void fetchOnce();
    const timer = setInterval(fetchOnce, 60_000);
    return () => {
      cancel = true;
      clearInterval(timer);
    };
  }, [apiKey]);
  return map;
}

// Pull a one-line summary of the most operator-relevant arg out of a
// pending approval, keyed on the gadget name. We deliberately keep
// this short (truncated at ~120 chars) so the collapsed card fits
// the narrow Side Panel; the operator can hit "펼치기" to see the
// full JSON.
function approvalSummaryLine(
  gadgetName: string | null,
  args: unknown,
): string | null {
  if (!args || typeof args !== "object") return null;
  const a = args as Record<string, unknown>;
  const name = gadgetName ?? "";
  // Tool-specific surface: pull the field the operator cares about.
  if (name === "server.bash" && typeof a.command === "string") {
    return `$ ${a.command}`;
  }
  if (name === "server.systemctl") {
    const action = typeof a.action === "string" ? a.action : "?";
    const unit = typeof a.unit === "string" ? a.unit : "?";
    return `systemctl ${action} ${unit}`;
  }
  if (name === "server.add" || name === "server.update") {
    const host = typeof a.host === "string" ? a.host : "?";
    const user = typeof a.ssh_user === "string" ? a.ssh_user : "?";
    return `${user}@${host}`;
  }
  if (name === "server.remove") {
    const host = typeof a.host === "string" ? a.host : "?";
    return `remove ${host}`;
  }
  // Fallback: compact JSON sans the host_id (which we surface above).
  const filtered: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(a)) {
    if (k === "id" || k === "host_id") continue;
    filtered[k] = v;
  }
  if (Object.keys(filtered).length === 0) return null;
  const s = JSON.stringify(filtered);
  return s.length > 200 ? `${s.slice(0, 200)}…` : s;
}

function approvalHostLine(
  args: unknown,
  hostMap: Record<string, string>,
): string | null {
  if (!args || typeof args !== "object") return null;
  const a = args as Record<string, unknown>;
  const id =
    (typeof a.id === "string" && a.id) ||
    (typeof a.host_id === "string" && a.host_id) ||
    null;
  if (!id) return null;
  const alias = hostMap[id];
  if (alias) return `${alias}`;
  return `${id.slice(0, 8)}…`;
}

function relativeAge(iso: string): string {
  const d = new Date(iso);
  const diff = Math.max(0, (Date.now() - d.getTime()) / 1000);
  if (diff < 60) return `${Math.floor(diff)}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  return `${Math.floor(diff / 3600)}h ago`;
}

function ApprovalCard({
  a,
  hostMap,
  decide,
}: {
  a: PendingApproval;
  hostMap: Record<string, string>;
  decide: (approvalId: string, approve: boolean, reason?: string) => Promise<void>;
}) {
  const [expanded, setExpanded] = useState(false);
  const summary = approvalSummaryLine(a.gadgetName, a.args);
  const hostLine = approvalHostLine(a.args, hostMap);
  const fullJson =
    a.args && typeof a.args === "object"
      ? JSON.stringify(a.args, null, 2)
      : String(a.args);
  return (
    <li className="border-b border-purple-900/50 bg-purple-950/20 px-3 py-2 text-[11px] text-purple-100">
      <div className="flex items-center justify-between gap-2">
        <span className="truncate font-mono font-semibold text-purple-100">
          ⏳ {a.gadgetName ?? a.actionId}
        </span>
        <span className="shrink-0 text-[9px] text-purple-400">
          {relativeAge(a.createdAt)}
        </span>
      </div>
      {hostLine && (
        <div className="mt-1 flex items-center gap-1 text-[10.5px] text-purple-300">
          <span className="text-purple-500">host:</span>
          <span className="truncate font-mono text-purple-100">{hostLine}</span>
        </div>
      )}
      {summary && (
        <div
          className="mt-1 break-all rounded bg-black/30 px-1.5 py-1 font-mono text-[10.5px] text-purple-100"
          title={summary}
        >
          {summary}
        </div>
      )}
      {expanded && (
        <pre className="mt-1 max-h-48 overflow-auto rounded bg-black/40 px-2 py-1 font-mono text-[10px] leading-snug text-purple-200">
          {fullJson}
        </pre>
      )}
      <div className="mt-2 flex items-center gap-1.5">
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className="rounded border border-purple-800 bg-purple-950/30 px-1.5 py-0.5 font-mono text-[10px] text-purple-300 hover:border-purple-500 hover:text-purple-100"
        >
          {expanded ? "▴ Collapse" : "▾ Full arguments"}
        </button>
        <span className="ml-auto" />
        <button
          type="button"
          onClick={() => void decide(a.id, true)}
          className="shrink-0 rounded border border-emerald-700 bg-emerald-950/40 px-2 py-0.5 font-mono text-[10.5px] font-semibold text-emerald-200 hover:border-emerald-500 hover:bg-emerald-900/60"
          title="Approve"
        >
          ⚡ Approve
        </button>
        <button
          type="button"
          onClick={() => void decide(a.id, false)}
          className="shrink-0 rounded border border-red-800 bg-red-950/30 px-2 py-0.5 font-mono text-[10.5px] font-semibold text-red-200 hover:border-red-500 hover:bg-red-900/40"
          title="Deny"
        >
          ✕ Deny
        </button>
      </div>
    </li>
  );
}

function usePendingApprovalsFeed(apiKey: string | null): PendingApproval[] {
  const [items, setItems] = useState<PendingApproval[]>([]);
  useEffect(() => {
    let cancel = false;
    let timer: ReturnType<typeof setInterval> | null = null;
    const fetchOnce = async () => {
      try {
        const res = await fetch(`${getApiBase()}/workbench/approvals/pending`, {
          credentials: "include",
          headers: apiKey ? { Authorization: `Bearer ${apiKey}` } : {},
        });
        if (!res.ok) return;
        const body = (await res.json()) as {
          approvals?: Array<{
            id: string;
            action_id: string;
            gadget_name: string | null;
            args: unknown;
            created_at: string;
          }>;
        };
        const next: PendingApproval[] = (body.approvals ?? []).map((r) => ({
          id: r.id,
          actionId: r.action_id,
          gadgetName: r.gadget_name,
          args: r.args,
          createdAt: r.created_at,
        }));
        if (!cancel) setItems(next);
      } catch {
        // keep existing on transient fail
      }
    };
    void fetchOnce();
    timer = setInterval(fetchOnce, 5_000);
    return () => {
      cancel = true;
      if (timer) clearInterval(timer);
    };
  }, [apiKey]);
  return items;
}

function severityTint(s: PendingAction["severity"]): string {
  switch (s) {
    case "critical":
      return "border-red-800 bg-red-950/30 text-red-200";
    case "high":
      return "border-amber-800 bg-amber-950/30 text-amber-200";
    case "medium":
      return "border-yellow-800 bg-yellow-950/30 text-yellow-200";
    default:
      return "border-zinc-800 bg-zinc-900/40 text-zinc-300";
  }
}

function ActionsTab({ apiKey }: { apiKey: string | null }) {
  const hostMap = useHostAliasMap(apiKey);
  const actions = useActionsFeed(apiKey);
  const approvals = usePendingApprovalsFeed(apiKey);
  const decide = useCallback(
    async (approvalId: string, approve: boolean, reason?: string) => {
      const verb = approve ? "approve" : "deny";
      if (
        !window.confirm(
          approve
            ? "Approve this pending action?"
            : "Deny this pending action?",
        )
      )
        return;
      try {
        const res = await fetch(
          `${getApiBase()}/workbench/approvals/${approvalId}/${verb}`,
          {
            method: "POST",
            credentials: "include",
            headers: {
              ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
              "Content-Type": "application/json",
            },
            body: approve
              ? "{}"
              : JSON.stringify({ reason: reason ?? "" }),
          },
        );
        if (!res.ok) {
          const body = await res.text();
          alert(`${verb} failed: ${res.status} ${body.slice(0, 200)}`);
        }
      } catch (e) {
        alert(`${verb} failed: ${(e as Error).message}`);
      }
    },
    [apiKey],
  );
  const run = useCallback(
    async (a: PendingAction) => {
      const label =
        a.remediation.label ??
        `${a.remediation.tool} ${JSON.stringify(a.remediation.args)}`;
      if (!window.confirm(`Run this action?\n\n${label}`)) return;
      try {
        const actionId = a.remediation.tool.replace(".", "-");
        const args = { ...a.remediation.args, id: a.hostId };
        await fetch(`${getApiBase()}/workbench/actions/${actionId}`, {
          method: "POST",
          credentials: "include",
          headers: {
            ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ args }),
        });
        // Auto-dismiss the finding so it stops surfacing.
        await fetch(
          `${getApiBase()}/workbench/actions/loganalysis-dismiss`,
          {
            method: "POST",
            credentials: "include",
            headers: {
              ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
              "Content-Type": "application/json",
            },
            body: JSON.stringify({ args: { id: a.findingId } }),
          },
        );
      } catch (e) {
        alert(`Run failed: ${(e as Error).message}`);
      }
    },
    [apiKey],
  );

  if (actions.length === 0 && approvals.length === 0) {
    return (
      <div
        className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center"
        data-testid="actions-empty"
      >
        <Zap className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-400">No pending actions</p>
        <p className="text-[11px] leading-relaxed text-zinc-600">
          Actionable suggestions from Penny queue up here.
          <br />
          The ⚡ button on a Logs finding runs through the same path.
        </p>
      </div>
    );
  }
  return (
    <ol className="flex-1 overflow-y-auto" data-testid="actions-list">
      {approvals.map((a) => (
        <ApprovalCard
          key={`approval-${a.id}`}
          a={a}
          hostMap={hostMap}
          decide={decide}
        />
      ))}
      {actions.map((a) => (
        <li
          key={`action-${a.findingId}`}
          className={`border-b border-zinc-900 px-3 py-2 text-[11px] ${severityTint(a.severity)}`}
        >
          <div className="flex items-center justify-between gap-2">
            <span className="truncate font-mono">{a.category}</span>
            <span className="shrink-0 rounded bg-black/20 px-1 text-[9px] uppercase">
              {a.severity}
            </span>
          </div>
          <div className="mt-0.5 truncate text-[11px] text-zinc-100">
            {a.summary}
          </div>
          <div className="mt-1 flex items-center gap-2">
            <code className="truncate rounded bg-black/30 px-1.5 py-0.5 font-mono text-[10px] text-zinc-300">
              {a.remediation.tool}{" "}
              {Object.entries(a.remediation.args)
                .map(([k, v]) => `${k}=${JSON.stringify(v)}`)
                .join(" ")}
            </code>
            <button
              type="button"
              onClick={() => void run(a)}
              className="ml-auto shrink-0 rounded border border-blue-700 bg-blue-950/40 px-2 py-0.5 font-mono text-[10px] font-semibold text-blue-200 hover:border-blue-500 hover:bg-blue-900/60"
            >
              ⚡ {a.remediation.label ?? "Run"}
            </button>
          </div>
        </li>
      ))}
    </ol>
  );
}

// ---------------------------------------------------------------------------
// Settings tab — per-bucket approval mode editor
// ---------------------------------------------------------------------------

type GadgetMode = "auto" | "ask" | "never";

interface WriteGadgetsConfig {
  default_mode: GadgetMode;
  wiki_write: GadgetMode;
  infra_write: GadgetMode;
  scheduler_write: GadgetMode;
  provider_mutate: GadgetMode;
  server_admin: GadgetMode;
  loganalysis_admin: GadgetMode;
}

interface DestructiveGadgetsConfig {
  enabled: boolean;
  max_per_hour: number;
  extra_confirmation: "none" | "env" | "file";
  extra_confirmation_token_file: string;
}

interface GadgetsConfig {
  read: GadgetMode;
  approval_timeout_secs: number;
  write: WriteGadgetsConfig;
  destructive: DestructiveGadgetsConfig;
}

const WRITE_BUCKETS: Array<{ key: keyof WriteGadgetsConfig; label: string; hint: string }> = [
  { key: "default_mode", label: "Default (default_mode)", hint: "Write tools not matched by a bucket" },
  { key: "wiki_write", label: "Wiki (wiki_write)", hint: "wiki.write / wiki.create / wiki.delete" },
  { key: "server_admin", label: "Server admin (server_admin)", hint: "server.bash / server.systemctl / server.add / server.remove" },
  { key: "loganalysis_admin", label: "Log analysis (loganalysis_admin)", hint: "loganalysis.dismiss / set_interval / comment_* (DB only)" },
  { key: "infra_write", label: "Infra (infra_write)", hint: "infra.*" },
  { key: "scheduler_write", label: "Scheduler (scheduler_write)", hint: "scheduler.*" },
  { key: "provider_mutate", label: "Provider (provider_mutate)", hint: "infra.rotate_api_key / infra.add_provider" },
];

function SettingsTab({ apiKey }: { apiKey: string | null }) {
  const [cfg, setCfg] = useState<GadgetsConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    setErr(null);
    fetch(`${getApiBase()}/workbench/agent/modes`, {
      credentials: "include",
      headers: apiKey ? { Authorization: `Bearer ${apiKey}` } : undefined,
    })
      .then(async (r) => {
        if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
        return r.json();
      })
      .then((j) => {
        if (alive) setCfg(j.gadgets as GadgetsConfig);
      })
      .catch((e) => alive && setErr((e as Error).message))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [apiKey]);

  const save = useCallback(
    async (next: GadgetsConfig, changedKey: string) => {
      setSaving(changedKey);
      setErr(null);
      try {
        const r = await fetch(`${getApiBase()}/workbench/agent/modes`, {
          method: "PATCH",
          credentials: "include",
          headers: {
            ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
            "Content-Type": "application/json",
          },
          body: JSON.stringify(next),
        });
        if (!r.ok) {
          const body = await r.text();
          throw new Error(`${r.status} ${body.slice(0, 200)}`);
        }
        const j = await r.json();
        setCfg(j.gadgets as GadgetsConfig);
      } catch (e) {
        setErr((e as Error).message);
      } finally {
        setSaving(null);
      }
    },
    [apiKey],
  );

  if (loading) {
    return (
      <div
        className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center"
        data-testid="settings-loading"
      >
        <SettingsIcon className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-400">Loading settings…</p>
      </div>
    );
  }
  if (err && !cfg) {
    return (
      <div
        className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center"
        data-testid="settings-error"
      >
        <SettingsIcon className="size-4 text-red-700" aria-hidden />
        <p className="text-xs font-medium text-red-400">Settings API error</p>
        <p className="break-all text-[10px] leading-relaxed text-zinc-500">{err}</p>
      </div>
    );
  }
  if (!cfg) return null;

  return (
    <div className="flex-1 overflow-y-auto" data-testid="settings-panel">
      <div className="border-b border-zinc-900 px-3 py-2 text-[11px] text-zinc-300">
        <div className="font-semibold text-zinc-200">Approval mode per write-tool bucket</div>
        <p className="mt-0.5 text-[10px] leading-relaxed text-zinc-500">
          <strong>Auto</strong>: run immediately · <strong>Ask</strong>: approval card · <strong>Never</strong>: blocked
        </p>
      </div>
      <ul className="px-2 py-2">
        {WRITE_BUCKETS.map(({ key, label, hint }) => {
          const value = cfg.write[key];
          return (
            <li
              key={key}
              className="flex items-start justify-between gap-2 border-b border-zinc-900/60 py-2 pl-1"
            >
              <div className="flex-1 min-w-0">
                <div className="truncate text-[11px] font-medium text-zinc-200">
                  {label}
                </div>
                <div className="mt-0.5 truncate text-[10px] text-zinc-500" title={hint}>
                  {hint}
                </div>
              </div>
              <select
                aria-label={`${label} mode`}
                data-testid={`settings-bucket-${key}`}
                disabled={saving !== null}
                value={value}
                onChange={(e) => {
                  const newMode = e.target.value as GadgetMode;
                  const next: GadgetsConfig = {
                    ...cfg,
                    write: { ...cfg.write, [key]: newMode },
                  };
                  setCfg(next);
                  void save(next, key);
                }}
                className="shrink-0 rounded border border-zinc-700 bg-zinc-900 px-1.5 py-0.5 font-mono text-[11px] text-zinc-100 hover:border-zinc-600 disabled:opacity-50"
              >
                <option value="auto">auto</option>
                <option value="ask">ask</option>
                <option value="never">never</option>
              </select>
            </li>
          );
        })}
        <li className="flex items-start justify-between gap-2 border-b border-zinc-900/60 py-2 pl-1">
          <div className="flex-1 min-w-0">
            <div className="truncate text-[11px] font-medium text-zinc-200">
              Destructive tools
            </div>
            <div className="mt-0.5 truncate text-[10px] text-zinc-500">
              T3 allowance — Ask is enforced; this toggle is only ON/OFF
            </div>
          </div>
          <label className="flex shrink-0 items-center gap-1 text-[11px] text-zinc-300">
            <input
              type="checkbox"
              data-testid="settings-destructive-enabled"
              disabled={saving !== null}
              checked={cfg.destructive.enabled}
              onChange={(e) => {
                const next: GadgetsConfig = {
                  ...cfg,
                  destructive: { ...cfg.destructive, enabled: e.target.checked },
                };
                setCfg(next);
                void save(next, "destructive");
              }}
            />
            enabled
          </label>
        </li>
      </ul>
      <div className="px-3 py-2 text-[10px] leading-relaxed text-zinc-500">
        Changes apply from the next Penny dispatch. Already-running subprocesses
        keep the <code>--allowed-tools</code> list fixed at spawn.
        {err && (
          <div className="mt-1 break-all text-red-400" data-testid="settings-save-error">
            Save failed: {err}
          </div>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Sources tab — current-conversation citations
// ---------------------------------------------------------------------------

function SourcesTab({ items }: { items: EvidenceItem[] }) {
  const filtered = useMemo(
    () => items.filter((i) => isKnowledgeKind(i.name)),
    [items],
  );
  if (filtered.length === 0) {
    return (
      <div
        className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center"
        data-testid="sources-empty"
      >
        <BookOpen className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-400">No citations</p>
        <p className="text-[11px] leading-relaxed text-zinc-600">
          Sources appear here when Penny consults wiki pages or the web.
        </p>
      </div>
    );
  }
  return (
    <ol
      className="flex-1 overflow-y-auto"
      aria-label="Sources feed"
      data-testid="sources-list"
    >
      {filtered.map((item) => (
        <EvidenceRow key={item.id} item={item} />
      ))}
    </ol>
  );
}

// ---------------------------------------------------------------------------
// Activity tab — full raw evidence log (old default)
// ---------------------------------------------------------------------------

function ActivityTab({ items }: { items: EvidenceItem[] }) {
  if (items.length === 0) {
    return (
      <div
        className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center"
        data-testid="activity-empty"
      >
        <Activity className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-400">No activity yet</p>
        <p className="text-[11px] leading-relaxed text-zinc-600">
          Penny's read-tier calls and workbench actions stream here live.
        </p>
      </div>
    );
  }
  return (
    <ol
      className="flex-1 overflow-y-auto"
      aria-label="Activity feed"
      data-testid="evidence-list"
    >
      {items.map((item) => (
        <EvidenceRow key={item.id} item={item} />
      ))}
    </ol>
  );
}

// ---------------------------------------------------------------------------
// Shared row
// ---------------------------------------------------------------------------

function renderArgsPreview(item: EvidenceItem): string | null {
  const parsed = item.argumentsParsed;
  if (parsed) {
    if (typeof parsed.name === "string") return String(parsed.name);
    if (typeof parsed.query === "string") return `"${String(parsed.query)}"`;
    if (typeof parsed.path === "string") return String(parsed.path);
  }
  return item.argumentsSummary ?? null;
}

function EvidenceRow({ item }: { item: EvidenceItem }) {
  const now = Date.now();
  const ok = item.outcome === "success" || item.outcome === "ok";
  const argsPreview = renderArgsPreview(item);
  const inner = (
    <>
      <div className="flex items-center justify-between gap-2">
        <span
          className={`truncate font-mono ${ok ? "text-zinc-300" : "text-amber-400"}`}
          title={item.name}
        >
          {item.name}
        </span>
        <span className="shrink-0 font-mono text-[10px] text-zinc-600">
          {formatRelative(item.at, now)}
        </span>
      </div>
      {argsPreview && (
        <div
          className="mt-0.5 truncate font-mono text-[10px] text-zinc-400"
          data-testid="evidence-args"
          title={item.argumentsSummary ?? argsPreview}
        >
          {argsPreview}
        </div>
      )}
      <div className="mt-0.5 flex items-center gap-2 text-[10px] text-zinc-600">
        <span className="font-mono">{item.kind}</span>
        {item.tier && (
          <span className="rounded bg-zinc-900 px-1 text-zinc-500">{item.tier}</span>
        )}
        {typeof item.elapsedMs === "number" && (
          <span className="font-mono">{item.elapsedMs}ms</span>
        )}
        {!ok && (
          <span className="rounded bg-red-950/50 px-1 text-red-400">
            {item.outcome}
          </span>
        )}
      </div>
    </>
  );
  const common =
    "block border-b border-zinc-900 px-3 py-2 text-[11px] hover:bg-zinc-900/40";
  if (item.href) {
    return (
      <li data-testid="evidence-item" data-kind={item.kind} data-outcome={item.outcome}>
        <a
          href={item.href}
          target="_blank"
          rel="noopener noreferrer"
          className={`${common} cursor-pointer no-underline`}
          data-testid="evidence-link"
        >
          {inner}
        </a>
      </li>
    );
  }
  return (
    <li
      data-testid="evidence-item"
      data-kind={item.kind}
      data-outcome={item.outcome}
      className={common}
    >
      {inner}
    </li>
  );
}

// ---------------------------------------------------------------------------
// Main pane
// ---------------------------------------------------------------------------

export function EvidencePane({ open, onToggle, width = 320 }: EvidencePaneProps) {
  const { items, wsStatus, clear } = useEvidence();
  const { apiKey } = useAuth();
  const [tab, setTab] = useState<TabId>("context");

  const actionsBadge = useActionsFeed(apiKey);
  const approvalsBadge = usePendingApprovalsFeed(apiKey);
  const pendingCount = actionsBadge.length + approvalsBadge.length;
  const sourcesBadge = useMemo(
    () => items.filter((i) => isKnowledgeKind(i.name)).length,
    [items],
  );
  const totalBadge = pendingCount + sourcesBadge;

  // Auto-open the panel + switch to Actions tab whenever a NEW pending
  // approval shows up. Operators were missing approval cards while the
  // panel was collapsed — an invisible queue is worse than an
  // auto-intrusion. We track the previous count across renders and
  // only trigger on the 0→N transition (a single approval landing),
  // not on every poll.
  const prevApprovalsCount = useMemo(() => ({ value: 0 }), []);
  useEffect(() => {
    const cur = approvalsBadge.length;
    const prev = prevApprovalsCount.value;
    prevApprovalsCount.value = cur;
    if (cur > prev && cur > 0) {
      setTab("actions");
      if (!open) {
        if (typeof window !== "undefined") {
          localStorage.setItem(
            "gadgetron.workbench.evidencePaneOpen",
            "true",
          );
        }
        onToggle(true);
      }
    }
  }, [approvalsBadge.length, open, onToggle, prevApprovalsCount]);

  if (!open) {
    return (
      <div
        className="flex w-8 shrink-0 flex-col items-center gap-2 border-l border-zinc-800 bg-zinc-950 pt-3"
        data-testid="evidence-pane-collapsed"
      >
        <button
          type="button"
          aria-label="Open side panel"
          data-testid="evidence-pane-expand-btn"
          onClick={() => {
            if (typeof window !== "undefined") {
              localStorage.setItem(
                "gadgetron.workbench.evidencePaneOpen",
                "true",
              );
            }
            onToggle(true);
          }}
          className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
        >
          <PanelRight className="size-3.5" aria-hidden />
        </button>
        {pendingCount > 0 && (
          <span
            data-testid="evidence-pane-badge"
            className="rounded bg-blue-900/50 px-1 font-mono text-[9px] text-blue-300"
            title={`${pendingCount} pending actions`}
          >
            ⚡{pendingCount}
          </span>
        )}
        {sourcesBadge > 0 && pendingCount === 0 && (
          <span
            data-testid="evidence-pane-badge-sources"
            className="rounded bg-zinc-800 px-1 font-mono text-[9px] text-zinc-400"
            title={`${sourcesBadge} citations`}
          >
            {sourcesBadge}
          </span>
        )}
      </div>
    );
  }

  return (
    <aside
      data-testid="evidence-pane"
      className="flex shrink-0 flex-col border-l border-zinc-800 bg-zinc-950"
      style={{ width }}
      aria-label="Side panel"
    >
      {/* Tab row + controls — icon-only so the narrow panel isn't dominated by labels */}
      <div className="flex h-9 shrink-0 items-center gap-0.5 border-b border-zinc-800 px-1">
        <TabButton
          active={tab === "context"}
          onClick={() => setTab("context")}
          label="Context"
          count={0}
          icon={<MessageSquareText className="size-3.5" aria-hidden />}
        />
        <TabButton
          active={tab === "actions"}
          onClick={() => setTab("actions")}
          label="Actions"
          count={pendingCount}
          icon={<Zap className="size-3.5" aria-hidden />}
        />
        <TabButton
          active={tab === "sources"}
          onClick={() => setTab("sources")}
          label="Sources"
          count={sourcesBadge}
          icon={<BookOpen className="size-3.5" aria-hidden />}
        />
        <TabButton
          active={tab === "activity"}
          onClick={() => setTab("activity")}
          label="Activity"
          count={items.length}
          icon={<Activity className="size-3.5" aria-hidden />}
        />
        <TabButton
          active={tab === "settings"}
          onClick={() => setTab("settings")}
          label="Settings"
          count={0}
          icon={<SettingsIcon className="size-3.5" aria-hidden />}
        />
        <div className="ml-auto flex items-center gap-1 pr-1">
          <span
            data-testid="evidence-ws-status"
            className={`rounded border px-1 py-px font-mono text-[9px] ${
              wsStatus === "open"
                ? "border-emerald-700/40 bg-emerald-900/20 text-emerald-400"
                : wsStatus === "connecting"
                  ? "border-amber-700/40 bg-amber-900/20 text-amber-400"
                  : "border-zinc-700 bg-zinc-900 text-zinc-500"
            }`}
            title={`WebSocket ${wsStatus}`}
          >
            {wsStatus === "open" ? "●" : wsStatus === "connecting" ? "…" : "○"}
          </span>
          {tab === "activity" && items.length > 0 && (
            <button
              type="button"
              aria-label="Clear activity"
              data-testid="evidence-pane-clear-btn"
              onClick={clear}
              className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
              title="Clear activity log"
            >
              <X className="size-3" aria-hidden />
            </button>
          )}
          <button
            type="button"
            aria-label="Collapse side panel"
            data-testid="evidence-pane-collapse-btn"
            onClick={() => {
              if (typeof window !== "undefined") {
                localStorage.setItem(
                  "gadgetron.workbench.evidencePaneOpen",
                  "false",
                );
              }
              onToggle(false);
            }}
            className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
          >
            <PanelRight className="size-3.5" aria-hidden />
          </button>
        </div>
      </div>

      {/* Tab body */}
      {tab === "context" && <ContextTab />}
      {tab === "actions" && <ActionsTab apiKey={apiKey} />}
      {tab === "sources" && <SourcesTab items={items} />}
      {tab === "activity" && <ActivityTab items={items} />}
      {tab === "settings" && <SettingsTab apiKey={apiKey} />}
      {/* Hidden totalBadge marker for selectors/tests */}
      <span className="hidden" data-testid="side-panel-total-badge">
        {totalBadge}
      </span>
    </aside>
  );
}

function TabButton({
  active,
  onClick,
  label,
  count,
  icon,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  count: number;
  icon: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      aria-label={label}
      title={count > 0 ? `${label} (${count})` : label}
      className={`relative flex size-7 shrink-0 items-center justify-center rounded transition-colors ${
        active
          ? "bg-zinc-800 text-zinc-100"
          : "text-zinc-500 hover:bg-zinc-900 hover:text-zinc-300"
      }`}
    >
      {icon}
      {count > 0 && (
        <span
          className={`absolute -right-0.5 -top-0.5 min-w-[14px] rounded-full px-1 font-mono text-[9px] leading-[14px] tabular-nums ${
            active ? "bg-blue-700 text-white" : "bg-blue-900/70 text-blue-100"
          }`}
        >
          {count > 99 ? "99+" : count}
        </span>
      )}
    </button>
  );
}
