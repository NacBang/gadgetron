"use client";

import { useEffect, useMemo, useState } from "react";
import { useAuth } from "../../lib/auth-context";
import { Sparkline, type SparkPoint } from "../sparkline";
import { HostDetailDrawer } from "../host-detail-drawer";
import { getApiBase, invokeAction, unwrapPayload, type ActionResponse } from "../../lib/workbench-client";

// Background polls must stay silent on failure — wrap the shared
// client's throwing invokeAction back into the null contract this
// grid's call sites rely on.
async function invokeActionOrNull(
  apiKey: string | null,
  actionId: string,
  args: Record<string, unknown>,
): Promise<ActionResponse | null> {
  try {
    return await invokeAction(apiKey, actionId, args);
  } catch {
    return null;
  }
}

function unwrapNullable(resp: ActionResponse | null): unknown {
  return resp ? unwrapPayload(resp) : null;
}

// Live host grid for the `/web/copilot` split route. Polls
// `server-list` every 5 s for inventory + `server-stats` every 5 s
// per registered host. Renders one card per host with the same
// information density as `/web/servers` (CPU bar, RAM bar, GPU
// summary, coolant headline, status footer) so the operator can
// stay in copilot and ask Penny questions while watching the
// fleet's live state.
//
// Visual contract mirrors `/web/servers` HostCard:
//   - StatRow `text-[11px]` typography with zinc-500 label /
//     zinc-300 mono value
//   - ProgressBar tone matches workbench convention (CPU=blue,
//     RAM=blue, GPU=amber)
//   - Solid border-zinc-800 (no opacity variants)
// A future follow-up will extract the shared HostCard so both
// /web/servers and /web/copilot drive off one component; this
// inline copy is the operator-priority path that matches the
// servers card visually right now.

interface HostRow {
  id: string;
  host: string;
  alias?: string | null;
  cpu_model?: string | null;
  cpu_cores?: number | null;
  gpus?: string[];
  last_ok_at?: string | null;
}

interface GpuStat {
  index: number;
  name: string;
  util_pct: number | null;
  mem_used_mib: number | null;
  mem_total_mib: number | null;
  temp_c: number | null;
  power_w: number | null;
}

interface CoolingStat {
  coolant_temp_inlet1_c?: number | null;
  coolant_temp_inlet2_c?: number | null;
  coolant_temp_outlet1_c?: number | null;
  coolant_temp_outlet2_c?: number | null;
  coolant_delta_t_c?: number | null;
  coolant_leak_detected?: boolean | null;
  coolant_level_ok?: boolean | null;
  air_temp_c?: number | null;
  air_humidity_pct?: number | null;
}

interface NetworkStat {
  iface: string;
  rx_bps: number;
  tx_bps: number;
}

interface TempStat {
  chip: string;
  label: string;
  celsius: number;
}

interface ServerStats {
  cpu: {
    util_pct: number;
    cores: number;
    load_1m: number;
    load_5m: number;
  } | null;
  mem: {
    total_bytes: number;
    used_bytes: number;
  } | null;
  gpus: GpuStat[];
  power: { psu_watts: number | null } | null;
  network?: NetworkStat[];
  temps?: TempStat[];
  gadgetini?: CoolingStat | null;
  uptime_secs: number | null;
}

const POLL_HOSTS_MS = 5_000;
/// Per-host stats poll cadence. Server-monitor polls 1 Hz internally
/// → copilot pulling every 5 s is plenty fresh and keeps the
/// gateway/SSH path quiet.
const POLL_STATS_MS = 5_000;
/// Sparkline window — last 5 minutes at the auto-tier `raw` resolution
/// matches what `/web/servers` uses, so a host card looks the same
/// across both surfaces.
const HISTORY_WINDOW_MS = 5 * 60 * 1000;
const HISTORY_REFRESH_MS = 5_000;
const CRITICAL_AGE_MS = 5 * 60 * 1000;
const WARNING_AGE_MS = 90 * 1000;

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MiB`;
  if (n < 1024 ** 4) return `${(n / 1024 ** 3).toFixed(1)} GiB`;
  return `${(n / 1024 ** 4).toFixed(1)} TiB`;
}

function ageMs(iso?: string | null, now: number = Date.now()): number {
  if (!iso) return Number.POSITIVE_INFINITY;
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return Number.POSITIVE_INFINITY;
  return now - t;
}

function relativeAge(ms: number): string {
  if (!Number.isFinite(ms)) return "never";
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 5) return "just now";
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  return `${Math.round(s / 3600)}h ago`;
}

type Tone = "ok" | "warning" | "critical" | "neutral";

function toneFromAge(ms: number): Tone {
  if (!Number.isFinite(ms)) return "critical";
  if (ms >= CRITICAL_AGE_MS) return "critical";
  if (ms >= WARNING_AGE_MS) return "warning";
  return "ok";
}

function toneBadge(tone: Tone): { label: string; cls: string } {
  switch (tone) {
    case "critical":
      return {
        label: "stale",
        cls: "border-red-900 text-red-300 bg-red-950/40",
      };
    case "warning":
      return {
        label: "lag",
        cls: "border-amber-900 text-amber-300 bg-amber-950/40",
      };
    case "ok":
      return {
        label: "live",
        cls: "border-emerald-900 text-emerald-300 bg-emerald-950/40",
      };
    default:
      return {
        label: "—",
        cls: "border-zinc-800 text-zinc-400 bg-zinc-900",
      };
  }
}

function StatRow({
  label,
  value,
}: {
  label: React.ReactNode;
  value: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-2 text-[11px]">
      <span className="text-zinc-500">{label}</span>
      <span className="truncate font-mono text-zinc-300">{value}</span>
    </div>
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

function shortenCpu(model: string): string {
  return model
    .replace(/Intel\(R\)|AMD|Xeon\(R\)|Core\(TM\)|EPYC|Processor/gi, "")
    .replace(/\s+/g, " ")
    .replace(/@.*$/, "")
    .replace(/\s+CPU/i, "")
    .trim()
    .slice(0, 36);
}

function shortenGpuName(name: string): string {
  return name
    .replace(/NVIDIA\s+/i, "")
    .replace(/GeForce\s+/i, "")
    .replace(/PCIe/i, "")
    .replace(/\s+/g, " ")
    .trim();
}

async function fetchHosts(apiKey: string | null): Promise<HostRow[]> {
  const resp = await invokeActionOrNull(apiKey, "server-list", {});
  const parsed = unwrapNullable(resp) as { hosts?: HostRow[] } | null;
  return Array.isArray(parsed?.hosts) ? parsed.hosts : [];
}

interface MetricsApiResponse {
  metric: string;
  unit: string | null;
  resolution: string;
  points: Array<{ ts: string; avg: number; min: number; max: number }>;
}

/// Pull a 5-minute window of one metric at the auto-resolution tier
/// (raw 1 Hz buckets). Returns an empty array on any failure — the
/// sparkline copes by showing nothing rather than dropping the
/// whole card.
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
  const headers: Record<string, string> = {};
  if (apiKey) headers["Authorization"] = `Bearer ${apiKey}`;
  try {
    const res = await fetch(url, { credentials: "include", headers });
    if (!res.ok) return [];
    const body = (await res.json()) as MetricsApiResponse;
    return body.points.map((p) => ({
      ts: p.ts,
      avg: p.avg,
      min: p.min,
      max: p.max,
    }));
  } catch {
    return [];
  }
}

/// Per-card sparkline source. Same shape as `/web/servers`'s hook so
/// both pages render visually-equivalent strips. The metrics list is
/// recomputed each render but the effect re-runs only when the joined
/// string changes — `metrics.join("|")` is the stable signature.
function useHostMetricHistory(
  apiKey: string | null,
  hostId: string,
  metrics: string[],
): Record<string, SparkPoint[]> {
  const [series, setSeries] = useState<Record<string, SparkPoint[]>>({});
  const metricsKey = metrics.join("|");
  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      const all = await Promise.all(
        metrics.map(
          async (m) =>
            [m, await fetchMetricHistory(apiKey, hostId, m)] as const,
        ),
      );
      if (cancelled) return;
      const next: Record<string, SparkPoint[]> = {};
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

function fmtBps(bps: number | undefined): string {
  if (bps == null || !Number.isFinite(bps)) return "—";
  if (bps < 1024) return `${bps.toFixed(0)}B/s`;
  if (bps < 1024 ** 2) return `${(bps / 1024).toFixed(1)}KB/s`;
  if (bps < 1024 ** 3) return `${(bps / 1024 ** 2).toFixed(1)}MB/s`;
  return `${(bps / 1024 ** 3).toFixed(1)}GB/s`;
}

/// Sum N timeseries by timestamp. Aligns input arrays by exact ts
/// match (host_metrics writes 1 Hz for all NICs in lockstep so this
/// is the common case). Missing timestamps in any input series count
/// as 0. Mirrors `(shell)/servers/page.tsx::sumByTimestamp` so both
/// surfaces produce the same NIC aggregate.
function sumSeriesByTs(serieses: SparkPoint[][]): SparkPoint[] {
  if (serieses.length === 0) return [];
  const buckets = new Map<string, { sum: number; count: number }>();
  for (const s of serieses) {
    for (const p of s) {
      const cur = buckets.get(p.ts);
      if (cur) {
        cur.sum += p.avg;
        cur.count += 1;
      } else {
        buckets.set(p.ts, { sum: p.avg, count: 1 });
      }
    }
  }
  return Array.from(buckets.entries())
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([ts, { sum }]) => ({ ts, avg: sum }));
}

async function fetchStats(
  apiKey: string | null,
  hostId: string,
): Promise<ServerStats | null> {
  try {
    const resp = await invokeActionOrNull(apiKey, "server-stats", { id: hostId });
    return unwrapNullable(resp) as ServerStats | null;
  } catch {
    return null;
  }
}

function CoolingTile({ cooling }: { cooling: CoolingStat }) {
  const headline =
    cooling.coolant_temp_inlet1_c ??
    cooling.coolant_temp_inlet2_c ??
    cooling.coolant_temp_outlet1_c ??
    cooling.coolant_temp_outlet2_c ??
    null;
  const value = headline != null ? `${headline.toFixed(1)}°C` : "—";
  return (
    <>
      <StatRow
        label="cooling"
        value={
          <>
            {value}
            {cooling.coolant_delta_t_c != null
              ? ` · Δ ${cooling.coolant_delta_t_c.toFixed(1)}°C`
              : ""}
          </>
        }
      />
      {(cooling.coolant_leak_detected ||
        cooling.coolant_level_ok === false) && (
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
        </div>
      )}
    </>
  );
}

function HostCard({
  host,
  stats,
  now,
  apiKey,
  onOpenDrawer,
}: {
  host: HostRow;
  stats: ServerStats | null;
  now: number;
  apiKey: string | null;
  /// Caller manages the drawer at the grid level so only one drawer is
  /// open at a time across the fleet — opening a card's drawer
  /// auto-closes any previously-open one.
  onOpenDrawer: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const ageOk = ageMs(host.last_ok_at, now);
  const tone = toneFromAge(ageOk);
  const badge = toneBadge(tone);
  const gpus = stats?.gpus ?? [];

  // Sparkline metric set — a small but expressive subset of what
  // `/web/servers` renders. CPU and memory are always present; per-
  // GPU util gets one chart for GPU 0 with the rest overlaid as
  // lower-opacity series; NIC totals collapse all interfaces into one
  // rx+tx pair; coolant inlet1 stands in for the cooling system. We
  // intentionally cap at ~4 charts per card so the strip stays
  // readable at the half-pane copilot width.
  const sparkMetrics = useMemo(() => {
    const m = ["cpu.util", "mem.used_bytes"];
    for (const g of gpus) m.push(`gpu.${g.index}.util`);
    if (stats?.network && stats.network.length > 0) {
      // host_metrics writes per-iface bandwidth as
      // `nic.<iface>.{rx,tx}_bps` (bytes-per-second rate, see
      // `bundles/server-monitor/src/metrics.rs::stats_to_samples`).
      // We pull rx + tx for every NIC and sum across interfaces to
      // get host-wide totals — matches what `(shell)/servers/page.tsx`
      // surfaces in its bottom sparkline.
      for (const n of stats.network) {
        m.push(`nic.${n.iface}.rx_bps`);
        m.push(`nic.${n.iface}.tx_bps`);
      }
    }
    if (stats?.gadgetini) {
      m.push("cooling.coolant_inlet1_temp");
    }
    return m;
  }, [gpus, stats?.network, stats?.gadgetini]);
  const history = useHostMetricHistory(apiKey, host.id, sparkMetrics);
  // Memory series: convert raw `mem.used_bytes` to percent so the
  // chart's Y axis matches the CPU/GPU util axes (0–100). Lets the
  // 2-column layout share visual scale without per-chart axes.
  const memSeries = useMemo(() => {
    const raw = history["mem.used_bytes"] ?? [];
    if (!stats?.mem || raw.length === 0) return raw;
    return raw.map((p) => ({
      ...p,
      avg: (p.avg / stats.mem!.total_bytes) * 100,
    }));
  }, [history, stats?.mem]);
  const gpuUtilAvg =
    gpus.length > 0
      ? gpus
          .map((g) => g.util_pct)
          .filter((u): u is number => u != null)
          .reduce((a, b) => a + b, 0) /
        Math.max(1, gpus.filter((g) => g.util_pct != null).length)
      : 0;
  const hottestGpu = gpus
    .map((g) => g.temp_c)
    .filter((t): t is number => t != null)
    .reduce((m, v) => (v > m ? v : m), -Infinity);
  const totalW = gpus
    .map((g) => g.power_w)
    .filter((p): p is number => p != null)
    .reduce((a, b) => a + b, 0);
  const gpuHeader: string[] = [];
  if (Number.isFinite(hottestGpu))
    gpuHeader.push(`${hottestGpu.toFixed(0)}°C`);
  if (totalW > 0) gpuHeader.push(`${totalW.toFixed(0)}W`);

  // `apiKey` is required by HostDetailDrawer for live metric fetches
  // even when the operator is on a session-cookie path; passing it
  // through here so the drawer can hit `/workbench/servers/{id}/
  // metrics` with the same auth as the rest of the workbench.
  void apiKey;

  return (
    <article
      className="flex flex-col gap-2 rounded border border-zinc-800 bg-zinc-900 p-3 text-xs"
      data-testid="copilot-host-card"
      data-host-id={host.id}
    >
      <header className="flex items-start justify-between gap-2">
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold text-zinc-100">
            {host.alias || host.host}
          </div>
          {host.alias && (
            <div className="truncate font-mono text-[11px] text-zinc-500">
              {host.host}
            </div>
          )}
          {host.cpu_model && (
            <div className="truncate text-[11px] text-zinc-500">
              {shortenCpu(host.cpu_model)}
              {host.cpu_cores ? ` · ${host.cpu_cores}c` : ""}
            </div>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <span
            className={`rounded border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider ${badge.cls}`}
          >
            {badge.label}
          </span>
          {/* Inline expand — toggles per-GPU detail + cooling probes
            * inside the card. Stays in the copilot pane so the chat
            * thread stays visible. */}
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            aria-expanded={expanded}
            aria-label={expanded ? "Collapse host detail" : "Expand host detail"}
            title={expanded ? "Collapse" : "Expand inline"}
            className="rounded border border-zinc-800 px-1.5 py-0.5 text-[11px] text-zinc-400 hover:border-blue-700 hover:text-blue-300"
            data-testid="copilot-host-expand"
          >
            {expanded ? "▾" : "▸"}
          </button>
          {/* Drawer — opens the full host-detail-drawer with charts,
            * time-range selector, per-series toggles. Same drawer
            * /web/servers uses; this wires copilot into the rich
            * surface without leaving the page. */}
          <button
            type="button"
            onClick={onOpenDrawer}
            aria-label="Open detail drawer"
            title="Open detail drawer"
            className="rounded border border-zinc-800 px-1.5 py-0.5 text-[11px] text-zinc-400 hover:border-blue-700 hover:text-blue-300"
            data-testid="copilot-host-open-drawer"
          >
            ⇱
          </button>
        </div>
      </header>

      {stats?.cpu && (
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
      {gpus.length > 0 && (
        <div className="flex flex-col gap-1">
          <div className="flex items-center justify-between text-[11px]">
            <span className="font-mono font-semibold text-zinc-300">
              GPU × {gpus.length}
              {gpus[0].name ? ` — ${shortenGpuName(gpus[0].name)}` : ""}
            </span>
            <span className="font-mono text-zinc-300">
              {gpuHeader.join(" · ")}
            </span>
          </div>
          <ProgressBar pct={gpuUtilAvg} label="" tone="amber" />
        </div>
      )}
      {stats?.power?.psu_watts != null && (
        <StatRow label="PSU" value={`${stats.power.psu_watts.toFixed(0)}W`} />
      )}
      {stats?.gadgetini && <CoolingTile cooling={stats.gadgetini} />}

      {/* Inline expanded detail — per-GPU bars + extra cooling probes
        * + uptime + fleet load. Adds detail without leaving the chat
        * pane; for full charts use the drawer button. */}
      {expanded && stats && (
        <div
          className="flex flex-col gap-1 border-t border-zinc-800 pt-2"
          data-testid="copilot-host-expanded"
        >
          {gpus.length > 0 &&
            gpus.map((g) => {
              const tempPower: string[] = [];
              if (g.temp_c != null) tempPower.push(`${g.temp_c}°C`);
              if (g.power_w != null) tempPower.push(`${g.power_w.toFixed(0)}W`);
              const memPct =
                g.mem_used_mib != null && g.mem_total_mib != null
                  ? (g.mem_used_mib / g.mem_total_mib) * 100
                  : null;
              return (
                <div key={g.index} className="flex flex-col gap-0.5">
                  <div className="flex items-center justify-between text-[11px]">
                    <span className="truncate font-mono text-zinc-400">
                      GPU {g.index}
                      {g.name ? ` — ${shortenGpuName(g.name)}` : ""}
                    </span>
                    <span className="font-mono text-zinc-300">
                      {tempPower.join(" · ")}
                    </span>
                  </div>
                  {memPct != null && (
                    <StatRow
                      label="VRAM"
                      value={`${(g.mem_used_mib! / 1024).toFixed(1)} / ${(g.mem_total_mib! / 1024).toFixed(0)} GiB · ${memPct.toFixed(0)}%`}
                    />
                  )}
                  {g.util_pct != null && (
                    <ProgressBar pct={g.util_pct} label="" tone="amber" />
                  )}
                </div>
              );
            })}
          {stats.gadgetini &&
            (stats.gadgetini.coolant_temp_inlet1_c != null ||
              stats.gadgetini.coolant_temp_outlet1_c != null) && (
              <StatRow
                label="in→out"
                value={
                  <>
                    {stats.gadgetini.coolant_temp_inlet1_c?.toFixed(1) ?? "—"}
                    {" → "}
                    {stats.gadgetini.coolant_temp_outlet1_c?.toFixed(1) ?? "—"}
                    {"°C"}
                  </>
                }
              />
            )}
          {stats.gadgetini &&
            (stats.gadgetini.air_temp_c != null ||
              stats.gadgetini.air_humidity_pct != null) && (
              <StatRow
                label="air"
                value={
                  <>
                    {stats.gadgetini.air_temp_c != null
                      ? `${stats.gadgetini.air_temp_c.toFixed(0)}°C`
                      : "—"}
                    {stats.gadgetini.air_humidity_pct != null
                      ? ` · ${stats.gadgetini.air_humidity_pct.toFixed(0)}% RH`
                      : ""}
                  </>
                }
              />
            )}
          {stats.cpu && (
            <StatRow
              label="load"
              value={`${stats.cpu.load_1m.toFixed(2)} · ${stats.cpu.load_5m.toFixed(2)}`}
            />
          )}
          {stats.uptime_secs != null && (
            <StatRow
              label="uptime"
              value={fmtUptime(stats.uptime_secs)}
            />
          )}
        </div>
      )}

      {/* Sparkline strip — always visible, gives the card the
        * "live monitoring board" feel even before expanding. Each
        * chart is small (24 px tall) and shares the bottom-of-card
        * space; click ▾ for the inline detail view above. */}
      {(history["cpu.util"]?.length ?? 0) > 0 ||
      (history["mem.used_bytes"]?.length ?? 0) > 0 ? (
        <div
          className="grid grid-cols-2 gap-x-3 gap-y-1 border-t border-zinc-800 pt-2"
          data-testid="copilot-host-sparklines"
        >
          <Sparkline
            label="cpu (5m)"
            current={
              stats?.cpu ? `${stats.cpu.util_pct.toFixed(0)}%` : "—"
            }
            points={history["cpu.util"] ?? []}
            tone="blue"
            yMin={0}
            yMax={100}
          />
          <Sparkline
            label="mem (5m)"
            current={
              stats?.mem
                ? `${((stats.mem.used_bytes / stats.mem.total_bytes) * 100).toFixed(0)}%`
                : "—"
            }
            points={memSeries}
            tone="emerald"
            yMin={0}
            yMax={stats?.mem ? 100 : undefined}
          />
          {gpus.length > 0 && (() => {
            const primary = history[`gpu.${gpus[0].index}.util`] ?? [];
            const overlay = gpus
              .slice(1)
              .map((g) => history[`gpu.${g.index}.util`] ?? []);
            const utils = gpus
              .map((g) => g.util_pct)
              .filter((u): u is number => u != null);
            const currentText =
              gpus.length <= 2
                ? gpus
                    .map((g) =>
                      g.util_pct != null ? g.util_pct.toFixed(0) : "—",
                    )
                    .join("/")
                : utils.length > 0
                  ? `max ${Math.max(...utils).toFixed(0)} · avg ${(utils.reduce((a, b) => a + b, 0) / utils.length).toFixed(0)}`
                  : "—";
            return (
              <Sparkline
                label={`gpu util${gpus.length > 1 ? ` × ${gpus.length}` : ""} (5m)`}
                current={`${currentText}%`}
                points={primary}
                series={overlay}
                tone="amber"
                yMin={0}
                yMax={100}
              />
            );
          })()}
          {stats?.network && stats.network.length > 0 && (() => {
            // Aggregate per-iface rx and tx into two host-wide series.
            // `Sparkline` renders the primary series (rx) at full
            // opacity and the overlay (tx) at lower opacity on the
            // same y-axis, so a glance shows both directions and any
            // imbalance between them.
            const rxSum = sumSeriesByTs(
              stats.network.map(
                (n) => history[`nic.${n.iface}.rx_bps`] ?? [],
              ),
            );
            const txSum = sumSeriesByTs(
              stats.network.map(
                (n) => history[`nic.${n.iface}.tx_bps`] ?? [],
              ),
            );
            const rxNow = stats.network.reduce((a, n) => a + n.rx_bps, 0);
            const txNow = stats.network.reduce((a, n) => a + n.tx_bps, 0);
            return (
              <Sparkline
                label={`nic rx+tx (${stats.network.length}) (5m)`}
                current={`↓${fmtBps(rxNow)} · ↑${fmtBps(txNow)}`}
                points={rxSum}
                series={[txSum]}
                tone="zinc"
                yMin={0}
              />
            );
          })()}
          {stats?.gadgetini && (
            <Sparkline
              label="coolant (5m)"
              current={
                stats.gadgetini.coolant_temp_inlet1_c != null
                  ? `${stats.gadgetini.coolant_temp_inlet1_c.toFixed(1)}°C`
                  : "—"
              }
              points={history["cooling.coolant_inlet1_temp"] ?? []}
              tone="blue"
            />
          )}
        </div>
      ) : null}

      <footer className="mt-auto flex items-center justify-between gap-2 border-t border-zinc-800 pt-2 text-[11px] text-zinc-500">
        <span className="flex items-center gap-1.5">
          <span
            aria-hidden
            className={`inline-block size-2 rounded-full ${
              tone === "critical"
                ? "bg-red-500"
                : tone === "warning"
                  ? "bg-amber-500"
                  : "bg-emerald-500"
            }`}
          />
          <span className="font-mono">last ok {relativeAge(ageOk)}</span>
        </span>
        <a
          href={`/web/servers?host=${encodeURIComponent(host.id)}`}
          target="_blank"
          rel="noreferrer"
          className="font-mono text-blue-400 hover:underline"
          data-testid="copilot-host-open-dashboard"
        >
          open →
        </a>
      </footer>
    </article>
  );
}

function fmtUptime(secs: number | null): string {
  if (secs == null || !Number.isFinite(secs)) return "—";
  const days = Math.floor(secs / 86400);
  const hours = Math.floor((secs % 86400) / 3600);
  const mins = Math.floor((secs % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m`;
}

export function MonitoringGrid() {
  const { apiKey, identity } = useAuth();
  const [hosts, setHosts] = useState<HostRow[]>([]);
  const [statsByHost, setStatsByHost] = useState<
    Record<string, ServerStats | null>
  >({});
  const [loaded, setLoaded] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  /// Single drawer at the grid level — opening one host's drawer
  /// auto-closes any other. The drawer is a heavyweight component
  /// (recharts + per-host metric polling), so keeping it singleton
  /// is also a perf decision.
  const [drawerHostId, setDrawerHostId] = useState<string | null>(null);

  // Hosts list — single endpoint, 5 s cadence.
  useEffect(() => {
    if (!apiKey && !identity) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = async () => {
      const rows = await fetchHosts(apiKey);
      if (cancelled) return;
      setHosts(rows);
      setLoaded(true);
      timer = setTimeout(tick, POLL_HOSTS_MS);
    };
    void tick();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [apiKey, identity]);

  // Stats per host — fanout over the registered hosts. Each host's
  // poll loop is owned by the effect; when hosts change, the effect
  // re-runs and re-creates the timers.
  const hostIdsKey = useMemo(() => hosts.map((h) => h.id).join(","), [hosts]);
  useEffect(() => {
    if (!apiKey && !identity) return;
    if (hosts.length === 0) return;
    let cancelled = false;
    const timers: ReturnType<typeof setTimeout>[] = [];
    const pollOne = async (hostId: string) => {
      if (cancelled) return;
      const s = await fetchStats(apiKey, hostId);
      if (cancelled) return;
      setStatsByHost((prev) => ({ ...prev, [hostId]: s }));
      timers.push(setTimeout(() => void pollOne(hostId), POLL_STATS_MS));
    };
    for (const h of hosts) {
      void pollOne(h.id);
    }
    return () => {
      cancelled = true;
      for (const t of timers) clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [apiKey, identity, hostIdsKey]);

  // Stable freshness clock — keeps "Xs ago" labels accurate without
  // re-fetching. Cheap (one setState per second).
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  return (
    <section
      className="flex h-full flex-col overflow-hidden bg-zinc-950"
      data-testid="copilot-monitoring-grid"
      aria-label="Server monitoring grid"
    >
      <header className="flex h-9 shrink-0 items-center justify-between border-b border-zinc-800 px-3 text-xs font-mono text-zinc-400">
        <span>Servers</span>
        <span className="text-zinc-600">{hosts.length} registered</span>
      </header>
      <div className="flex-1 overflow-y-auto p-3">
        {!loaded && (
          <div className="text-[11px] text-zinc-500">loading hosts…</div>
        )}
        {loaded && hosts.length === 0 && (
          <div
            className="rounded border border-dashed border-zinc-800 bg-zinc-900/30 p-3 text-[11px] text-zinc-500"
            data-testid="copilot-monitoring-empty"
          >
            No hosts registered yet. Add one from{" "}
            <a
              href="/web/servers"
              className="text-blue-400 hover:underline"
              target="_blank"
              rel="noreferrer"
            >
              /web/servers
            </a>
            .
          </div>
        )}
        {loaded && hosts.length > 0 && (
          <div className="grid grid-cols-1 gap-2 @[720px]:grid-cols-2">
            {hosts.map((h) => (
              <HostCard
                key={h.id}
                host={h}
                stats={statsByHost[h.id] ?? null}
                now={now}
                apiKey={apiKey}
                onOpenDrawer={() => setDrawerHostId(h.id)}
              />
            ))}
          </div>
        )}
      </div>
      {(() => {
        if (drawerHostId == null) return null;
        const drawerHost = hosts.find((h) => h.id === drawerHostId);
        const drawerStats = statsByHost[drawerHostId] ?? null;
        if (!drawerHost) return null;
        // The host-detail-drawer needs an `available` shape that
        // enumerates the host's GPUs / NICs / temp chips so its
        // metric-group dropdown can render only what exists. Derive
        // from the most recent stats; if stats haven't arrived yet
        // pass conservative empty arrays — the drawer copes by
        // hiding empty groups.
        const available = {
          gpus: (drawerStats?.gpus ?? []).map((g) => ({
            index: g.index,
            name: g.name,
          })),
          nics: (drawerStats?.network ?? []).map((n) => n.iface),
          temps: (drawerStats?.temps ?? []).map((t) =>
            t.label && t.label !== t.chip ? `${t.chip}/${t.label}` : t.chip,
          ),
          cooling: drawerStats?.gadgetini != null,
        };
        return (
          <HostDetailDrawer
            open
            onClose={() => setDrawerHostId(null)}
            apiKey={apiKey}
            hostId={drawerHostId}
            hostLabel={drawerHost.alias || drawerHost.host}
            available={available}
            context={{ totalRamBytes: drawerStats?.mem?.total_bytes }}
          />
        );
      })()}
    </section>
  );
}
