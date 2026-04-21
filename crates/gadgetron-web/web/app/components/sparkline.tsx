"use client";

import { useMemo } from "react";

// Plain-SVG sparkline. No external charting dep — keeps the bundle
// lean and the bytes shipped per host card under 2 KB. The detail
// drawer (task #24) can opt in to a heavier lib like recharts.

export interface SparkPoint {
  ts: string;
  avg: number;
  min?: number;
  max?: number;
}

interface SparklineProps {
  points: SparkPoint[];
  /** Height in pixels — width auto-stretches to the parent. */
  height?: number;
  /** Reserve a vertical range so a flat line doesn't fill the box.
   * `undefined` autoscales to data extent. */
  yMin?: number;
  yMax?: number;
  /** Tailwind color class for the line/area. */
  tone?: "blue" | "amber" | "emerald" | "red" | "zinc";
  /** Tooltip-friendly label (rendered above the SVG). */
  label?: string;
  /** Right-aligned current value text. Caller formats. */
  current?: string;
}

const TONE: Record<NonNullable<SparklineProps["tone"]>, string> = {
  blue: "stroke-blue-400 fill-blue-400/10",
  amber: "stroke-amber-400 fill-amber-400/10",
  emerald: "stroke-emerald-400 fill-emerald-400/10",
  red: "stroke-red-400 fill-red-400/10",
  zinc: "stroke-zinc-500 fill-zinc-500/10",
};

export function Sparkline({
  points,
  height = 24,
  yMin,
  yMax,
  tone = "blue",
  label,
  current,
}: SparklineProps) {
  const path = useMemo(() => {
    if (points.length < 2) return null;
    const values = points.map((p) => p.avg);
    const lo = yMin ?? Math.min(...values);
    const hi = yMax ?? Math.max(...values);
    const range = hi - lo || 1;
    const w = 100; // SVG viewBox units; CSS scales to container width
    const h = height;
    const dx = w / (points.length - 1);
    const xy = points.map((p, i) => {
      const x = i * dx;
      const y = h - ((p.avg - lo) / range) * h;
      return [x, Number.isFinite(y) ? y : h] as const;
    });
    const linePath =
      "M " + xy.map(([x, y]) => `${x.toFixed(2)},${y.toFixed(2)}`).join(" L ");
    const areaPath = `${linePath} L ${w},${h} L 0,${h} Z`;
    return { linePath, areaPath, w, h };
  }, [points, height, yMin, yMax]);

  return (
    <div className="flex flex-col gap-0.5" data-testid="sparkline">
      {(label || current) && (
        <div className="flex items-center justify-between text-[10px]">
          {label && <span className="text-zinc-500">{label}</span>}
          {current && (
            <span className="font-mono text-zinc-300">{current}</span>
          )}
        </div>
      )}
      {path ? (
        <svg
          viewBox={`0 0 ${path.w} ${path.h}`}
          preserveAspectRatio="none"
          className="w-full"
          style={{ height: `${height}px` }}
        >
          <path d={path.areaPath} className={`stroke-0 ${TONE[tone]}`} />
          <path
            d={path.linePath}
            className={TONE[tone]}
            fill="none"
            strokeWidth={1.4}
            vectorEffect="non-scaling-stroke"
          />
        </svg>
      ) : (
        <div
          className="w-full rounded bg-zinc-900/50"
          style={{ height: `${height}px` }}
          aria-label="not enough data yet"
        />
      )}
    </div>
  );
}
