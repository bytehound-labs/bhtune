import { mkdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, type Page } from "@playwright/test";

type ScreenshotScenario = {
  id: string;
  coveredSections: string[];
  output: string;
};

type ScreenshotManifest = {
  scenarios: ScreenshotScenario[];
};

const screenshotDir = resolve(
  process.env.DOCS_SCREENSHOT_DIR ?? "../website/static/generated/web-ui",
);
const manifestPath = resolve(
  process.cwd(),
  "../docs/reference/web-ui-screenshots.json",
);
const manifest = JSON.parse(
  readFileSync(manifestPath, "utf8"),
) as ScreenshotManifest;

mkdirSync(screenshotDir, { recursive: true });

export async function settle(page: Page) {
  await page.waitForLoadState("domcontentloaded");
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.evaluate(() => document.fonts?.ready);
  await page.waitForTimeout(250);
}

export async function captureScenario(page: Page, scenarioId: string) {
  const scenario = manifest.scenarios.find(({ id }) => id === scenarioId);
  if (!scenario) {
    throw new Error(
      `Screenshot scenario is missing from the manifest: ${scenarioId}`,
    );
  }

  for (const sectionId of scenario.coveredSections) {
    await expect(
      page.locator(`[data-doc-section="${sectionId}"]`).first(),
      `Scenario ${scenarioId} must show documentation section ${sectionId}`,
    ).toBeVisible();
  }

  await page.screenshot({
    path: resolve(screenshotDir, scenario.output),
    fullPage: true,
  });
}
