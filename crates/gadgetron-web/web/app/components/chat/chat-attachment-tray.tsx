"use client";

import {
  AlertCircle,
  CheckCircle2,
  FileUp,
  Link as LinkIcon,
  Loader2,
  Paperclip,
  RefreshCw,
  Save,
  Trash2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { useAuth } from "@/lib/auth-context";
import { useI18n, type Dictionary } from "@/lib/i18n";
import {
  deleteChatAttachment,
  fetchChatAttachment,
  fetchKnowledgeSource,
  listChatAttachments,
  listKnowledgeSpaces,
  listKnowledgeVaults,
  promoteChatAttachment,
  retryChatAttachment,
  uploadChatAttachment,
  uploadKnowledgeSource,
  type KnowledgeSource,
} from "@/lib/knowledge-workbench-api";

type RetentionChoice = "chat" | "vault";

interface VaultChoice {
  id: string;
  label: string;
}

function statusView(
  source: KnowledgeSource,
  copy: Dictionary["chat"]["attachments"],
) {
  if (source.status === "extracted") {
    return { label: copy.citationReady, tone: "text-emerald-300", icon: CheckCircle2 };
  }
  if (source.status === "failed") {
    return { label: copy.failed, tone: "text-red-300", icon: AlertCircle };
  }
  if (source.status === "needs_ocr") {
    return { label: copy.needsOcr, tone: "text-amber-300", icon: AlertCircle };
  }
  return { label: copy.extracting, tone: "text-sky-300", icon: Loader2 };
}

export function ChatAttachmentTray({ conversationId }: { conversationId: string | null }) {
  const { apiKey } = useAuth();
  const { labels } = useI18n();
  const copy = labels.chat.attachments;
  const inputRef = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [retention, setRetention] = useState<RetentionChoice>("chat");
  const [url, setUrl] = useState("");
  const [attachments, setAttachments] = useState<KnowledgeSource[]>([]);
  const [vaults, setVaults] = useState<VaultChoice[]>([]);
  const [vaultsLoaded, setVaultsLoaded] = useState(false);
  const [vaultId, setVaultId] = useState("");
  const [busy, setBusy] = useState(false);
  const [loadingVaults, setLoadingVaults] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!conversationId) {
      setAttachments([]);
      return;
    }
    try {
      const rows = await listChatAttachments(apiKey, conversationId);
      setAttachments(Array.isArray(rows) ? rows : []);
    } catch {
      // A newly minted conversation does not need an empty-state error.
      setAttachments([]);
    }
  }, [apiKey, conversationId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!open || retention !== "vault" || vaultsLoaded || loadingVaults) return;
    setLoadingVaults(true);
    void listKnowledgeSpaces(apiKey)
      .then(async (spaces) => {
        const rows = await Promise.all(
          spaces.map(async (space) => ({
            space,
            vaults: await listKnowledgeVaults(apiKey, space.id),
          })),
        );
        setVaults(
          rows.flatMap(({ space, vaults: spaceVaults }) =>
            spaceVaults.map((vault) => ({
              id: vault.id,
              label: `${space.title} · ${vault.home_bundle_id}`,
            })),
          ),
        );
      })
      .catch((reason: unknown) => {
        setError(reason instanceof Error ? reason.message : copy.vaultsFailed);
      })
      .finally(() => {
        setVaultsLoaded(true);
        setLoadingVaults(false);
      });
  }, [apiKey, copy.vaultsFailed, loadingVaults, open, retention, vaultsLoaded]);

  const requireTarget = () => {
    if (!conversationId) {
      setError(copy.conversationPending);
      return false;
    }
    if (retention === "vault" && !vaultId) {
      setError(copy.chooseVaultFirst);
      return false;
    }
    return true;
  };

  const addFile = async (file: File) => {
    if (!requireTarget() || !conversationId) return;
    setBusy(true);
    setError(null);
    try {
      if (retention === "chat") {
        await uploadChatAttachment(apiKey, conversationId, file);
      } else {
        await uploadKnowledgeSource(apiKey, vaultId, file, file.name, conversationId);
      }
      await refresh();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : copy.fileFailed);
      await refresh();
    } finally {
      setBusy(false);
      if (inputRef.current) inputRef.current.value = "";
    }
  };

  const addUrl = async () => {
    const clean = url.trim();
    if (!clean || !requireTarget() || !conversationId) return;
    setBusy(true);
    setError(null);
    try {
      if (retention === "chat") {
        await fetchChatAttachment(apiKey, conversationId, clean);
      } else {
        await fetchKnowledgeSource(apiKey, vaultId, clean, clean, conversationId);
      }
      setUrl("");
      await refresh();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : copy.urlFailed);
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  const retry = async (source: KnowledgeSource) => {
    setBusy(true);
    setError(null);
    try {
      await retryChatAttachment(apiKey, source.id, source.revision);
      await refresh();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : copy.retryFailed);
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  const remove = async (source: KnowledgeSource) => {
    if (!conversationId) return;
    setBusy(true);
    setError(null);
    try {
      await deleteChatAttachment(apiKey, conversationId, source.id, source.revision);
      await refresh();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : copy.removeFailed);
    } finally {
      setBusy(false);
    }
  };

  const promote = async (source: KnowledgeSource) => {
    if (!conversationId || !vaultId) {
      setOpen(true);
      setRetention("vault");
      setError(copy.chooseVaultBeforeSaving);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await promoteChatAttachment(apiKey, conversationId, source.id, vaultId);
      toast.success(copy.savedToVault);
      await refresh();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : copy.saveFailed);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className="w-full"
      data-testid="chat-attachment-tray"
      onDragOver={(event) => {
        event.preventDefault();
        setOpen(true);
      }}
      onDrop={(event) => {
        event.preventDefault();
        const file = event.dataTransfer.files.item(0);
        if (file) void addFile(file);
      }}
    >
      <div className="flex min-h-7 flex-wrap items-center gap-1.5 px-1 pb-1.5">
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded px-1.5 py-1 text-xs text-zinc-400 hover:bg-zinc-800 hover:text-zinc-100"
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
        >
          <Paperclip className="size-3.5" />
          {copy.attach}
        </button>
        {busy && (
          <span className="inline-flex items-center gap-1 text-xs text-sky-300">
            <Loader2 className="size-3 animate-spin" /> {copy.uploadExtract}
          </span>
        )}
        {attachments.map((source) => {
          const status = statusView(source, copy);
          const StatusIcon = status.icon;
          return (
            <span
              key={source.id}
              className="inline-flex max-w-full items-center gap-1 rounded border border-zinc-700 bg-zinc-900 px-1.5 py-1 text-xs"
              data-status={source.status}
            >
              <StatusIcon
                className={`size-3 shrink-0 ${status.tone} ${source.status === "pending" ? "animate-spin" : ""}`}
              />
              <span className="max-w-40 truncate text-zinc-200">
                {source.title || source.original_name || copy.attachment}
              </span>
              <span className={status.tone}>{status.label}</span>
              {source.source_kind === "chat_attachment" && source.status === "extracted" && (
                <button
                  type="button"
                  className="text-zinc-400 hover:text-zinc-100"
                  aria-label={copy.saveToVaultLabel}
                  onClick={() => void promote(source)}
                >
                  <Save className="size-3" />
                </button>
              )}
              {source.source_kind === "chat_attachment" &&
                (source.status === "failed" || source.status === "needs_ocr") && (
                  <button
                    type="button"
                    className="text-zinc-400 hover:text-zinc-100"
                    aria-label={copy.retryLabel}
                    onClick={() => void retry(source)}
                  >
                    <RefreshCw className="size-3" />
                  </button>
                )}
              {source.source_kind === "chat_attachment" && (
                <button
                  type="button"
                  className="text-zinc-500 hover:text-red-300"
                  aria-label={copy.removeLabel}
                  onClick={() => void remove(source)}
                >
                  <Trash2 className="size-3" />
                </button>
              )}
            </span>
          );
        })}
      </div>

      {open && (
        <div className="mb-2 space-y-2 rounded border border-zinc-700 bg-zinc-950 p-2 text-xs">
          <div className="flex items-center justify-between gap-2">
            <div className="flex rounded border border-zinc-700 p-0.5" aria-label={copy.retentionLabel}>
              <button
                type="button"
                className={`rounded px-2 py-1 ${retention === "chat" ? "bg-zinc-700 text-white" : "text-zinc-400"}`}
                onClick={() => {
                  setRetention("chat");
                  setError(null);
                }}
              >
                {copy.thisChatOnly}
              </button>
              <button
                type="button"
                className={`rounded px-2 py-1 ${retention === "vault" ? "bg-zinc-700 text-white" : "text-zinc-400"}`}
                onClick={() => {
                  setRetention("vault");
                  setError(null);
                }}
              >
                {copy.saveToVault}
              </button>
            </div>
            <button type="button" aria-label={copy.closeLabel} onClick={() => setOpen(false)}>
              <X className="size-3.5 text-zinc-500 hover:text-zinc-100" />
            </button>
          </div>

          {retention === "vault" && (
            <label className="block space-y-1 text-zinc-400">
              <span>{copy.vaultSelection}</span>
              <select
                value={vaultId}
                onChange={(event) => setVaultId(event.target.value)}
                className="w-full rounded border border-zinc-700 bg-zinc-900 px-2 py-1.5 text-zinc-100"
                disabled={loadingVaults}
              >
                <option value="">{loadingVaults ? copy.loadingVaults : copy.chooseVault}</option>
                {vaults.map((vault) => (
                  <option key={vault.id} value={vault.id}>{vault.label}</option>
                ))}
              </select>
            </label>
          )}

          <div className="flex flex-wrap gap-2">
            <input
              ref={inputRef}
              type="file"
              className="hidden"
              accept=".md,.txt,.html,.htm,.json,.pdf,text/markdown,text/plain,text/html,application/json,application/pdf"
              onChange={(event) => {
                const file = event.target.files?.[0];
                if (file) void addFile(file);
              }}
            />
            <Button type="button" size="sm" variant="outline" onClick={() => inputRef.current?.click()} disabled={busy}>
              <FileUp className="mr-1 size-3.5" /> {copy.chooseFile}
            </Button>
            <div className="flex min-w-56 flex-1">
              <input
                value={url}
                onChange={(event) => setUrl(event.target.value)}
                placeholder={copy.urlPlaceholder}
                className="min-w-0 flex-1 rounded-l border border-zinc-700 bg-zinc-900 px-2 py-1.5 text-zinc-100 outline-none focus:border-zinc-500"
              />
              <Button type="button" size="sm" className="rounded-l-none" onClick={() => void addUrl()} disabled={busy || !url.trim()}>
                <LinkIcon className="mr-1 size-3.5" /> {copy.addUrl}
              </Button>
            </div>
          </div>
          <p className="text-zinc-500">
            {retention === "chat"
              ? copy.chatRetention
              : copy.vaultRetention}
          </p>
          {error && <p role="alert" className="text-red-300">{error}</p>}
        </div>
      )}
    </div>
  );
}
