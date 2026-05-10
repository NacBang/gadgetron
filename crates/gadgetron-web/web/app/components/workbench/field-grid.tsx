import { type ReactNode } from "react";

import { cn } from "@/lib/utils";

export function FieldGrid({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return <div className={cn("grid gap-3", className)}>{children}</div>;
}

export function FieldRow({
  label,
  htmlFor,
  help,
  error,
  children,
}: {
  label: string;
  htmlFor?: string;
  help?: ReactNode;
  error?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="grid gap-1.5 md:grid-cols-[180px_minmax(0,1fr)] md:items-start md:gap-3">
      <div className="pt-1">
        <label htmlFor={htmlFor} className="text-xs font-medium text-zinc-300">
          {label}
        </label>
        {help && <div className="mt-1 text-[11px] leading-4 text-zinc-500">{help}</div>}
      </div>
      <div className="min-w-0">
        {children}
        {error && <div className="mt-1 text-[11px] leading-4 text-red-300">{error}</div>}
      </div>
    </div>
  );
}
