"use client";

// Three-mode host registration form (key_path / key_paste /
// password_bootstrap) with the client-side bootstrap progress
// animation. Split out of /web/servers (ISSUE 54).

import { useState } from "react";
import { toast } from "sonner";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Textarea } from "../ui/textarea";
import { invokeAction, unwrapPayload } from "../../lib/workbench-client";

// ---------------------------------------------------------------------------
// Add-host form
// ---------------------------------------------------------------------------

type AuthMode = "key_path" | "key_paste" | "password_bootstrap";

type StepStatus = "pending" | "running" | "ok" | "failed" | "skipped";

interface ProgressStep {
  key: string;
  label: string;
  /** Expected wall-clock in ms. Client uses this to advance the
   * animated cursor; the final server response still overrides. */
  etaMs: number;
}

const STEPS_KEY_PATH: ProgressStep[] = [
  { key: "verify_key", label: "Verifying SSH key", etaMs: 2000 },
  { key: "save_inventory", label: "Saving to inventory", etaMs: 200 },
];

const STEPS_KEY_PASTE: ProgressStep[] = [
  { key: "write_key", label: "Writing private key (0600)", etaMs: 100 },
  { key: "verify_key", label: "Verifying SSH key", etaMs: 2000 },
  { key: "save_inventory", label: "Saving to inventory", etaMs: 200 },
];

const STEPS_PW_BOOTSTRAP: ProgressStep[] = [
  { key: "keygen", label: "Generating ed25519 keypair", etaMs: 500 },
  { key: "authorize", label: "Pushing public key to authorized_keys", etaMs: 2500 },
  { key: "sudoers", label: "Installing /etc/sudoers.d/gadgetron-monitor", etaMs: 3000 },
  { key: "apt_base", label: "apt-get install lm-sensors smartmontools ipmitool", etaMs: 25000 },
  { key: "detect_gpu", label: "Detecting NVIDIA GPU", etaMs: 1000 },
  { key: "dcgm", label: "Installing DCGM (if GPU present)", etaMs: 45000 },
  { key: "verify_key", label: "Verifying key-only login", etaMs: 3000 },
  { key: "save_inventory", label: "Saving to inventory", etaMs: 200 },
];

function stepsFor(mode: AuthMode): ProgressStep[] {
  if (mode === "key_path") return STEPS_KEY_PATH;
  if (mode === "key_paste") return STEPS_KEY_PASTE;
  return STEPS_PW_BOOTSTRAP;
}

function StepRow({
  step,
  status,
  elapsed,
}: {
  step: ProgressStep;
  status: StepStatus;
  elapsed: number | null;
}) {
  const glyph =
    status === "ok"
      ? "✓"
      : status === "failed"
        ? "✕"
        : status === "running"
          ? "◐"
          : status === "skipped"
            ? "·"
            : "○";
  const tone =
    status === "ok"
      ? "text-emerald-400"
      : status === "failed"
        ? "text-red-400"
        : status === "running"
          ? "text-amber-400 animate-pulse"
          : status === "skipped"
            ? "text-zinc-600"
            : "text-zinc-700";
  return (
    <li
      data-testid={`bootstrap-step-${step.key}`}
      data-status={status}
      className="flex items-center gap-2 py-0.5 text-[11px]"
    >
      <span className={`w-4 text-center font-mono ${tone}`}>{glyph}</span>
      <span className={status === "ok" || status === "failed" ? "text-zinc-300" : "text-zinc-500"}>
        {step.label}
      </span>
      {elapsed != null && (
        <span className="font-mono text-[10px] text-zinc-600">
          {(elapsed / 1000).toFixed(1)}s
        </span>
      )}
    </li>
  );
}

export function AddHostForm({
  apiKey,
  onAdded,
  collapsed,
  onCollapsedChange,
}: {
  apiKey: string | null;
  onAdded: () => void;
  collapsed: boolean;
  onCollapsedChange: (next: boolean) => void;
}) {
  const [host, setHost] = useState("");
  const [user, setUser] = useState("");
  const [port, setPort] = useState(22);
  const [mode, setMode] = useState<AuthMode>("password_bootstrap");
  const [keyPath, setKeyPath] = useState("~/.ssh/id_ed25519");
  const [keyPaste, setKeyPaste] = useState("");
  const [sshPw, setSshPw] = useState("");
  const [sudoPw, setSudoPw] = useState("");
  const [includeGadgetini, setIncludeGadgetini] = useState(false);
  const [gadgetiniMode, setGadgetiniMode] = useState<"usb" | "direct">("usb");
  const [gadgetiniHostName, setGadgetiniHostName] = useState("");
  const [customGadgetiniPassword, setCustomGadgetiniPassword] = useState(false);
  const [gadgetiniPassword, setGadgetiniPassword] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [progress, setProgress] = useState<Array<{ status: StepStatus; elapsed: number | null }>>(
    [],
  );

  const steps = stepsFor(mode);

  const submit = async () => {
    if (!host.trim() || !user.trim()) {
      toast.error("host and ssh_user are required");
      return;
    }
    const args: Record<string, unknown> = {
      host: host.trim(),
      ssh_user: user.trim(),
      ssh_port: port,
      auth_mode: mode,
    };
    if (mode === "key_path") args.ssh_key_path = keyPath.trim();
    if (mode === "key_paste") args.ssh_private_key = keyPaste;
    if (mode === "password_bootstrap") {
      args.ssh_password = sshPw;
      args.sudo_password = sudoPw;
    }
    setSubmitting(true);

    // Client-side progress animation. Advances through the predefined
    // step list at the ETA cadence; real server response at the end
    // finalizes the checklist (remaining steps → ok, skipped DCGM →
    // skipped, failed step → failed with error detail below).
    const now = () => performance.now();
    const startedAt = now();
    const stepStartedAt: number[] = steps.map(() => 0);
    setProgress(steps.map(() => ({ status: "pending", elapsed: null })));
    let cursor = 0;
    const advance = () => {
      if (cursor >= steps.length) return;
      stepStartedAt[cursor] = now();
      setProgress((prev) => {
        const next = prev.slice();
        next[cursor] = { status: "running", elapsed: null };
        return next;
      });
    };
    advance();
    const timers: number[] = [];
    let cumulative = 0;
    for (let i = 0; i < steps.length - 1; i++) {
      cumulative += steps[i].etaMs;
      const idx = i;
      const t = window.setTimeout(() => {
        setProgress((prev) => {
          const next = prev.slice();
          next[idx] = { status: "ok", elapsed: now() - stepStartedAt[idx] };
          return next;
        });
        cursor = idx + 1;
        advance();
      }, cumulative);
      timers.push(t);
    }

    try {
      const resp = await invokeAction(apiKey, "server-add", args);
      const payload = unwrapPayload(resp) as {
        id: string;
        bootstrap: {
          installed_pkgs?: string[];
          skipped_pkgs?: string[];
          gpu_detected?: boolean;
          dcgm_enabled?: boolean;
          notes?: string[];
        };
      };
      let gadgetiniError: string | null = null;
      if (includeGadgetini) {
        const gadgetini: Record<string, unknown> = {
          enabled: true,
          mode: gadgetiniMode,
        };
        if (gadgetiniMode === "direct") {
          gadgetini.host_name = gadgetiniHostName.trim();
        }
        if (customGadgetiniPassword && gadgetiniPassword.trim()) {
          gadgetini.password = gadgetiniPassword;
        }
        try {
          await invokeAction(apiKey, "server-update", {
            id: payload.id,
            gadgetini,
          });
        } catch (e) {
          gadgetiniError = (e as Error).message;
        }
      }
      // Cancel remaining timers and mark everything done.
      timers.forEach((t) => clearTimeout(t));
      setProgress((prev) => {
        const totalElapsed = now() - startedAt;
        return steps.map((s, i) => {
          if (s.key === "dcgm" && !payload.bootstrap.gpu_detected) {
            return { status: "skipped", elapsed: null };
          }
          const prevEntry = prev[i];
          const elapsed = prevEntry?.elapsed ?? totalElapsed / steps.length;
          return { status: "ok", elapsed };
        });
      });
      toast.success(`Registered ${host} → ${payload.id.slice(0, 8)}`, {
        description:
          (payload.bootstrap.installed_pkgs?.join(", ") || "no pkg install") +
          (payload.bootstrap.gpu_detected
            ? ` · GPU(${payload.bootstrap.dcgm_enabled ? "DCGM" : "nvidia-smi"})`
            : "") +
          (includeGadgetini && !gadgetiniError ? " · Gadgetini" : ""),
      });
      if (gadgetiniError) {
        toast.error("Gadgetini setup failed", { description: gadgetiniError });
      }
      setHost("");
      setUser("");
      setSshPw("");
      setSudoPw("");
      setKeyPaste("");
      setGadgetiniPassword("");
      onAdded();
      // Auto-minimize 2.5s after success so the operator can see the
      // final ✓ row before the form collapses out of the way. Cancelled
      // if the operator re-expanded in the meantime.
      window.setTimeout(() => onCollapsedChange(true), 2500);
    } catch (e) {
      timers.forEach((t) => clearTimeout(t));
      // Mark the currently-running step failed; leave earlier ok, later
      // pending so the operator can see how far we got.
      setProgress((prev) => {
        const next = prev.slice();
        const current = next.findIndex((p) => p.status === "running");
        if (current >= 0) {
          next[current] = { status: "failed", elapsed: now() - stepStartedAt[current] };
        }
        return next;
      });
      toast.error("server.add failed", { description: (e as Error).message });
    } finally {
      setSubmitting(false);
    }
  };

  if (collapsed) {
    return (
      <section
        data-testid="server-add-form-collapsed"
        className="flex items-center justify-between border-b border-zinc-800 bg-zinc-950 px-4 py-2"
      >
        <span className="text-[11px] text-zinc-500">
          Need another server? Use the button to re-open the register form.
        </span>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => onCollapsedChange(false)}
          data-testid="server-add-reopen"
          className="h-6 gap-1 px-2 text-[11px]"
        >
          + Add server
        </Button>
      </section>
    );
  }
  return (
    <section
      data-testid="server-add-form"
      className="flex flex-col gap-3 border-b border-zinc-800 bg-zinc-950 p-4"
    >
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold text-zinc-200">Register a server</h2>
          <button
            type="button"
            onClick={() => onCollapsedChange(true)}
            data-testid="server-add-minimize"
            title="Collapse — reopen later via '+ Add server'"
            aria-label="Collapse the register form"
            className="rounded border border-zinc-800 px-1.5 py-px text-[10px] text-zinc-500 hover:border-zinc-700 hover:text-zinc-300"
          >
            minimize
          </button>
          <button
            type="button"
            onClick={() => onCollapsedChange(true)}
            data-testid="server-add-close"
            title="Close the form"
            aria-label="Close the register form"
            className="rounded border border-zinc-800 px-1.5 py-px font-mono text-[10px] text-zinc-500 hover:border-zinc-700 hover:text-zinc-300"
          >
            ×
          </button>
        </div>
        <div className="flex gap-1" role="tablist" aria-label="auth mode">
          {(["password_bootstrap", "key_paste", "key_path"] as AuthMode[]).map((m) => (
            <button
              key={m}
              type="button"
              role="tab"
              aria-selected={mode === m}
              onClick={() => setMode(m)}
              data-testid={`auth-tab-${m}`}
              className={`rounded border px-2 py-0.5 text-[11px] ${
                mode === m
                  ? "border-blue-500 bg-blue-950/40 text-blue-200"
                  : "border-zinc-700 bg-zinc-900 text-zinc-500 hover:text-zinc-300"
              }`}
            >
              {m === "password_bootstrap" ? "Password (auto-setup)" : m === "key_paste" ? "Paste key" : "Key path"}
            </button>
          ))}
        </div>
      </div>
      <div className="grid grid-cols-1 gap-2 md:grid-cols-3">
        <Input
          placeholder="host (10.0.0.5 or hostname)"
          value={host}
          onChange={(e) => setHost(e.target.value)}
          className="font-mono text-xs"
        />
        <Input
          placeholder="ssh_user (ubuntu)"
          value={user}
          onChange={(e) => setUser(e.target.value)}
          className="font-mono text-xs"
        />
        <Input
          type="number"
          min={1}
          max={65535}
          value={port}
          onChange={(e) => setPort(parseInt(e.target.value || "22", 10))}
          className="font-mono text-xs"
        />
      </div>
      {mode === "key_path" && (
        <Input
          placeholder="/home/user/.ssh/id_ed25519"
          value={keyPath}
          onChange={(e) => setKeyPath(e.target.value)}
          className="font-mono text-xs"
        />
      )}
      {mode === "key_paste" && (
        <Textarea
          placeholder="-----BEGIN OPENSSH PRIVATE KEY-----&#10;..."
          value={keyPaste}
          onChange={(e) => setKeyPaste(e.target.value)}
          className="h-28 font-mono text-[11px]"
        />
      )}
      {mode === "password_bootstrap" && (
        <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
          <Input
            type="password"
            placeholder="ssh password"
            value={sshPw}
            onChange={(e) => setSshPw(e.target.value)}
            className="font-mono text-xs"
          />
          <Input
            type="password"
            placeholder="sudo password (often same as ssh)"
            value={sudoPw}
            onChange={(e) => setSudoPw(e.target.value)}
            className="font-mono text-xs"
          />
        </div>
      )}
      <div className="rounded border border-zinc-800 bg-zinc-900/40 p-2">
        <label className="flex items-center gap-2 text-[11px] text-zinc-300">
          <input
            type="checkbox"
            checked={includeGadgetini}
            onChange={(e) => setIncludeGadgetini(e.target.checked)}
            className="accent-blue-500"
          />
          Include Gadgetini
        </label>
        {includeGadgetini && (
          <div className="mt-2 flex flex-col gap-2">
            <div className="flex flex-col gap-1">
              <span className="text-[10px] uppercase tracking-wide text-zinc-500">
                Connection
              </span>
              <div className="flex items-center gap-3 text-[11px] text-zinc-300">
                <label className="flex items-center gap-1.5">
                  <input
                    type="radio"
                    name="gadgetini-mode"
                    value="usb"
                    checked={gadgetiniMode === "usb"}
                    onChange={() => setGadgetiniMode("usb")}
                    className="accent-blue-500"
                  />
                  USB (parent host)
                </label>
                <label className="flex items-center gap-1.5">
                  <input
                    type="radio"
                    name="gadgetini-mode"
                    value="direct"
                    checked={gadgetiniMode === "direct"}
                    onChange={() => setGadgetiniMode("direct")}
                    className="accent-blue-500"
                  />
                  Direct IP
                </label>
              </div>
              <p className="text-[10px] text-zinc-600">
                {gadgetiniMode === "usb"
                  ? "SSH proxied through the parent host's USB CDC link (default — IPv6 fd12:3456:789a:1::2 over usb0)."
                  : "SSH directly to the gadgetini's own IP. No parent involvement."}
              </p>
            </div>
            {gadgetiniMode === "direct" && (
              <Input
                type="text"
                placeholder="Gadgetini host or IP (e.g. 192.168.10.42)"
                value={gadgetiniHostName}
                onChange={(e) => setGadgetiniHostName(e.target.value)}
                className="font-mono text-xs"
              />
            )}
            <label className="flex items-center gap-2 text-[11px] text-zinc-500">
              <input
                type="checkbox"
                checked={customGadgetiniPassword}
                onChange={(e) => setCustomGadgetiniPassword(e.target.checked)}
                className="accent-blue-500"
              />
              Custom Gadgetini password
            </label>
            {customGadgetiniPassword && (
              <Input
                type="password"
                placeholder="Gadgetini password"
                value={gadgetiniPassword}
                onChange={(e) => setGadgetiniPassword(e.target.value)}
                className="font-mono text-xs"
              />
            )}
          </div>
        )}
      </div>
      <div className="flex items-center justify-between gap-2">
        <p className="text-[10px] text-zinc-600">
          {mode === "password_bootstrap"
            ? "Passwords are used once to push an ed25519 key + NOPASSWD sudoers line, then zeroized. Never stored."
            : mode === "key_paste"
              ? "Pasted key is written 0600 under ~/.gadgetron/server-monitor/keys/. Not synced anywhere."
              : "Key file path must be readable by the gadgetron process."}
        </p>
        <Button onClick={submit} disabled={submitting}>
          {submitting ? "Registering…" : "Register"}
        </Button>
      </div>
      {(submitting || progress.some((p) => p.status !== "pending")) && (
        <div
          data-testid="bootstrap-progress"
          className="rounded border border-zinc-800 bg-zinc-900/50 p-3"
        >
          <div className="mb-2 text-[10px] uppercase tracking-wide text-zinc-500">
            Bootstrap progress ({mode})
          </div>
          <ol className="flex flex-col">
            {steps.map((s, i) => {
              const entry = progress[i] ?? { status: "pending" as StepStatus, elapsed: null };
              return (
                <StepRow key={s.key} step={s} status={entry.status} elapsed={entry.elapsed} />
              );
            })}
          </ol>
        </div>
      )}
    </section>
  );
}
