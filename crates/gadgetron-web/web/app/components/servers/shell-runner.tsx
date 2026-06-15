"use client";

// Floating modal that runs a bash command on one host via the
// server.bash gadget (mandatory confirm per call). Split out of
// /web/servers (ISSUE 54).

import { useCallback, useState } from "react";
import { Button } from "../ui/button";
import { Textarea } from "../ui/textarea";
import { invokeAction, unwrapPayload } from "../../lib/workbench-client";
import { useConfirm } from "../ui/confirm";
import type { Host } from "../../lib/server-types";

// ---------------------------------------------------------------------------
// Host card (grid cell)
// ---------------------------------------------------------------------------

// Floating modal — attaches to one host, runs a bash command via the
// server.bash gadget. Mandatory confirm dialog before dispatch (policy
// #2: every invocation requires an explicit operator click). Output is
// shown inline once the call completes.
export function ShellRunner({
  apiKey,
  host,
  onClose,
}: {
  apiKey: string | null;
  host: Host;
  onClose: () => void;
}) {
  const [cmd, setCmd] = useState("");
  const [useSudo, setUseSudo] = useState(false);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<
    | {
        code: number | null;
        stdout: string;
        stderr: string;
      }
    | null
  >(null);
  const label = host.alias ?? host.host;
  const confirm = useConfirm();

  const run = useCallback(async () => {
    const trimmed = cmd.trim();
    if (!trimmed || running) return;
    const preview = trimmed.length > 120 ? trimmed.slice(0, 117) + "..." : trimmed;
    const ok = await confirm({
      title: `Run on ${label}?`,
      description: `${useSudo ? "[sudo] " : ""}${preview}`,
      confirmLabel: "Run",
    });
    if (!ok) return;
    setRunning(true);
    setResult(null);
    try {
      const resp = await invokeAction(apiKey, "server-bash", {
        id: host.id,
        command: trimmed,
        use_sudo: useSudo,
      });
      const payload = unwrapPayload(resp) as
        | { code?: number | null; stdout?: string; stderr?: string }
        | undefined;
      setResult({
        code: payload?.code ?? null,
        stdout: payload?.stdout ?? "",
        stderr: payload?.stderr ?? "",
      });
    } catch (e) {
      setResult({ code: null, stdout: "", stderr: (e as Error).message });
    } finally {
      setRunning(false);
    }
  }, [apiKey, confirm, cmd, host.id, label, running, useSudo]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        className="flex max-h-[90vh] w-full max-w-2xl flex-col gap-3 overflow-hidden rounded border border-zinc-700 bg-zinc-950 p-4"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <div className="text-sm font-semibold text-zinc-100">
            Shell @ <span className="font-mono text-blue-300">{label}</span>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="text-zinc-500 hover:text-zinc-200"
          >
            ✕
          </button>
        </div>
        <div className="text-[11px] text-zinc-500">
          Every run requires a confirmation dialog. sudo only works when
          NOPASSWD is installed.
        </div>
        <Textarea
          value={cmd}
          onChange={(e) => setCmd(e.target.value)}
          onKeyDown={(e) => {
            if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
              e.preventDefault();
              void run();
            }
          }}
          placeholder="e.g. systemctl restart nvidia-dcgm && dmesg | tail -20"
          rows={4}
          className="font-mono text-xs"
          disabled={running}
        />
        <div className="flex items-center justify-between gap-2">
          <label className="flex items-center gap-1.5 text-[11px] text-zinc-400">
            <input
              type="checkbox"
              checked={useSudo}
              onChange={(e) => setUseSudo(e.target.checked)}
              className="accent-blue-500"
            />
            Run with sudo
          </label>
          <Button
            type="button"
            size="sm"
            onClick={() => void run()}
            disabled={running || cmd.trim().length === 0}
          >
            {running ? "Running..." : "Run (Ctrl+Enter)"}
          </Button>
        </div>
        {result && (
          <div className="flex-1 overflow-hidden">
            <div className="mb-1 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">
              exit {result.code ?? "—"}
            </div>
            <pre className="max-h-64 overflow-auto rounded bg-zinc-900/70 p-2 font-mono text-[11px] text-zinc-200 whitespace-pre-wrap break-all">
              {result.stdout || result.stderr || "(no output)"}
            </pre>
            {result.stderr && result.stdout && (
              <pre className="mt-1 max-h-32 overflow-auto rounded bg-red-950/40 p-2 font-mono text-[11px] text-red-300 whitespace-pre-wrap break-all">
                {result.stderr}
              </pre>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
