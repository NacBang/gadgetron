"use client";

import { Menu, PanelRight, X } from "lucide-react";
import { usePathname } from "next/navigation";
import { useCallback, useEffect, useRef, useState } from "react";

import {
  Dialog,
  DialogContent,
  DialogTitle,
} from "@/components/ui/dialog";
import { EvidencePane } from "./evidence-pane";
import { LeftRail } from "./left-rail";

export function ResponsiveShellControls({
  mobile,
  narrow,
  viewportWidth,
  showInspector = true,
}: {
  mobile: boolean;
  narrow: boolean;
  viewportWidth: number;
  showInspector?: boolean;
}) {
  const pathname = usePathname();
  const [navigationOpen, setNavigationOpen] = useState(false);
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const navigationTrigger = useRef<HTMLButtonElement>(null);
  const inspectorTrigger = useRef<HTMLButtonElement>(null);

  const changeNavigation = useCallback((open: boolean) => {
    setNavigationOpen(open);
    if (!open) window.setTimeout(() => navigationTrigger.current?.focus(), 0);
  }, []);
  const changeInspector = useCallback((open: boolean) => {
    setInspectorOpen(open);
    if (!open) window.setTimeout(() => inspectorTrigger.current?.focus(), 0);
  }, []);

  useEffect(() => {
    setNavigationOpen(false);
    setInspectorOpen(false);
  }, [pathname]);

  if (!narrow || (!mobile && !showInspector)) return null;

  const drawerNavigationWidth = Math.min(
    320,
    Math.max(280, viewportWidth - 16),
  );
  const drawerInspectorWidth = Math.min(
    416,
    Math.max(280, viewportWidth - 16),
  );

  return (
    <>
      <div
        className="flex h-10 shrink-0 items-center justify-between border-b border-zinc-800 bg-zinc-950 px-2"
        data-testid="responsive-shell-toolbar"
        aria-label="Workspace controls"
      >
        {mobile ? (
          <button
            ref={navigationTrigger}
            type="button"
            className="flex h-8 items-center gap-2 rounded px-2 text-xs font-medium text-zinc-300 hover:bg-zinc-800 hover:text-zinc-100"
            onClick={() => changeNavigation(true)}
            aria-label="Open navigation"
          >
            <Menu className="size-4" aria-hidden />
            Menu
          </button>
        ) : (
          <span />
        )}
        {showInspector && (
          <button
            ref={inspectorTrigger}
            type="button"
            className="flex h-8 items-center gap-2 rounded px-2 text-xs font-medium text-zinc-300 hover:bg-zinc-800 hover:text-zinc-100"
            onClick={() => changeInspector(true)}
            aria-label="Open inspector"
          >
            <PanelRight className="size-4" aria-hidden />
            Inspector
          </button>
        )}
      </div>

      {mobile && (
        <Dialog open={navigationOpen} onOpenChange={changeNavigation}>
          <DialogContent
            showCloseButton={false}
            data-testid="navigation-drawer"
            className="left-0 top-0 h-dvh max-w-none translate-x-0 translate-y-0 grid-rows-[3rem_minmax(0,1fr)] gap-0 rounded-none border-r border-zinc-800 p-0 sm:max-w-none"
            style={{ width: drawerNavigationWidth }}
          >
            <div className="flex items-center border-b border-zinc-800 px-3">
              <DialogTitle className="text-sm font-semibold text-zinc-100">
                Navigation
              </DialogTitle>
              <button
                type="button"
                className="ml-auto flex size-8 items-center justify-center rounded text-zinc-500 hover:bg-zinc-800 hover:text-zinc-200"
                onClick={() => changeNavigation(false)}
                aria-label="Close navigation"
              >
                <X className="size-4" aria-hidden />
              </button>
            </div>
            <div
              className="min-h-0 overflow-hidden"
              onClickCapture={(event) => {
                if ((event.target as Element).closest("a[href]")) {
                  setNavigationOpen(false);
                }
              }}
            >
              <LeftRail
                collapsed={false}
                onCollapse={() => undefined}
                width={drawerNavigationWidth}
                showCollapseControl={false}
              />
            </div>
          </DialogContent>
        </Dialog>
      )}

      {showInspector && (
        <Dialog open={inspectorOpen} onOpenChange={changeInspector}>
          <DialogContent
            showCloseButton={false}
            data-testid="inspector-drawer"
            className="bottom-0 left-auto right-0 top-0 h-dvh max-w-none translate-x-0 translate-y-0 gap-0 rounded-none border-l border-zinc-800 p-0 sm:max-w-none"
            style={{ width: drawerInspectorWidth }}
          >
            <DialogTitle className="sr-only">Inspector</DialogTitle>
            <EvidencePane
              open
              width={drawerInspectorWidth}
              onToggle={(open) => {
                if (!open) changeInspector(false);
              }}
            />
          </DialogContent>
        </Dialog>
      )}
    </>
  );
}
