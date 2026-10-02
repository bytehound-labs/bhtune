import { AxeBuilder } from "@axe-core/playwright";
import { expect, type Page } from "@playwright/test";

const WCAG_TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"];

export async function expectNoAccessibilityViolations(
  page: Page,
  scope?: string,
) {
  let axe = new AxeBuilder({ page }).withTags(WCAG_TAGS);
  if (scope) axe = axe.include(scope);
  const { violations } = await axe.analyze();
  const details = violations
    .map(
      (violation) =>
        `${violation.id} (${violation.impact ?? "unknown"}): ${violation.help}\n${violation.nodes
          .map(
            (node) =>
              `  ${node.target.join(", ")}\n${node.failureSummary ?? ""}`,
          )
          .join("\n")}`,
    )
    .join("\n\n");

  expect(violations, details).toHaveLength(0);
}

export async function setTheme(page: Page, theme: "dark" | "light") {
  const root = page.locator("html");
  if ((await root.getAttribute("data-theme")) !== theme) {
    await page
      .getByRole("button", { name: `Switch to Catppuccin ${theme} theme` })
      .click();
    await page.waitForFunction(() =>
      document.getAnimations().every((animation) => {
        const timing = animation.effect?.getComputedTiming();
        return (
          timing?.endTime === Number.POSITIVE_INFINITY ||
          animation.playState === "finished" ||
          animation.playState === "idle"
        );
      }),
    );
  }
  await expect(root).toHaveAttribute("data-theme", theme);
}

export async function expectAccessibilityInBothThemes(
  page: Page,
  scope?: string,
) {
  await setTheme(page, "dark");
  await expectNoAccessibilityViolations(page, scope);
  await setTheme(page, "light");
  await expectNoAccessibilityViolations(page, scope);
}
