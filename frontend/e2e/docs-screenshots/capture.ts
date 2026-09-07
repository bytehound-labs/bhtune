import { mkdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, type Page } from "@playwright/test";

type ScreenshotScenario = {
  id: string;
  coveredSections: string[];
  output: string;
  capture?: "full-page" | "content-fit";
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

const CONTENT_FIT_MIN_HEIGHT = 480;
const CONTENT_FIT_BOTTOM_MARGIN = 32;

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

  const path = resolve(screenshotDir, scenario.output);
  if (scenario.capture !== "content-fit") {
    await page.screenshot({ path, fullPage: true });
    return;
  }

  const viewport = page.viewportSize();
  if (!viewport) {
    throw new Error(`Scenario ${scenarioId} has no configured viewport`);
  }

  await page.evaluate(() => window.scrollTo(0, 0));
  const content = page.locator("[data-doc-screenshot-content]").first();
  const contentBox = await content.boundingBox();
  if (!contentBox) {
    throw new Error(
      `Scenario ${scenarioId} cannot measure the screenshot content root`,
    );
  }

  const contentHeight = Math.max(
    CONTENT_FIT_MIN_HEIGHT,
    Math.ceil(contentBox.y + contentBox.height + CONTENT_FIT_BOTTOM_MARGIN),
  );

  if (contentHeight >= viewport.height) {
    await page.screenshot({ path, fullPage: true });
    return;
  }

  await page.screenshot({
    path,
    clip: {
      x: 0,
      y: 0,
      width: viewport.width,
      height: contentHeight,
    },
  });
}
