import { type ReactNode } from "react";

import { cn } from "@/lib/utils";

export function PageToolbar({
  children,
  status,
  className,
}: {
  children?: ReactNode;
  status?: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex min-h-12 shrink-0 flex-wrap items-center justify-between gap-3 border-b border-zinc-800 bg-zinc-950/80 px-5 py-2",
        className,
      )}
    >
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">{children}</div>
      {status && <div className="flex shrink-0 items-center gap-2">{status}</div>}
    </div>
  );
}
