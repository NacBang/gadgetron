import { type ReactNode } from "react";

import { cn } from "@/lib/utils";

export function WorkbenchPage({
  title,
  subtitle,
  actions,
  toolbar,
  children,
  className,
  headerTestId,
}: {
  title: string;
  subtitle?: ReactNode;
  actions?: ReactNode;
  toolbar?: ReactNode;
  children: ReactNode;
  className?: string;
  headerTestId?: string;
}) {
  return (
    <div className={cn("flex min-h-0 flex-1 flex-col overflow-hidden", className)}>
      <header
        className="shrink-0 border-b border-zinc-800 bg-zinc-950/90 px-5 py-4"
        data-testid={headerTestId}
      >
        <div className="flex min-w-0 items-start justify-between gap-4">
          <div className="min-w-0">
            <h1 className="truncate text-base font-semibold tracking-normal text-zinc-100">
              {title}
            </h1>
            {subtitle && <div className="mt-1 text-sm leading-5 text-zinc-400">{subtitle}</div>}
          </div>
          {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
        </div>
      </header>
      {toolbar}
      <div className="min-h-0 flex-1 overflow-auto p-5">{children}</div>
    </div>
  );
}
