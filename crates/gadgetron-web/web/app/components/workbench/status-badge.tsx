import { type ComponentType } from "react";
import {
  AlertCircle,
  CheckCircle2,
  CircleDashed,
  HelpCircle,
  LockKeyhole,
  Settings,
  WifiOff,
} from "lucide-react";

import { cn } from "@/lib/utils";

export type WorkbenchStatus =
  | "ready"
  | "healthy"
  | "degraded"
  | "offline"
  | "pending"
  | "needs_setup"
  | "unauthorized"
  | "unknown";

const STATUS_META: Record<
  WorkbenchStatus,
  {
    label: string;
    className: string;
    icon: ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
  }
> = {
  ready: {
    label: "Ready",
    className: "border-sky-500/30 bg-sky-500/10 text-sky-200",
    icon: CheckCircle2,
  },
  healthy: {
    label: "Healthy",
    className: "border-emerald-500/30 bg-emerald-500/10 text-emerald-200",
    icon: CheckCircle2,
  },
  degraded: {
    label: "Degraded",
    className: "border-amber-500/35 bg-amber-500/10 text-amber-200",
    icon: AlertCircle,
  },
  offline: {
    label: "Offline",
    className: "border-red-500/35 bg-red-500/10 text-red-200",
    icon: WifiOff,
  },
  pending: {
    label: "Pending",
    className: "border-zinc-500/30 bg-zinc-500/10 text-zinc-300",
    icon: CircleDashed,
  },
  needs_setup: {
    label: "Needs setup",
    className: "border-violet-500/30 bg-violet-500/10 text-violet-200",
    icon: Settings,
  },
  unauthorized: {
    label: "Unauthorized",
    className: "border-red-500/35 bg-red-500/10 text-red-200",
    icon: LockKeyhole,
  },
  unknown: {
    label: "Unknown",
    className: "border-zinc-600 bg-zinc-900 text-zinc-400",
    icon: HelpCircle,
  },
};

export function statusLabel(status: WorkbenchStatus): string {
  return STATUS_META[status].label;
}

export function StatusBadge({
  status,
  label,
  className,
}: {
  status: WorkbenchStatus;
  label?: string;
  className?: string;
}) {
  const meta = STATUS_META[status];
  const Icon = meta.icon;

  return (
    <span
      data-status={status}
      className={cn(
        "inline-flex h-6 shrink-0 items-center gap-1.5 rounded-md border px-2 text-[11px] font-medium leading-none",
        meta.className,
        className,
      )}
    >
      <Icon className="size-3" aria-hidden />
      {label ?? meta.label}
    </span>
  );
}
