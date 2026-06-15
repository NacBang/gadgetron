"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Toaster, toast } from "sonner";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { HostDetailDrawer } from "../../components/host-detail-drawer";
import { TopologyGraphView } from "../../components/topology-graph";
import { ServerTileGrid } from "../../components/server-tile-grid";
import { ServerTable } from "../../components/server-table";
import { AddHostForm } from "../../components/servers/add-host-form";
import { HostCard } from "../../components/servers/host-card";
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
import { useConfirm } from "../../components/ui/confirm";
import { invokeAction, unwrapPayload } from "../../lib/workbench-client";
import type { Host, ServerStats, StatsMap } from "../../lib/server-types";

// ---------------------------------------------------------------------------
// /web/servers — server-monitor bundle UI.
//
// Three-mode add form (key_path / key_paste / password_bootstrap) on top,
// grid of registered hosts below. Each card polls `server.stats` every
// 5 seconds; clicking the card opens a detail sheet with per-GPU, per-disk,
// and per-chip temperature breakdowns.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

const POLL_INTERVAL_MS = 1000;

export default function ServersPage() {
  const { apiKey } = useAuth();
  const confirm = useConfirm();
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
  const [view, setView] = useState<"cards" | "tiles" | "graph" | "table">(
    "cards",
  );
  const [topology, setTopology] = useState<TopologyGraph | null>(null);
  const [topologyError, setTopologyError] = useState<string | null>(null);
  // Shared filter / sort bar (ISSUE 48) — applies to cards, tiles, and
  // the table view (every non-graph view).
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
      if (!(await confirm({ title: `Remove ${host}?`, tone: "danger", confirmLabel: "Remove" }))) return;
      try {
        await invokeAction(apiKey, "server-remove", { id });
        toast.success(`Removed ${host}`);
        await refreshList();
      } catch (e) {
        toast.error("server.remove failed", { description: (e as Error).message });
      }
    },
    [apiKey, confirm, refreshList],
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
            {(["cards", "tiles", "table", "graph"] as const).map((v) => (
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
                {v === "cards"
                  ? "Cards"
                  : v === "tiles"
                    ? "Tiles"
                    : v === "table"
                      ? "Table"
                      : "Graph"}
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
                placeholder="Search — alias · host · GPU model"
                className="h-7 w-60 font-mono text-xs"
                data-testid="servers-filter-query"
              />
              <div className="flex items-center gap-1">
                {(
                  [
                    ["all", "All"],
                    ["online", "Online"],
                    ["offline", "Offline"],
                    ["warn", "Warnings"],
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
                Sort
                <select
                  value={sortKey}
                  onChange={(e) => setSortKey(e.target.value as ServerSortKey)}
                  data-testid="servers-sort-select"
                  className="rounded border border-zinc-800 bg-zinc-900 px-1.5 py-0.5 text-[11px] text-zinc-300"
                >
                  <option value="name">Name</option>
                  <option value="cpu">CPU util</option>
                  <option value="gpu">GPU util</option>
                  <option value="temp">GPU temp</option>
                  <option value="warn">Warnings</option>
                </select>
              </label>
              {(query.trim() !== "" || statusFilter !== "all") && (
                <span
                  className="text-[11px] text-zinc-600"
                  data-testid="servers-filter-count"
                >
                  {hostList.length}/{hosts.length} shown
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
        ) : view === "table" ? (
          <section data-testid="table-section">
            <ServerTable
              hosts={hostList}
              fleet={fleet}
              sortKey={sortKey}
              onSortChange={setSortKey}
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
                  title="No servers match the current filters"
                  description="Adjust the search or status filters."
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
