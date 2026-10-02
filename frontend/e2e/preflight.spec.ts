import { expect, test } from "@playwright/test";
import type { PreflightResponse, StartRunRequest } from "../src/api/runs";

const report: PreflightResponse = {
  status: "fail",
  checks: [
    {
      name: "Configuration and tuning",
      status: "pass",
      detail: "Timing bounds are valid.",
    },
    {
      name: "Gateway compatibility",
      status: "warn",
      detail: "The gateway advertises an unknown protocol version.",
    },
    {
      name: "Initial state",
      status: "fail",
      detail: "The initial MV is outside the configured range.",
    },
  ],
  tag_reads: [
    {
      tag: "Area1.LIC101.PV",
      roles: ["process_variable"],
      value: "50",
      quality: "Good",
      status: "pass",
      detail: "quality Good",
    },
    {
      tag: "Area1.LIC101.MV",
      roles: ["manipulated_variable"],
      value: "50",
      quality: "Good",
      status: "fail",
      detail: "value outside the expected range",
    },
  ],
};

test.describe("read-only tune preflight", () => {
  test.beforeEach(async ({ page }) => {
    await page.route("**/api/runs/draft", async (route) => {
      if (route.request().method() === "GET") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: "null",
        });
      } else {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: route.request().postData() ?? "{}",
        });
      }
    });
    await page.route("**/api/runs/last-request", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: "null",
      }),
    );
  });

  test("shows report checks without starting a tune or issuing writes", async ({
    page,
  }) => {
    const preflightRequests: StartRunRequest[] = [];
    const otherPostRequests: string[] = [];
    page.on("request", (request) => {
      const url = new URL(request.url());
      if (
        request.method() === "POST" &&
        url.pathname !== "/api/runs/preflight"
      ) {
        otherPostRequests.push(url.pathname);
      }
    });

    await page.route("**/api/runs", async (route) => {
      if (route.request().method() === "POST") {
        otherPostRequests.push("/api/runs");
        await route.fulfill({
          status: 500,
          contentType: "application/json",
          body: JSON.stringify({ error: "unexpected tune start" }),
        });
        return;
      }
      await route.continue();
    });
    await page.route("**/api/runs/preflight", async (route) => {
      preflightRequests.push(route.request().postDataJSON() as StartRunRequest);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(report),
      });
    });

    await page.goto("/runs/new");
    const checkButton = page.getByRole("button", {
      name: "Check readiness",
    });
    await expect(checkButton).toBeVisible();
    await checkButton.click();

    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await expect(
      dialog.getByRole("heading", { name: "Readiness check" }),
    ).toBeVisible();
    await expect(dialog.getByText("Overall result")).toBeVisible();
    await expect(
      dialog.getByRole("status").getByText("fail", { exact: true }),
    ).toBeVisible();
    await expect(
      dialog.getByRole("heading", { name: "Configuration and tuning" }),
    ).toBeVisible();
    await expect(dialog.getByText("Timing bounds are valid.")).toBeVisible();
    const configurationCheck = dialog
      .locator("li")
      .filter({ hasText: "Configuration and tuning" });
    await expect(
      configurationCheck.getByText("pass", { exact: true }),
    ).toBeVisible();
    await expect(
      dialog.getByRole("heading", { name: "Gateway compatibility" }),
    ).toBeVisible();
    const compatibilityCheck = dialog
      .locator("li")
      .filter({ hasText: "Gateway compatibility" });
    await expect(
      compatibilityCheck.getByText("warn", { exact: true }),
    ).toBeVisible();
    await expect(
      dialog.getByText("The gateway advertises an unknown protocol version."),
    ).toBeVisible();
    const initialStateCheck = dialog
      .locator("li")
      .filter({ hasText: "Initial state" });
    await expect(
      initialStateCheck.getByText("fail", { exact: true }),
    ).toBeVisible();
    await expect(
      dialog.getByRole("heading", { name: "Tag reads" }),
    ).toBeVisible();
    await expect(dialog.getByText("Area1.LIC101.PV")).toBeVisible();
    const mvRead = dialog
      .getByRole("row")
      .filter({ hasText: "Area1.LIC101.MV" });
    await expect(mvRead.getByText("fail", { exact: true })).toBeVisible();
    await expect(
      mvRead.getByText("value outside the expected range"),
    ).toBeVisible();

    expect(preflightRequests).toHaveLength(1);
    expect(preflightRequests[0]).toMatchObject({
      template: expect.any(String),
      process_type: expect.any(String),
      controller_type: expect.any(String),
      relay_amp: expect.any(Number),
      driver: expect.any(String),
    });
    expect(otherPostRequests).toEqual([]);
  });

  test("surfaces a preflight request error in the readiness dialog", async ({
    page,
  }) => {
    await page.route("**/api/runs/preflight", (route) =>
      route.fulfill({
        status: 400,
        contentType: "application/json",
        body: JSON.stringify({ error: "invalid preflight request" }),
      }),
    );

    await page.goto("/runs/new");
    await page.getByRole("button", { name: "Check readiness" }).click();

    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("alert")).toHaveText(
      "Unable to check tune readiness.",
    );
    await expect(dialog.getByRole("heading", { name: "Checks" })).toHaveCount(
      0,
    );
  });
});
