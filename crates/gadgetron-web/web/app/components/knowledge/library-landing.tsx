"use client";

import { BookOpen, FileText, FileUp } from "lucide-react";
import { useMemo } from "react";

import { useI18n } from "../../lib/i18n";
import {
  type KnowledgeObject,
  type KnowledgeSource,
  type KnowledgeVault,
} from "../../lib/knowledge-workbench-api";
import { Button } from "../ui/button";
import { EmptyState } from "../workbench";
import { displayDate, humanizeIdentifier, noteTitle } from "./display";

type LibraryRow = {
  id: string;
  kind: "material" | "knowledge";
  title: string;
  domain: string;
  updatedAt: string;
};

export function LibraryLanding({
  sources,
  objects,
  vaults,
  loading,
  onOpenSource,
  onOpenObject,
  onAddMaterial,
}: {
  sources: KnowledgeSource[];
  objects: KnowledgeObject[];
  vaults: KnowledgeVault[];
  loading: boolean;
  onOpenSource: (sourceId: string) => void;
  onOpenObject: (objectId: string) => void;
  onAddMaterial: () => void;
}) {
  const { labels } = useI18n();
  const rows = useMemo<LibraryRow[]>(() => {
    const domainsByVault = new Map(vaults.map((vault) => [vault.id, vault.home_bundle_id]));
    return [
      ...sources.map((source) => ({
        id: source.id,
        kind: "material" as const,
        title: source.title || source.original_name,
        domain: domainsByVault.get(source.vault_id) ?? "core",
        updatedAt: source.updated_at,
      })),
      ...objects.map((object) => ({
        id: object.id,
        kind: "knowledge" as const,
        title: object.title || noteTitle(object.path),
        domain: object.home_bundle_id,
        updatedAt: object.updated_at,
      })),
    ].sort((left, right) =>
      right.updatedAt.localeCompare(left.updatedAt)
      || left.kind.localeCompare(right.kind)
      || left.id.localeCompare(right.id));
  }, [objects, sources, vaults]);

  return (
    <section className="space-y-3" data-testid="knowledge-library-landing" aria-busy={loading}>
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium text-zinc-100">{labels.knowledge.library}</h2>
          <p className="mt-1 text-xs text-zinc-500">{labels.knowledge.libraryDescription}</p>
        </div>
        <Button size="sm" onClick={onAddMaterial}>
          <FileUp className="mr-1.5 size-3.5" aria-hidden />
          {labels.knowledge.addMaterial}
        </Button>
      </header>

      {!loading && rows.length === 0 ? (
        <EmptyState
          title={labels.knowledge.libraryEmptyTitle}
          description={labels.knowledge.libraryEmptyDescription}
          action={<Button size="sm" onClick={onAddMaterial}>{labels.knowledge.addMaterial}</Button>}
        />
      ) : (
        <div className="overflow-hidden rounded border border-zinc-800 bg-zinc-950/50">
          <div className="flex items-center justify-between border-b border-zinc-800 px-4 py-2">
            <span className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">
              {labels.knowledge.recentLibraryItems}
            </span>
            <span className="font-mono text-[10px] text-zinc-600">
              {labels.knowledge.libraryCounts(sources.length, objects.length)}
            </span>
          </div>
          <ul className="divide-y divide-zinc-800" aria-label={labels.knowledge.recentLibraryItems}>
            {rows.map((row) => {
              const material = row.kind === "material";
              const Icon = material ? FileText : BookOpen;
              return (
                <li key={`${row.kind}:${row.id}`}>
                  <button
                    type="button"
                    className="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-zinc-900/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-[#B87333]"
                    onClick={() => material ? onOpenSource(row.id) : onOpenObject(row.id)}
                  >
                    <Icon className="size-4 shrink-0 text-zinc-600" aria-hidden />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-xs font-medium text-zinc-200">{row.title}</span>
                      <span className="mt-1 block truncate text-[10px] text-zinc-500">
                        {humanizeIdentifier(row.domain)} · {displayDate(row.updatedAt)}
                      </span>
                    </span>
                    <span className="shrink-0 rounded border border-zinc-700 px-2 py-0.5 text-[10px] text-zinc-400">
                      {material ? labels.knowledge.resultMaterial : labels.knowledge.resultKnowledge}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </section>
  );
}
