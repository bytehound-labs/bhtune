import { expect, test, type APIRequestContext } from "@playwright/test";

interface ConfigResponse {
  revision: string;
  toml: {
    allow_uncertain_quality: boolean | null;
    retention_days: number | null;
  };
  effective: {
    allow_uncertain_quality: boolean;
    retention_days: number | null;
  };
}

async function getConfig(request: APIRequestContext): Promise<ConfigResponse> {
  const response = await request.get("/api/config");
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as ConfigResponse;
}

async function saveConfig(
  request: APIRequestContext,
  config: ConfigResponse,
  values: {
    allow_uncertain_quality: boolean;
    retention_days: number | null;
  },
) {
  const response = await request.put("/api/config", {
    data: {
      revision: config.revision,
      ...values,
    },
  });
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as ConfigResponse;
}

test.describe("global configuration", () => {
  test.describe.configure({ mode: "serial" });

  let initialConfig: ConfigResponse;

  test.beforeAll(async ({ request }) => {
    initialConfig = await getConfig(request);
  });

  test.afterAll(async ({ request }) => {
    const current = await getConfig(request);
    await saveConfig(request, current, {
      allow_uncertain_quality:
        initialConfig.toml.allow_uncertain_quality ??
        initialConfig.effective.allow_uncertain_quality,
      retention_days: initialConfig.toml.retention_days,
    });
  });

  test("accepts Uncertain quality by default and removes the per-tune control", async ({
    page,
  }) => {
    await page.goto("/config");

    await expect(
      page.getByRole("heading", { name: "Configuration", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("checkbox", { name: "Allow Uncertain quality" }),
    ).toBeChecked();

    await page.goto("/runs/new");
    await expect(
      page.getByRole("checkbox", { name: /Allow uncertain quality/i }),
    ).toHaveCount(0);
  });

  test("saves the global quality and retention policies", async ({ page }) => {
    await page.goto("/config");
    await page
      .getByRole("checkbox", { name: "Allow Uncertain quality" })
      .uncheck();
    await page.getByLabel("Delete older runs automatically").check();
    await page.getByLabel("Retention days").fill("30");

    await Promise.all([
      page.waitForResponse(
        (response) =>
          response.url().endsWith("/api/config") &&
          response.request().method() === "PUT" &&
          response.ok(),
      ),
      page.getByRole("button", { name: "Save configuration" }).click(),
    ]);

    await expect(
      page.getByText("Configuration saved successfully."),
    ).toBeVisible();
    await expect(
      page.getByText(
        "Effective policy: Uncertain quality is rejected; retention is 30 days.",
      ),
    ).toBeVisible();

    await page.reload();
    await expect(
      page.getByRole("checkbox", { name: "Allow Uncertain quality" }),
    ).not.toBeChecked();
    await expect(page.getByLabel("Retention days")).toHaveValue("30");
    await expect(page.getByLabel("Retention days")).toBeEnabled();

    await page
      .getByRole("checkbox", { name: "Allow Uncertain quality" })
      .check();
    await Promise.all([
      page.waitForResponse(
        (response) =>
          response.url().endsWith("/api/config") &&
          response.request().method() === "PUT" &&
          response.ok(),
      ),
      page.getByRole("button", { name: "Save configuration" }).click(),
    ]);
    await expect(page.getByText(/bhtune\.toml\.backup-.*\.bak/)).toBeVisible();
  });

  test("validates retention days and supports disabling retention", async ({
    page,
  }) => {
    await page.goto("/config");
    await page.getByLabel("Retain forever").check();
    await expect(page.getByLabel("Retention days")).toBeDisabled();

    await Promise.all([
      page.waitForResponse(
        (response) =>
          response.url().endsWith("/api/config") &&
          response.request().method() === "PUT" &&
          response.ok(),
      ),
      page.getByRole("button", { name: "Save configuration" }).click(),
    ]);
    await expect(
      page.getByText(
        "Effective policy: Uncertain quality is accepted; retention is disabled.",
      ),
    ).toBeVisible();

    await page.getByLabel("Delete older runs automatically").check();
    const retentionDays = page.getByLabel("Retention days");
    await retentionDays.fill("0");
    await page.getByRole("button", { name: "Save configuration" }).click();

    expect(
      await retentionDays.evaluate(
        (element) => (element as HTMLInputElement).validity.valid,
      ),
    ).toBe(false);
  });

  test("reports a stale revision instead of overwriting another save", async ({
    page,
    request,
  }) => {
    await page.goto("/config");
    await expect(
      page.getByRole("button", { name: "Save configuration" }),
    ).toBeDisabled();
    const stale = await getConfig(request);
    const external = await saveConfig(request, stale, {
      allow_uncertain_quality: !stale.effective.allow_uncertain_quality,
      retention_days: stale.toml.retention_days,
    });

    await page
      .getByRole("checkbox", { name: "Allow Uncertain quality" })
      .click();
    await page.getByRole("button", { name: "Save configuration" }).click();

    await expect(
      page.getByText(
        "The configuration changed elsewhere. Reload the latest values before saving again.",
      ),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Reload configuration" }),
    ).toBeVisible();

    expect(external.revision).not.toBe(stale.revision);
  });
});
