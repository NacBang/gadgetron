"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Toaster, toast } from "sonner";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { Textarea } from "../../components/ui/textarea";
import { Sparkline, type SparkPoint } from "../../components/sparkline";
import { HostDetailDrawer } from "../../components/host-detail-drawer";
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
  alias?: string | null;
  machine_id?: string | null;
  cpu_model?: string | null;
  cpu_cores?: number | null;
  gpus?: string[] | null;
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
  // DCGM-only. Surfaced as badges on the card.
  mem_temp_c?: number | null;
  ecc_dbe_total?: number | null;
  xid_last?: number | null;
  throttle_reasons?: number | null;
  throttle_reason_label?: string | null;
}

interface NetworkStats {
  iface: string;
  rx_bps: number;
  tx_bps: number;
  rx_bytes_total: number;
  tx_bytes_total: number;
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
  network: NetworkStats[];
  uptime_secs: number | null;
  fetched_at: string;
  warnings: string[];
}

type StatsMap = Record<
  string,
  {
    loading: boolean;
    stats?: ServerStats;
    error?: string;
    /** Wall-clock ms of the most recent server.stats HTTP round-trip. */
    lastFetchMs?: number;
    /** `performance.now()` timestamp when the most recent response
     * completed — used to compute "updated Xs ago". */
    lastFetchedAt?: number;
  }
>;

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

/// Drops marketing fluff from lscpu model names so the card stays
/// readable: "AMD EPYC 7763 64-Core Processor" → "AMD EPYC 7763",
/// "Intel(R) Xeon(R) Gold 6248R CPU @ 3.00GHz" → "Intel Xeon Gold 6248R".
/// The full name still lives in the tooltip.
function shortenCpu(model: string): string {
  return model
    .replace(/\([Rr]\)/g, "")
    .replace(/\([Tt][Mm]\)/g, "")
    .replace(/\s+CPU\s+@.*$/i, "")
    .replace(/\s+Processor$/i, "")
    .replace(/\s+\d+-Core$/i, "")
    .replace(/\s{2,}/g, " ")
    .trim();
}

/// Collapses a list like ["NVIDIA RTX 4090", "NVIDIA RTX 4090", "NVIDIA RTX 4090"]
/// into "3× NVIDIA RTX 4090"; mixed models render as "2× X + 1× Y".
/// Also trims the redundant "NVIDIA " prefix when every entry shares it
/// so the card reads "4× RTX 4090" in the common homogeneous case.
function shortenGpuList(gpus: string[]): string {
  if (gpus.length === 0) return "";
  const counts = new Map<string, number>();
  for (const g of gpus) counts.set(g, (counts.get(g) ?? 0) + 1);
  const allNvidia = [...counts.keys()].every((k) => k.startsWith("NVIDIA "));
  const parts = [...counts.entries()].map(([name, n]) => {
    const stripped = allNvidia ? name.replace(/^NVIDIA /, "") : name;
    return n === 1 ? stripped : `${n}× ${stripped}`;
  });
  return parts.join(" + ");
}

/// Compact health badges surfaced next to the GPU label on the host
/// card. Each badge maps to a DCGM signal that means "this GPU needs
/// operator attention". Uses color intensity as the severity cue:
///   - ECC DBE ≥ 1  → red (uncorrectable memory error — replace)
///   - XID ≠ 0      → amber (driver/HW event)
///   - Throttled    → orange (running slower than requested)
function GpuHealthBadges({ gpu }: { gpu: GpuStats }) {
  const badges: Array<{ text: string; tone: string; title: string }> = [];
  if (gpu.ecc_dbe_total != null && gpu.ecc_dbe_total > 0) {
    badges.push({
      text: `ECC×${gpu.ecc_dbe_total}`,
      tone: "bg-red-950/60 text-red-300 border-red-800",
      title: `Uncorrectable ECC double-bit errors: ${gpu.ecc_dbe_total}. Consider RMA.`,
    });
  }
  if (gpu.xid_last != null && gpu.xid_last > 0) {
    badges.push({
      text: `XID ${gpu.xid_last}`,
      tone: "bg-amber-950/60 text-amber-300 border-amber-800",
      title: `Most recent NVIDIA XID error: ${gpu.xid_last}`,
    });
  }
  if (gpu.throttle_reason_label) {
    // Shorthand picks the most severe reason for the badge text —
    // HW thermal > HW power brake > SW thermal > SW power cap.
    // Full decoded list goes in the tooltip.
    const label = gpu.throttle_reason_label.toLowerCase();
    let short = "THRTL";
    if (label.includes("hw thermal")) short = "HW-THERM";
    else if (label.includes("hw power brake")) short = "HW-PWR";
    else if (label.includes("hw slowdown")) short = "HW-SLOW";
    else if (label.includes("sw thermal")) short = "SW-THERM";
    else if (label.includes("sw power")) short = "SW-PWR";
    const tempHint =
      gpu.temp_c != null && gpu.temp_c > 80 ? ` · temp ${gpu.temp_c}°C` : "";
    const powerHint =
      gpu.power_w != null &&
      gpu.power_limit_w != null &&
      gpu.power_w > gpu.power_limit_w * 0.95
        ? ` · ${gpu.power_w.toFixed(0)}/${gpu.power_limit_w.toFixed(0)}W`
        : "";
    badges.push({
      text: short,
      tone: "bg-orange-950/60 text-orange-300 border-orange-800",
      title: `Throttled: ${gpu.throttle_reason_label}${tempHint}${powerHint}\n\n` +
        `HW thermal = die/HBM too hot (check airflow, fan curve)\n` +
        `HW power brake = PSU/VRM current limit tripped\n` +
        `SW thermal = driver backoff before HW threshold\n` +
        `SW power cap = persistent power limit (nvidia-smi -pl) or policy`,
    });
  }
  if (badges.length === 0) return null;
  return (
    <span className="flex items-center gap-1">
      {badges.map((b) => (
        <span
          key={b.text}
          title={b.title}
          className={`rounded border px-1 py-[1px] text-[9px] font-bold uppercase tracking-wider ${b.tone}`}
        >
          {b.text}
        </span>
      ))}
    </span>
  );
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
  collapsed,
  onCollapsedChange,
}: {
  apiKey: string | null;
  onAdded: () => void;
  collapsed: boolean;
  onCollapsedChange: (next: boolean) => void;
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
      // Auto-minimize 2.5s after success so the operator can see the
      // final ✓ row before the form collapses out of the way. Cancelled
      // if the operator re-expanded in the meantime.
      window.setTimeout(() => onCollapsedChange(true), 2500);
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

  if (collapsed) {
    return (
      <section
        data-testid="server-add-form-collapsed"
        className="flex items-center justify-between border-b border-zinc-800 bg-zinc-950 px-4 py-2"
      >
        <span className="text-[11px] text-zinc-500">
          Need another server? Use the button to re-open the register form.
        </span>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => onCollapsedChange(false)}
          data-testid="server-add-reopen"
          className="h-6 gap-1 px-2 text-[11px]"
        >
          + Add server
        </Button>
      </section>
    );
  }
  return (
    <section
      data-testid="server-add-form"
      className="flex flex-col gap-3 border-b border-zinc-800 bg-zinc-950 p-4"
    >
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold text-zinc-200">Register a server</h2>
          <button
            type="button"
            onClick={() => onCollapsedChange(true)}
            data-testid="server-add-minimize"
            title="Collapse — reopen later via '+ Add server'"
            aria-label="Collapse the register form"
            className="rounded border border-zinc-800 px-1.5 py-px text-[10px] text-zinc-500 hover:border-zinc-700 hover:text-zinc-300"
          >
            minimize
          </button>
          <button
            type="button"
            onClick={() => onCollapsedChange(true)}
            data-testid="server-add-close"
            title="Close the form"
            aria-label="Close the register form"
            className="rounded border border-zinc-800 px-1.5 py-px font-mono text-[10px] text-zinc-500 hover:border-zinc-700 hover:text-zinc-300"
          >
            ×
          </button>
        </div>
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

// Floating modal — attaches to one host, runs a bash command via the
// server.bash gadget. Mandatory confirm dialog before dispatch (policy
// #2: every invocation requires an explicit operator click). Output is
// shown inline once the call completes.
function ShellRunner({
  apiKey,
  host,
  onClose,
}: {
  apiKey: string | null;
  host: Host;
  onClose: () => void;
}) {
  const [cmd, setCmd] = useState("");
  const [useSudo, setUseSudo] = useState(false);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<
    | {
        code: number | null;
        stdout: string;
        stderr: string;
      }
    | null
  >(null);
  const label = host.alias ?? host.host;

  const run = useCallback(async () => {
    const trimmed = cmd.trim();
    if (!trimmed || running) return;
    const preview = trimmed.length > 120 ? trimmed.slice(0, 117) + "…" : trimmed;
    const ok = window.confirm(
      `${label}에서 실행할까요?\n\n${useSudo ? "[sudo] " : ""}${preview}`,
    );
    if (!ok) return;
    setRunning(true);
    setResult(null);
    try {
      const resp = await invokeAction(apiKey, "server-bash", {
        id: host.id,
        command: trimmed,
        use_sudo: useSudo,
      });
      const payload = unwrapPayload(resp) as
        | { code?: number | null; stdout?: string; stderr?: string }
        | undefined;
      setResult({
        code: payload?.code ?? null,
        stdout: payload?.stdout ?? "",
        stderr: payload?.stderr ?? "",
      });
    } catch (e) {
      setResult({ code: null, stdout: "", stderr: (e as Error).message });
    } finally {
      setRunning(false);
    }
  }, [apiKey, cmd, host.id, label, running, useSudo]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        className="flex max-h-[90vh] w-full max-w-2xl flex-col gap-3 overflow-hidden rounded border border-zinc-700 bg-zinc-950 p-4"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <div className="text-sm font-semibold text-zinc-100">
            🔧 shell @ <span className="font-mono text-blue-300">{label}</span>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="text-zinc-500 hover:text-zinc-200"
          >
            ✕
          </button>
        </div>
        <div className="text-[11px] text-zinc-500">
          모든 실행은 확인 다이얼로그를 거칩니다. sudo는 NOPASSWD 설치된 상태에서만 동작.
        </div>
        <Textarea
          value={cmd}
          onChange={(e) => setCmd(e.target.value)}
          onKeyDown={(e) => {
            if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
              e.preventDefault();
              void run();
            }
          }}
          placeholder="예) systemctl restart nvidia-dcgm && dmesg | tail -20"
          rows={4}
          className="font-mono text-xs"
          disabled={running}
        />
        <div className="flex items-center justify-between gap-2">
          <label className="flex items-center gap-1.5 text-[11px] text-zinc-400">
            <input
              type="checkbox"
              checked={useSudo}
              onChange={(e) => setUseSudo(e.target.checked)}
              className="accent-blue-500"
            />
            sudo 로 실행
          </label>
          <Button
            type="button"
            size="sm"
            onClick={() => void run()}
            disabled={running || cmd.trim().length === 0}
          >
            {running ? "실행 중…" : "실행 (Ctrl+Enter)"}
          </Button>
        </div>
        {result && (
          <div className="flex-1 overflow-hidden">
            <div className="mb-1 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">
              exit {result.code ?? "—"}
            </div>
            <pre className="max-h-64 overflow-auto rounded bg-zinc-900/70 p-2 font-mono text-[11px] text-zinc-200 whitespace-pre-wrap break-all">
              {result.stdout || result.stderr || "(no output)"}
            </pre>
            {result.stderr && result.stdout && (
              <pre className="mt-1 max-h-32 overflow-auto rounded bg-red-950/40 p-2 font-mono text-[11px] text-red-300 whitespace-pre-wrap break-all">
                {result.stderr}
              </pre>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function HostCard({
  host,
  data,
  onRemove,
  onOpenDetail,
  onAliasChange,
  findingsCount,
  nowMs,
  apiKey,
}: {
  host: Host;
  data: StatsMap[string] | undefined;
  onRemove: () => void;
  onOpenDetail: () => void;
  /** Called after a successful `server.update` so the parent can update
   * its `hosts` array without waiting for the next list-refresh tick. */
  onAliasChange: (newAlias: string | null) => void;
  /** Open log-analyzer findings for this host, by severity. */
  findingsCount?: { critical: number; high: number; medium: number; info: number };
  /** Tick value from the parent's `setInterval` so the "updated Xs ago"
   * label re-renders every second without coupling the card to its
   * own timer. */
  nowMs: number;
  apiKey: string | null;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(host.alias ?? "");
  const [saving, setSaving] = useState(false);
  const [shellOpen, setShellOpen] = useState(false);
  useEffect(() => {
    setDraft(host.alias ?? "");
  }, [host.alias]);
  const saveAlias = useCallback(async () => {
    const trimmed = draft.trim();
    const next = trimmed.length > 0 ? trimmed : null;
    if (next === (host.alias ?? null)) {
      setEditing(false);
      return;
    }
    setSaving(true);
    try {
      await invokeAction(apiKey, "server-update", {
        id: host.id,
        alias: next,
      });
      onAliasChange(next);
      setEditing(false);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setSaving(false);
    }
  }, [apiKey, draft, host.alias, host.id, onAliasChange]);
  const stats = data?.stats;
  const err = data?.error;
  const ageS =
    data?.lastFetchedAt != null
      ? Math.max(0, (nowMs - data.lastFetchedAt) / 1000)
      : null;
  const fetchMs = data?.lastFetchMs ?? null;
  const loading = data?.loading ?? false;

  // Build the metric list dynamically — only ask the API for series we
  // can actually render. CPU + Mem are always present; NIC is per-iface.
  const sparkMetrics = useMemo(() => {
    const m = ["cpu.util", "mem.used_bytes"];
    const firstGpu = stats?.gpus?.[0];
    if (firstGpu) {
      m.push(`gpu.${firstGpu.index}.util`);
    }
    const firstNic = stats?.network?.[0];
    if (firstNic) {
      m.push(`nic.${firstNic.iface}.rx_bps`);
    }
    return m;
  }, [stats?.gpus, stats?.network]);

  const history = useHostMetricHistory(apiKey, host.id, sparkMetrics);

  // Helpers for current-value annotations (used in sparkline labels).
  const memPct = stats?.mem
    ? `${((stats.mem.used_bytes / stats.mem.total_bytes) * 100).toFixed(0)}%`
    : "—";
  const gpu0Pct =
    stats?.gpus?.[0]?.util_pct != null
      ? `${stats.gpus[0].util_pct.toFixed(0)}%`
      : "—";
  const nic0 = stats?.network?.[0];
  const nicCurrent = nic0 ? fmtBps(nic0.rx_bps) : "—";
  return (
    <div
      data-testid={`host-card-${host.host}`}
      className="group/card flex flex-col gap-2 rounded border border-zinc-800 bg-zinc-900 p-3 text-xs"
    >
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0 flex-1">
          {editing ? (
            <div className="flex items-center gap-1">
              <Input
                autoFocus
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void saveAlias();
                  else if (e.key === "Escape") {
                    setEditing(false);
                    setDraft(host.alias ?? "");
                  }
                }}
                placeholder="alias (empty = clear)"
                maxLength={64}
                className="h-6 px-1 py-0 text-sm"
                disabled={saving}
              />
              <button
                type="button"
                onClick={() => void saveAlias()}
                disabled={saving}
                className="rounded border border-emerald-900/60 px-1.5 py-0.5 text-[10px] text-emerald-300 hover:bg-emerald-950/40 disabled:opacity-50"
                title="Save (Enter)"
              >
                ✓
              </button>
              <button
                type="button"
                onClick={() => {
                  setEditing(false);
                  setDraft(host.alias ?? "");
                }}
                disabled={saving}
                className="rounded border border-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-500 hover:text-zinc-300"
                title="Cancel (Esc)"
              >
                ✕
              </button>
            </div>
          ) : (
            <div className="flex items-center gap-1">
              <div
                className="truncate text-sm font-semibold text-zinc-100"
                title={host.alias ?? host.host}
              >
                {host.alias ?? host.host}
              </div>
              <button
                type="button"
                onClick={() => setEditing(true)}
                className="shrink-0 rounded px-1 text-[10px] text-zinc-600 opacity-0 transition group-hover/card:opacity-100 hover:bg-zinc-800 hover:text-zinc-300"
                title="Rename alias"
                data-testid={`host-alias-edit-${host.id}`}
              >
                ✎
              </button>
            </div>
          )}
          {host.alias && !editing && (
            <div className="truncate font-mono text-[11px] text-zinc-500">
              {host.host}
            </div>
          )}
          <div className="truncate text-[10px] text-zinc-600">
            {host.ssh_user}@{host.host}:{host.ssh_port}
          </div>
          {(host.cpu_model || (host.gpus && host.gpus.length > 0)) && (
            <div
              className="mt-0.5 truncate text-[10px] text-zinc-500"
              title={[
                host.cpu_model
                  ? `CPU: ${host.cpu_model}${host.cpu_cores ? ` (${host.cpu_cores}c)` : ""}`
                  : null,
                host.gpus && host.gpus.length > 0
                  ? `GPU: ${host.gpus.join(" / ")}`
                  : null,
              ]
                .filter(Boolean)
                .join("\n")}
            >
              {host.cpu_model && (
                <span>
                  {shortenCpu(host.cpu_model)}
                  {host.cpu_cores ? ` · ${host.cpu_cores}c` : ""}
                </span>
              )}
              {host.cpu_model && host.gpus && host.gpus.length > 0 && (
                <span className="mx-1 text-zinc-700">·</span>
              )}
              {host.gpus && host.gpus.length > 0 && (
                <span>{shortenGpuList(host.gpus)}</span>
              )}
            </div>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {findingsCount && (() => {
            const total =
              findingsCount.critical +
              findingsCount.high +
              findingsCount.medium +
              findingsCount.info;
            if (total === 0) return null;
            const tone =
              findingsCount.critical > 0
                ? "border-red-800 bg-red-950/40 text-red-200"
                : findingsCount.high > 0
                  ? "border-amber-800 bg-amber-950/40 text-amber-200"
                  : findingsCount.medium > 0
                    ? "border-yellow-800 bg-yellow-950/30 text-yellow-200"
                    : "border-zinc-700 bg-zinc-800 text-zinc-300";
            return (
              <a
                href={`/web/findings?host=${host.id}`}
                title={`critical ${findingsCount.critical} · high ${findingsCount.high} · medium ${findingsCount.medium} · info ${findingsCount.info}`}
                className={`rounded border px-1.5 py-0.5 text-[10px] font-bold ${tone}`}
                data-testid={`host-findings-${host.id}`}
              >
                ⚠ {total}
              </a>
            );
          })()}
          <button
            type="button"
            data-testid={`host-detail-${host.host}`}
            onClick={onOpenDetail}
            className="rounded border border-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-500 hover:text-blue-400"
            title="Open detail charts"
          >
            detail
          </button>
          <button
            type="button"
            data-testid={`host-shell-${host.host}`}
            onClick={() => setShellOpen(true)}
            className="rounded border border-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-500 hover:border-blue-600 hover:text-blue-300"
            title="원격 bash 실행 (매 호출 승인 필요)"
          >
            🔧 shell
          </button>
          <button
            type="button"
            data-testid={`host-remove-${host.host}`}
            onClick={onRemove}
            className="rounded border border-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-500 hover:text-red-400"
            title="Remove host"
          >
            remove
          </button>
        </div>
      </div>
      {shellOpen && (
        <ShellRunner
          apiKey={apiKey}
          host={host}
          onClose={() => setShellOpen(false)}
        />
      )}
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
          {stats.gpus.map((g) => {
            const metaBits: string[] = [];
            if (g.mem_used_mib != null && g.mem_total_mib != null) {
              metaBits.push(
                `${(g.mem_used_mib / 1024).toFixed(1)}/${(g.mem_total_mib / 1024).toFixed(0)} GiB`,
              );
            }
            if (g.temp_c != null) metaBits.push(`${g.temp_c}°C`);
            if (g.mem_temp_c != null) {
              metaBits.push(`mem ${Math.round(g.mem_temp_c)}°C`);
            }
            if (g.power_w != null) metaBits.push(`${g.power_w.toFixed(0)}W`);
            return (
              <div key={g.index} className="flex flex-col gap-0.5">
                <div className="flex items-center justify-between text-[10px] text-zinc-400">
                  <span
                    className="flex items-center gap-1.5 truncate font-mono"
                    title={`${g.name} (source: ${g.source})`}
                  >
                    <span className="truncate">
                      GPU {g.index}
                      {(() => {
                        // Skip the " — name" suffix when the collector
                        // only returned a placeholder like "GPU 3"
                        // (DCGM hostengine down / nvidia-smi failed).
                        const n = (g.name ?? "").trim();
                        if (!n) return "";
                        if (/^GPU\s*\d+$/i.test(n)) return "";
                        return ` — ${n.replace(/^NVIDIA /, "")}`;
                      })()}
                    </span>
                    <GpuHealthBadges gpu={g} />
                  </span>
                  <span className="truncate font-mono text-zinc-300">
                    {metaBits.join(" · ")}
                  </span>
                </div>
                {g.util_pct != null && (
                  <ProgressBar pct={g.util_pct} label="" tone="amber" />
                )}
              </div>
            );
          })}
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
      {sparkMetrics.length > 0 && (
        <div
          className="mt-1 grid grid-cols-2 gap-x-3 gap-y-1 border-t border-zinc-800 pt-2"
          data-testid="host-sparklines"
        >
          <Sparkline
            label="cpu (5m)"
            current={stats?.cpu ? `${stats.cpu.util_pct.toFixed(1)}%` : "—"}
            points={history["cpu.util"] ?? []}
            tone="blue"
            yMin={0}
            yMax={100}
          />
          <Sparkline
            label="mem (5m)"
            current={memPct}
            points={
              history["mem.used_bytes"]
                ?.map((p) => ({
                  ...p,
                  // Project bytes → percent for visual normalization
                  // when total is known; otherwise keep raw.
                  avg: stats?.mem
                    ? (p.avg / stats.mem.total_bytes) * 100
                    : p.avg,
                })) ?? []
            }
            tone="emerald"
            yMin={0}
            yMax={stats?.mem ? 100 : undefined}
          />
          {stats?.gpus?.[0] && (
            <Sparkline
              label={`gpu${stats.gpus[0].index} util (5m)`}
              current={gpu0Pct}
              points={history[`gpu.${stats.gpus[0].index}.util`] ?? []}
              tone="amber"
              yMin={0}
              yMax={100}
            />
          )}
          {nic0 && (
            <Sparkline
              label={`nic ${nic0.iface} rx (5m)`}
              current={nicCurrent}
              points={history[`nic.${nic0.iface}.rx_bps`] ?? []}
              tone="zinc"
              yMin={0}
            />
          )}
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
      <div
        data-testid="host-card-footer"
        className="mt-1 flex items-center justify-between gap-2 border-t border-zinc-800 pt-1 text-[10px] text-zinc-600"
      >
        <span className="flex items-center gap-1">
          {/* Static status dot. Color reflects last poll outcome; no
            * pulse/animate so 1 Hz polling doesn't feel like strobe
            * lighting. Err wins over stale. */}
          <span
            aria-hidden
            className={`inline-block size-1.5 rounded-full ${
              err
                ? "bg-red-500"
                : ageS != null && ageS > 10
                  ? "bg-amber-500"
                  : "bg-emerald-500"
            }`}
          />
          <span className="font-mono">
            {ageS == null
              ? "no data yet"
              : ageS < 3
                ? "live"
                : `updated ${ageS.toFixed(0)}s ago`}
          </span>
        </span>
        {fetchMs != null && (
          <span className="font-mono" title="Last round-trip latency (gadgetron ↔ target via SSH)">
            fetch {fetchMs < 1000 ? `${fetchMs.toFixed(0)}ms` : `${(fetchMs / 1000).toFixed(2)}s`}
          </span>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

const POLL_INTERVAL_MS = 1000;
/** History window the per-card sparklines display. 5 min × 1 Hz =
 *  ~300 samples per series — well within the auto-tier `raw` cutoff. */
const HISTORY_WINDOW_MS = 5 * 60 * 1000;
/** Refresh cadence for the history fetches. We don't need to re-pull
 *  every poll — the sparkline updates fine at 5 s. */
const HISTORY_REFRESH_MS = 5_000;

interface MetricsApiResponse {
  metric: string;
  unit: string | null;
  resolution: string;
  points: Array<{
    ts: string;
    avg: number;
    min: number;
    max: number;
    samples: number;
  }>;
  refresh_lag_seconds: number;
  dropped_frames: number;
}

async function fetchMetricHistory(
  apiKey: string | null,
  hostId: string,
  metric: string,
): Promise<SparkPoint[]> {
  const to = new Date();
  const from = new Date(to.getTime() - HISTORY_WINDOW_MS);
  const url =
    `${getApiBase()}/workbench/servers/${hostId}/metrics` +
    `?metric=${encodeURIComponent(metric)}` +
    `&from=${from.toISOString()}` +
    `&to=${to.toISOString()}` +
    `&bucket=auto`;
  const res = await fetch(url, {
    credentials: "include", headers: apiKey ? { Authorization: `Bearer ${apiKey}` } : {},
  });
  if (!res.ok) return [];
  const body = (await res.json()) as MetricsApiResponse;
  return body.points.map((p) => ({
    ts: p.ts,
    avg: p.avg,
    min: p.min,
    max: p.max,
  }));
}

/** Per-host history fetcher. Returns the latest series for each
 *  metric requested; refreshes on a separate timer from the live
 *  `server.stats` poll so a slow `host_metrics` query never starves
 *  the live snapshot path. */
function useHostMetricHistory(
  apiKey: string | null,
  hostId: string,
  metrics: string[],
): Record<string, SparkPoint[]> {
  const [series, setSeries] = useState<Record<string, SparkPoint[]>>({});
  // Stable signature so the effect doesn't re-fire each render.
  const metricsKey = metrics.join("|");
  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      const next: Record<string, SparkPoint[]> = {};
      const all = await Promise.all(
        metrics.map(async (m) => [m, await fetchMetricHistory(apiKey, hostId, m)] as const),
      );
      if (cancelled) return;
      for (const [m, pts] of all) next[m] = pts;
      setSeries(next);
    };
    void tick();
    const id = window.setInterval(tick, HISTORY_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [apiKey, hostId, metricsKey]);
  return series;
}

function fmtBps(bps: number): string {
  if (bps < 1024) return `${bps.toFixed(0)} B/s`;
  if (bps < 1024 ** 2) return `${(bps / 1024).toFixed(1)} KiB/s`;
  if (bps < 1024 ** 3) return `${(bps / 1024 ** 2).toFixed(1)} MiB/s`;
  return `${(bps / 1024 ** 3).toFixed(2)} GiB/s`;
}

export default function ServersPage() {
  const { apiKey } = useAuth();
  const [hosts, setHosts] = useState<Host[]>([]);
  const [statsMap, setStatsMap] = useState<StatsMap>({});
  const [listError, setListError] = useState<string | null>(null);
  // Register form auto-collapses on the first host-list render that
  // already has hosts (fresh page load) and auto-collapses 2.5 s after
  // a successful registration (handled inside AddHostForm).
  const [addFormCollapsed, setAddFormCollapsed] = useState(false);
  const [detailHost, setDetailHost] = useState<Host | null>(null);
  // `nowMs` ticks once per second so the "updated Xs ago" label on
  // each host card stays live without per-card timers. We intentionally
  // decouple this from `POLL_INTERVAL_MS` — the label should keep
  // counting even if a poll skips (e.g. in-flight-guard suppression).
  const [nowMs, setNowMs] = useState(() => (typeof performance !== "undefined" ? performance.now() : 0));
  useEffect(() => {
    const t = setInterval(() => {
      setNowMs(performance.now());
    }, 1000);
    return () => clearInterval(t);
  }, []);

  const refreshList = useCallback(async () => {
    try {
      setListError(null);
      const resp = await invokeAction(apiKey, "server-list", {});
      const payload = unwrapPayload(resp) as { hosts?: Host[] } | undefined;
      setHosts(payload?.hosts ?? []);
    } catch (e) {
      setListError((e as Error).message);
    }
  }, [apiKey]);

  // Findings counts per host — drives the ⚠ badge on each card. Cheap
  // single API call returning ALL open findings; we group client-side.
  const [findingsByHost, setFindingsByHost] = useState<
    Record<string, { critical: number; high: number; medium: number; info: number }>
  >({});
  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const resp = await invokeAction(apiKey, "loganalysis-list", {
          limit: 1000,
        });
        const payload = unwrapPayload(resp) as
          | { findings?: Array<{ host_id: string; severity: string }> }
          | undefined;
        const next: Record<string, { critical: number; high: number; medium: number; info: number }> = {};
        for (const f of payload?.findings ?? []) {
          if (!next[f.host_id]) {
            next[f.host_id] = { critical: 0, high: 0, medium: 0, info: 0 };
          }
          const sev = f.severity as "critical" | "high" | "medium" | "info";
          if (sev in next[f.host_id]) next[f.host_id][sev]++;
        }
        if (!cancelled) setFindingsByHost(next);
      } catch {
        // background fetch — drop silently
      }
    };
    void tick();
    const t = window.setInterval(tick, 15_000);
    return () => {
      cancelled = true;
      window.clearInterval(t);
    };
  }, [apiKey]);

  // Per-host in-flight guard. With `POLL_INTERVAL_MS = 1000` and a
  // typical server.stats round-trip landing in 500-800 ms (ssh handshake
  // + `/proc/stat` delta 300 ms sleep + JSON parse), ticks can start
  // piling up. A per-host ref flag skips a tick when the previous
  // request for that host hasn't returned yet — preserves the "∼1 Hz
  // telemetry" UX without driving sshd to its MaxSessions ceiling.
  const inFlightRef = useRef<Record<string, boolean>>({});

  const refreshStats = useCallback(
    async (id: string) => {
      if (inFlightRef.current[id]) return;
      inFlightRef.current[id] = true;
      const started = performance.now();
      setStatsMap((m) => {
        const prev = m[id];
        return {
          ...m,
          [id]: {
            loading: true,
            stats: prev?.stats,
            error: prev?.error,
            lastFetchMs: prev?.lastFetchMs,
            lastFetchedAt: prev?.lastFetchedAt,
          },
        };
      });
      try {
        const resp = await invokeAction(apiKey, "server-stats", { id });
        const parsed = unwrapPayload(resp) as ServerStats;
        const elapsed = performance.now() - started;
        setStatsMap((m) => ({
          ...m,
          [id]: {
            loading: false,
            stats: parsed,
            lastFetchMs: elapsed,
            lastFetchedAt: performance.now(),
          },
        }));
      } catch (e) {
        const elapsed = performance.now() - started;
        setStatsMap((m) => ({
          ...m,
          [id]: {
            loading: false,
            error: (e as Error).message,
            stats: m[id]?.stats,
            lastFetchMs: elapsed,
            lastFetchedAt: m[id]?.lastFetchedAt,
          },
        }));
      } finally {
        inFlightRef.current[id] = false;
      }
    },
    [apiKey],
  );

  const remove = useCallback(
    async (id: string, host: string) => {
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

  // Auto-collapse the register form the first time we learn there is
  // already at least one host — saves a click for repeat visitors.
  const autoCollapsedOnce = useMemo(() => ({ done: false }), []);
  useEffect(() => {
    if (!autoCollapsedOnce.done && hosts.length > 0) {
      autoCollapsedOnce.done = true;
      setAddFormCollapsed(true);
    }
  }, [hosts.length, autoCollapsedOnce]);

  // Per-host polling loop — one `server.stats` round-trip every
  // `POLL_INTERVAL_MS`. Each call hits the target once via ssh and
  // returns CPU / RAM / disk / temp / GPU / PSU in a single response.
  useEffect(() => {
    if (hosts.length === 0) return;
    hosts.forEach((h) => void refreshStats(h.id));
    const t = setInterval(() => {
      hosts.forEach((h) => void refreshStats(h.id));
    }, POLL_INTERVAL_MS);
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
          {hostList.length > 0 && (
            <span
              className="rounded border border-emerald-700/40 bg-emerald-900/20 px-1.5 py-0.5 font-mono text-[10px] text-emerald-400"
              title={`server.stats is polled every ${POLL_INTERVAL_MS / 1000}s per host`}
              data-testid="servers-poll-badge"
            >
              polling · {POLL_INTERVAL_MS / 1000}s
            </span>
          )}
        </div>
        <Button variant="ghost" size="sm" onClick={refreshList} className="h-6 px-2 text-[11px]">
          Refresh
        </Button>
      </header>

      <div className="flex flex-1 flex-col overflow-auto">
        <AddHostForm
          apiKey={apiKey ?? ""}
          onAdded={refreshList}
          collapsed={addFormCollapsed}
          onCollapsedChange={setAddFormCollapsed}
        />

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
                  onOpenDetail={() => setDetailHost(h)}
                  onAliasChange={(next) =>
                    setHosts((prev) =>
                      prev.map((x) =>
                        x.id === h.id ? { ...x, alias: next } : x,
                      ),
                    )
                  }
                  findingsCount={findingsByHost[h.id]}
                  nowMs={nowMs}
                  apiKey={apiKey}
                />
              ))}
            </div>
          )}
        </section>
      </div>
      {detailHost && (
        <HostDetailDrawer
          open={true}
          onClose={() => setDetailHost(null)}
          apiKey={apiKey}
          hostId={detailHost.id}
          hostLabel={detailHost.host}
          available={{
            gpus:
              statsMap[detailHost.id]?.stats?.gpus.map((g) => ({
                index: g.index,
                name: g.name,
              })) ?? [],
            nics:
              statsMap[detailHost.id]?.stats?.network.map((n) => n.iface) ?? [],
            temps:
              statsMap[detailHost.id]?.stats?.temps.map(
                (t) => `temp.${t.chip}.${t.label}`,
              ) ?? [],
          }}
          context={{
            totalRamBytes: statsMap[detailHost.id]?.stats?.mem?.total_bytes,
          }}
        />
      )}
    </>
  );
}
