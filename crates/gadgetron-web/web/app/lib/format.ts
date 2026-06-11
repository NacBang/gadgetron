// Shared display formatters (ISSUE 54). These were copy-pasted between
// /web/servers and the MonitoringGrid with drifting behavior — the grid
// printed "KB/s" where the cards printed "KiB/s", and its shortenCpu
// stripped vendor names entirely ("AMD EPYC 7763" → "7763"). One
// canonical set: binary units, null-tolerant, vendor names kept.

export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MiB`;
  if (n < 1024 ** 4) return `${(n / 1024 ** 3).toFixed(1)} GiB`;
  return `${(n / 1024 ** 4).toFixed(1)} TiB`;
}

export function fmtBps(bps: number | null | undefined): string {
  if (bps == null || !Number.isFinite(bps)) return "—";
  if (bps < 1024) return `${bps.toFixed(0)} B/s`;
  if (bps < 1024 ** 2) return `${(bps / 1024).toFixed(1)} KiB/s`;
  if (bps < 1024 ** 3) return `${(bps / 1024 ** 2).toFixed(1)} MiB/s`;
  return `${(bps / 1024 ** 3).toFixed(2)} GiB/s`;
}

export function fmtUptime(secs: number | null | undefined): string {
  if (secs == null || !Number.isFinite(secs) || secs <= 0) return "—";
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return d > 0 ? `${d}d ${h}h` : h > 0 ? `${h}h ${m}m` : `${m}m`;
}

/** `"58/—"`-style pair for dual-probe readings (coolant inlets etc.). */
export function fmtPair(a?: number | null, b?: number | null): string {
  const fa = a != null ? a.toFixed(0) : "—";
  const fb = b != null ? b.toFixed(0) : "—";
  return `${fa}/${fb}`;
}

/**
 * Drops marketing fluff from lscpu model names so cards stay readable:
 * "AMD EPYC 7763 64-Core Processor" → "AMD EPYC 7763",
 * "Intel(R) Xeon(R) Gold 6248R CPU @ 3.00GHz" → "Intel Xeon Gold 6248R".
 * Keep the full name in tooltips.
 */
export function shortenCpu(model: string): string {
  return model
    .replace(/\([Rr]\)/g, "")
    .replace(/\([Tt][Mm]\)/g, "")
    .replace(/\s+CPU\s+@.*$/i, "")
    .replace(/\s+Processor$/i, "")
    .replace(/\s+\d+-Core$/i, "")
    .replace(/\s{2,}/g, " ")
    .trim();
}

/**
 * Trims a single GPU product name for tight rows: drops the "NVIDIA " /
 * "GeForce " prefixes, the "Server Edition" suffix, and the form-factor
 * tail so "NVIDIA GeForce RTX 4090" → "RTX 4090". Keep the full string
 * in tooltips.
 */
export function shortenGpuName(name: string): string {
  return name
    .replace(/^NVIDIA\s+/, "")
    .replace(/^GeForce\s+/, "")
    .replace(/\s+Server Edition$/, "")
    .replace(/\s+(SXM[0-9]?|PCIe)$/i, "")
    .trim();
}

/**
 * Collapses ["NVIDIA RTX 4090", ×3] into "3× RTX 4090"; mixed models
 * render as "2× X + 1× Y". The shared "NVIDIA " prefix is dropped when
 * every entry carries it.
 */
export function shortenGpuList(gpus: string[]): string {
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
