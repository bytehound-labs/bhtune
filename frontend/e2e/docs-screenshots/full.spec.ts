import { expect, test, type Locator, type Page } from "@playwright/test";
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

async function assertConfirmationCopy(
  page: Page,
  title: string,
  body: string,
  continuation: string,
  confirmLabel: string,
) {
  const dialog = page.getByRole("dialog", { name: title });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("heading", { name: title })).toBeVisible();
  await expect(dialog.getByText(body, { exact: true })).toBeVisible();
  await expect(dialog.getByText(continuation, { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
  await expect(
    dialog.getByRole("button", { name: confirmLabel, exact: true }),
  ).toBeEnabled();
  return dialog;
}

async function assertPendingLocks(
  page: Page,
  dialog: Locator,
  pendingLabel: string,
) {
  await expect(dialog.getByRole("status")).toHaveText(
    `${pendingLabel} Do not close this dialog.`,
  );
  await expect(
    dialog.getByRole("button", { name: pendingLabel, exact: true }),
  ).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "Close" })).toBeDisabled();
  const backdrop = dialog.locator("..").getByRole("button", {
    name: "Dismiss modal backdrop",
  });
  await expect(backdrop).toBeDisabled();
  await backdrop.dispatchEvent("click");
  await page.keyboard.press("Escape");
  await expect(dialog).toBeVisible();
}

function trackDeleteRequests(page: Page, pathname: string) {
  let count = 0;
  page.on("request", (request) => {
    if (
      request.method() === "DELETE" &&
      new URL(request.url()).pathname === pathname
    ) {
      count += 1;
    }
  });
  return () => count;
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

test("full-opc-search-index-delete-confirmation", async ({ page }) => {
  await installFullRoutes(page, { searchIndexReady: true });
  await openOpcTune(page);
  await page.getByRole("button", { name: "Browse tags" }).click();
  await expect(
    page.getByRole("button", { name: "Delete index", exact: true }),
  ).toBeVisible();

  const deleteCount = trackDeleteRequests(page, "/api/opc/search-index");
  await page.getByRole("button", { name: "Delete index", exact: true }).click();
  const dialog = await assertConfirmationCopy(
    page,
    "Delete tag index?",
    "Delete the namespace index for Yokogawa.Example?",
    "Indexed search data and this server's index enrollment will be removed. Lazy browsing, direct ItemID entry, live reads, and tuning remain available.",
    "Delete index",
  );
  const confirm = dialog.getByRole("button", {
    name: "Delete index",
    exact: true,
  });
  const firstResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "DELETE" &&
      new URL(response.url()).pathname === "/api/opc/search-index",
  );
  await confirm.click();
  await assertPendingLocks(page, dialog, "Deleting index…");
  await firstResponse;
  await expect(
    dialog.getByText("The server could not complete the request. Try again.", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(dialog).toBeVisible();
  await expect(
    page
      .locator('[data-doc-section="new-tune.opc-tag-browser"]')
      .getByRole("button", { name: "Delete index", exact: true }),
  ).toHaveCount(1);
  expect(deleteCount()).toBe(1);
  await captureScenario(page, "full-opc-search-index-delete-confirmation");

  const secondResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "DELETE" &&
      new URL(response.url()).pathname === "/api/opc/search-index",
  );
  await confirm.click();
  await secondResponse;
  await expect(dialog).toBeHidden();
  await expect(
    page
      .locator('[data-doc-section="new-tune.opc-tag-browser"]')
      .getByRole("button", { name: "Delete index", exact: true }),
  ).toHaveCount(0);
  expect(deleteCount()).toBe(2);
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

test("full-history-delete-confirmation", async ({ page }) => {
  await page.goto("/runs/4242");
  await settle(page);
  const deleteCount = trackDeleteRequests(page, "/api/runs/4242");
  await page.getByRole("button", { name: "Delete tune", exact: true }).click();
  const dialog = await assertConfirmationCopy(
    page,
    "Delete tune?",
    "Delete tune #4242? This cannot be undone.",
    "Its recorded measurements, calculated results, and PID write history will be removed.",
    "Delete tune",
  );
  const confirm = dialog.getByRole("button", {
    name: "Delete tune",
    exact: true,
  });
  const firstResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "DELETE" &&
      new URL(response.url()).pathname === "/api/runs/4242",
  );
  await confirm.click();
  await assertPendingLocks(page, dialog, "Deleting tune…");
  await firstResponse;
  await expect(
    dialog.getByText("The server could not complete the request. Try again.", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(dialog).toBeVisible();
  await expect(
    page.getByText("Area01.FIC101.PV", { exact: true }),
  ).toBeVisible();
  expect(deleteCount()).toBe(1);
  await captureScenario(page, "full-history-delete-confirmation");

  const secondResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "DELETE" &&
      new URL(response.url()).pathname === "/api/runs/4242",
  );
  await confirm.click();
  await secondResponse;
  await expect(dialog).toBeHidden();
  await expect(page).toHaveURL(/\/runs$/);
  await expect(page.getByRole("heading", { name: "History" })).toBeVisible();
  await expect(
    page.getByRole("link", { name: "#4242", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("link", { name: "#4243", exact: true }),
  ).toBeVisible();
  expect(deleteCount()).toBe(2);
});

const templateScenarios = [
  {
    id: "full-template-list",
    route: "/templates",
    heading: "Templates",
  },
  {
    id: "full-template-detail",
    route: "/templates/Yokogawa%20CentumVP",
    heading: "Yokogawa CentumVP",
  },
  {
    id: "full-template-create",
    route: "/templates/new",
    heading: "New template",
  },
  {
    id: "full-template-edit",
    route: "/templates/Example%20Template/edit",
    heading: "Edit Example Template",
  },
] as const;

for (const scenario of templateScenarios) {
  test(scenario.id, async ({ page }) => {
    await page.goto(scenario.route);
    await settle(page);
    await expect(
      page.getByRole("heading", { name: scenario.heading, exact: true }),
    ).toBeVisible();
    await captureScenario(page, scenario.id);
  });
}

test("full-template-delete-confirmation", async ({ page }) => {
  await page.goto("/templates");
  await settle(page);
  const row = page
    .locator("tr")
    .filter({ hasText: "Example Template" })
    .first();
  await expect(row).toBeVisible();
  const deleteCount = trackDeleteRequests(
    page,
    "/api/templates/Example%20Template",
  );
  await row.getByRole("button", { name: "Delete", exact: true }).click();
  const dialog = await assertConfirmationCopy(
    page,
    "Delete template?",
    "Delete Example Template? This removes the template from the current database and cannot be undone here.",
    "Built-in and catalog templates will return on the next startup while their source definition remains available.",
    "Delete template",
  );
  const confirm = dialog.getByRole("button", {
    name: "Delete template",
    exact: true,
  });
  const firstResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "DELETE" &&
      new URL(response.url()).pathname === "/api/templates/Example%20Template",
  );
  await confirm.click();
  await assertPendingLocks(page, dialog, "Deleting template…");
  await firstResponse;
  await expect(
    dialog.getByText("The server could not complete the request. Try again.", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(dialog).toBeVisible();
  await expect(row).toBeVisible();
  expect(deleteCount()).toBe(1);
  await captureScenario(page, "full-template-delete-confirmation");

  const secondResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "DELETE" &&
      new URL(response.url()).pathname === "/api/templates/Example%20Template",
  );
  await confirm.click();
  await secondResponse;
  await expect(dialog).toBeHidden();
  await expect(
    page.getByRole("row").filter({ hasText: "Example Template" }),
  ).toHaveCount(0);
  expect(deleteCount()).toBe(2);
});

test("full-config", async ({ page }) => {
  await page.goto("/config");
  await settle(page);
  await expect(
    page.getByRole("heading", { name: "Configuration", exact: true }),
  ).toBeVisible();
  await captureScenario(page, "full-config");
});
