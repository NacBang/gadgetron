import { describe, expect, it } from "vitest";

import { searchCommandIndex, type CommandIndexEntry } from "../../app/lib/command-index";

const commands: CommandIndexEntry[] = [
  { id: "dashboard", label: "Dashboard", description: "Open overview", keywords: ["status"] },
  { id: "knowledge", label: "Knowledge", description: "Open library", keywords: ["materials"] },
  { id: "search", label: "Search Knowledge", description: "Find accessible sources", keywords: ["find"] },
];

describe("shared command index", () => {
  it("keeps prefix matches before token-complete substring matches", () => {
    expect(searchCommandIndex(commands, "knowledge").map((command) => command.id)).toEqual([
      "knowledge",
      "search",
    ]);
  });

  it("matches localized or alias keywords without changing declared order", () => {
    expect(searchCommandIndex(commands, "materials").map((command) => command.id)).toEqual([
      "knowledge",
    ]);
    expect(searchCommandIndex(commands, "open overview").map((command) => command.id)).toEqual([
      "dashboard",
    ]);
  });
});
