import { type ReactNode } from "react";

import { cn } from "@/lib/utils";

export function OperationalPanel({
  title,
  description,
  actions,
  notice,
  children,
  className,
}: {
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
  notice?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={cn("rounded-lg border border-zinc-800 bg-zinc-950/70", className)}>
      <div className="flex items-start justify-between gap-4 border-b border-zinc-800 px-4 py-3">
        <div className="min-w-0">
          <h2 className="text-sm font-medium text-zinc-100">{title}</h2>
          {description && <div className="mt-1 text-xs leading-5 text-zinc-400">{description}</div>}
        </div>
        {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
      </div>
      {notice && <div className="border-b border-zinc-800 px-4 py-3">{notice}</div>}
      <div className="p-4">{children}</div>
    </section>
  );
}
