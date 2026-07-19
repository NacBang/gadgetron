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
});
