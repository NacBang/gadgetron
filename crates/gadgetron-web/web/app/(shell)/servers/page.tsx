"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { Toaster, toast } from "sonner";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { Textarea } from "../../components/ui/textarea";
import { useAuth } from "../../lib/auth-context";
import { safeRandomUUID } from "../../lib/uuid";

// ---------------------------------------------------------------------------
// /web/servers — server-monitor bundle UI.
//
// Three-mode add form (key_path / key_paste / password_bootstrap) on top,
// grid of registered hosts below. Each card polls `server.stats` every
// 5 seconds; clicking the card opens a detail sheet with per-GPU, per-disk,
// and per-chip temperature breakdowns.
// ---------------------------------------------------------------------------

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

interface Host {
  id: string;
  host: string;
  ssh_user: string;
  ssh_port: number;
  created_at: string;
  last_ok_at: string | null;
}

interface GpuStats {
  index: number;
  name: string;
  util_pct: number | null;
  mem_used_mib: number | null;
  mem_total_mib: number | null;
  temp_c: number | null;
  power_w: number | null;
  power_limit_w: number | null;
  source: string;
}

interface ServerStats {
  cpu: { util_pct: number; load_1m: number; load_5m: number; cores: number } | null;
  mem: {
    total_bytes: number;
    used_bytes: number;
    available_bytes: number;
    swap_used_bytes: number;
    swap_total_bytes: number;
  } | null;
  disks: Array<{ mount: string; fs: string; total_bytes: number; used_bytes: number }>;
  temps: Array<{ chip: string; label: string; celsius: number }>;
  gpus: GpuStats[];
  power: { psu_watts: number | null; gpu_watts: number | null } | null;
  uptime_secs: number | null;
  fetched_at: string;
  warnings: string[];
}

type StatsMap = Record<string, { loading: boolean; stats?: ServerStats; error?: string }>;

async function invokeAction(
  apiKey: string,
  actionId: string,
  args: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  const res = await fetch(`${getApiBase()}/workbench/actions/${actionId}`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${apiKey}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ args, client_invocation_id: safeRandomUUID() }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`${actionId}: ${res.status} ${text.slice(0, 300)}`);
  }
  return await res.json();
}

function unwrapPayload(resp: Record<string, unknown>): unknown {
  const payload = (resp as { result?: { payload?: unknown } }).result?.payload;
  if (Array.isArray(payload)) {
    // Gadget path: content returns `[{type:"text", text:"<json>"}]`.
    const first = payload[0] as { text?: string } | undefined;
    if (first?.text) {
      try {
        return JSON.parse(first.text);
      } catch {
        return first.text;
      }
    }
  }
  return payload;
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MiB`;
  if (n < 1024 ** 4) return `${(n / 1024 ** 3).toFixed(1)} GiB`;
  return `${(n / 1024 ** 4).toFixed(1)} TiB`;
}

function fmtUptime(secs: number | null): string {
  if (!secs) return "—";
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return d > 0 ? `${d}d ${h}h` : h > 0 ? `${h}h ${m}m` : `${m}m`;
}

function ProgressBar({
  pct,
  label,
  tone = "blue",
}: {
  pct: number;
  label: string;
  tone?: "blue" | "amber" | "red";
}) {
  const clamped = Math.min(100, Math.max(0, pct));
  const color =
    clamped > 85
      ? "bg-red-500"
      : clamped > 65
        ? "bg-amber-500"
        : tone === "amber"
          ? "bg-amber-500"
          : tone === "red"
            ? "bg-red-500"
            : "bg-blue-500";
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between text-[11px]">
        <span className="text-zinc-400">{label}</span>
        <span className="font-mono text-zinc-300">{clamped.toFixed(0)}%</span>
      </div>
      <div className="h-1.5 w-full overflow-hidden rounded bg-zinc-800">
        <div className={`h-full ${color}`} style={{ width: `${clamped}%` }} />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Add-host form
// ---------------------------------------------------------------------------

type AuthMode = "key_path" | "key_paste" | "password_bootstrap";

type StepStatus = "pending" | "running" | "ok" | "failed" | "skipped";

interface ProgressStep {
  key: string;
  label: string;
  /** Expected wall-clock in ms. Client uses this to advance the
   * animated cursor; the final server response still overrides. */
  etaMs: number;
}

const STEPS_KEY_PATH: ProgressStep[] = [
  { key: "verify_key", label: "Verifying SSH key", etaMs: 2000 },
  { key: "save_inventory", label: "Saving to inventory", etaMs: 200 },
];

const STEPS_KEY_PASTE: ProgressStep[] = [
  { key: "write_key", label: "Writing private key (0600)", etaMs: 100 },
  { key: "verify_key", label: "Verifying SSH key", etaMs: 2000 },
  { key: "save_inventory", label: "Saving to inventory", etaMs: 200 },
];

const STEPS_PW_BOOTSTRAP: ProgressStep[] = [
  { key: "keygen", label: "Generating ed25519 keypair", etaMs: 500 },
  { key: "authorize", label: "Pushing public key to authorized_keys", etaMs: 2500 },
  { key: "sudoers", label: "Installing /etc/sudoers.d/gadgetron-monitor", etaMs: 3000 },
  { key: "apt_base", label: "apt-get install lm-sensors smartmontools ipmitool", etaMs: 25000 },
  { key: "detect_gpu", label: "Detecting NVIDIA GPU", etaMs: 1000 },
  { key: "dcgm", label: "Installing DCGM (if GPU present)", etaMs: 45000 },
  { key: "verify_key", label: "Verifying key-only login", etaMs: 3000 },
  { key: "save_inventory", label: "Saving to inventory", etaMs: 200 },
];

function stepsFor(mode: AuthMode): ProgressStep[] {
  if (mode === "key_path") return STEPS_KEY_PATH;
  if (mode === "key_paste") return STEPS_KEY_PASTE;
  return STEPS_PW_BOOTSTRAP;
}

function StepRow({
  step,
  status,
  elapsed,
}: {
  step: ProgressStep;
  status: StepStatus;
  elapsed: number | null;
}) {
  const glyph =
    status === "ok"
      ? "✓"
      : status === "failed"
        ? "✕"
        : status === "running"
          ? "◐"
          : status === "skipped"
            ? "·"
            : "○";
  const tone =
    status === "ok"
      ? "text-emerald-400"
      : status === "failed"
        ? "text-red-400"
        : status === "running"
          ? "text-amber-400 animate-pulse"
          : status === "skipped"
            ? "text-zinc-600"
            : "text-zinc-700";
  return (
    <li
      data-testid={`bootstrap-step-${step.key}`}
      data-status={status}
      className="flex items-center gap-2 py-0.5 text-[11px]"
    >
      <span className={`w-4 text-center font-mono ${tone}`}>{glyph}</span>
      <span className={status === "ok" || status === "failed" ? "text-zinc-300" : "text-zinc-500"}>
        {step.label}
      </span>
      {elapsed != null && (
        <span className="font-mono text-[10px] text-zinc-600">
          {(elapsed / 1000).toFixed(1)}s
        </span>
      )}
    </li>
  );
}

function AddHostForm({
  apiKey,
  onAdded,
}: {
  apiKey: string;
  onAdded: () => void;
}) {
  const [host, setHost] = useState("");
  const [user, setUser] = useState("");
  const [port, setPort] = useState(22);
  const [mode, setMode] = useState<AuthMode>("password_bootstrap");
  const [keyPath, setKeyPath] = useState("~/.ssh/id_ed25519");
  const [keyPaste, setKeyPaste] = useState("");
  const [sshPw, setSshPw] = useState("");
  const [sudoPw, setSudoPw] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [progress, setProgress] = useState<Array<{ status: StepStatus; elapsed: number | null }>>(
    [],
  );

  const steps = stepsFor(mode);

  const submit = async () => {
    if (!host.trim() || !user.trim()) {
      toast.error("host and ssh_user are required");
      return;
    }
    const args: Record<string, unknown> = {
      host: host.trim(),
      ssh_user: user.trim(),
      ssh_port: port,
      auth_mode: mode,
    };
    if (mode === "key_path") args.ssh_key_path = keyPath.trim();
    if (mode === "key_paste") args.ssh_private_key = keyPaste;
    if (mode === "password_bootstrap") {
      args.ssh_password = sshPw;
      args.sudo_password = sudoPw;
    }
    setSubmitting(true);

    // Client-side progress animation. Advances through the predefined
    // step list at the ETA cadence; real server response at the end
    // finalizes the checklist (remaining steps → ok, skipped DCGM →
    // skipped, failed step → failed with error detail below).
    const now = () => performance.now();
    const startedAt = now();
    const stepStartedAt: number[] = steps.map(() => 0);
    setProgress(steps.map(() => ({ status: "pending", elapsed: null })));
    let cursor = 0;
    const advance = () => {
      if (cursor >= steps.length) return;
      stepStartedAt[cursor] = now();
      setProgress((prev) => {
        const next = prev.slice();
        next[cursor] = { status: "running", elapsed: null };
        return next;
      });
    };
    advance();
    const timers: number[] = [];
    let cumulative = 0;
    for (let i = 0; i < steps.length - 1; i++) {
      cumulative += steps[i].etaMs;
      const idx = i;
      const t = window.setTimeout(() => {
        setProgress((prev) => {
          const next = prev.slice();
          next[idx] = { status: "ok", elapsed: now() - stepStartedAt[idx] };
          return next;
        });
        cursor = idx + 1;
        advance();
      }, cumulative);
      timers.push(t);
    }

    try {
      const resp = await invokeAction(apiKey, "server-add", args);
      const payload = unwrapPayload(resp) as {
        id: string;
        bootstrap: {
          installed_pkgs?: string[];
          skipped_pkgs?: string[];
          gpu_detected?: boolean;
          dcgm_enabled?: boolean;
          notes?: string[];
        };
      };
      // Cancel remaining timers and mark everything done.
      timers.forEach((t) => clearTimeout(t));
      setProgress((prev) => {
        const totalElapsed = now() - startedAt;
        return steps.map((s, i) => {
          if (s.key === "dcgm" && !payload.bootstrap.gpu_detected) {
            return { status: "skipped", elapsed: null };
          }
          const prevEntry = prev[i];
          const elapsed = prevEntry?.elapsed ?? totalElapsed / steps.length;
          return { status: "ok", elapsed };
        });
      });
      toast.success(`Registered ${host} → ${payload.id.slice(0, 8)}`, {
        description:
          (payload.bootstrap.installed_pkgs?.join(", ") || "no pkg install") +
          (payload.bootstrap.gpu_detected
            ? ` · GPU(${payload.bootstrap.dcgm_enabled ? "DCGM" : "nvidia-smi"})`
            : ""),
      });
      setHost("");
      setUser("");
      setSshPw("");
      setSudoPw("");
      setKeyPaste("");
      onAdded();
    } catch (e) {
      timers.forEach((t) => clearTimeout(t));
      // Mark the currently-running step failed; leave earlier ok, later
      // pending so the operator can see how far we got.
      setProgress((prev) => {
        const next = prev.slice();
        const current = next.findIndex((p) => p.status === "running");
        if (current >= 0) {
          next[current] = { status: "failed", elapsed: now() - stepStartedAt[current] };
        }
        return next;
      });
      toast.error("server.add failed", { description: (e as Error).message });
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <section
      data-testid="server-add-form"
      className="flex flex-col gap-3 border-b border-zinc-800 bg-zinc-950 p-4"
    >
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-semibold text-zinc-200">Register a server</h2>
        <div className="flex gap-1" role="tablist" aria-label="auth mode">
          {(["password_bootstrap", "key_paste", "key_path"] as AuthMode[]).map((m) => (
            <button
              key={m}
              type="button"
              role="tab"
              aria-selected={mode === m}
              onClick={() => setMode(m)}
              data-testid={`auth-tab-${m}`}
              className={`rounded border px-2 py-0.5 text-[11px] ${
                mode === m
                  ? "border-blue-500 bg-blue-950/40 text-blue-200"
                  : "border-zinc-700 bg-zinc-900 text-zinc-500 hover:text-zinc-300"
              }`}
            >
              {m === "password_bootstrap" ? "Password (auto-setup)" : m === "key_paste" ? "Paste key" : "Key path"}
            </button>
          ))}
        </div>
      </div>
      <div className="grid grid-cols-1 gap-2 md:grid-cols-3">
        <Input
          placeholder="host (10.0.0.5 or hostname)"
          value={host}
          onChange={(e) => setHost(e.target.value)}
          className="font-mono text-xs"
        />
        <Input
          placeholder="ssh_user (ubuntu)"
          value={user}
          onChange={(e) => setUser(e.target.value)}
          className="font-mono text-xs"
        />
        <Input
          type="number"
          min={1}
          max={65535}
          value={port}
          onChange={(e) => setPort(parseInt(e.target.value || "22", 10))}
          className="font-mono text-xs"
        />
      </div>
      {mode === "key_path" && (
        <Input
          placeholder="/home/user/.ssh/id_ed25519"
          value={keyPath}
          onChange={(e) => setKeyPath(e.target.value)}
          className="font-mono text-xs"
        />
      )}
      {mode === "key_paste" && (
        <Textarea
          placeholder="-----BEGIN OPENSSH PRIVATE KEY-----&#10;..."
          value={keyPaste}
          onChange={(e) => setKeyPaste(e.target.value)}
          className="h-28 font-mono text-[11px]"
        />
      )}
      {mode === "password_bootstrap" && (
        <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
          <Input
            type="password"
            placeholder="ssh password"
            value={sshPw}
            onChange={(e) => setSshPw(e.target.value)}
            className="font-mono text-xs"
          />
          <Input
            type="password"
            placeholder="sudo password (often same as ssh)"
            value={sudoPw}
            onChange={(e) => setSudoPw(e.target.value)}
            className="font-mono text-xs"
          />
        </div>
      )}
      <div className="flex items-center justify-between gap-2">
        <p className="text-[10px] text-zinc-600">
          {mode === "password_bootstrap"
            ? "Passwords are used once to push an ed25519 key + NOPASSWD sudoers line, then zeroized. Never stored."
            : mode === "key_paste"
              ? "Pasted key is written 0600 under ~/.gadgetron/server-monitor/keys/. Not synced anywhere."
              : "Key file path must be readable by the gadgetron process."}
        </p>
        <Button onClick={submit} disabled={submitting}>
          {submitting ? "Registering…" : "Register"}
        </Button>
      </div>
      {(submitting || progress.some((p) => p.status !== "pending")) && (
        <div
          data-testid="bootstrap-progress"
          className="rounded border border-zinc-800 bg-zinc-900/50 p-3"
        >
          <div className="mb-2 text-[10px] uppercase tracking-wide text-zinc-500">
            Bootstrap progress ({mode})
          </div>
          <ol className="flex flex-col">
            {steps.map((s, i) => {
              const entry = progress[i] ?? { status: "pending" as StepStatus, elapsed: null };
              return (
                <StepRow key={s.key} step={s} status={entry.status} elapsed={entry.elapsed} />
              );
            })}
          </ol>
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Host card (grid cell)
// ---------------------------------------------------------------------------

function HostCard({
  host,
  data,
  onRemove,
}: {
  host: Host;
  data: StatsMap[string] | undefined;
  onRemove: () => void;
}) {
  const stats = data?.stats;
  const err = data?.error;
  return (
    <div
      data-testid={`host-card-${host.host}`}
      className="flex flex-col gap-2 rounded border border-zinc-800 bg-zinc-900 p-3 text-xs"
    >
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="truncate font-mono text-sm text-zinc-200" title={host.host}>
            {host.host}
          </div>
          <div className="truncate text-[10px] text-zinc-600">
            {host.ssh_user}@{host.host}:{host.ssh_port}
          </div>
        </div>
        <button
          type="button"
          data-testid={`host-remove-${host.host}`}
          onClick={onRemove}
          className="shrink-0 rounded border border-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-500 hover:text-red-400"
          title="Remove host"
        >
          remove
        </button>
      </div>
      {err && (
        <div className="rounded border border-red-900/60 bg-red-950/40 px-2 py-1 text-[10px] text-red-300">
          {err}
        </div>
      )}
      {stats?.cpu && (
        <ProgressBar pct={stats.cpu.util_pct} label={`CPU (${stats.cpu.cores} cores)`} />
      )}
      {stats?.mem && (
        <ProgressBar
          pct={(stats.mem.used_bytes / stats.mem.total_bytes) * 100}
          label={`RAM ${fmtBytes(stats.mem.used_bytes)} / ${fmtBytes(stats.mem.total_bytes)}`}
        />
      )}
      {stats?.gpus && stats.gpus.length > 0 && (
        <div className="flex flex-col gap-1">
          {stats.gpus.map((g) => (
            <div key={g.index} className="flex flex-col gap-0.5">
              <div className="flex items-center justify-between text-[10px] text-zinc-400">
                <span className="truncate font-mono" title={g.name}>
                  GPU {g.index} · {g.source}
                </span>
                <span className="font-mono text-zinc-300">
                  {g.temp_c != null ? `${g.temp_c}°C` : ""}
                  {g.power_w != null ? ` · ${g.power_w.toFixed(0)}W` : ""}
                </span>
              </div>
              {g.util_pct != null && (
                <ProgressBar pct={g.util_pct} label={g.name} tone="amber" />
              )}
            </div>
          ))}
        </div>
      )}
      {stats?.power?.psu_watts != null && (
        <div className="flex items-center justify-between text-[10px] text-zinc-500">
          <span>PSU</span>
          <span className="font-mono text-zinc-300">{stats.power.psu_watts.toFixed(0)}W</span>
        </div>
      )}
      {stats?.temps && stats.temps.length > 0 && (
        <div className="flex items-center justify-between text-[10px] text-zinc-500">
          <span>max temp</span>
          <span className="font-mono text-zinc-300">
            {Math.max(...stats.temps.map((t) => t.celsius)).toFixed(0)}°C
          </span>
        </div>
      )}
      {stats?.uptime_secs != null && (
        <div className="flex items-center justify-between text-[10px] text-zinc-500">
          <span>uptime</span>
          <span className="font-mono text-zinc-300">{fmtUptime(stats.uptime_secs)}</span>
        </div>
      )}
      {stats?.warnings && stats.warnings.length > 0 && (
        <details className="text-[10px] text-zinc-500">
          <summary className="cursor-pointer">warnings ({stats.warnings.length})</summary>
          <ul className="mt-1 space-y-0.5 pl-3">
            {stats.warnings.map((w, i) => (
              <li key={i} className="list-disc">
                {w}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function ServersPage() {
  const { apiKey } = useAuth();
  const [hosts, setHosts] = useState<Host[]>([]);
  const [statsMap, setStatsMap] = useState<StatsMap>({});
  const [listError, setListError] = useState<string | null>(null);

  const refreshList = useCallback(async () => {
    if (!apiKey) return;
    try {
      setListError(null);
      const resp = await invokeAction(apiKey, "server-list", {});
      const payload = unwrapPayload(resp) as { hosts?: Host[] } | undefined;
      setHosts(payload?.hosts ?? []);
    } catch (e) {
      setListError((e as Error).message);
    }
  }, [apiKey]);

  const refreshStats = useCallback(
    async (id: string) => {
      if (!apiKey) return;
      setStatsMap((m) => {
        const prev = m[id];
        return {
          ...m,
          [id]: {
            loading: true,
            stats: prev?.stats,
            error: prev?.error,
          },
        };
      });
      try {
        const resp = await invokeAction(apiKey, "server-stats", { id });
        const parsed = unwrapPayload(resp) as ServerStats;
        setStatsMap((m) => ({ ...m, [id]: { loading: false, stats: parsed } }));
      } catch (e) {
        setStatsMap((m) => ({
          ...m,
          [id]: { loading: false, error: (e as Error).message, stats: m[id]?.stats },
        }));
      }
    },
    [apiKey],
  );

  const remove = useCallback(
    async (id: string, host: string) => {
      if (!apiKey) return;
      if (!window.confirm(`Remove ${host}?`)) return;
      try {
        await invokeAction(apiKey, "server-remove", { id });
        toast.success(`Removed ${host}`);
        await refreshList();
      } catch (e) {
        toast.error("server.remove failed", { description: (e as Error).message });
      }
    },
    [apiKey, refreshList],
  );

  useEffect(() => {
    void refreshList();
  }, [refreshList]);

  // 5-second polling loop per host.
  useEffect(() => {
    if (!apiKey || hosts.length === 0) return;
    hosts.forEach((h) => void refreshStats(h.id));
    const t = setInterval(() => {
      hosts.forEach((h) => void refreshStats(h.id));
    }, 5000);
    return () => clearInterval(t);
  }, [apiKey, hosts, refreshStats]);

  const hostList = useMemo(() => hosts, [hosts]);

  return (
    <>
      <Toaster theme="dark" richColors position="bottom-right" />
      <header
        className="flex h-10 shrink-0 items-center justify-between border-b border-zinc-800 bg-zinc-950 px-4"
        data-testid="servers-header"
      >
        <div className="flex items-center gap-3">
          <span className="text-xs font-semibold text-zinc-300">Servers</span>
          <span className="text-[11px] text-zinc-600" data-testid="servers-count">
            · {hostList.length} host{hostList.length === 1 ? "" : "s"}
          </span>
        </div>
        <Button variant="ghost" size="sm" onClick={refreshList} className="h-6 px-2 text-[11px]">
          Refresh
        </Button>
      </header>

      <div className="flex flex-1 flex-col overflow-auto">
        <AddHostForm apiKey={apiKey ?? ""} onAdded={refreshList} />

        {listError && (
          <div className="m-4 rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
            {listError}
          </div>
        )}

        <section className="flex-1 p-4">
          {hostList.length === 0 ? (
            <div
              className="flex min-h-[200px] items-center justify-center text-center text-[11px] text-zinc-600"
              data-testid="servers-empty"
            >
              <div>
                <p>No hosts registered yet.</p>
                <p className="mt-1">Use the form above to add one.</p>
              </div>
            </div>
          ) : (
            <div
              className="grid grid-cols-1 gap-3 md:grid-cols-2 lg:grid-cols-3"
              data-testid="host-grid"
            >
              {hostList.map((h) => (
                <HostCard
                  key={h.id}
                  host={h}
                  data={statsMap[h.id]}
                  onRemove={() => void remove(h.id, h.host)}
                />
              ))}
            </div>
          )}
        </section>
      </div>
    </>
  );
}
