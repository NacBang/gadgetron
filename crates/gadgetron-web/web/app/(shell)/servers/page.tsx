"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Toaster, toast } from "sonner";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { Textarea } from "../../components/ui/textarea";
import { Sparkline, type SparkPoint } from "../../components/sparkline";
import { HostDetailDrawer } from "../../components/host-detail-drawer";
import { TopologyGraphView } from "../../components/topology-graph";
import { ServerTileGrid } from "../../components/server-tile-grid";
import {
  topologySignature,
  type HostStatus,
  type TopologyGraph,
} from "../../lib/topology-elements";
import {
  filterSortHosts,
  type FleetHostRow,
  type ServerSortKey,
  type ServerStatusFilter,
  type TileColorBy,
} from "../../lib/server-fleet-view";
import {
  EmptyState,
  InlineNotice,
  PageToolbar,
  StatusBadge,
  WorkbenchPage,
} from "../../components/workbench";
import { useAuth } from "../../lib/auth-context";
import { counterToRollingRate } from "../../lib/metric-series";
import { getApiBase, invokeAction, unwrapPayload } from "../../lib/workbench-client";

// ---------------------------------------------------------------------------
// /web/servers — server-monitor bundle UI.
//
// Three-mode add form (key_path / key_paste / password_bootstrap) on top,
// grid of registered hosts below. Each card polls `server.stats` every
// 5 seconds; clicking the card opens a detail sheet with per-GPU, per-disk,
// and per-chip temperature breakdowns.
// ---------------------------------------------------------------------------

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
  gadgetini?: GadgetiniConfig | null;
}

interface GadgetiniConfig {
  enabled: boolean;
  host_name?: string | null;
  ssh_user?: string | null;
  ssh_port?: number | null;
  parent_iface?: string | null;
  ipv6_link_local?: string | null;
  mac?: string | null;
  web_port?: number | null;
  last_ok_at?: string | null;
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

interface GadgetiniStats {
  air_humidity_pct?: number | null;
  air_temp_c?: number | null;
  chassis_stable?: boolean | null;
  coolant_delta_t_c?: number | null;
  coolant_leak_detected?: boolean | null;
  coolant_level_ok?: boolean | null;
  coolant_temp_inlet1_c?: number | null;
  coolant_temp_inlet2_c?: number | null;
  coolant_temp_outlet1_c?: number | null;
  coolant_temp_outlet2_c?: number | null;
  host_status_code?: number | null;
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
  gadgetini?: GadgetiniStats | null;
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

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MiB`;
  if (n < 1024 ** 4) return `${(n / 1024 ** 3).toFixed(1)} GiB`;
  return `${(n / 1024 ** 4).toFixed(1)} TiB`;
}

// Aligns N rate series by timestamp and sums them. Used to combine
// per-NIC rx/tx counter rates into a single host-wide bandwidth line.
// Assumes the backend bucketing yields identical timestamps across
// metrics of the same host (true for /v1/server.metrics_history) — if a
// timestamp is missing on one series we treat its contribution as 0
// rather than dropping the bucket entirely so a brief gap on one iface
// doesn't blackhole the whole sparkline.
function sumByTimestamp(serieses: SparkPoint[][]): SparkPoint[] {
  if (serieses.length === 0) return [];
  const tsSet = new Set<string>();
  for (const s of serieses) for (const p of s) tsSet.add(p.ts);
  const tsList = Array.from(tsSet).sort();
  return tsList.map((ts) => {
    let sum = 0;
    for (const s of serieses) {
      const hit = s.find((p) => p.ts === ts);
      if (hit) sum += hit.avg;
    }
    return { ts, avg: sum };
  });
}

function fmtPair(a?: number | null, b?: number | null): string {
  const fa = a != null ? a.toFixed(0) : "—";
  const fb = b != null ? b.toFixed(0) : "—";
  return `${fa}/${fb}`;
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

/// Further trims a single GPU product name for the per-GPU card row
/// where horizontal space is tight. Drops the "NVIDIA " / "GeForce "
/// marketing prefixes, the "Server Edition" suffix, and the form-factor
/// tail (SXM/PCIe) so "NVIDIA GeForce RTX 4090" → "RTX 4090" and
/// "NVIDIA RTX PRO 6000 Blackwell Server Edition" → "RTX PRO 6000
/// Blackwell". Keep the full string in tooltips — this is only for the
/// visible row.
function shortenGpuName(name: string): string {
  return name
    .replace(/^NVIDIA\s+/, "")
    .replace(/^GeForce\s+/, "")
    .replace(/\s+Server Edition$/, "")
    .replace(/\s+(SXM[0-9]?|PCIe)$/i, "")
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
  // Single-line layout: [label .......... bar ......... %]. Label can
  // grow up to its natural width then truncate; the bar takes the
  // remaining space via flex-1. `text-[10px]` matches every other
  // label row on the card (GPU, VRAM, PSU, uptime) so type sizes no
  // longer pop visually between sections.
  return (
    <div className="flex items-center gap-2 text-[11px]">
      {label && (
        <span className="min-w-0 max-w-[60%] shrink-0 truncate font-mono font-semibold text-zinc-300">
          {label}
        </span>
      )}
      <div className="h-1.5 flex-1 overflow-hidden rounded bg-zinc-800">
        <div className={`h-full ${color}`} style={{ width: `${clamped}%` }} />
      </div>
      <span className="w-8 shrink-0 text-right font-mono tabular-nums text-zinc-300">
        {clamped.toFixed(0)}%
      </span>
    </div>
  );
}

/// Shared label/value row for the host card body. Picks one
/// consistent typography pair (zinc-500 label · zinc-300 mono value)
/// so the dozen of "PSU 600W" / "uptime 5d" / "max temp 58°C" lines
/// don't drift in font-size and color across the card. Keeping the
/// component co-located with `HostCard` (vs splitting into its own
/// file) is intentional — it's purely presentational and shared by
/// `HostCard` only for now.
function StatRow({
  label,
  value,
  testId,
  indent = false,
}: {
  label: React.ReactNode;
  value: React.ReactNode;
  testId?: string;
  /** Visually nests the row under the section above (cooling sub-rows). */
  indent?: boolean;
}) {
  return (
    <div
      className={`flex items-center justify-between gap-2 text-[11px] ${
        indent ? "pl-2" : ""
      }`}
      data-testid={testId}
    >
      <span className="text-zinc-500">{label}</span>
      <span className="truncate font-mono text-zinc-300">{value}</span>
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
  const [includeGadgetini, setIncludeGadgetini] = useState(false);
  const [gadgetiniMode, setGadgetiniMode] = useState<"usb" | "direct">("usb");
  const [gadgetiniHostName, setGadgetiniHostName] = useState("");
  const [customGadgetiniPassword, setCustomGadgetiniPassword] = useState(false);
  const [gadgetiniPassword, setGadgetiniPassword] = useState("");
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
      let gadgetiniError: string | null = null;
      if (includeGadgetini) {
        const gadgetini: Record<string, unknown> = {
          enabled: true,
          mode: gadgetiniMode,
        };
        if (gadgetiniMode === "direct") {
          gadgetini.host_name = gadgetiniHostName.trim();
        }
        if (customGadgetiniPassword && gadgetiniPassword.trim()) {
          gadgetini.password = gadgetiniPassword;
        }
        try {
          await invokeAction(apiKey, "server-update", {
            id: payload.id,
            gadgetini,
          });
        } catch (e) {
          gadgetiniError = (e as Error).message;
        }
      }
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
            : "") +
          (includeGadgetini && !gadgetiniError ? " · Gadgetini" : ""),
      });
      if (gadgetiniError) {
        toast.error("Gadgetini setup failed", { description: gadgetiniError });
      }
      setHost("");
      setUser("");
      setSshPw("");
      setSudoPw("");
      setKeyPaste("");
      setGadgetiniPassword("");
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
      <div className="rounded border border-zinc-800 bg-zinc-900/40 p-2">
        <label className="flex items-center gap-2 text-[11px] text-zinc-300">
          <input
            type="checkbox"
            checked={includeGadgetini}
            onChange={(e) => setIncludeGadgetini(e.target.checked)}
            className="accent-blue-500"
          />
          Include Gadgetini
        </label>
        {includeGadgetini && (
          <div className="mt-2 flex flex-col gap-2">
            <div className="flex flex-col gap-1">
              <span className="text-[10px] uppercase tracking-wide text-zinc-500">
                Connection
              </span>
              <div className="flex items-center gap-3 text-[11px] text-zinc-300">
                <label className="flex items-center gap-1.5">
                  <input
                    type="radio"
                    name="gadgetini-mode"
                    value="usb"
                    checked={gadgetiniMode === "usb"}
                    onChange={() => setGadgetiniMode("usb")}
                    className="accent-blue-500"
                  />
                  USB (parent host)
                </label>
                <label className="flex items-center gap-1.5">
                  <input
                    type="radio"
                    name="gadgetini-mode"
                    value="direct"
                    checked={gadgetiniMode === "direct"}
                    onChange={() => setGadgetiniMode("direct")}
                    className="accent-blue-500"
                  />
                  Direct IP
                </label>
              </div>
              <p className="text-[10px] text-zinc-600">
                {gadgetiniMode === "usb"
                  ? "SSH proxied through the parent host's USB CDC link (default — IPv6 fd12:3456:789a:1::2 over usb0)."
                  : "SSH directly to the gadgetini's own IP. No parent involvement."}
              </p>
            </div>
            {gadgetiniMode === "direct" && (
              <Input
                type="text"
                placeholder="Gadgetini host or IP (e.g. 192.168.10.42)"
                value={gadgetiniHostName}
                onChange={(e) => setGadgetiniHostName(e.target.value)}
                className="font-mono text-xs"
              />
            )}
            <label className="flex items-center gap-2 text-[11px] text-zinc-500">
              <input
                type="checkbox"
                checked={customGadgetiniPassword}
                onChange={(e) => setCustomGadgetiniPassword(e.target.checked)}
                className="accent-blue-500"
              />
              Custom Gadgetini password
            </label>
            {customGadgetiniPassword && (
              <Input
                type="password"
                placeholder="Gadgetini password"
                value={gadgetiniPassword}
                onChange={(e) => setGadgetiniPassword(e.target.value)}
                className="font-mono text-xs"
              />
            )}
          </div>
        )}
      </div>
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
// Gadgetini manager — attach / edit / detach the gadgetini child board
// for a single host without going through full server-add. Wraps the
// `server-update` action's `gadgetini` field.
// ---------------------------------------------------------------------------

function GadgetiniManager({
  apiKey,
  host,
  onClose,
  onSaved,
}: {
  apiKey: string | null;
  host: Host;
  onClose: () => void;
  onSaved: () => void;
}) {
  const initial = host.gadgetini ?? null;
  // Inventory.json may have records without `mode` (pre-Direct-mode
  // deployments) — treat absent as "usb" so the radio defaults match the
  // record's actual behavior.
  // biome-ignore lint/suspicious/noExplicitAny: legacy field access
  const initialMode = ((initial as any)?.mode as string | undefined) ?? "usb";
  const [enabled, setEnabled] = useState<boolean>(initial?.enabled ?? true);
  const [mode, setMode] = useState<"usb" | "direct">(
    initialMode === "direct" ? "direct" : "usb",
  );
  const [hostName, setHostName] = useState<string>(initial?.host_name ?? "");
  // USB-mode override: name of the parent-host iface that talks to the
  // gadgetini. Defaults to `usb0`; some chassis expose the link as
  // `enpXsYf1np1` etc. Empty string means "use server-side default".
  const [parentIface, setParentIface] = useState<string>(initial?.parent_iface ?? "");
  const [setPassword, setSetPassword] = useState<boolean>(false);
  const [password, setPassword2] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const label = host.alias ?? host.host;
  const hasExisting = initial !== null;

  const save = useCallback(async () => {
    if (busy) return;
    if (mode === "direct" && !hostName.trim()) {
      toast.error("Direct mode requires gadgetini IP / hostname");
      return;
    }
    setBusy(true);
    try {
      const gadgetini: Record<string, unknown> = { enabled, mode };
      if (mode === "direct") {
        gadgetini.host_name = hostName.trim();
      } else if (parentIface.trim()) {
        gadgetini.parent_iface = parentIface.trim();
      }
      if (setPassword && password.trim()) {
        gadgetini.password = password;
      }
      await invokeAction(apiKey, "server-update", {
        id: host.id,
        gadgetini,
      });
      toast.success(
        hasExisting ? `Gadgetini updated on ${label}` : `Gadgetini attached to ${label}`,
      );
      onSaved();
      onClose();
    } catch (e) {
      toast.error("Gadgetini setup failed", { description: (e as Error).message });
    } finally {
      setBusy(false);
    }
  }, [apiKey, busy, enabled, hasExisting, host.id, hostName, label, mode, onClose, onSaved, password, setPassword]);

  const detach = useCallback(async () => {
    if (busy) return;
    if (!window.confirm(`Detach gadgetini from ${label}?\n\nThe gadgetini's redis-side data is left untouched; gadgetron just stops collecting from it. The SSH key on the gadgetini remains installed.`)) {
      return;
    }
    setBusy(true);
    try {
      await invokeAction(apiKey, "server-update", {
        id: host.id,
        gadgetini: null,
      });
      toast.success(`Gadgetini detached from ${label}`);
      onSaved();
      onClose();
    } catch (e) {
      toast.error("Gadgetini detach failed", { description: (e as Error).message });
    } finally {
      setBusy(false);
    }
  }, [apiKey, busy, host.id, label, onClose, onSaved]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        className="flex w-full max-w-md flex-col gap-3 rounded border border-zinc-700 bg-zinc-950 p-4"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <div className="text-sm font-semibold text-zinc-100">
            Gadgetini @ <span className="font-mono text-blue-300">{label}</span>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="text-zinc-500 hover:text-zinc-200"
          >
            ✕
          </button>
        </div>
        <div className="text-[10px] text-zinc-500">
          {hasExisting
            ? "Update connection settings, change mode, or rotate the bootstrap password."
            : "Attach a gadgetini to this host. Choose USB (parent-proxied) or Direct (own IP)."}
        </div>
        <label className="flex items-center gap-2 text-[11px] text-zinc-300">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
            className="accent-blue-500"
          />
          Enabled (collect stats every poll cycle)
        </label>
        <div className="flex flex-col gap-1">
          <span className="text-[10px] uppercase tracking-wide text-zinc-500">
            Connection
          </span>
          <div className="flex items-center gap-3 text-[11px] text-zinc-300">
            <label className="flex items-center gap-1.5">
              <input
                type="radio"
                name="gadgetini-mgr-mode"
                value="usb"
                checked={mode === "usb"}
                onChange={() => setMode("usb")}
                className="accent-blue-500"
              />
              USB (parent host)
            </label>
            <label className="flex items-center gap-1.5">
              <input
                type="radio"
                name="gadgetini-mgr-mode"
                value="direct"
                checked={mode === "direct"}
                onChange={() => setMode("direct")}
                className="accent-blue-500"
              />
              Direct IP
            </label>
          </div>
          <p className="text-[10px] text-zinc-600">
            {mode === "usb"
              ? "SSH lands on the parent host over LAN, then proxies via `nc -6 fd12:3456:789a:1::2 22` over the parent's USB-CDC iface to the gadgetini."
              : "SSH directly to the gadgetini's own IP. host_name required."}
          </p>
        </div>
        {mode === "direct" && (
          <div className="flex flex-col gap-1">
            <span className="text-[10px] uppercase tracking-wide text-zinc-500">
              Gadgetini host
            </span>
            <Input
              type="text"
              placeholder="e.g. 192.168.10.42"
              value={hostName}
              onChange={(e) => setHostName(e.target.value)}
              className="font-mono text-xs"
            />
          </div>
        )}
        {mode === "usb" && (
          <div className="flex flex-col gap-1">
            <span className="text-[10px] uppercase tracking-wide text-zinc-500">
              Parent USB iface
            </span>
            <Input
              type="text"
              placeholder="usb0 (default) — or enp3s0f1np1, etc."
              value={parentIface}
              onChange={(e) => setParentIface(e.target.value)}
              className="font-mono text-xs"
            />
            <p className="text-[10px] text-zinc-600">
              Name of the parent's USB-CDC ethernet iface. Differs by
              chassis; check with <code className="text-zinc-500">ip -6 route get fd12:3456:789a:1::2</code> on the parent if unsure.
            </p>
          </div>
        )}
        <label className="flex items-center gap-2 text-[11px] text-zinc-300">
          <input
            type="checkbox"
            checked={setPassword}
            onChange={(e) => setSetPassword(e.target.checked)}
            className="accent-blue-500"
          />
          Custom password (re-bootstrap)
        </label>
        {setPassword && (
          <Input
            type="password"
            placeholder="Gadgetini password"
            value={password}
            onChange={(e) => setPassword2(e.target.value)}
            className="font-mono text-xs"
          />
        )}
        <p className="text-[10px] text-zinc-600">
          Password is only used to push a fresh ed25519 key to the
          gadgetini, never stored. Falls back to{" "}
          <code className="text-zinc-500">$GADGETRON_GADGETINI_FACTORY_PASSWORD</code>{" "}
          when not provided.
        </p>
        <div className="mt-1 flex items-center justify-between gap-2">
          {hasExisting ? (
            <button
              type="button"
              onClick={detach}
              disabled={busy}
              className="rounded border border-red-900/60 px-2 py-1 text-[11px] text-red-300 hover:bg-red-950/40 disabled:opacity-50"
            >
              Detach
            </button>
          ) : (
            <span />
          )}
          <div className="flex items-center gap-2">
            <Button onClick={onClose} variant="outline" size="sm">
              Cancel
            </Button>
            <Button onClick={save} disabled={busy} size="sm">
              {busy ? "Saving…" : hasExisting ? "Save" : "Attach"}
            </Button>
          </div>
        </div>
      </div>
    </div>
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
    const preview = trimmed.length > 120 ? trimmed.slice(0, 117) + "..." : trimmed;
    const ok = window.confirm(
      `Run on ${label}?\n\n${useSudo ? "[sudo] " : ""}${preview}`,
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
            Shell @ <span className="font-mono text-blue-300">{label}</span>
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
          Every run requires a confirmation dialog. sudo only works when
          NOPASSWD is installed.
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
          placeholder="e.g. systemctl restart nvidia-dcgm && dmesg | tail -20"
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
            Run with sudo
          </label>
          <Button
            type="button"
            size="sm"
            onClick={() => void run()}
            disabled={running || cmd.trim().length === 0}
          >
            {running ? "Running..." : "Run (Ctrl+Enter)"}
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
  onRefresh,
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
  /** Triggers a full refetch of the host list — used when a sub-record
   * (e.g. gadgetini) was attached/edited/detached and we want the card
   * to reflect the new server-side state without partial-updating every
   * field by hand. */
  onRefresh: () => void;
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
  const [gadgetiniOpen, setGadgetiniOpen] = useState(false);
  const [gpuExpanded, setGpuExpanded] = useState(false);
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
    // Include every GPU so the stacked sparkline at the bottom shows
    // the whole fleet on one chart (primary line = GPU 0, others
    // overlaid at lower opacity).
    for (const g of stats?.gpus ?? []) {
      m.push(`gpu.${g.index}.util`);
    }
    // Pull rx + tx counters for every NIC so the bottom sparkline can
    // sum them into a single combined-bandwidth line.
    for (const n of stats?.network ?? []) {
      m.push(`nic.${n.iface}.rx_bytes_total`);
      m.push(`nic.${n.iface}.tx_bytes_total`);
    }
    if (stats?.gadgetini) {
      // Inlet1 stands in for the per-host card's coolant sparkline now
      // that the singular `cooling.coolant_temp` aggregate is gone (the
      // gadgetini firmware writes garbage values into it). The drawer
      // still plots all 4 inlet/outlet probes for the full picture.
      m.push("cooling.coolant_inlet1_temp");
    }
    return m;
  }, [stats?.gadgetini, stats?.gpus, stats?.network]);

  const history = useHostMetricHistory(apiKey, host.id, sparkMetrics);

  // Helpers for current-value annotations (used in sparkline labels).
  const memPct = stats?.mem
    ? `${((stats.mem.used_bytes / stats.mem.total_bytes) * 100).toFixed(0)}%`
    : "—";
  const gpu0Pct =
    stats?.gpus?.[0]?.util_pct != null
      ? `${stats.gpus[0].util_pct.toFixed(0)}%`
      : "—";
  // Sum the per-NIC counter rates into a single rx and a single tx
  // series so multi-NIC hosts (mgmt + bond + IB) show one combined
  // sparkline rather than a noisy per-iface list at the top of the card.
  const nicAggregateHistory = useMemo(() => {
    const ifaces = stats?.network?.map((n) => n.iface) ?? [];
    const rxRates = ifaces.map((i) =>
      counterToRollingRate(history[`nic.${i}.rx_bytes_total`] ?? []),
    );
    const txRates = ifaces.map((i) =>
      counterToRollingRate(history[`nic.${i}.tx_bytes_total`] ?? []),
    );
    return { rx: sumByTimestamp(rxRates), tx: sumByTimestamp(txRates) };
  }, [history, stats?.network]);
  const nicRxLast =
    nicAggregateHistory.rx.length > 0
      ? nicAggregateHistory.rx[nicAggregateHistory.rx.length - 1]?.avg
      : null;
  const nicTxLast =
    nicAggregateHistory.tx.length > 0
      ? nicAggregateHistory.tx[nicAggregateHistory.tx.length - 1]?.avg
      : null;
  const nicSummedFromLive = (stats?.network ?? []).reduce(
    (acc, n) => ({ rx: acc.rx + n.rx_bps, tx: acc.tx + n.tx_bps }),
    { rx: 0, tx: 0 },
  );
  const nicCurrent =
    stats?.network && stats.network.length > 0
      ? `↓ ${fmtBps(nicRxLast ?? nicSummedFromLive.rx)} · ↑ ${fmtBps(
          nicTxLast ?? nicSummedFromLive.tx,
        )}`
      : "—";
  const cooling = stats?.gadgetini;
  // Pick a sensible "headline" coolant temp for the compact card line
  // and the bottom sparkline. Prefer inlet1; fall back through inlet2
  // → outlet1 → outlet2 so a partially-wired board still reads.
  const coolantHeadline =
    cooling?.coolant_temp_inlet1_c ??
    cooling?.coolant_temp_inlet2_c ??
    cooling?.coolant_temp_outlet1_c ??
    cooling?.coolant_temp_outlet2_c ??
    null;
  const coolantCurrent =
    coolantHeadline != null ? `${coolantHeadline.toFixed(1)}°C` : "—";
  return (
    <div
      data-testid={`host-card-${host.host}`}
      className="group/card flex h-[480px] flex-col gap-2 overflow-hidden rounded border border-zinc-800 bg-zinc-900 p-3 text-xs"
    >
      <div className="flex min-w-0 flex-col gap-2">
        <div className="min-w-0" data-testid="host-card-title-row">
          {editing ? (
            <div className="flex min-w-0 items-center gap-1">
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
                className="rounded border border-emerald-900 px-1.5 py-0.5 text-[11px] text-emerald-300 hover:bg-emerald-950/40 disabled:opacity-50"
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
                className="rounded border border-zinc-800 px-1.5 py-0.5 text-[11px] text-zinc-400 hover:text-zinc-200"
                title="Cancel (Esc)"
              >
                ✕
              </button>
            </div>
          ) : (
            <div className="flex min-w-0 items-center gap-1">
              <div
                className="min-w-0 flex-1 truncate text-sm font-semibold text-zinc-100"
                title={host.alias ?? host.host}
              >
                {host.alias ?? host.host}
              </div>
              <button
                type="button"
                onClick={() => setEditing(true)}
                className="shrink-0 rounded px-1 text-[11px] text-zinc-500 opacity-0 transition group-hover/card:opacity-100 hover:bg-zinc-800 hover:text-zinc-300"
                title="Rename alias"
                data-testid={`host-alias-edit-${host.id}`}
              >
                ✎
              </button>
            </div>
          )}
          {/* One connection line — the alias case used to add a second
            * line repeating the bare host, which read as clutter. */}
          <div className="truncate font-mono text-[11px] text-zinc-500">
            {host.ssh_user}@{host.host}:{host.ssh_port}
          </div>
          {(host.cpu_model || (host.gpus && host.gpus.length > 0)) && (
            <div
              className="mt-0.5 truncate text-[11px] text-zinc-500"
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
                <span className="mx-1 text-zinc-600">·</span>
              )}
              {host.gpus && host.gpus.length > 0 && (
                <span>{shortenGpuList(host.gpus)}</span>
              )}
            </div>
          )}
        </div>
        <div
          className="flex flex-wrap items-center justify-end gap-1"
          data-testid={`host-card-actions-${host.id}`}
        >
          {findingsCount && (() => {
            const total =
              findingsCount.critical +
              findingsCount.high +
              findingsCount.medium +
              findingsCount.info;
            if (total === 0) return null;
            const tone =
              findingsCount.critical > 0
                ? "border-red-900 bg-red-950/40 text-red-200"
                : findingsCount.high > 0
                  ? "border-amber-900 bg-amber-950/40 text-amber-200"
                  : findingsCount.medium > 0
                    ? "border-yellow-900 bg-yellow-950/30 text-yellow-200"
                    : "border-zinc-700 bg-zinc-800 text-zinc-300";
            return (
              <a
                href={`/web/findings?host=${host.id}`}
                title={`critical ${findingsCount.critical} · high ${findingsCount.high} · medium ${findingsCount.medium} · info ${findingsCount.info}`}
                className={`rounded border px-1.5 py-0.5 font-mono text-[11px] font-semibold ${tone}`}
                data-testid={`host-findings-${host.id}`}
              >
                ⚠ {total}
              </a>
            );
          })()}
          {/* Uniform icon-only action row. The old mix of emoji+text
            * buttons ("🔧 shell", "+ gadgetini", "remove") wrapped to
            * two lines on narrow cards; same-size icon buttons with
            * tooltips keep the row to one tidy line. Testids and
            * behavior unchanged. */}
          <button
            type="button"
            data-testid={`host-expand-${host.host}`}
            onClick={() => setGpuExpanded((v) => !v)}
            aria-expanded={gpuExpanded}
            aria-label={gpuExpanded ? "Collapse host detail" : "Expand host detail"}
            title={gpuExpanded ? "Collapse" : "Expand inline"}
            className="inline-flex size-6 items-center justify-center rounded border border-zinc-800 text-[11px] text-zinc-400 hover:border-blue-700 hover:text-blue-300"
          >
            {gpuExpanded ? "▾" : "▸"}
          </button>
          <button
            type="button"
            data-testid={`host-detail-${host.host}`}
            onClick={onOpenDetail}
            className="inline-flex size-6 items-center justify-center rounded border border-zinc-800 text-[11px] text-zinc-400 hover:border-blue-700 hover:text-blue-300"
            title="Open detail drawer"
            aria-label="Open detail drawer"
          >
            ⇱
          </button>
          <button
            type="button"
            data-testid={`host-shell-${host.host}`}
            onClick={() => setShellOpen(true)}
            className="inline-flex size-6 items-center justify-center rounded border border-zinc-800 font-mono text-[10px] text-zinc-400 hover:border-blue-700 hover:text-blue-300"
            title="Run remote bash (approval required per call)"
            aria-label="Run remote bash"
          >
            {">_"}
          </button>
          <button
            type="button"
            data-testid={`host-gadgetini-${host.host}`}
            onClick={() => setGadgetiniOpen(true)}
            className={`inline-flex size-6 items-center justify-center rounded border text-[11px] hover:border-blue-700 hover:text-blue-300 ${
              host.gadgetini
                ? "border-blue-900/70 text-blue-300"
                : "border-zinc-800 text-zinc-400"
            }`}
            title={
              host.gadgetini
                ? "Edit gadgetini connection"
                : "Attach a gadgetini child board"
            }
            aria-label="Gadgetini settings"
          >
            📡
          </button>
          <button
            type="button"
            data-testid={`host-remove-${host.host}`}
            onClick={onRemove}
            className="inline-flex size-6 items-center justify-center rounded border border-zinc-800 text-[11px] text-zinc-400 hover:border-red-800 hover:text-red-400"
            title="Remove host"
            aria-label="Remove host"
          >
            ✕
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
      {gadgetiniOpen && (
        <GadgetiniManager
          apiKey={apiKey}
          host={host}
          onClose={() => setGadgetiniOpen(false)}
          onSaved={onRefresh}
        />
      )}
      {err && (
        <InlineNotice
          tone="warn"
          title="Host check reported a problem"
          details={err}
          className="p-2 text-[10px]"
        >
          This host returned an operational warning. Open details for the raw
          output.
        </InlineNotice>
      )}
      {stats?.cpu && (
        // Model name lives in the header line already — repeating it in
        // the bar label just truncated the percentage column.
        <ProgressBar
          pct={stats.cpu.util_pct}
          label={`CPU · ${stats.cpu.cores}c`}
        />
      )}
      {stats?.mem && (
        <ProgressBar
          pct={(stats.mem.used_bytes / stats.mem.total_bytes) * 100}
          label={`RAM ${fmtBytes(stats.mem.used_bytes)} / ${fmtBytes(stats.mem.total_bytes)}`}
        />
      )}
      {stats?.gpus && stats.gpus.length > 0 && (() => {
        const gpus = stats.gpus;
        const utils = gpus
          .map((g) => g.util_pct)
          .filter((u): u is number => u != null);
        const avgUtil =
          utils.length > 0 ? utils.reduce((a, b) => a + b, 0) / utils.length : 0;
        const hottest = gpus
          .map((g) => g.temp_c)
          .filter((t): t is number => t != null)
          .reduce((m, v) => (v > m ? v : m), -Infinity);
        const totalW = gpus
          .map((g) => g.power_w)
          .filter((p): p is number => p != null)
          .reduce((a, b) => a + b, 0);
        const headerBits: string[] = [];
        if (Number.isFinite(hottest)) headerBits.push(`${hottest.toFixed(0)}°C`);
        if (totalW > 0) headerBits.push(`${totalW.toFixed(0)}W`);
        return (
          <div className="flex min-h-0 shrink-0 flex-col gap-1">
            {/* Summary row — always visible. Click to toggle the per-GPU
             * detail block. Collapsed view shows the GPU fleet average
             * + max temp + total wattage, so the card tells you "are
             * my GPUs working?" without expanding. */}
            <button
              type="button"
              onClick={() => setGpuExpanded((v) => !v)}
              className="flex items-center gap-2 text-[11px] text-zinc-400 hover:text-zinc-200"
            >
              {/* Label column — left-aligned, same visual column as the
               * CPU / RAM ProgressBar labels. Arrow moved to the right
               * edge so the "GPU" text lines up with "CPU" and "RAM"
               * across the card. Bold + zinc-300 matches ProgressBar. */}
              <span className="min-w-0 flex-1 truncate text-left font-mono font-semibold text-zinc-300">
                GPU × {gpus.length}
                {gpus.length > 0 && gpus[0].name
                  ? ` — ${shortenGpuName(gpus[0].name)}`
                  : ""}
              </span>
              <span className="shrink-0 truncate font-mono text-zinc-300">
                {headerBits.join(" · ")}
              </span>
              <span
                aria-hidden
                className="w-3 shrink-0 text-center font-mono text-zinc-500"
              >
                {gpuExpanded ? "▾" : "▸"}
              </span>
            </button>
            <ProgressBar pct={avgUtil} label="" tone="amber" />
            {gpuExpanded && (
              <div className="mt-1 flex max-h-[150px] flex-col gap-1 overflow-y-auto border-t border-zinc-800 pt-2 pr-1">
                {gpus.map((g) => {
                  const tempPowerBits: string[] = [];
                  if (g.temp_c != null) tempPowerBits.push(`${g.temp_c}°C`);
                  if (g.mem_temp_c != null) {
                    tempPowerBits.push(`mem ${Math.round(g.mem_temp_c)}°C`);
                  }
                  if (g.power_w != null)
                    tempPowerBits.push(`${g.power_w.toFixed(0)}W`);
                  const hasVram =
                    g.mem_used_mib != null && g.mem_total_mib != null;
                  return (
                    <div key={g.index} className="flex flex-col gap-0.5">
                      <div className="flex items-center justify-between text-[11px] text-zinc-400">
                        <span
                          className="flex items-center gap-1.5 truncate font-mono"
                          title={`${g.name} (source: ${g.source})`}
                        >
                          <span className="truncate">
                            GPU {g.index}
                            {(() => {
                              const n = (g.name ?? "").trim();
                              if (!n) return "";
                              if (/^GPU\s*\d+$/i.test(n)) return "";
                              return ` — ${shortenGpuName(n)}`;
                            })()}
                          </span>
                          <GpuHealthBadges gpu={g} />
                        </span>
                        <span className="truncate font-mono text-zinc-300">
                          {tempPowerBits.join(" · ")}
                        </span>
                      </div>
                      {hasVram && (
                        <div className="flex items-center justify-between text-[11px] text-zinc-500">
                          <span>VRAM</span>
                          <span className="font-mono text-zinc-300">
                            {(g.mem_used_mib! / 1024).toFixed(1)} /{" "}
                            {(g.mem_total_mib! / 1024).toFixed(0)} GiB
                            {" · "}
                            {(
                              (g.mem_used_mib! / g.mem_total_mib!) *
                              100
                            ).toFixed(0)}
                            %
                          </span>
                        </div>
                      )}
                      {g.util_pct != null && (
                        <ProgressBar
                          pct={g.util_pct}
                          label=""
                          tone="amber"
                        />
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        );
      })()}
      {/* Single vitals line — PSU, hottest sensor, and uptime used to
        * be three separate label/value rows; one row reads cleaner and
        * frees vertical space for the sparklines. */}
      {stats &&
        (stats.power?.psu_watts != null ||
          (stats.temps?.length ?? 0) > 0 ||
          stats.uptime_secs != null) && (
          <div
            className="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-[11px] text-zinc-500"
            data-testid="host-vitals-row"
          >
            {stats.power?.psu_watts != null && (
              <span>
                PSU{" "}
                <span className="font-mono text-zinc-300">
                  {stats.power.psu_watts.toFixed(0)}W
                </span>
              </span>
            )}
            {stats.temps && stats.temps.length > 0 && (
              <span>
                max temp{" "}
                <span className="font-mono text-zinc-300">
                  {Math.max(...stats.temps.map((t) => t.celsius)).toFixed(0)}
                  °C
                </span>
              </span>
            )}
            {stats.uptime_secs != null && (
              <span>
                up{" "}
                <span className="font-mono text-zinc-300">
                  {fmtUptime(stats.uptime_secs)}
                </span>
              </span>
            )}
          </div>
        )}
      {cooling && (
        <div
          className="flex flex-col gap-0.5"
          data-testid={`host-cooling-${host.id}`}
        >
          <StatRow
            label="cooling"
            value={
              <>
                {coolantCurrent}
                {cooling.coolant_delta_t_c != null
                  ? ` · Δ ${cooling.coolant_delta_t_c.toFixed(1)}°C`
                  : ""}
              </>
            }
          />
          {(cooling.coolant_temp_inlet1_c != null ||
            cooling.coolant_temp_inlet2_c != null ||
            cooling.coolant_temp_outlet1_c != null ||
            cooling.coolant_temp_outlet2_c != null) && (
            <StatRow
              indent
              label="in→out"
              value={
                <>
                  {fmtPair(
                    cooling.coolant_temp_inlet1_c,
                    cooling.coolant_temp_inlet2_c,
                  )}
                  {" → "}
                  {fmtPair(
                    cooling.coolant_temp_outlet1_c,
                    cooling.coolant_temp_outlet2_c,
                  )}
                  {"°C"}
                </>
              }
            />
          )}
          {(cooling.air_temp_c != null || cooling.air_humidity_pct != null) && (
            <StatRow
              indent
              label="air"
              value={
                <>
                  {cooling.air_temp_c != null
                    ? `${cooling.air_temp_c.toFixed(0)}°C`
                    : "—"}
                  {cooling.air_humidity_pct != null
                    ? ` · ${cooling.air_humidity_pct.toFixed(0)}% RH`
                    : ""}
                </>
              }
            />
          )}
          {(cooling.coolant_leak_detected ||
            cooling.coolant_level_ok === false ||
            cooling.chassis_stable === false) && (
            <div className="flex flex-wrap gap-1 pt-0.5 pl-2">
              {cooling.coolant_leak_detected && (
                <span className="rounded border border-red-900 bg-red-950/60 px-1.5 py-0.5 font-mono text-[10px] font-semibold uppercase tracking-wider text-red-300">
                  Leak
                </span>
              )}
              {cooling.coolant_level_ok === false && (
                <span className="rounded border border-amber-900 bg-amber-950/60 px-1.5 py-0.5 font-mono text-[10px] font-semibold uppercase tracking-wider text-amber-300">
                  Level
                </span>
              )}
              {cooling.chassis_stable === false && (
                <span className="rounded border border-amber-900 bg-amber-950/60 px-1.5 py-0.5 font-mono text-[10px] font-semibold uppercase tracking-wider text-amber-300">
                  Chassis
                </span>
              )}
            </div>
          )}
        </div>
      )}
      {sparkMetrics.length > 0 && (
        <div
          className="mt-auto grid grid-cols-2 gap-x-3 gap-y-1 border-t border-zinc-800 pt-2"
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
          {stats?.gpus && stats.gpus.length > 0 && (() => {
            const gpus = stats.gpus;
            // Primary line = GPU 0, all other GPUs overlaid at lower
            // opacity so a 4× fleet reads as one chart rather than four
            // duplicate panels.
            const primaryPts = history[`gpu.${gpus[0].index}.util`] ?? [];
            const extraSeries = gpus
              .slice(1)
              .map((g) => history[`gpu.${g.index}.util`] ?? []);
            // For ≤2 GPUs show every value (`100/45%`). For 3+ GPUs the
            // per-GPU list wraps the sparkline header to 2 lines on
            // narrow cards and breaks the grid alignment with the
            // adjacent NIC/cooling sparklines, so summarize as max/avg.
            const utils = gpus
              .map((g) => g.util_pct)
              .filter((v): v is number => v != null);
            const currentVals =
              gpus.length <= 2
                ? gpus
                    .map((g) =>
                      g.util_pct != null ? g.util_pct.toFixed(0) : "—",
                    )
                    .join("/")
                : utils.length > 0
                  ? `max ${Math.max(...utils).toFixed(0)} · avg ${(
                      utils.reduce((a, b) => a + b, 0) / utils.length
                    ).toFixed(0)}`
                  : "—";
            const labelSuffix =
              gpus.length > 1 ? ` × ${gpus.length}` : "";
            return (
              <Sparkline
                label={`gpu util${labelSuffix} (5m)`}
                current={`${currentVals}%`}
                points={primaryPts}
                series={extraSeries}
                tone="amber"
                yMin={0}
                yMax={100}
              />
            );
          })()}
          {stats?.network && stats.network.length > 0 && (
            <Sparkline
              label={`nic total (${stats.network.length}) rx+tx (5m)`}
              current={nicCurrent}
              points={nicAggregateHistory.rx}
              series={[nicAggregateHistory.tx]}
              tone="zinc"
              yMin={0}
            />
          )}
          {cooling && (
            <Sparkline
              label="coolant inlet1 (5m)"
              current={coolantCurrent}
              points={history["cooling.coolant_inlet1_temp"] ?? []}
              tone="blue"
            />
          )}
        </div>
      )}
      {stats?.warnings && stats.warnings.length > 0 && (
        <details className="text-[11px] text-zinc-500">
          <summary className="cursor-pointer hover:text-zinc-300">
            warnings ({stats.warnings.length})
          </summary>
          <ul className="mt-1 space-y-0.5 pl-4">
            {stats.warnings.map((w, i) => (
              <li key={i} className="list-disc text-zinc-400">
                {w}
              </li>
            ))}
          </ul>
        </details>
      )}
      <div
        data-testid="host-card-footer"
        className="mt-auto flex items-center justify-between gap-2 border-t border-zinc-800 pt-2 text-[11px] text-zinc-500"
      >
        <span className="flex items-center gap-1.5">
          {/* Static status dot. Color reflects last poll outcome; no
            * pulse/animate so 1 Hz polling doesn't feel like strobe
            * lighting. Err wins over stale. */}
          <span
            aria-hidden
            className={`inline-block size-2 rounded-full ${
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
          <span
            className="font-mono"
            title="Last round-trip latency (gadgetron ↔ target via SSH)"
          >
            fetch{" "}
            {fetchMs < 1000
              ? `${fetchMs.toFixed(0)}ms`
              : `${(fetchMs / 1000).toFixed(2)}s`}
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
  // Cards vs topology graph (ISSUE 41). The graph fetches once on
  // entry + every 60 s — topology changes on cabling work, not
  // per-second, and the action is an inventory read (no SSH).
  const [view, setView] = useState<"cards" | "tiles" | "graph">("cards");
  const [topology, setTopology] = useState<TopologyGraph | null>(null);
  const [topologyError, setTopologyError] = useState<string | null>(null);
  // Shared filter / sort bar (ISSUE 48) — applies to cards AND tiles.
  // Status + metric values come from the single-call `server-fleet`
  // action; without it (legacy no-DB mode) filters degrade to no-ops.
  const [query, setQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<ServerStatusFilter>("all");
  const [sortKey, setSortKey] = useState<ServerSortKey>("name");
  const [tileColorBy, setTileColorBy] = useState<TileColorBy>("status");
  const [fleet, setFleet] = useState<ReadonlyMap<string, FleetHostRow>>(
    new Map(),
  );

  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const resp = await invokeAction(apiKey, "server-fleet", {});
        const payload = unwrapPayload(resp) as
          | { snapshots_available?: boolean; hosts?: FleetHostRow[] }
          | undefined;
        if (!cancelled) {
          // Without a snapshot store (legacy no-DB mode) every host
          // reads "offline", which is a lie — degrade to the no-data
          // mode instead: filters no-op, tiles gray (ISSUE 50).
          setFleet(
            payload?.snapshots_available === false
              ? new Map()
              : new Map((payload?.hosts ?? []).map((r) => [r.id, r])),
          );
        }
      } catch {
        // Fleet summary is an enhancement — filter/sort degrade
        // gracefully without it.
      }
    };
    void tick();
    const t = window.setInterval(() => void tick(), 10_000);
    return () => {
      cancelled = true;
      window.clearInterval(t);
    };
  }, [apiKey]);

  // Graph border colors (ISSUE 49) — same fleet data, narrowed to the
  // status fields the topology view needs.
  const fleetStatus = useMemo(
    () =>
      new Map<string, HostStatus>(
        [...fleet].map(([id, r]) => [
          id,
          { online: r.online, warnings: r.warnings },
        ]),
      ),
    [fleet],
  );
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

  useEffect(() => {
    if (view !== "graph") return;
    let cancelled = false;
    const fetchTopology = async () => {
      try {
        const resp = await invokeAction(apiKey, "server-topology", {});
        if (!cancelled) {
          const next = unwrapPayload(resp) as TopologyGraph;
          // Same-content refetches keep the previous object — a new
          // identity would re-run cytoscape layout and reset the
          // operator's pan/zoom every 60 s.
          setTopology((prev) =>
            prev && topologySignature(prev) === topologySignature(next)
              ? prev
              : next,
          );
          setTopologyError(null);
        }
      } catch (e) {
        if (!cancelled) setTopologyError((e as Error).message);
      }
    };
    void fetchTopology();
    const t = setInterval(() => void fetchTopology(), 60_000);
    return () => {
      cancelled = true;
      clearInterval(t);
    };
  }, [view, apiKey]);

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

  const hostList = useMemo(
    () => filterSortHosts(hosts, fleet, query, statusFilter, sortKey),
    [hosts, fleet, query, statusFilter, sortKey],
  );
  const hasHostErrors = Object.values(statsMap).some((entry) => entry.error);

  return (
    <>
      <Toaster theme="dark" richColors position="bottom-right" />
      <WorkbenchPage
        title="Servers"
        headerTestId="servers-header"
        actions={
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void refreshList()}
            className="h-7 px-2 text-[11px]"
          >
            Refresh
          </Button>
        }
        toolbar={
          <PageToolbar
            status={
              <StatusBadge
                status={listError || hasHostErrors ? "degraded" : "ready"}
              />
            }
          >
            <span
              className="text-[11px] text-zinc-600"
              data-testid="servers-count"
            >
              {hostList.length} host{hostList.length === 1 ? "" : "s"}
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
          </PageToolbar>
        }
      >
      <div className="space-y-4">
        <AddHostForm
          apiKey={apiKey ?? ""}
          onAdded={refreshList}
          collapsed={addFormCollapsed}
          onCollapsedChange={setAddFormCollapsed}
        />

        {listError && (
          <InlineNotice
            tone="error"
            title="Server inventory request failed"
            details={listError}
          >
            Gadgetron could not load or update the managed server list.
          </InlineNotice>
        )}

        <div className="flex flex-wrap items-center gap-3">
          <div className="flex items-center gap-1" data-testid="servers-view-toggle">
            {(["cards", "tiles", "graph"] as const).map((v) => (
              <button
                key={v}
                type="button"
                onClick={() => setView(v)}
                className={`rounded-md border px-2.5 py-1 text-xs font-mono transition-colors ${
                  view === v
                    ? "border-zinc-600 bg-zinc-800 text-zinc-100"
                    : "border-zinc-800 text-zinc-500 hover:text-zinc-300"
                }`}
              >
                {v === "cards" ? "카드" : v === "tiles" ? "타일" : "그래프"}
              </button>
            ))}
          </div>
          {view !== "graph" && (
            <div
              className="flex flex-wrap items-center gap-2"
              data-testid="servers-filter-bar"
            >
              <Input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="검색 — alias · host · GPU 모델"
                className="h-7 w-60 font-mono text-xs"
                data-testid="servers-filter-query"
              />
              <div className="flex items-center gap-1">
                {(
                  [
                    ["all", "전체"],
                    ["online", "온라인"],
                    ["offline", "오프라인"],
                    ["warn", "경고"],
                  ] as const
                ).map(([key, label]) => (
                  <button
                    key={key}
                    type="button"
                    onClick={() => setStatusFilter(key)}
                    aria-pressed={statusFilter === key}
                    className={`rounded-full border px-2 py-0.5 text-[11px] transition-colors ${
                      statusFilter === key
                        ? "border-blue-700 bg-blue-950/40 text-blue-300"
                        : "border-zinc-800 text-zinc-500 hover:text-zinc-300"
                    }`}
                  >
                    {label}
                  </button>
                ))}
              </div>
              <label className="flex items-center gap-1 text-[11px] text-zinc-500">
                정렬
                <select
                  value={sortKey}
                  onChange={(e) => setSortKey(e.target.value as ServerSortKey)}
                  data-testid="servers-sort-select"
                  className="rounded border border-zinc-800 bg-zinc-900 px-1.5 py-0.5 text-[11px] text-zinc-300"
                >
                  <option value="name">이름</option>
                  <option value="cpu">CPU 사용률</option>
                  <option value="gpu">GPU 사용률</option>
                  <option value="temp">GPU 온도</option>
                </select>
              </label>
              {(query.trim() !== "" || statusFilter !== "all") && (
                <span
                  className="text-[11px] text-zinc-600"
                  data-testid="servers-filter-count"
                >
                  {hostList.length}/{hosts.length} 표시
                </span>
              )}
            </div>
          )}
        </div>

        {view === "graph" ? (
          <section data-testid="topology-section">
            {topologyError && (
              <InlineNotice
                tone="error"
                title="Topology request failed"
                details={topologyError}
              >
                Gadgetron could not load the cluster topology graph.
              </InlineNotice>
            )}
            {topology ? (
              <TopologyGraphView
                graph={topology}
                status={fleetStatus}
                onSelectHost={(id) => {
                  const h = hosts.find((x) => x.id === id);
                  if (h) setDetailHost(h);
                }}
              />
            ) : (
              !topologyError && (
                <div className="text-xs text-zinc-500" data-testid="topology-loading">
                  loading topology…
                </div>
              )
            )}
          </section>
        ) : view === "tiles" ? (
          <section data-testid="tiles-section">
            <ServerTileGrid
              hosts={hostList}
              fleet={fleet}
              colorBy={tileColorBy}
              onColorByChange={setTileColorBy}
              onSelect={(id) => {
                const h = hosts.find((x) => x.id === id);
                if (h) setDetailHost(h);
              }}
            />
          </section>
        ) : (
        <section>
          {hostList.length === 0 ? (
            <div data-testid="servers-empty">
              {hosts.length === 0 ? (
                <EmptyState
                  title="No hosts registered yet"
                  description="Use the registration form to add a managed server."
                />
              ) : (
                <EmptyState
                  title="조건에 맞는 서버가 없습니다"
                  description="검색어 또는 상태 필터를 조정해 보세요."
                />
              )}
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
                  onRefresh={() => void refreshList()}
                  findingsCount={findingsByHost[h.id]}
                  nowMs={nowMs}
                  apiKey={apiKey}
                />
              ))}
            </div>
          )}
        </section>
        )}
      </div>
      </WorkbenchPage>
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
            cooling: Boolean(statsMap[detailHost.id]?.stats?.gadgetini),
          }}
          context={{
            totalRamBytes: statsMap[detailHost.id]?.stats?.mem?.total_bytes,
          }}
        />
      )}
    </>
  );
}
