import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  ACTIVE_CONVERSATION_EVENT,
  clearActiveConversationId,
  ensureActiveConversationId,
  getActiveConversationId,
  setActiveConversationId,
} from "../../app/lib/conversation-id";

const createStorageMock = () => {
  let store: Record<string, string> = {};
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => {
      store[key] = value;
    },
    removeItem: (key: string) => {
      delete store[key];
    },
    clear: () => {
      store = {};
    },
  };
};

const localStorageMock = createStorageMock();
const sessionStorageMock = createStorageMock();

Object.defineProperty(window, "localStorage", {
  value: localStorageMock,
  configurable: true,
});
Object.defineProperty(window, "sessionStorage", {
  value: sessionStorageMock,
  configurable: true,
});

describe("conversation id storage", () => {
  beforeEach(() => {
    localStorageMock.clear();
    sessionStorageMock.clear();
    vi.restoreAllMocks();
  });

  it("mints and stores a tab-local id when the shell has none", () => {
    const onActive = vi.fn();
    window.addEventListener(ACTIVE_CONVERSATION_EVENT, onActive);

    expect(ensureActiveConversationId(() => "conv-shell")).toBe("conv-shell");

    expect(getActiveConversationId()).toBe("conv-shell");
    expect(window.sessionStorage.getItem("gadgetron_conversation_id")).toBe(
      "conv-shell",
    );
    expect(window.localStorage.getItem("gadgetron_conversation_id")).toBe(
      "conv-shell",
    );
    expect(onActive).toHaveBeenCalledTimes(1);

    window.removeEventListener(ACTIVE_CONVERSATION_EVENT, onActive);
  });

  it("reuses the active tab id instead of minting a second backend id", () => {
    const createId = vi.fn(() => "wrong-new-id");
    setActiveConversationId("conv-existing");

    expect(ensureActiveConversationId(createId)).toBe("conv-existing");
    expect(createId).not.toHaveBeenCalled();
  });

  it("promotes the legacy cross-tab seed before minting", () => {
    window.localStorage.setItem("gadgetron_conversation_id", "legacy-conv");

    expect(ensureActiveConversationId(() => "wrong-new-id")).toBe("legacy-conv");
    expect(window.sessionStorage.getItem("gadgetron_conversation_id")).toBe(
      "legacy-conv",
    );
  });

  it("can clear the tab id for delete/logout flows", () => {
    setActiveConversationId("conv-delete");

    clearActiveConversationId();

    expect(window.sessionStorage.getItem("gadgetron_conversation_id")).toBeNull();
    expect(window.localStorage.getItem("gadgetron_conversation_id")).toBeNull();
  });
});
