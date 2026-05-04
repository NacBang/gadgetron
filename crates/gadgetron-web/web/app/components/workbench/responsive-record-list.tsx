import { type ReactNode } from "react";

import { cn } from "@/lib/utils";

export function ResponsiveRecordList({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return <div className={cn("grid gap-3", className)}>{children}</div>;
}

export function RecordRow({
  title,
  meta,
  status,
  actions,
  children,
  className,
}: {
  title: ReactNode;
  meta?: ReactNode;
  status?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <article className={cn("rounded-lg border border-zinc-800 bg-zinc-950/60 p-4", className)}>
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="truncate text-sm font-medium text-zinc-100">{title}</div>
          {meta && <div className="mt-1 text-xs leading-5 text-zinc-500">{meta}</div>}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {status}
          {actions}
        </div>
      </div>
      {children && <div className="mt-3">{children}</div>}
    </article>
  );
}
