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

const SUBJECT_PREFIX = "gadgetron_subject_";
const SUBJECT_EVENT = "gadgetron:workbench-subject";

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
}

interface WorkbenchSubjectContextValue {
  activeConversationId: string | null;
  subject: WorkbenchSubject | null;
  refresh: () => void;
  clearActiveSubject: () => void;
}

interface StartPennyDiscussionOptions {
  conversationId?: string;
  autoSubmit?: boolean;
  navigateTo?: string;
  navigate?: (href: string) => void;
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

function parseSubject(value: unknown): WorkbenchSubject | null {
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
    return parseSubject(JSON.parse(raw));
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
  const lines: string[] = [];
  lines.push(
    subject.prompt ??
      "Review this workbench context with me and recommend the next step.",
  );
  lines.push("");
  lines.push(`Subject: ${subject.title}`);
  if (subject.subtitle) lines.push(`Subtitle: ${subject.subtitle}`);
  lines.push(`Bundle: ${subject.bundle}`);
  lines.push(`Kind: ${subject.kind}`);
  if (subject.href) lines.push(`Source: ${subject.href}`);
  if (subject.summary) {
    lines.push("");
    lines.push("Summary:");
    lines.push(subject.summary);
  }
  if (subject.facts && Object.keys(subject.facts).length > 0) {
    lines.push("");
    lines.push("Facts:");
    lines.push("```json");
    lines.push(JSON.stringify(subject.facts, null, 2));
    lines.push("```");
  }
  return lines.join("\n");
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
    clearActiveSubject: () => {
      if (state.activeConversationId) {
        clearConversationSubject(state.activeConversationId);
      }
    },
  };
}
