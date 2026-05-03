"use client";

import { type ComponentType, type ReactNode, useState } from "react";
import { AlertCircle, CheckCircle2, Info, TriangleAlert } from "lucide-react";

import { cn } from "@/lib/utils";

export type NoticeTone = "info" | "warn" | "error" | "success";

const NOTICE_META: Record<
  NoticeTone,
  {
    className: string;
    icon: ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
  }
> = {
  info: {
    className: "border-sky-500/20 bg-sky-500/10 text-sky-100",
    icon: Info,
  },
  warn: {
    className: "border-amber-500/25 bg-amber-500/10 text-amber-100",
    icon: TriangleAlert,
  },
  error: {
    className: "border-red-500/25 bg-red-500/10 text-red-100",
    icon: AlertCircle,
  },
  success: {
    className: "border-emerald-500/25 bg-emerald-500/10 text-emerald-100",
    icon: CheckCircle2,
  },
};

export function InlineNotice({
  tone = "info",
  title,
  children,
  details,
  className,
}: {
  tone?: NoticeTone;
  title: string;
  children?: ReactNode;
  details?: ReactNode;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const meta = NOTICE_META[tone];
  const Icon = meta.icon;

  return (
    <div className={cn("rounded-lg border p-3", meta.className, className)}>
      <div className="flex items-start gap-2">
        <Icon className="mt-0.5 size-4 shrink-0" aria-hidden />
        <div className="min-w-0 flex-1">
          <div className="text-sm font-medium">{title}</div>
          {children && <div className="mt-1 text-xs leading-5 opacity-85">{children}</div>}
          {details && (
            <div className="mt-2">
              <button
                type="button"
                className="text-xs font-medium underline underline-offset-4 opacity-80 hover:opacity-100"
                onClick={() => setOpen((value) => !value)}
              >
                Details
              </button>
              {open && (
                <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded border border-current/15 bg-black/20 p-2 text-[11px] leading-4 opacity-90">
                  {details}
                </pre>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
