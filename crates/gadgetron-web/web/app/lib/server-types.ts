// Shared server-monitor payload types (ISSUE 54). One source of truth
// for the shapes that /web/servers, the MonitoringGrid, and the
// HostDetailDrawer previously each re-declared.

export interface GadgetiniConfig {
  enabled: boolean;
  mode?: "usb" | "direct" | null;
  host_name?: string | null;
  ssh_user?: string | null;
  ssh_port?: number | null;
  parent_iface?: string | null;
  ipv6_link_local?: string | null;
  mac?: string | null;
  web_port?: number | null;
  last_ok_at?: string | null;
}

export interface Host {
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

export interface GpuStats {
  index: number;
  name: string;
  util_pct: number | null;
  mem_used_mib: number | null;
  mem_total_mib: number | null;
  temp_c: number | null;
  power_w: number | null;
  power_limit_w: number | null;
  source: string;
  // DCGM-only signals, surfaced as health badges.
  mem_temp_c?: number | null;
  ecc_dbe_total?: number | null;
  xid_last?: number | null;
  throttle_reasons?: number | null;
  throttle_reason_label?: string | null;
}

export interface NetworkStats {
  iface: string;
  rx_bps: number;
  tx_bps: number;
  rx_bytes_total: number;
  tx_bytes_total: number;
}

export interface GadgetiniStats {
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

export interface ServerStats {
  cpu: {
    util_pct: number;
    load_1m: number;
    load_5m: number;
    cores: number;
  } | null;
  mem: {
    total_bytes: number;
    used_bytes: number;
    available_bytes: number;
    swap_used_bytes: number;
    swap_total_bytes: number;
  } | null;
  disks: Array<{
    mount: string;
    fs: string;
    total_bytes: number;
    used_bytes: number;
  }>;
  temps: Array<{ chip: string; label: string; celsius: number }>;
  gpus: GpuStats[];
  power: { psu_watts: number | null; gpu_watts: number | null } | null;
  network: NetworkStats[];
  gadgetini?: GadgetiniStats | null;
  uptime_secs: number | null;
  fetched_at: string;
  warnings: string[];
}

/** Per-host polling state kept by the servers page. */
export type StatsMap = Record<
  string,
  {
    loading: boolean;
    stats?: ServerStats;
    error?: string;
    /** Wall-clock ms of the most recent server.stats HTTP round-trip. */
    lastFetchMs?: number;
    /** `performance.now()` when the most recent response completed —
     * drives the "updated Xs ago" label. */
    lastFetchedAt?: number;
  }
>;
