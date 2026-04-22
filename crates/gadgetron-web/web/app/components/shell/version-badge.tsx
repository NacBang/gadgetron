"use client";

// Renders the running gadgetron version in the bottom-right corner as
// a small, unobtrusive badge. Value is inlined at build time from the
// workspace's `CARGO_PKG_VERSION` via `NEXT_PUBLIC_GADGETRON_VERSION`
// (see `crates/gadgetron-web/build_logic.rs`). Falls back to "dev"
// when the var is missing (local npm runs without cargo).

export function VersionBadge() {
  const version = process.env.NEXT_PUBLIC_GADGETRON_VERSION;
  if (!version) return null;
  return (
    <div
      className="pointer-events-none fixed bottom-2 right-3 z-40 select-none text-[10px] font-mono text-zinc-700"
      aria-hidden
      data-testid="version-badge"
    >
      v{version}
    </div>
  );
}
