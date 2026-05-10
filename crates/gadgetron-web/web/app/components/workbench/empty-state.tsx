import { type ReactNode } from "react";

import { cn } from "@/lib/utils";

export function EmptyState({
  title,
  description,
  action,
  className,
}: {
  title: string;
  description: string;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <section
      className={cn(
        "flex min-h-40 flex-col items-start justify-center rounded-lg border border-dashed border-zinc-800 bg-zinc-950/40 p-6",
        className,
      )}
    >
      <h3 className="text-sm font-medium text-zinc-100">{title}</h3>
      <p className="mt-2 max-w-2xl text-sm leading-6 text-zinc-400">{description}</p>
      {action && <div className="mt-4">{action}</div>}
    </section>
  );
}
