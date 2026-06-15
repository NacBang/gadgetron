"use client";

// Attach / edit / detach the gadgetini child board for a single host —
// wraps the `server-update` action's `gadgetini` field. Split out of
// /web/servers (ISSUE 54).

import { useCallback, useState } from "react";
import { toast } from "sonner";
import { X } from "lucide-react";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { invokeAction } from "../../lib/workbench-client";
import { useConfirm } from "../ui/confirm";
import type { Host } from "../../lib/server-types";

// ---------------------------------------------------------------------------
// Gadgetini manager — attach / edit / detach the gadgetini child board
// for a single host without going through full server-add. Wraps the
// `server-update` action's `gadgetini` field.
// ---------------------------------------------------------------------------

export function GadgetiniManager({
  apiKey,
  host,
  onClose,
  onSaved,
}: {
  apiKey: string | null;
  host: Host;
  onClose: () => void;
  onSaved: () => void;
}) {
  const initial = host.gadgetini ?? null;
  // Inventory.json may have records without `mode` (pre-Direct-mode
  // deployments) — treat absent as "usb" so the radio defaults match the
  // record's actual behavior.
  // biome-ignore lint/suspicious/noExplicitAny: legacy field access
  const initialMode = ((initial as any)?.mode as string | undefined) ?? "usb";
  const [enabled, setEnabled] = useState<boolean>(initial?.enabled ?? true);
  const [mode, setMode] = useState<"usb" | "direct">(
    initialMode === "direct" ? "direct" : "usb",
  );
  const [hostName, setHostName] = useState<string>(initial?.host_name ?? "");
  // USB-mode override: name of the parent-host iface that talks to the
  // gadgetini. Defaults to `usb0`; some chassis expose the link as
  // `enpXsYf1np1` etc. Empty string means "use server-side default".
  const [parentIface, setParentIface] = useState<string>(initial?.parent_iface ?? "");
  const [setPassword, setSetPassword] = useState<boolean>(false);
  const [password, setPassword2] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const label = host.alias ?? host.host;
  const confirm = useConfirm();
  const hasExisting = initial !== null;

  const save = useCallback(async () => {
    if (busy) return;
    if (mode === "direct" && !hostName.trim()) {
      toast.error("Direct mode requires gadgetini IP / hostname");
      return;
    }
    setBusy(true);
    try {
      const gadgetini: Record<string, unknown> = { enabled, mode };
      if (mode === "direct") {
        gadgetini.host_name = hostName.trim();
      } else if (parentIface.trim()) {
        gadgetini.parent_iface = parentIface.trim();
      }
      if (setPassword && password.trim()) {
        gadgetini.password = password;
      }
      await invokeAction(apiKey, "server-update", {
        id: host.id,
        gadgetini,
      });
      toast.success(
        hasExisting ? `Gadgetini updated on ${label}` : `Gadgetini attached to ${label}`,
      );
      onSaved();
      onClose();
    } catch (e) {
      toast.error("Gadgetini setup failed", { description: (e as Error).message });
    } finally {
      setBusy(false);
    }
  }, [apiKey, busy, enabled, hasExisting, host.id, hostName, label, mode, onClose, onSaved, password, setPassword]);

  const detach = useCallback(async () => {
    if (busy) return;
    if (
      !(await confirm({
        title: `Detach gadgetini from ${label}?`,
        description:
          "The gadgetini's redis-side data is left untouched; gadgetron just stops collecting from it. The SSH key on the gadgetini remains installed.",
        confirmLabel: "Detach",
      }))
    ) {
      return;
    }
    setBusy(true);
    try {
      await invokeAction(apiKey, "server-update", {
        id: host.id,
        gadgetini: null,
      });
      toast.success(`Gadgetini detached from ${label}`);
      onSaved();
      onClose();
    } catch (e) {
      toast.error("Gadgetini detach failed", { description: (e as Error).message });
    } finally {
      setBusy(false);
    }
  }, [apiKey, busy, host.id, label, onClose, onSaved]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        className="flex w-full max-w-md flex-col gap-3 rounded border border-zinc-700 bg-zinc-950 p-4"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <div className="text-sm font-semibold text-zinc-100">
            Gadgetini @ <span className="font-mono text-blue-300">{label}</span>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            onClick={onClose}
            aria-label="Close"
            className="text-zinc-500"
          >
            <X aria-hidden />
          </Button>
        </div>
        <div className="text-[10px] text-zinc-500">
          {hasExisting
            ? "Update connection settings, change mode, or rotate the bootstrap password."
            : "Attach a gadgetini to this host. Choose USB (parent-proxied) or Direct (own IP)."}
        </div>
        <label className="flex items-center gap-2 text-[11px] text-zinc-300">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
            className="accent-blue-500"
          />
          Enabled (collect stats every poll cycle)
        </label>
        <div className="flex flex-col gap-1">
          <span className="text-[10px] uppercase tracking-wide text-zinc-500">
            Connection
          </span>
          <div className="flex items-center gap-3 text-[11px] text-zinc-300">
            <label className="flex items-center gap-1.5">
              <input
                type="radio"
                name="gadgetini-mgr-mode"
                value="usb"
                checked={mode === "usb"}
                onChange={() => setMode("usb")}
                className="accent-blue-500"
              />
              USB (parent host)
            </label>
            <label className="flex items-center gap-1.5">
              <input
                type="radio"
                name="gadgetini-mgr-mode"
                value="direct"
                checked={mode === "direct"}
                onChange={() => setMode("direct")}
                className="accent-blue-500"
              />
              Direct IP
            </label>
          </div>
          <p className="text-[10px] text-zinc-600">
            {mode === "usb"
              ? "SSH lands on the parent host over LAN, then proxies via `nc -6 fd12:3456:789a:1::2 22` over the parent's USB-CDC iface to the gadgetini."
              : "SSH directly to the gadgetini's own IP. host_name required."}
          </p>
        </div>
        {mode === "direct" && (
          <div className="flex flex-col gap-1">
            <span className="text-[10px] uppercase tracking-wide text-zinc-500">
              Gadgetini host
            </span>
            <Input
              type="text"
              placeholder="e.g. 192.168.10.42"
              value={hostName}
              onChange={(e) => setHostName(e.target.value)}
              className="font-mono text-xs"
            />
          </div>
        )}
        {mode === "usb" && (
          <div className="flex flex-col gap-1">
            <span className="text-[10px] uppercase tracking-wide text-zinc-500">
              Parent USB iface
            </span>
            <Input
              type="text"
              placeholder="usb0 (default) — or enp3s0f1np1, etc."
              value={parentIface}
              onChange={(e) => setParentIface(e.target.value)}
              className="font-mono text-xs"
            />
            <p className="text-[10px] text-zinc-600">
              Name of the parent's USB-CDC ethernet iface. Differs by
              chassis; check with <code className="text-zinc-500">ip -6 route get fd12:3456:789a:1::2</code> on the parent if unsure.
            </p>
          </div>
        )}
        <label className="flex items-center gap-2 text-[11px] text-zinc-300">
          <input
            type="checkbox"
            checked={setPassword}
            onChange={(e) => setSetPassword(e.target.checked)}
            className="accent-blue-500"
          />
          Custom password (re-bootstrap)
        </label>
        {setPassword && (
          <Input
            type="password"
            placeholder="Gadgetini password"
            value={password}
            onChange={(e) => setPassword2(e.target.value)}
            className="font-mono text-xs"
          />
        )}
        <p className="text-[10px] text-zinc-600">
          Password is only used to push a fresh ed25519 key to the
          gadgetini, never stored. Falls back to{" "}
          <code className="text-zinc-500">$GADGETRON_GADGETINI_FACTORY_PASSWORD</code>{" "}
          when not provided.
        </p>
        <div className="mt-1 flex items-center justify-between gap-2">
          {hasExisting ? (
            <Button
              type="button"
              variant="destructive"
              size="sm"
              onClick={detach}
              disabled={busy}
            >
              Detach
            </Button>
          ) : (
            <span />
          )}
          <div className="flex items-center gap-2">
            <Button onClick={onClose} variant="outline" size="sm">
              Cancel
            </Button>
            <Button onClick={save} disabled={busy} size="sm">
              {busy ? "Saving…" : hasExisting ? "Save" : "Attach"}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
