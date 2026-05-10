export interface MetricSeriesPoint {
  ts: string;
  avg: number;
  min?: number;
  max?: number;
  samples?: number;
}

const DEFAULT_RATE_WINDOW_MS = 10_000;

export function counterToRollingRate(
  points: MetricSeriesPoint[],
  windowMs = DEFAULT_RATE_WINDOW_MS,
): MetricSeriesPoint[] {
  if (points.length === 0) return [];
  return points.map((point, i) => {
    const ts = new Date(point.ts).getTime();
    let base = points[Math.max(0, i - 1)];
    for (let j = i - 1; j >= 0; j--) {
      const candidateTs = new Date(points[j].ts).getTime();
      if (!Number.isFinite(candidateTs)) continue;
      if (ts - candidateTs > windowMs) break;
      base = points[j];
    }
    const baseTs = new Date(base.ts).getTime();
    const dtSeconds = (ts - baseTs) / 1000;
    const delta = point.avg - base.avg;
    const rate =
      dtSeconds > 0 && Number.isFinite(delta) && delta >= 0
        ? delta / dtSeconds
        : 0;
    return {
      ts: point.ts,
      avg: rate,
      min: rate,
      max: rate,
      samples: point.samples,
    };
  });
}
