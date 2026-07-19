import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import {
  dictionaries,
  LocaleProvider,
  LOCALE_STORAGE_KEY,
  useI18n,
  type Dictionary,
} from "../../app/lib/i18n";

function LocaleProbe() {
  const { locale, labels, setLocale } = useI18n();
  return (
    <div>
      <span data-testid="locale">{locale}</span>
      <span data-testid="knowledge-label">{labels.knowledge.title}</span>
      <button type="button" onClick={() => setLocale("ko")}>Kor</button>
    </div>
  );
}

describe("locale dictionary", () => {
  beforeEach(() => window.localStorage.clear());

  it("defaults to English, switches to Korean, and persists the selection", async () => {
    render(<LocaleProvider><LocaleProbe /></LocaleProvider>);

    expect(screen.getByTestId("locale")).toHaveTextContent("en");
    expect(screen.getByTestId("knowledge-label")).toHaveTextContent("Knowledge");

    fireEvent.click(screen.getByRole("button", { name: "Kor" }));
    expect(screen.getByTestId("locale")).toHaveTextContent("ko");
    expect(screen.getByTestId("knowledge-label")).toHaveTextContent("지식");
    await waitFor(() => expect(window.localStorage.getItem(LOCALE_STORAGE_KEY)).toBe("ko"));
    expect(document.documentElement.lang).toBe("ko");
  });

  it("keeps every locale on the compile-time dictionary contract", () => {
    const english: Dictionary = dictionaries.en;
    const korean: Dictionary = dictionaries.ko;
    expect(Object.keys(korean.knowledge)).toEqual(Object.keys(english.knowledge));

    // This assertion is intentionally compiled: deleting a dictionary key or
    // addressing an undeclared key must make `tsc --noEmit` fail.
    // @ts-expect-error unknown dictionary keys are not allowed
    void english.knowledge.missingKey;
  });

  it("keeps the I18N-T2 chat and login surfaces paired", () => {
    expect(dictionaries.en.chat.page.ready).toBe("Ready");
    expect(dictionaries.ko.chat.page.ready).toBe("준비됨");
    expect(dictionaries.en.chat.attachments.attach).toBe("Attach");
    expect(dictionaries.ko.chat.attachments.attach).toBe("첨부");
    expect(dictionaries.en.login.submit).toBe("Sign in");
    expect(dictionaries.ko.login.submit).toBe("로그인");
  });

  it("keeps the pilot-visible presentation copy paired", () => {
    expect(dictionaries.en.dashboard.knowledgeFeaturesAvailable).toBe(
      "knowledge features available",
    );
    expect(dictionaries.ko.dashboard.knowledgeFeaturesAvailable).toBe(
      "지식 기능 사용 가능",
    );
    expect(dictionaries.en.review.wikiListOutcomeTitle).toBe("List wiki pages");
    expect(dictionaries.ko.review.wikiListOutcomeTitle).toBe("위키 페이지 목록 확인");
    expect(dictionaries.en.workspace.logsInspectTitle).toBe("Inspect server logs");
    expect(dictionaries.ko.workspace.logsScanTitle).toBe("로그 문제 스캔");
    expect(dictionaries.en.notes.trustExplainerTrigger).toBe("What is this?");
    expect(dictionaries.ko.notes.trustExplainerTrigger).toBe("이게 뭐지?");
    expect(dictionaries.en.notes.createdNextStep).toContain("trusted team knowledge");
    expect(dictionaries.ko.notes.createdNextStep).toBe("검토를 거치면 팀 지식으로 올라갑니다.");
  });
});
