"use client";

// One host card in the /web/servers grid: live stat bars, GPU health
// badges, cooling block, sparklines, and the per-host action row.
// Split out of /web/servers (ISSUE 54).

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Droplets,
  Maximize2,
  Pencil,
  SquareTerminal,
  TriangleAlert,
  X,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Sparkline, type SparkPoint } from "../sparkline";
import { InlineNotice } from "../workbench";
import { counterToRollingRate } from "../../lib/metric-series";
import { getApiBase, invokeAction } from "../../lib/workbench-client";
import {
  fmtBps,
  fmtBytes,
  fmtPair,
  fmtUptime,
  shortenCpu,
  shortenGpuList,
  shortenGpuName,
} from "../../lib/format";
import type { GpuStats, Host, StatsMap } from "../../lib/server-types";
import { GadgetiniManager } from "./gadgetini-manager";
import { ShellRunner } from "./shell-runner";

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

export function HostCard({
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

  // Single source of truth for the host's health, shared by the left
  // accent bar, the header dot, and the footer dot (ISSUE 63). Error
  // beats stale beats live; "unknown" until the first poll lands.
  const health: "ok" | "stale" | "error" | "unknown" = err
    ? "error"
    : ageS == null
      ? "unknown"
      : ageS > 10
        ? "stale"
        : "ok";
  const healthDot = {
    ok: "bg-emerald-500",
    stale: "bg-amber-500",
    error: "bg-red-500",
    unknown: "bg-zinc-600",
  }[health];
  // Left accent bar: colored for problems, transparent when healthy so
  // the grid stays calm and only trouble hosts draw the eye.
  const healthAccent = {
    ok: "before:bg-transparent",
    stale: "before:bg-amber-500/70",
    error: "before:bg-red-500",
    unknown: "before:bg-zinc-700/60",
  }[health];

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
      className={`surface-1 is-interactive group/card relative flex h-[480px] flex-col gap-2 overflow-hidden rounded-lg p-3 text-xs before:absolute before:inset-y-0 before:left-0 before:w-1 before:rounded-l-lg before:transition-colors ${healthAccent}`}
    >
      <div className="flex min-w-0 flex-col gap-2 border-b border-white/[0.06] pb-2">
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
              <Button
                type="button"
                variant="outline"
                size="icon-xs"
                onClick={() => void saveAlias()}
                disabled={saving}
                className="border-emerald-900 text-emerald-300 hover:bg-emerald-950/40 hover:text-emerald-200"
                title="Save (Enter)"
                aria-label="Save alias"
              >
                <Check aria-hidden />
              </Button>
              <Button
                type="button"
                variant="outline"
                size="icon-xs"
                onClick={() => {
                  setEditing(false);
                  setDraft(host.alias ?? "");
                }}
                disabled={saving}
                title="Cancel (Esc)"
                aria-label="Cancel alias edit"
              >
                <X aria-hidden />
              </Button>
            </div>
          ) : (
            <div className="flex min-w-0 items-center gap-1.5">
              {/* Status dot — same health source as the footer / accent
                * bar, here so the card reads at a glance (ISSUE 63). */}
              <span
                aria-hidden
                title={`status: ${health}`}
                className={`size-2 shrink-0 rounded-full ${healthDot}`}
              />
              <div
                className="min-w-0 flex-1 truncate text-sm font-semibold text-zinc-100"
                title={host.alias ?? host.host}
              >
                {host.alias ?? host.host}
              </div>
              {/* Findings badge promoted to the title row so a warning
                * isn't buried among the action icons (ISSUE 63). */}
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
                    className={`inline-flex shrink-0 items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-[10px] font-semibold ${tone}`}
                    data-testid={`host-findings-${host.id}`}
                  >
                    <TriangleAlert className="size-3" aria-hidden /> {total}
                  </a>
                );
              })()}
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                onClick={() => setEditing(true)}
                className="shrink-0 text-zinc-500 opacity-0 transition group-hover/card:opacity-100"
                title="Rename alias"
                aria-label="Rename alias"
                data-testid={`host-alias-edit-${host.id}`}
              >
                <Pencil aria-hidden />
              </Button>
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
          className="flex flex-wrap items-center justify-end gap-1 opacity-65 transition-opacity group-hover/card:opacity-100 focus-within:opacity-100"
          data-testid={`host-card-actions-${host.id}`}
        >
          {/* Uniform icon-only action row (findings badge moved to the
            * title row in ISSUE 63). Row is dimmed at rest and comes
            * forward on card hover so the body stays the focus. Testids
            * and behavior unchanged. */}
          <Button
            type="button"
            variant="outline"
            size="icon-xs"
            data-testid={`host-expand-${host.host}`}
            onClick={() => setGpuExpanded((v) => !v)}
            aria-expanded={gpuExpanded}
            aria-label={gpuExpanded ? "Collapse host detail" : "Expand host detail"}
            title={gpuExpanded ? "Collapse" : "Expand inline"}
          >
            {gpuExpanded ? (
              <ChevronDown aria-hidden />
            ) : (
              <ChevronRight aria-hidden />
            )}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="icon-xs"
            data-testid={`host-detail-${host.host}`}
            onClick={onOpenDetail}
            title="Open detail drawer"
            aria-label="Open detail drawer"
          >
            <Maximize2 aria-hidden />
          </Button>
          <Button
            type="button"
            variant="outline"
            size="icon-xs"
            data-testid={`host-shell-${host.host}`}
            onClick={() => setShellOpen(true)}
            title="Run remote bash (approval required per call)"
            aria-label="Run remote bash"
          >
            <SquareTerminal aria-hidden />
          </Button>
          <Button
            type="button"
            variant="outline"
            size="icon-xs"
            data-testid={`host-gadgetini-${host.host}`}
            onClick={() => setGadgetiniOpen(true)}
            className={
              host.gadgetini ? "border-blue-900/70 text-blue-300" : undefined
            }
            title={
              host.gadgetini
                ? "Edit gadgetini connection"
                : "Attach a gadgetini child board"
            }
            aria-label="Gadgetini settings"
          >
            <Droplets aria-hidden />
          </Button>
          <Button
            type="button"
            variant="outline"
            size="icon-xs"
            data-testid={`host-remove-${host.host}`}
            onClick={onRemove}
            className="hover:border-destructive/40 hover:text-destructive"
            title="Remove host"
            aria-label="Remove host"
          >
            <X aria-hidden />
          </Button>
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
                {gpuExpanded ? (
                  <ChevronDown className="inline size-3" aria-hidden />
                ) : (
                  <ChevronRight className="inline size-3" aria-hidden />
                )}
              </span>
            </button>
            <ProgressBar pct={avgUtil} label="" tone="amber" />
            {gpuExpanded && (
              <div className="mt-1 flex max-h-[150px] flex-col gap-1 overflow-y-auto border-t border-white/[0.06] pt-2 pr-1">
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
          className="mt-auto grid grid-cols-2 gap-x-3 gap-y-1 border-t border-white/[0.06] pt-2"
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
        className="mt-auto flex items-center justify-between gap-2 border-t border-white/[0.06] pt-2 text-[11px] text-zinc-500"
      >
        <span className="flex items-center gap-1.5">
          {/* Static status dot — shared health source (ISSUE 63); no
            * pulse so 1 Hz polling doesn't feel like strobe lighting. */}
          <span
            aria-hidden
            className={`inline-block size-2 rounded-full ${healthDot}`}
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
