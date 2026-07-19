"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import {
  ACTIVE_CONVERSATION_EVENT,
  getActiveConversationId,
  setActiveConversationId,
} from "./conversation-id";
import { safeRandomUUID } from "./uuid";
import { currentDictionary } from "./i18n";

const SUBJECT_PREFIX = "gadgetron_subject_";
const SUBJECT_EVENT = "gadgetron:workbench-subject";
export const PENNY_COMPANION_EVENT = "gadgetron:penny-companion-open";

export interface WorkbenchSubject {
  id: string;
  kind: string;
  bundle: string;
  title: string;
  subtitle?: string;
  href?: string;
  summary?: string;
  facts?: Record<string, unknown>;
  prompt?: string;
  createdAt?: string;
  related?: WorkbenchRelatedRef[];
}

export interface WorkbenchRelatedRef {
  id: string;
  kind:
    | "server"
    | "log_finding"
    | "metric"
    | "knowledge_page"
    | "approval"
    | "activity";
  title: string;
  subtitle?: string;
  href?: string;
  status?: "ok" | "info" | "warning" | "critical" | "pending";
  summary?: string;
}

interface WorkbenchSubjectContextValue {
  activeConversationId: string | null;
  subject: WorkbenchSubject | null;
  refresh: () => void;
  refreshSubject: () => void;
  clearActiveSubject: () => void;
}

interface StartPennyDiscussionOptions {
  conversationId?: string;
  autoSubmit?: boolean;
  navigateTo?: string;
  navigate?: (href: string) => void;
  surface?: "page" | "companion";
}

const WorkbenchSubjectCtx =
  createContext<WorkbenchSubjectContextValue | null>(null);

function storageKey(conversationId: string): string {
  return `${SUBJECT_PREFIX}${conversationId}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value != null && typeof value === "object" && !Array.isArray(value);
}

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function parseRelatedRef(value: unknown): WorkbenchRelatedRef | null {
  if (!isRecord(value)) return null;
  const id = value.id;
  const kind = value.kind;
  const title = value.title;
  if (
    typeof id !== "string" ||
    typeof kind !== "string" ||
    typeof title !== "string"
  ) {
    return null;
  }
  const allowedKinds = new Set([
    "server",
    "log_finding",
    "metric",
    "knowledge_page",
    "approval",
    "activity",
  ]);
  if (!allowedKinds.has(kind)) return null;
  const status =
    typeof value.status === "string" &&
    ["ok", "info", "warning", "critical", "pending"].includes(value.status)
      ? (value.status as WorkbenchRelatedRef["status"])
      : undefined;
  return {
    id,
    kind: kind as WorkbenchRelatedRef["kind"],
    title,
    subtitle: optionalString(value.subtitle),
    href: optionalString(value.href),
    status,
    summary: optionalString(value.summary),
  };
}

function parseRelated(value: unknown): WorkbenchRelatedRef[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const related = value
    .map(parseRelatedRef)
    .filter((ref): ref is WorkbenchRelatedRef => Boolean(ref));
  return related.length > 0 ? related : undefined;
}

export function parseWorkbenchSubject(value: unknown): WorkbenchSubject | null {
  if (!isRecord(value)) return null;
  const id = value.id;
  const kind = value.kind;
  const bundle = value.bundle;
  const title = value.title;
  if (
    typeof id !== "string" ||
    typeof kind !== "string" ||
    typeof bundle !== "string" ||
    typeof title !== "string"
  ) {
    return null;
  }
  const facts = isRecord(value.facts) ? value.facts : undefined;
  return {
    id,
    kind,
    bundle,
    title,
    subtitle: optionalString(value.subtitle),
    href: optionalString(value.href),
    summary: optionalString(value.summary),
    facts,
    prompt: optionalString(value.prompt),
    createdAt: optionalString(value.createdAt),
    related: parseRelated(value.related),
  };
}

function emitSubjectChange(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new Event(SUBJECT_EVENT));
}

export function readConversationSubject(
  conversationId: string,
): WorkbenchSubject | null {
  if (typeof window === "undefined") return null;
  const raw = window.localStorage.getItem(storageKey(conversationId));
  if (!raw) return null;
  try {
    return parseWorkbenchSubject(JSON.parse(raw));
  } catch {
    return null;
  }
}

export function writeConversationSubject(
  conversationId: string,
  subject: WorkbenchSubject,
): void {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(storageKey(conversationId), JSON.stringify(subject));
  emitSubjectChange();
}

export function clearConversationSubject(conversationId: string): void {
  if (typeof window === "undefined") return;
  window.localStorage.removeItem(storageKey(conversationId));
  emitSubjectChange();
}

export function buildSubjectDraft(subject: WorkbenchSubject): string {
  const copy = currentDictionary().chat.subject;
  const lines: string[] = [];
  lines.push(
    subject.prompt ??
      copy.defaultPrompt,
  );
  lines.push("");
  lines.push(`${copy.subject}: ${subject.title}`);
  if (subject.subtitle) lines.push(`${copy.subtitle}: ${subject.subtitle}`);
  lines.push(`${copy.bundle}: ${subject.bundle}`);
  lines.push(`${copy.kind}: ${subject.kind}`);
  if (subject.href) lines.push(`${copy.source}: ${subject.href}`);
  if (subject.summary) {
    lines.push("");
    lines.push(`${copy.summary}:`);
    lines.push(subject.summary);
  }
  if (subject.facts && Object.keys(subject.facts).length > 0) {
    lines.push("");
    lines.push(`${copy.facts}:`);
    lines.push("```json");
    lines.push(JSON.stringify(subject.facts, null, 2));
    lines.push("```");
  }
  if (subject.related && subject.related.length > 0) {
    lines.push("");
    lines.push(`${copy.related}:`);
    for (const ref of subject.related) {
      const parts = [ref.kind, ref.status, ref.href].filter(Boolean).join(" | ");
      lines.push(`- ${ref.title}${parts ? ` (${parts})` : ""}`);
      if (ref.summary) lines.push(`  ${ref.summary}`);
    }
  }
  return lines.join("\n");
}

/**
 * Guaranteed-delivery helper (ISSUE 53): combine the active
 * conversation's pinned subject context with an outgoing first
 * message. The seeded auto-send can silently lose its race with the
 * chat runtime spinning up after a full-page navigation — when that
 * happens the operator types a vague question about "this bug"
 * and Penny has no idea which bug it means. Callers apply this to the
 * FIRST message of a conversation; later turns rely on the transcript.
 * Returns `text` unchanged when there is no subject, the text is a
 * slash command, or the text already carries the draft.
 */
export function withSubjectContext(text: string): string {
  const trimmed = text.trim();
  if (!trimmed || trimmed.startsWith("/")) return text;
  const conversationId = getActiveConversationId();
  if (!conversationId) return text;
  const subject = readConversationSubject(conversationId);
  if (!subject) return text;
  const draft = buildSubjectDraft(subject);
  if (trimmed.startsWith(draft.slice(0, 80))) return text;
  return `${draft}\n\n---\n\n${currentDictionary().chat.subject.question}: ${trimmed}`;
}

export function startPennyDiscussion(
  subject: WorkbenchSubject,
  options: StartPennyDiscussionOptions = {},
): string {
  const conversationId = options.conversationId ?? safeRandomUUID();
  const navigateTo = options.navigateTo ?? "/web";
  const nextSubject = {
    ...subject,
    createdAt: subject.createdAt ?? new Date().toISOString(),
  };

  setActiveConversationId(conversationId);
  writeConversationSubject(conversationId, nextSubject);

  if (typeof window !== "undefined") {
    window.localStorage.setItem(
      `gadgetron_draft_${conversationId}`,
      buildSubjectDraft(nextSubject),
    );
    if (options.autoSubmit) {
      window.localStorage.setItem(
        `gadgetron_pending_submit_${conversationId}`,
        "1",
      );
    } else {
      window.localStorage.removeItem(
        `gadgetron_pending_submit_${conversationId}`,
      );
    }
  }

  if (options.surface === "companion") {
    if (typeof window !== "undefined") {
      window.dispatchEvent(new Event(PENNY_COMPANION_EVENT));
    }
    return conversationId;
  }

  const navigate =
    options.navigate ??
    ((href: string) => {
      if (typeof window !== "undefined") window.location.assign(href);
    });
  navigate(navigateTo);
  return conversationId;
}

function readActiveState(): {
  activeConversationId: string | null;
  subject: WorkbenchSubject | null;
} {
  const activeConversationId = getActiveConversationId();
  return {
    activeConversationId,
    subject: activeConversationId
      ? readConversationSubject(activeConversationId)
      : null,
  };
}

export function WorkbenchSubjectProvider({
  children,
}: {
  children: ReactNode;
}) {
  const [state, setState] = useState(readActiveState);
  const refresh = useCallback(() => {
    setState(readActiveState());
  }, []);
  const clearActiveSubject = useCallback(() => {
    if (state.activeConversationId) {
      clearConversationSubject(state.activeConversationId);
    }
    refresh();
  }, [refresh, state.activeConversationId]);

  useEffect(() => {
    refresh();
    window.addEventListener(ACTIVE_CONVERSATION_EVENT, refresh);
    window.addEventListener(SUBJECT_EVENT, refresh);
    window.addEventListener("focus", refresh);
    window.addEventListener("storage", refresh);
    return () => {
      window.removeEventListener(ACTIVE_CONVERSATION_EVENT, refresh);
      window.removeEventListener(SUBJECT_EVENT, refresh);
      window.removeEventListener("focus", refresh);
      window.removeEventListener("storage", refresh);
    };
  }, [refresh]);

  return (
    <WorkbenchSubjectCtx.Provider
      value={{
        activeConversationId: state.activeConversationId,
        subject: state.subject,
        refresh,
        refreshSubject: refresh,
        clearActiveSubject,
      }}
    >
      {children}
    </WorkbenchSubjectCtx.Provider>
  );
}

export function useWorkbenchSubject(): WorkbenchSubjectContextValue {
  const ctx = useContext(WorkbenchSubjectCtx);
  if (ctx) return ctx;
  const state = readActiveState();
  return {
    activeConversationId: state.activeConversationId,
    subject: state.subject,
    refresh: () => {},
    refreshSubject: () => {},
    clearActiveSubject: () => {
      if (state.activeConversationId) {
        clearConversationSubject(state.activeConversationId);
      }
    },
  };
}
