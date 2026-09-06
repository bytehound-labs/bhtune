import { expect, test, type Page } from "@playwright/test";
import { captureScenario, settle } from "./capture";
import { installDemoRoutes, runningDemoRun } from "./fixtures";

async function openDemoTune(page: Page) {
  await page.goto("/runs/new");
  await settle(page);
  await expect(
    page.getByRole("heading", { name: "BHTune Simulator Demo" }),
  ).toBeVisible();
}

async function showLiveRun(page: Page) {
  await installDemoRoutes(page);
  await page.route("**/api/runs/4242", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(runningDemoRun),
    }),
  );
  await page.goto("/runs/4242");
  await settle(page);
  await expect(
    page.getByText("Tune in progress — collecting live measurements."),
  ).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  await installDemoRoutes(page);
});

test("demo-tune", async ({ page }) => {
  await openDemoTune(page);
  await captureScenario(page, "demo-tune");
});

test("demo-history", async ({ page }) => {
  await page.goto("/runs");
  await settle(page);
  await expect(page.getByRole("heading", { name: "History" })).toBeVisible();
  await captureScenario(page, "demo-history");
});

test("demo-run-live", async ({ page }) => {
  await showLiveRun(page);
  await captureScenario(page, "demo-run-live");
});

test("demo-run-complete", async ({ page }) => {
  await page.goto("/runs/4242");
  await settle(page);
  await expect(
    page.locator("dd").filter({ hasText: /^Completed$/ }),
  ).toBeVisible();
  await captureScenario(page, "demo-run-complete");
});
