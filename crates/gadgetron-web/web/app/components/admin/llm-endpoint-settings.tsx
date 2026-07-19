"use client";

// LLM endpoint management for /web/admin (ISSUE 61). Split out of the
// monolithic admin page; shares the admin API helpers/types via ./api.

import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { ArrowRight } from "lucide-react";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { useConfirm } from "../ui/confirm";
import { InlineNotice, StatusBadge } from "../workbench";
import {
  autodetectLlmEndpoint,
  createCcrBridge,
  createLlmEndpoint,
  deleteLlmEndpoint,
  listLlmEndpoints,
  listRegisteredServers,
  probeLlmEndpoint,
  useLlmEndpoint,
  type LlmEndpointRow,
  type ManagedHostRow,
} from "../../(shell)/admin/api";

export function LlmEndpointSettings({
  apiKey,
  canCall,
}: {
  apiKey: string | null;
  canCall: boolean;
}) {
  const confirm = useConfirm();
  const [endpoints, setEndpoints] = useState<LlmEndpointRow[]>([]);
  const [name, setName] = useState("");
  const [kind, setKind] = useState<LlmEndpointRow["kind"]>("vllm");
  const [protocol, setProtocol] = useState<LlmEndpointRow["protocol"]>("openai_chat");
  const [baseUrl, setBaseUrl] = useState("");
  const [modelId, setModelId] = useState("");
  const [detectHost, setDetectHost] = useState("");
  const [detectPort, setDetectPort] = useState("");
  const [detectAlias, setDetectAlias] = useState("");
  const [detectScheme, setDetectScheme] = useState<"http" | "https">("http");
  const [detectModel, setDetectModel] = useState("");
  const [detectAuthEnv, setDetectAuthEnv] = useState("");
  const [detectAuthToken, setDetectAuthToken] = useState("");
  const [probeModelByEndpoint, setProbeModelByEndpoint] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [managedHosts, setManagedHosts] = useState<ManagedHostRow[]>([]);
  const [bridgeSource, setBridgeSource] = useState<LlmEndpointRow | null>(null);
  const [bridgeName, setBridgeName] = useState("");
  const [bridgeTargetKind, setBridgeTargetKind] = useState<"local" | "registered_server">("local");
  const [bridgeHostId, setBridgeHostId] = useState("");
  const [bridgePort, setBridgePort] = useState("3456");
  const [bridgeBaseUrl, setBridgeBaseUrl] = useState("http://127.0.0.1:3456");
  const [bridgeAuthEnv, setBridgeAuthEnv] = useState("PENNY_CCR_AUTH_TOKEN");
  const [useTokenEndpoint, setUseTokenEndpoint] = useState<LlmEndpointRow | null>(null);
  const [useTokenValue, setUseTokenValue] = useState("");

  const refresh = useCallback(async () => {
    if (!canCall) return;
    setLoading(true);
    setErr(null);
    try {
      setEndpoints(await listLlmEndpoints(apiKey));
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [apiKey, canCall]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const submit = useCallback(async () => {
    if (!name.trim() || !baseUrl.trim()) {
      toast.error("Endpoint name and URL are required");
      return;
    }
    setBusy("create");
    setErr(null);
    try {
      await createLlmEndpoint(apiKey, {
        name: name.trim(),
        kind,
        protocol,
        base_url: baseUrl.trim(),
        model_id: modelId.trim() || undefined,
      });
      toast.success(`Endpoint added: ${name.trim()}`);
      setName("");
      setBaseUrl("");
      setModelId("");
      await refresh();
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(null);
    }
  }, [apiKey, baseUrl, kind, modelId, name, protocol, refresh]);

  const autodetect = useCallback(async () => {
    const port = Number(detectPort);
    if (!detectHost.trim() || !Number.isInteger(port) || port < 1 || port > 65535) {
      toast.error("Check the host and port");
      return;
    }
    setBusy("autodetect");
    setErr(null);
    try {
      const result = await autodetectLlmEndpoint(apiKey, {
        host: detectHost.trim(),
        port,
        alias: detectAlias.trim() || undefined,
        scheme: detectScheme,
        model_id: detectModel.trim() || undefined,
        auth_token_env: detectAuthEnv.trim() || undefined,
        auth_token_value: detectAuthToken.trim() || undefined,
      });
      setEndpoints((prev) => {
        const rest = prev.filter((endpoint) => endpoint.id !== result.endpoint.id);
        return [result.endpoint, ...rest];
      });
      toast[result.ok ? "success" : "error"](
        `${result.endpoint.name}: ${result.message}`,
      );
      setDetectAuthToken("");
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(null);
    }
  }, [
    apiKey,
    detectAlias,
    detectAuthEnv,
    detectAuthToken,
    detectHost,
    detectModel,
    detectPort,
    detectScheme,
  ]);

  const probe = useCallback(
    async (endpoint: LlmEndpointRow) => {
      setBusy(`probe:${endpoint.id}`);
      setErr(null);
      try {
        const result = await probeLlmEndpoint(apiKey, endpoint.id, {
          model_id:
            probeModelByEndpoint[endpoint.id] || endpoint.model_id || undefined,
        });
        toast[result.ok ? "success" : "error"](
          result.models.length > 0
            ? `${endpoint.name}: ${result.models.length} models`
            : `${endpoint.name}: ${result.message}`,
        );
        await refresh();
      } catch (e) {
        setErr((e as Error).message);
      } finally {
        setBusy(null);
      }
    },
    [apiKey, probeModelByEndpoint, refresh],
  );

  const remove = useCallback(
    async (endpoint: LlmEndpointRow) => {
      if (!(await confirm({ title: `Delete endpoint ${endpoint.name}?`, tone: "danger", confirmLabel: "Delete" }))) return;
      setBusy(`delete:${endpoint.id}`);
      setErr(null);
      try {
        await deleteLlmEndpoint(apiKey, endpoint.id);
        toast.success(`Endpoint deleted: ${endpoint.name}`);
        await refresh();
      } catch (e) {
        setErr((e as Error).message);
      } finally {
        setBusy(null);
      }
    },
    [apiKey, refresh],
  );

  const useForPenny = useCallback(
    async (endpoint: LlmEndpointRow, authTokenValue?: string) => {
      setBusy(`use:${endpoint.id}`);
      setErr(null);
      try {
        const token = authTokenValue?.trim();
        await useLlmEndpoint(
          apiKey,
          endpoint.id,
          {
            model_id:
              probeModelByEndpoint[endpoint.id] ||
              endpoint.tool_model_id ||
              endpoint.model_id ||
              undefined,
            ...(token ? { external_auth_token_value: token } : {}),
          },
        );
        setUseTokenEndpoint(null);
        setUseTokenValue("");
        toast.success(`New-chat default endpoint: ${endpoint.name}`);
      } catch (e) {
        setErr((e as Error).message);
      } finally {
        setBusy(null);
      }
    },
    [apiKey, probeModelByEndpoint],
  );

  const startUseForPenny = useCallback(
    (endpoint: LlmEndpointRow) => {
      if (endpoint.auth_token_env) {
        setUseTokenEndpoint(endpoint);
        setUseTokenValue("");
        setErr(null);
        return;
      }
      void useForPenny(endpoint);
    },
    [useForPenny],
  );

  const openBridgeForm = useCallback(
    async (endpoint: LlmEndpointRow) => {
      setBridgeSource(endpoint);
      setBridgeName(`${endpoint.name}-ccr`);
      setBridgeTargetKind("local");
      setBridgeHostId("");
      setBridgePort("3456");
      setBridgeBaseUrl("http://127.0.0.1:3456");
      setBridgeAuthEnv("PENNY_CCR_AUTH_TOKEN");
      setBusy(`hosts:${endpoint.id}`);
      try {
        setManagedHosts(await listRegisteredServers(apiKey));
      } catch (e) {
        toast.error("Could not load registered servers", {
          description: (e as Error).message,
        });
      } finally {
        setBusy(null);
      }
    },
    [apiKey],
  );

  const chooseBridgeTargetKind = useCallback(
    (next: "local" | "registered_server") => {
      setBridgeTargetKind(next);
      if (next === "local") {
        setBridgeHostId("");
        setBridgeBaseUrl(`http://127.0.0.1:${bridgePort || "3456"}`);
        return;
      }
      const first = managedHosts[0];
      setBridgeHostId(first?.id ?? "");
      if (first) {
        setBridgeBaseUrl(`http://${first.host}:${bridgePort || "3456"}`);
      }
    },
    [bridgePort, managedHosts],
  );

  const chooseBridgeHost = useCallback(
    (hostId: string) => {
      setBridgeHostId(hostId);
      const host = managedHosts.find((item) => item.id === hostId);
      if (host) {
        setBridgeBaseUrl(`http://${host.host}:${bridgePort || "3456"}`);
      }
    },
    [bridgePort, managedHosts],
  );

  const updateBridgePort = useCallback(
    (value: string) => {
      setBridgePort(value);
      const port = value || "3456";
      if (bridgeTargetKind === "local") {
        setBridgeBaseUrl(`http://127.0.0.1:${port}`);
        return;
      }
      const host = managedHosts.find((item) => item.id === bridgeHostId);
      if (host) {
        setBridgeBaseUrl(`http://${host.host}:${port}`);
      }
    },
    [bridgeHostId, bridgeTargetKind, managedHosts],
  );

  const createBridge = useCallback(async () => {
    if (!bridgeSource) return;
    const port = Number(bridgePort);
    if (!bridgeName.trim() || !bridgeBaseUrl.trim()) {
      toast.error("Bridge name and URL are required");
      return;
    }
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      toast.error("Check the bridge port");
      return;
    }
    if (bridgeTargetKind === "registered_server" && !bridgeHostId) {
      toast.error("Select a registered server target");
      return;
    }
    setBusy("ccr:create");
    setErr(null);
    try {
      const next = await createCcrBridge(apiKey, bridgeSource.id, {
        name: bridgeName.trim(),
        target_kind: bridgeTargetKind,
        target_host_id: bridgeTargetKind === "registered_server" ? bridgeHostId : undefined,
        base_url: bridgeBaseUrl.trim(),
        port,
        auth_token_env: bridgeAuthEnv.trim() || undefined,
      });
      setEndpoints((prev) => [next, ...prev.filter((endpoint) => endpoint.id !== next.id)]);
      setBridgeSource(null);
      toast.success(`CCR bridge target registered: ${next.name}`);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(null);
    }
  }, [
    apiKey,
    bridgeAuthEnv,
    bridgeBaseUrl,
    bridgeHostId,
    bridgeName,
    bridgePort,
    bridgeSource,
    bridgeTargetKind,
  ]);

  return (
    <section className="rounded border border-zinc-800 bg-zinc-900 p-4">
      <header className="mb-3 flex items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium text-zinc-200">Local model endpoints</h2>
          <p className="text-[11px] text-zinc-500">
            Connect → detect protocol → verify an actual model tool call
          </p>
        </div>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void refresh()}
          disabled={loading || !canCall}
          className="h-7 px-2 text-[11px]"
        >
          {loading ? "…" : "Refresh"}
        </Button>
      </header>

      {err && (
        <div className="mb-3 rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
          {err}
        </div>
      )}

      <div className="mb-3 grid grid-cols-1 gap-2 md:grid-cols-3">
        <div className="rounded border border-zinc-800 bg-zinc-950/50 px-3 py-2">
          <div className="text-[11px] font-medium text-zinc-200">Endpoint</div>
          <div className="mt-1 text-[10px] text-zinc-500">
            {endpoints.filter((endpoint) => endpoint.protocol === "openai_chat").length} Chat-only
          </div>
        </div>
        <div className="rounded border border-zinc-800 bg-zinc-950/50 px-3 py-2">
          <div className="text-[11px] font-medium text-zinc-200">CCR Bridge</div>
          <div className="mt-1 flex flex-wrap gap-1 text-[10px] text-zinc-400">
            <span className="rounded bg-zinc-800 px-1.5 py-0.5">Local web server</span>
            <span className="rounded bg-zinc-800 px-1.5 py-0.5">Registered server</span>
          </div>
        </div>
        <div className="rounded border border-zinc-800 bg-zinc-950/50 px-3 py-2">
          <div className="text-[11px] font-medium text-zinc-200">Penny</div>
          <div className="mt-1 text-[10px] text-zinc-500">
            {endpoints.filter((endpoint) => endpoint.tool_status === "passed").length} tool-verified
          </div>
        </div>
      </div>

      <div className="rounded border border-zinc-800 bg-zinc-950/50 p-3">
        <div className="grid grid-cols-1 gap-3 lg:grid-cols-7">
          <div className="lg:col-span-2">
            <label className="mb-1 block text-[11px] text-zinc-500">
              Alias
            </label>
            <Input
              value={detectAlias}
              onChange={(e) => setDetectAlias(e.target.value)}
              placeholder="gemma4"
              autoComplete="off"
              aria-label="Endpoint Alias"
            />
          </div>
          <div className="lg:col-span-2">
            <label className="mb-1 block text-[11px] text-zinc-500">
              Host / IP
            </label>
            <Input
              value={detectHost}
              onChange={(e) => setDetectHost(e.target.value)}
              placeholder="10.100.1.5"
              autoComplete="off"
              aria-label="Endpoint Host"
            />
          </div>
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Port</label>
            <Input
              value={detectPort}
              onChange={(e) => setDetectPort(e.target.value)}
              placeholder="8100"
              inputMode="numeric"
              autoComplete="off"
              aria-label="Endpoint Port"
            />
          </div>
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Scheme</label>
            <select
              value={detectScheme}
              onChange={(e) => setDetectScheme(e.target.value as "http" | "https")}
              className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
              aria-label="Endpoint Scheme"
            >
              <option value="http">http</option>
              <option value="https">https</option>
            </select>
          </div>
          <div className="flex items-end">
            <Button
              onClick={() => void autodetect()}
              disabled={busy === "autodetect" || !canCall}
              className="w-full"
            >
              {busy === "autodetect" ? "Detecting…" : "Auto-detect"}
            </Button>
          </div>
        </div>
        <details className="mt-3 border-t border-zinc-800 pt-2">
          <summary className="cursor-pointer text-[10px] text-zinc-500">
            Authentication or explicit model (optional)
          </summary>
          <div className="mt-2 grid grid-cols-1 gap-3 md:grid-cols-3">
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">Model ID</label>
              <Input
                value={detectModel}
                onChange={(event) => setDetectModel(event.target.value)}
                placeholder="Auto from /v1/models"
                autoComplete="off"
                aria-label="Detection Model ID"
              />
            </div>
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">Token Env</label>
              <Input
                value={detectAuthEnv}
                onChange={(event) => setDetectAuthEnv(event.target.value)}
                placeholder="LOCAL_LLM_API_KEY"
                autoComplete="off"
                aria-label="Detection Token Env"
              />
            </div>
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">Token Value</label>
              <Input
                type="password"
                value={detectAuthToken}
                onChange={(event) => setDetectAuthToken(event.target.value)}
                placeholder="Write-only; not stored"
                autoComplete="new-password"
                aria-label="Detection Token Value"
              />
            </div>
          </div>
        </details>
      </div>

      <details className="mt-3 rounded border border-zinc-800 bg-zinc-950/30 p-3">
        <summary className="cursor-pointer text-[11px] text-zinc-400">
          Advanced registration
        </summary>
        <div className="mt-3 grid grid-cols-1 gap-3 lg:grid-cols-6">
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Name</label>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Gemma 4"
              autoComplete="off"
            />
          </div>
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Kind</label>
            <select
              value={kind}
              onChange={(e) => setKind(e.target.value as LlmEndpointRow["kind"])}
              className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
            >
              <option value="vllm">vllm</option>
              <option value="sglang">sglang</option>
              <option value="openai_compatible">openai_compatible</option>
              <option value="ccr">ccr</option>
              <option value="anthropic_proxy">anthropic_proxy</option>
            </select>
          </div>
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Protocol</label>
            <select
              value={protocol}
              onChange={(e) =>
                setProtocol(e.target.value as LlmEndpointRow["protocol"])
              }
              className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
            >
              <option value="openai_chat">openai_chat</option>
              <option value="openai_responses">openai_responses</option>
              <option value="anthropic_messages">anthropic_messages</option>
            </select>
          </div>
          <div className="lg:col-span-2">
            <label className="mb-1 block text-[11px] text-zinc-500">Base URL</label>
            <Input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="http://10.100.1.5:8100"
              autoComplete="off"
            />
          </div>
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Model</label>
            <Input
              value={modelId}
              onChange={(e) => setModelId(e.target.value)}
              placeholder="cyankiwi/gemma-4-31B-it-AWQ-4bit"
              autoComplete="off"
            />
          </div>
        </div>
        <div className="mt-3 flex justify-end">
          <Button onClick={() => void submit()} disabled={busy === "create" || !canCall}>
            {busy === "create" ? "Adding…" : "Add endpoint"}
          </Button>
        </div>
      </details>

      {bridgeSource && (
        <div className="mt-3 rounded border border-zinc-800 bg-zinc-950/50 p-3">
          <div className="mb-3 flex items-start justify-between gap-3">
            <div>
              <h3 className="text-xs font-medium text-zinc-200">CCR bridge registry target</h3>
              <p className="flex items-center gap-1.5 text-[11px] text-zinc-500">
                <span>{bridgeSource.name}</span>
                <ArrowRight
                  className="size-3 text-zinc-600"
                  aria-hidden="true"
                  data-testid="ccr-bridge-direction-icon"
                />
                <span>Anthropic-compatible endpoint</span>
              </p>
              <p className="mt-1 text-[10px] text-amber-300/80">
                This registers the target only. Start the bridge separately, then probe it before use.
              </p>
            </div>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => setBridgeSource(null)}
              className="h-7 px-2 text-[11px]"
            >
              Close
            </Button>
          </div>
          <div className="grid grid-cols-1 gap-3 lg:grid-cols-7">
            <div className="lg:col-span-2">
              <label className="mb-1 block text-[11px] text-zinc-500">Bridge name</label>
              <Input
                value={bridgeName}
                onChange={(e) => setBridgeName(e.target.value)}
                autoComplete="off"
              />
            </div>
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">Target</label>
              <select
                value={bridgeTargetKind}
                onChange={(e) =>
                  chooseBridgeTargetKind(e.target.value as "local" | "registered_server")
                }
                className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
              >
                <option value="local">local</option>
                <option value="registered_server">server</option>
              </select>
            </div>
            {bridgeTargetKind === "registered_server" && (
              <div className="lg:col-span-2">
                <label className="mb-1 block text-[11px] text-zinc-500">Server</label>
                <select
                  value={bridgeHostId}
                  onChange={(e) => chooseBridgeHost(e.target.value)}
                  className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
                >
                  <option value="">Select</option>
                  {managedHosts.map((host) => (
                    <option key={host.id} value={host.id}>
                      {host.alias || host.host}
                    </option>
                  ))}
                </select>
              </div>
            )}
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">Port</label>
              <Input
                value={bridgePort}
                onChange={(e) => updateBridgePort(e.target.value)}
                inputMode="numeric"
                autoComplete="off"
              />
            </div>
            <div className="lg:col-span-2">
              <label className="mb-1 block text-[11px] text-zinc-500">Gateway URL</label>
              <Input
                value={bridgeBaseUrl}
                onChange={(e) => setBridgeBaseUrl(e.target.value)}
                autoComplete="off"
              />
            </div>
            <div className="lg:col-span-2">
              <label className="mb-1 block text-[11px] text-zinc-500">Auth Token Env</label>
              <Input
                value={bridgeAuthEnv}
                onChange={(e) => setBridgeAuthEnv(e.target.value)}
                autoComplete="off"
              />
            </div>
            <div className="flex items-end">
              <Button
                type="button"
                onClick={() => void createBridge()}
                disabled={busy === "ccr:create" || !canCall}
                className="w-full"
              >
                  {busy === "ccr:create" ? "Registering…" : "Register target"}
              </Button>
            </div>
          </div>
        </div>
      )}

      {useTokenEndpoint && (
        <div className="mt-3 rounded border border-[var(--copper)] bg-[var(--surface-2)] p-3">
          <div className="mb-3 flex items-start justify-between gap-3">
            <div>
              <h3 className="text-xs font-medium text-[var(--copper-hi)]">
                Apply {useTokenEndpoint.name} to Penny
              </h3>
              <p className="text-[11px] leading-5 text-[var(--ink-2)]">
                Paste the auth token for {useTokenEndpoint.auth_token_env}. If the
                server already has that environment variable, use the existing env.
              </p>
            </div>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => {
                setUseTokenEndpoint(null);
                setUseTokenValue("");
              }}
              className="h-7 px-2 text-[11px]"
            >
              Close
            </Button>
          </div>
          <div className="grid grid-cols-1 gap-2 md:grid-cols-[minmax(0,1fr)_auto_auto] md:items-end">
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">
                Endpoint Auth Token
              </label>
              <Input
                type="password"
                value={useTokenValue}
                onChange={(e) => setUseTokenValue(e.target.value)}
                placeholder="Paste token value"
                autoComplete="new-password"
                aria-label="Endpoint Auth Token"
              />
            </div>
            <Button
              type="button"
              variant="secondary"
              onClick={() => void useForPenny(useTokenEndpoint)}
              disabled={busy === `use:${useTokenEndpoint.id}` || !canCall}
            >
              Use existing env
            </Button>
            <Button
              type="button"
              onClick={() => void useForPenny(useTokenEndpoint, useTokenValue)}
              disabled={
                busy === `use:${useTokenEndpoint.id}` ||
                !canCall ||
                !useTokenValue.trim()
              }
            >
              Apply to Penny
            </Button>
          </div>
        </div>
      )}

      <div className="mt-4 overflow-x-auto rounded border border-zinc-800">
        <table className="w-full text-sm">
          <thead className="bg-zinc-950 text-[11px] uppercase text-zinc-500">
            <tr>
              <th className="px-3 py-2 text-left font-normal">Name</th>
              <th className="px-3 py-2 text-left font-normal">Protocol</th>
              <th className="px-3 py-2 text-left font-normal">URL</th>
              <th className="px-3 py-2 text-left font-normal">Model</th>
              <th className="px-3 py-2 text-left font-normal">Status</th>
              <th className="w-48 px-3 py-2 text-right font-normal"></th>
            </tr>
          </thead>
          <tbody>
            {endpoints.map((endpoint) => {
              const selectedModel =
                probeModelByEndpoint[endpoint.id] || endpoint.model_id || "";
              return (
              <tr key={endpoint.id} className="border-t border-zinc-800 text-zinc-300">
                <td className="px-3 py-2">
                  <div className="font-medium text-zinc-200">{endpoint.name}</div>
                  <div className="text-[10px] text-zinc-500">
                    {endpoint.kind}
                    {endpoint.target_kind && endpoint.target_kind !== "external"
                      ? ` · ${endpoint.target_kind}${endpoint.listen_port ? `:${endpoint.listen_port}` : ""}`
                      : ""}
                  </div>
                </td>
                <td className="px-3 py-2 text-xs">{endpoint.protocol}</td>
                <td className="max-w-56 truncate px-3 py-2 font-mono text-xs">
                  {endpoint.base_url}
                </td>
                <td className="max-w-64 px-3 py-2 font-mono text-xs">
                  {(endpoint.discovered_models ?? []).length > 0 ? (
                    <select
                      aria-label={`Probe model for ${endpoint.name}`}
                      value={selectedModel}
                      onChange={(event) =>
                        setProbeModelByEndpoint((current) => ({
                          ...current,
                          [endpoint.id]: event.target.value,
                        }))
                      }
                      className="h-7 max-w-64 rounded border border-zinc-800 bg-zinc-950 px-1 text-[10px]"
                    >
                      {(endpoint.discovered_models ?? []).map((model) => (
                        <option key={model} value={model}>{model}</option>
                      ))}
                    </select>
                  ) : (
                    endpoint.model_id || "No model detected"
                  )}
                </td>
                <td className="px-3 py-2">
                  <span
                    className={
                      endpoint.health_status === "ok"
                        ? "rounded bg-emerald-950/40 px-1.5 py-0.5 text-[10px] text-emerald-300"
                        : endpoint.health_status === "error"
                          ? "rounded bg-red-950/40 px-1.5 py-0.5 text-[10px] text-red-300"
                          : "rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400"
                    }
                  >
                    connection:{endpoint.health_status}
                  </span>
                  <span className="ml-1 rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400">
                    {endpoint.runtime_compatibility}
                  </span>
                  <span
                    className={
                      endpoint.tool_status === "passed"
                        ? "ml-1 rounded bg-emerald-950/40 px-1.5 py-0.5 text-[10px] text-emerald-300"
                        : endpoint.tool_status === "failed"
                          ? "ml-1 rounded bg-red-950/40 px-1.5 py-0.5 text-[10px] text-red-300"
                          : "ml-1 rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400"
                    }
                  >
                    tool:{endpoint.tool_status}
                  </span>
                  {endpoint.last_latency_ms != null && (
                    <span className="ml-2 text-[10px] text-zinc-500">
                      {endpoint.last_latency_ms}ms
                    </span>
                  )}
                  {endpoint.last_error && (
                    <div className="mt-1 max-w-56 truncate text-[10px] text-red-300">
                      {endpoint.last_error}
                    </div>
                  )}
                  {endpoint.last_tool_error && (
                    <div className="mt-1 max-w-72 truncate text-[10px] text-red-300">
                      {endpoint.last_tool_error}
                    </div>
                  )}
                </td>
                <td className="px-3 py-2 text-right">
                  <div className="flex justify-end gap-1">
                    <Button
                      type="button"
                      size="sm"
                      variant="ghost"
                      onClick={() => void probe(endpoint)}
                      disabled={busy === `probe:${endpoint.id}`}
                      className="h-6 px-2 text-[11px]"
                    >
                      Probe
                    </Button>
                    <Button
                      type="button"
                      size="sm"
                      variant="ghost"
                      onClick={() =>
                        endpoint.runtime_compatibility !== "bridge_required"
                          ? startUseForPenny(endpoint)
                          : void openBridgeForm(endpoint)
                      }
                      disabled={
                        busy === `use:${endpoint.id}` ||
                        busy === `hosts:${endpoint.id}` ||
                        (endpoint.runtime_compatibility !== "bridge_required" &&
                          endpoint.tool_status !== "passed")
                      }
                      className="h-6 px-2 text-[11px]"
                      title={
                        endpoint.runtime_compatibility !== "bridge_required"
                          ? "Use the tool-verified model as the new-chat default"
                          : "Register a CCR target; this does not install or start the bridge"
                      }
                    >
                      {endpoint.runtime_compatibility !== "bridge_required" ? "Use" : "Register CCR"}
                    </Button>
                    <Button
                      type="button"
                      size="sm"
                      variant="ghost"
                      onClick={() => void remove(endpoint)}
                      disabled={busy === `delete:${endpoint.id}`}
                      className="h-6 px-2 text-[11px] text-red-400 hover:text-red-300"
                    >
                      Delete
                    </Button>
                  </div>
                </td>
              </tr>
              );
            })}
            {endpoints.length === 0 && (
              <tr>
                <td colSpan={6} className="px-3 py-6 text-center text-[11px] text-zinc-600">
                  No LLM endpoints registered.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------
// AddUserForm — collapsible block above the table.
// ---------------------------------------------------------------------------
