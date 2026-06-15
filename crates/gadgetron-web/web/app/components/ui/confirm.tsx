"use client";

// Promise-based confirm dialog (ISSUE 58). Replaces the scattered
// window.confirm() calls — those render the browser's native chrome,
// which breaks the product look and can't be styled or tested. Usage:
//
//   const confirm = useConfirm();
//   if (!(await confirm({ title: "Delete user?", tone: "danger" }))) return;
//
// One provider at the shell root owns a single dialog; each call resolves
// the boolean when the operator picks an action (or dismisses → false).

import {
  createContext,
  useCallback,
  useContext,
  useRef,
  useState,
  type ReactNode,
} from "react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./dialog";
import { Button } from "./button";

export interface ConfirmOptions {
  title: string;
  /** Optional supporting line(s) under the title. */
  description?: ReactNode;
  /** Confirm-button label. Default "Confirm". */
  confirmLabel?: string;
  /** Cancel-button label. Default "Cancel". */
  cancelLabel?: string;
  /** "danger" styles the confirm button destructive (irreversible ops). */
  tone?: "default" | "danger";
}

type ConfirmFn = (opts: ConfirmOptions) => Promise<boolean>;

const ConfirmCtx = createContext<ConfirmFn | null>(null);

export function useConfirm(): ConfirmFn {
  const ctx = useContext(ConfirmCtx);
  // Fallback to the native confirm when no provider is mounted (unit
  // tests rendering a page in isolation, or any future surface outside
  // the shell). The shell always provides the styled dialog.
  return ctx ?? nativeConfirm;
}

const nativeConfirm: ConfirmFn = (opts) =>
  Promise.resolve(
    typeof window === "undefined"
      ? false
      : window.confirm(
          opts.description
            ? `${opts.title}\n\n${typeof opts.description === "string" ? opts.description : ""}`
            : opts.title,
        ),
  );

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const [opts, setOpts] = useState<ConfirmOptions | null>(null);
  const resolverRef = useRef<((value: boolean) => void) | null>(null);

  const confirm = useCallback<ConfirmFn>((next) => {
    setOpts(next);
    setOpen(true);
    return new Promise<boolean>((resolve) => {
      resolverRef.current = resolve;
    });
  }, []);

  const settle = useCallback((value: boolean) => {
    resolverRef.current?.(value);
    resolverRef.current = null;
    setOpen(false);
  }, []);

  return (
    <ConfirmCtx.Provider value={confirm}>
      {children}
      <Dialog
        open={open}
        // Dismiss (Esc / overlay / X) resolves false — same as Cancel.
        onOpenChange={(next) => {
          if (!next) settle(false);
        }}
      >
        <DialogContent className="max-w-sm" data-testid="confirm-dialog">
          <DialogHeader>
            <DialogTitle>{opts?.title}</DialogTitle>
            {opts?.description && (
              <DialogDescription className="whitespace-pre-wrap">
                {opts.description}
              </DialogDescription>
            )}
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              size="sm"
              onClick={() => settle(false)}
              data-testid="confirm-cancel"
            >
              {opts?.cancelLabel ?? "Cancel"}
            </Button>
            <Button
              variant={opts?.tone === "danger" ? "destructive" : "default"}
              size="sm"
              onClick={() => settle(true)}
              data-testid="confirm-accept"
              autoFocus
            >
              {opts?.confirmLabel ?? "Confirm"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </ConfirmCtx.Provider>
  );
}
