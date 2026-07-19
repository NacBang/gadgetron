import AxeBuilder from "@axe-core/playwright";
import { expect, type Page } from "@playwright/test";

const wcagTags = [
  "wcag2a",
  "wcag2aa",
  "wcag21a",
  "wcag21aa",
  "wcag22a",
  "wcag22aa",
];

export async function expectAccessible(page: Page, include?: string) {
  const builder = new AxeBuilder({ page }).withTags(wcagTags);
  const result = await (include ? builder.include(include) : builder).analyze();
  expect(result.violations.flatMap((violation) =>
    violation.nodes.map((node) => ({
      rule: violation.id,
      target: node.target.join(" > "),
    })),
  )).toEqual([]);
}

export async function expectReadableTextControls(
  page: Page,
  include?: string,
) {
  const controls = include
    ? page.locator(include).locator("button, input, select, textarea, summary")
    : page.locator("button, input, select, textarea, summary");
  const undersized = await controls.evaluateAll((elements) =>
    elements.flatMap((element) => {
      const html = element as HTMLElement;
      const rect = html.getBoundingClientRect();
      const tag = html.tagName.toLocaleLowerCase();
      const hasText = Boolean(html.textContent?.trim())
        || tag === "input"
        || tag === "select"
        || tag === "textarea";
      const size = Number.parseFloat(getComputedStyle(html).fontSize);
      if (!hasText || rect.width === 0 || rect.height === 0 || size >= 12) {
        return [];
      }
      return [{
        tag,
        label:
          html.getAttribute("aria-label")
          || html.textContent?.trim().slice(0, 80),
        font_size: size,
      }];
    }),
  );
  expect(undersized).toEqual([]);
}
