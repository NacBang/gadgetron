import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ChatAttachmentTray } from "@/components/chat/chat-attachment-tray";
import { LocaleProvider } from "@/lib/i18n";
import type { KnowledgeSource } from "@/lib/knowledge-workbench-api";

const api = vi.hoisted(() => ({
  deleteChatAttachment: vi.fn(),
  fetchChatAttachment: vi.fn(),
  fetchKnowledgeSource: vi.fn(),
  listChatAttachments: vi.fn(),
  listKnowledgeSpaces: vi.fn(),
  listKnowledgeVaults: vi.fn(),
  promoteChatAttachment: vi.fn(),
  retryChatAttachment: vi.fn(),
  uploadChatAttachment: vi.fn(),
  uploadKnowledgeSource: vi.fn(),
}));

vi.mock("@/lib/auth-context", () => ({
  useAuth: () => ({ apiKey: null }),
}));

vi.mock("@/lib/knowledge-workbench-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/knowledge-workbench-api")>(
    "@/lib/knowledge-workbench-api",
  );
  return { ...actual, ...api };
});

function source(status: KnowledgeSource["status"], suffix: string): KnowledgeSource {
  return {
    id: `source-${suffix}`,
    vault_id: "vault-personal",
    conversation_id: "conversation-1",
    source_kind: "chat_attachment",
    status,
    title: `Attachment ${suffix}`,
    original_name: `${suffix}.md`,
    attempt_count: 1,
    revision: 3,
    created_at: "2026-07-17T00:00:00Z",
    updated_at: "2026-07-17T00:00:00Z",
  };
}

describe("ChatAttachmentTray", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.listChatAttachments.mockResolvedValue([]);
    api.listKnowledgeSpaces.mockResolvedValue([]);
    api.listKnowledgeVaults.mockResolvedValue([]);
  });

  it("shows the upload-to-citation states beside the composer", async () => {
    api.listChatAttachments.mockResolvedValue([
      source("pending", "pending"),
      source("extracted", "ready"),
      source("failed", "failed"),
      source("needs_ocr", "ocr"),
    ]);

    render(<ChatAttachmentTray conversationId="conversation-1" />);

    expect(await screen.findByText("Citation-ready")).toBeInTheDocument();
    expect(screen.getByText("Extracting")).toBeInTheDocument();
    expect(screen.getByText("Failed")).toBeInTheDocument();
    expect(screen.getByText("Needs OCR")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Retry attachment" })).toHaveLength(2);
  });

  it("defaults to chat-only and sends a dropped file to the purgeable endpoint", async () => {
    api.uploadChatAttachment.mockResolvedValue({
      source: source("extracted", "ready"),
      object: {},
    });
    const { container } = render(<ChatAttachmentTray conversationId="conversation-1" />);
    fireEvent.click(screen.getByRole("button", { name: "Attach" }));
    expect(screen.getByRole("button", { name: "This chat only" })).toHaveClass("bg-zinc-700");

    const input = container.querySelector<HTMLInputElement>('input[type="file"]');
    expect(input).not.toBeNull();
    const file = new File(["# bounded"], "bounded.md", { type: "text/markdown" });
    fireEvent.change(input!, { target: { files: [file] } });

    await waitFor(() => expect(api.uploadChatAttachment).toHaveBeenCalledWith(
      null,
      "conversation-1",
      file,
    ));
    expect(api.uploadKnowledgeSource).not.toHaveBeenCalled();
  });

  it("requires an explicit Vault selection and never auto-promotes", async () => {
    api.listKnowledgeSpaces.mockResolvedValue([
      { id: "team-space", title: "Operations", kind: "team" },
    ]);
    api.listKnowledgeVaults.mockResolvedValue([
      { id: "team-vault", space_id: "team-space", home_bundle_id: "server-administrator" },
    ]);
    const { container } = render(<ChatAttachmentTray conversationId="conversation-1" />);
    fireEvent.click(screen.getByRole("button", { name: "Attach" }));
    fireEvent.click(screen.getByRole("button", { name: "Save to Vault" }));

    const input = container.querySelector<HTMLInputElement>('input[type="file"]');
    const file = new File(["explicit"], "explicit.txt", { type: "text/plain" });
    fireEvent.change(input!, { target: { files: [file] } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Choose a Space and Vault first.",
    );
    expect(api.uploadKnowledgeSource).not.toHaveBeenCalled();

    await screen.findByRole("option", { name: "Operations · server-administrator" });
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "team-vault" } });
    fireEvent.change(input!, { target: { files: [file] } });
    await waitFor(() => expect(api.uploadKnowledgeSource).toHaveBeenCalledWith(
      null,
      "team-vault",
      file,
      "explicit.txt",
      "conversation-1",
    ));
  });

  it("switches attachment controls and status copy to Korean", async () => {
    api.listChatAttachments.mockResolvedValue([source("extracted", "ready")]);

    render(
      <LocaleProvider initialLocale="ko">
        <ChatAttachmentTray conversationId="conversation-1" />
      </LocaleProvider>,
    );

    expect(await screen.findByText("인용 준비 완료")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "첨부" }));
    expect(screen.getByRole("button", { name: "이 대화에만" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Vault에 저장" })).toBeInTheDocument();
  });
});
