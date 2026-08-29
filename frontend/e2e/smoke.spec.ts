import { expect, test } from "@playwright/test";

/**
 * Basic navigation/rendering smoke coverage: the app shell loads on the Tune screen, the
 * health indicator reaches the server, and the four built-in DCS/PLC templates that
 * `bhtune-db`'s `seed_builtin_templates` seeds into every fresh database (see
 * `db-seed-templates`) render in the Templates list. Deliberately no interaction with
 * tune/write-back flows here -- that's `tune.spec.ts`'s job.
 */
test.describe("app shell", () => {
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
  });

  test("loads, reaches a healthy driver, and lands on the Tune screen", async ({
    page,
  }) => {
    await page.goto("/");

    // The index route redirects to /runs/new (see App.tsx's route table) -- starting a
    // tune is the app's default landing page (`ui-tune-nav`).
    await expect(page).toHaveURL(/\/runs\/new$/);
    await expect(
      page.locator("header").getByText("BHTune", { exact: true }),
    ).toBeVisible();

    await expect(page.getByRole("link", { name: "Tune" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Templates" })).toBeVisible();
    await expect(page.getByRole("link", { name: "History" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Config" })).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Reset to defaults" }),
    ).toBeVisible();

    // The health indicator polls a real `/api/health` -- confirms this isn't a static mock.
    const healthStatus = page.locator("output");
    await expect(healthStatus).toHaveText(
      "Connected to BHTune server: Connected — the BHTune HTTP service is reachable. This does not test OPC DA connectivity.",
    );
    await expect(page.locator(".health-indicator-dot")).toBeVisible();
    await expect(page.locator(".health-indicator-dot")).toHaveAttribute(
      "title",
      "Connected — the BHTune HTTP service is reachable. This does not test OPC DA connectivity.",
    );

    const healthResponse = await page.request.get("/api/health");
    expect(healthResponse.ok()).toBeTruthy();
    const health = (await healthResponse.json()) as { version: string };
    await expect(
      page.getByText(`v${health.version}`, { exact: true }),
    ).toBeVisible();
  });

  test("disables only driver-inert controls in Simulator mode", async ({
    page,
  }) => {
    await page.route("**/api/runs/last-request", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: "null",
      }),
    );

    await page.goto("/runs/new");
    await expect(page.getByLabel("Driver")).toHaveValue("simulator");

    const template = page.getByLabel("Template");
    await expect(template).not.toHaveValue("");
    await expect(template).toBeEnabled();
    await expect(
      page.getByText(
        "The simulator ignores DCS tag mappings, but the template still formats calculated PID values (for example, gain versus proportional band).",
      ),
    ).toBeVisible();

    const startsWithLabel = (label: string) =>
      new RegExp(`^${label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`);

    for (const label of ["Bridge host", "OPC DA server ProgID", "Tag name"]) {
      await expect(
        page.getByRole("textbox", { name: startsWithLabel(label) }),
      ).toBeDisabled();
    }
    for (const label of ["Communication timeout (s)", "Restore timeout (s)"]) {
      await expect(
        page.getByRole("spinbutton", { name: startsWithLabel(label) }),
      ).toBeDisabled();
    }
    for (const label of ["Allow automatic PID write"]) {
      await expect(
        page.getByRole("checkbox", { name: startsWithLabel(label) }),
      ).toBeDisabled();
    }
    await expect(
      page.getByRole("combobox", {
        name: startsWithLabel("Apply PID settings on completion"),
      }),
    ).toBeDisabled();

    const mapping = page
      .locator("details")
      .filter({ has: page.locator("summary", { hasText: "Loop mapping" }) });
    await expect(mapping).toHaveAttribute("open", "");
    await expect(
      mapping.getByRole("group", { name: "Controller direction", exact: true }),
    ).toBeVisible();
    for (const label of ["Process type", "Controller type"]) {
      await expect(
        page.getByRole("combobox", { name: startsWithLabel(label) }),
      ).toBeEnabled();
    }
    await expect(
      mapping.getByRole("combobox", {
        name: startsWithLabel("Controller direction fixed value"),
      }),
    ).toBeEnabled();
    for (const label of [
      "Relay amplitude (%)",
      "Cycles to count",
      "Poll interval (ms)",
      "Run timeout (s)",
      "PV range high",
      "MV range high",
      "Process gain",
      "Time constant τ (s)",
    ]) {
      await expect(
        page.getByRole("spinbutton", { name: startsWithLabel(label) }),
      ).toBeEnabled();
    }
  });

  test("opens every New Tune section by default and toggles each section", async ({
    page,
  }) => {
    await page.goto("/runs/new");

    for (const title of [
      "Connection",
      "Test parameters",
      "Loop mapping",
      "Simulator parameters",
      "Automatic PID settings",
    ]) {
      const section = page.locator("details").filter({
        has: page.locator("summary", { hasText: title }),
      });
      await expect(section).toHaveAttribute("open", "");
      await section.locator("summary").click();
      await expect(section).not.toHaveAttribute("open", "");
      await section.locator("summary").click();
      await expect(section).toHaveAttribute("open", "");
    }
  });

  test("toggles and persists the light/dark theme", async ({ page }) => {
    await page.goto("/");

    const appShell = page.locator("#root > div").first();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    await expect
      .poll(() =>
        appShell.evaluate(
          (element) =>
            element.ownerDocument.defaultView?.getComputedStyle(element)
              .backgroundColor,
        ),
      )
      .toBe("rgb(30, 30, 46)");
    await page
      .getByRole("button", { name: "Switch to Catppuccin light theme" })
      .click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
    await expect
      .poll(() =>
        appShell.evaluate(
          (element) =>
            element.ownerDocument.defaultView?.getComputedStyle(element)
              .backgroundColor,
        ),
      )
      .toBe("rgb(239, 241, 245)");
    await expect(
      page.getByRole("button", { name: "Switch to Catppuccin dark theme" }),
    ).toBeVisible();

    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
    await expect(
      page.getByRole("button", { name: "Switch to Catppuccin dark theme" }),
    ).toBeVisible();
  });

  test("lists the seeded templates", async ({ page }) => {
    await page.goto("/templates");

    for (const name of [
      "Yokogawa CentumVP",
      "Honeywell Experion",
      "Schneider Modicon",
      "Allen-Bradley PlantPAx",
    ]) {
      await expect(page.getByText(name, { exact: true })).toBeVisible();
    }
  });

  test("prefills tune settings without carrying forward notes", async ({
    page,
  }) => {
    await page.route("**/api/runs/last-request", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          tagname: "Sim.Loop1.PV",
          template: "Yokogawa CentumVP",
          process_type: "flow",
          controller_type: "pi",
          relay_amp: 10,
          driver: "simulator",
          notes: "Do not copy this note",
          poll_interval_ms: 5,
          direction: "reverse",
          pv_range_high: 100,
          pv_range_low: 0,
          mv_range_high: 100,
          mv_range_low: 0,
        }),
      }),
    );

    await page.goto("/runs/new");
    await expect(
      page.getByText("Loaded settings from the most recent tune."),
    ).toBeVisible();
    await expect(page.getByLabel("Poll interval (ms)")).toHaveValue("5");
    await expect(page.getByLabel("Notes")).toHaveValue("");
  });

  test("navigates between Tune, History, Templates, and Config via the header nav", async ({
    page,
  }) => {
    await page.goto("/templates");

    await page.getByRole("link", { name: "History" }).click();
    await expect(page).toHaveURL(/\/runs$/);
    await expect(page.getByRole("heading", { name: "History" })).toBeVisible();

    await page.getByRole("link", { name: "Templates" }).click();
    await expect(page).toHaveURL(/\/templates$/);

    await page.getByRole("link", { name: "Tune", exact: true }).click();
    await expect(page).toHaveURL(/\/runs\/new$/);

    await page.getByRole("link", { name: "Config" }).click();
    await expect(page).toHaveURL(/\/config$/);
    await expect(
      page.getByRole("heading", { name: "Configuration", exact: true }),
    ).toBeVisible();
  });
});
