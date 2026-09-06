import { expect, test, type Page } from "@playwright/test";
import { captureScenario, settle } from "./capture";
import { installFullRoutes, runningFullRun } from "./fixtures";

async function openNewTune(page: Page) {
  await page.goto("/runs/new");
  await settle(page);
  await expect(page.getByRole("heading", { name: "New tune" })).toBeVisible();
}

async function openOpcTune(page: Page) {
  await openNewTune(page);
  await page.getByLabel("Driver").selectOption("opcda");
  await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.Example");
  await page.getByLabel("Tag name").fill("Area01.FIC101.PV");
  await page.getByText("Loop mapping", { exact: true }).click();
  await settle(page);
}

async function showLiveRun(page: Page) {
  await installFullRoutes(page);
  await page.route("**/api/runs/4242", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(runningFullRun),
    }),
  );
  await page.goto("/runs/4242");
  await settle(page);
  await expect(
    page.getByText("Tune in progress — collecting live measurements."),
  ).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  await installFullRoutes(page);
});

test("full-tune-simulator", async ({ page }) => {
  await openNewTune(page);
  await captureScenario(page, "full-tune-simulator");
});

test("full-tune-opc", async ({ page }) => {
  await openOpcTune(page);
  await captureScenario(page, "full-tune-opc");
});

test("full-opc-server-picker", async ({ page }) => {
  await openOpcTune(page);
  await page.getByRole("button", { name: "Browse servers" }).click();
  await expect(
    page.getByRole("heading", { name: "Browse OPC DA servers" }),
  ).toBeVisible();
  await captureScenario(page, "full-opc-server-picker");
});

test("full-opc-tag-browser", async ({ page }) => {
  await openOpcTune(page);
  await page.getByRole("button", { name: "Browse tags" }).click();
  await expect(
    page.getByRole("heading", { name: /Browse tags on/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Area01", exact: true }).click();
  await page
    .getByRole("button", { name: "Area01.FIC101", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Area01.FIC101.OUT", exact: true })
    .click();
  await page.getByRole("button", { name: "Read selected tag" }).click();
  await expect(page.getByText("Good", { exact: true })).toBeVisible();
  await captureScenario(page, "full-opc-tag-browser");
});

test("full-opc-tag-applied", async ({ page }) => {
  await openOpcTune(page);
  await page.getByRole("button", { name: "Browse tags" }).click();
  await page.getByRole("button", { name: "Area01", exact: true }).click();
  await page
    .getByRole("button", { name: "Area01.FIC101", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Area01.FIC101.OUT", exact: true })
    .click();
  await page.getByRole("button", { name: "Select tag" }).click();
  await expect(page.getByLabel("Tag name")).toHaveValue("Area01.FIC101.PV");
  await page.getByText("Loop mapping", { exact: true }).click();
  await settle(page);
  await captureScenario(page, "full-opc-tag-applied");
});

test("full-history", async ({ page }) => {
  await page.goto("/runs");
  await settle(page);
  await expect(page.getByRole("heading", { name: "History" })).toBeVisible();
  await captureScenario(page, "full-history");
});

test("full-run-live", async ({ page }) => {
  await showLiveRun(page);
  await captureScenario(page, "full-run-live");
});

test("full-run-complete", async ({ page }) => {
  await page.goto("/runs/4242");
  await settle(page);
  await expect(
    page.locator("dd").filter({ hasText: /^Completed$/ }),
  ).toBeVisible();
  await captureScenario(page, "full-run-complete");
});

test("full-pid-review", async ({ page }) => {
  await page.goto("/runs/4242");
  await settle(page);
  await page.getByRole("button", { name: "Review & write" }).first().click();
  await expect(
    page.getByRole("heading", { name: "Review PID settings" }),
  ).toBeVisible();
  await captureScenario(page, "full-pid-review");
});

test("full-template-list", async ({ page }) => {
  await page.goto("/templates");
  await settle(page);
  await expect(page.getByRole("heading", { name: "Templates" })).toBeVisible();
  await captureScenario(page, "full-template-list");
});

test("full-template-detail", async ({ page }) => {
  await page.goto("/templates/Yokogawa%20CentumVP");
  await settle(page);
  await expect(
    page.getByRole("heading", { name: "Yokogawa CentumVP" }),
  ).toBeVisible();
  await captureScenario(page, "full-template-detail");
});

test("full-template-create", async ({ page }) => {
  await page.goto("/templates/new");
  await settle(page);
  await expect(
    page.getByRole("heading", { name: "New template", exact: true }),
  ).toBeVisible();
  await captureScenario(page, "full-template-create");
});

test("full-template-edit", async ({ page }) => {
  await page.goto("/templates/Example%20Template/edit");
  await settle(page);
  await expect(
    page.getByRole("heading", { name: "Edit Example Template", exact: true }),
  ).toBeVisible();
  await captureScenario(page, "full-template-edit");
});

test("full-config", async ({ page }) => {
  await page.goto("/config");
  await settle(page);
  await expect(
    page.getByRole("heading", { name: "Configuration", exact: true }),
  ).toBeVisible();
  await captureScenario(page, "full-config");
});
