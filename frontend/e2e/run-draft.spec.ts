import { expect, test, type Page } from "@playwright/test";

const defaultDraft = {
  driver: "simulator",
  template: "Yokogawa CentumVP",
  tagname: "Sim.Loop1.PV",
  server: "",
  bridge_host: "",
  process_type: "flow",
  controller_type: "pi",
  relay_amp: 10,
  cycles_skip: null,
  cycles_count: null,
  noise_protection_secs: null,
  mrft_delay: 0,
  poll_interval_ms: 800,
  timeout_secs: 3600,
  op_timeout_secs: 30,
  restore_timeout_secs: 30,
  direction: "reverse",
  pv_range_high: 100,
  pv_range_low: 0,
  mv_range_high: 100,
  mv_range_low: 0,
  sim_gain: 1,
  sim_tau: 2,
  sim_dead_time: 5,
  sim_noise: 0,
  sim_seed: 0,
  sim_initial_pv: 50,
  sim_initial_mv: 50,
  tag_overrides: null,
  source_driver: "simulator",
  source_direction: "reverse",
  source_pv_range_high: 100,
  source_pv_range_low: 0,
  source_mv_range_high: 100,
  source_mv_range_low: 0,
  tag_sources: {
    process_variable: "template",
    manipulated_variable: "template",
    setpoint_variable: "template",
    controller_mode: "template",
    mode_attribute: "template",
    proportional_constant: "template",
    integral_constant: "template",
    derivative_constant: "template",
  },
  value_sources: {
    direction: "tag",
    pv_range_high: "tag",
    pv_range_low: "tag",
    mv_range_high: "tag",
    mv_range_low: "tag",
  },
  write_pid: null,
  yes: false,
};

async function waitForDraftSave(page: Page) {
  await page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/runs/draft") &&
      response.request().method() === "PUT" &&
      response.ok(),
    { timeout: 5_000 },
  );
}

async function waitForDraftHydration(page: Page) {
  await expect(page.getByText("Loaded your saved Tune draft.")).toBeVisible();
}

function loopMapping(page: Page) {
  return page
    .locator("details")
    .filter({ has: page.locator("summary", { hasText: "Loop mapping" }) });
}

function mappingRow(page: Page, label: string) {
  return loopMapping(page).getByRole("group", { name: label, exact: true });
}

async function setCustomTag(page: Page, label: string, value: string) {
  const row = mappingRow(page, label);
  await row.getByRole("button", { name: "Custom tag", exact: true }).click();
  await row.getByLabel(`${label} custom tag`).fill(value);
}

test.describe("New Tune draft persistence", () => {
  test.describe.configure({ mode: "serial" });

  test.beforeEach(async ({ request }) => {
    const response = await request.put("/api/runs/draft", {
      data: defaultDraft,
    });
    expect(response.ok()).toBeTruthy();
  });

  test("quietly falls back when the saved-draft endpoint is unavailable", async ({
    page,
  }) => {
    await page.route("**/api/runs/draft", async (route) => {
      if (route.request().method() === "GET") {
        await route.fulfill({
          status: 400,
          contentType: "application/json",
          body: JSON.stringify({
            error: "Invalid URL: Cannot parse `draft` to a `i64`",
          }),
        });
        return;
      }
      await route.continue();
    });
    await page.route("**/api/runs/last-request", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: "null",
      }),
    );

    await page.goto("/runs/new");

    await expect(page.getByLabel("Tag name")).toHaveValue("Sim.Loop1.PV");
    await expect(
      page.getByText(
        "Unable to load the saved Tune draft; using the available fallback.",
        { exact: true },
      ),
    ).toHaveCount(0);
  });

  test("restores connection values after reload and keeps Notes blank", async ({
    page,
  }) => {
    await page.goto("/runs/new");
    await waitForDraftHydration(page);
    await page
      .getByRole("combobox", { name: "Driver", exact: true })
      .selectOption("opcda");
    await expect(page.getByLabel("Bridge host")).toBeEnabled();
    await page.getByLabel("Bridge host").fill("gateway.example:7600");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByLabel("Tag name").fill("FIC101");
    await setCustomTag(page, "Manipulated variable (MV)", "FIC101.PY");
    const directionRow = mappingRow(page, "Controller direction");
    await directionRow
      .getByRole("button", { name: "Custom tag", exact: true })
      .click();
    await directionRow
      .getByLabel("Controller direction custom read tag")
      .fill("FIC101.ACTION");
    const pvHighRow = mappingRow(page, "PV range high");
    await pvHighRow
      .getByRole("button", { name: "Fixed value", exact: true })
      .click();
    await pvHighRow.getByLabel("PV range high fixed value").fill("90");
    await page.getByLabel("Notes").fill("do not persist this");
    await waitForDraftSave(page);

    await page.reload();

    await expect(
      page.getByRole("combobox", { name: "Driver", exact: true }),
    ).toHaveValue("opcda");
    await expect(page.getByLabel("Bridge host")).toHaveValue(
      "gateway.example:7600",
    );
    await expect(page.getByLabel("OPC DA server ProgID")).toHaveValue(
      "Yokogawa.CSHIS_OPC.1",
    );
    await expect(page.getByLabel("Tag name")).toHaveValue("FIC101");
    await expect(
      mappingRow(page, "Manipulated variable (MV)").getByLabel(
        "Manipulated variable (MV) custom tag",
      ),
    ).toHaveValue("FIC101.PY");
    await expect(
      mappingRow(page, "Controller direction").getByRole("button", {
        name: "Custom tag",
        exact: true,
      }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      mappingRow(page, "Controller direction").getByLabel(
        "Controller direction custom read tag",
      ),
    ).toHaveValue("FIC101.ACTION");
    await expect(page.getByLabel("Notes")).toHaveValue("");
    await expect(page.getByText("Loaded your saved Tune draft.")).toBeVisible();

    await page
      .getByRole("combobox", { name: "Driver", exact: true })
      .selectOption("opcda");
    await expect(
      mappingRow(page, "Controller direction").getByRole("button", {
        name: "Custom tag",
        exact: true,
      }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      mappingRow(page, "PV range high").getByLabel("PV range high fixed value"),
    ).toHaveValue("90");
    await expect(
      mappingRow(page, "PV range low").getByRole("button", {
        name: "Template tag",
        exact: true,
      }),
    ).toHaveAttribute("aria-pressed", "true");
  });

  test("retains OPC connection values when switching to Simulator", async ({
    page,
  }) => {
    await page.goto("/runs/new");
    await waitForDraftHydration(page);
    await page
      .getByRole("combobox", { name: "Driver", exact: true })
      .selectOption("opcda");
    await expect(page.getByLabel("Bridge host")).toBeEnabled();
    await page.getByLabel("Bridge host").fill("gateway.example:7600");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByLabel("Tag name").fill("FIC101");
    await setCustomTag(page, "Manipulated variable (MV)", "FIC101.PY");
    await waitForDraftSave(page);

    await page
      .getByRole("combobox", { name: "Driver", exact: true })
      .selectOption("simulator");
    await waitForDraftSave(page);
    await page.reload();

    await expect(
      page.getByRole("combobox", { name: "Driver", exact: true }),
    ).toHaveValue("simulator");
    await expect(page.getByLabel("Bridge host")).toHaveValue(
      "gateway.example:7600",
    );
    await expect(page.getByLabel("OPC DA server ProgID")).toHaveValue(
      "Yokogawa.CSHIS_OPC.1",
    );
    await expect(page.getByLabel("Tag name")).toHaveValue("FIC101");
    await expect(
      mappingRow(page, "Manipulated variable (MV)").getByLabel(
        "Manipulated variable (MV) custom tag",
      ),
    ).toHaveValue("FIC101.PY");
  });

  test("persists Duplicate this run through a browser reload", async ({
    page,
    request,
  }) => {
    test.setTimeout(45_000);
    const start = await request.post("/api/runs", {
      data: {
        tagname: "Duplicate.Loop.PV",
        template: "Yokogawa CentumVP",
        process_type: "flow",
        controller_type: "pi",
        relay_amp: 10,
        cycles_skip: 1,
        cycles_count: 2,
        noise_protection_secs: 0,
        mrft_delay: 0,
        driver: "simulator",
        sim_gain: 1,
        sim_tau: 0.01,
        sim_dead_time: 0.025,
        sim_noise: 0,
        sim_seed: 0,
        sim_initial_pv: 50,
        sim_initial_mv: 50,
        pv_range_high: 100,
        pv_range_low: 0,
        mv_range_high: 100,
        mv_range_low: 0,
        direction: "reverse",
        tag_overrides: {
          manipulated_variable: "Duplicate.Loop.PY",
        },
        poll_interval_ms: 5,
        timeout_secs: 30,
        op_timeout_secs: 30,
        restore_timeout_secs: 30,
        yes: false,
      },
    });
    expect(start.status()).toBe(201);
    const run = (await start.json()) as { id: number };

    await expect
      .poll(
        async () => {
          const response = await request.get(`/api/runs/${run.id}`);
          const body = (await response.json()) as { outcome: string };
          return body.outcome;
        },
        { timeout: 30_000 },
      )
      .toBe("completed");

    await page.goto(`/runs/${run.id}`);
    await page.getByRole("button", { name: "Duplicate this run" }).click();
    await expect(page).toHaveURL(/\/runs\/new$/);
    await expect(page.getByLabel("Tag name")).toHaveValue("Duplicate.Loop.PV");
    await expect(
      mappingRow(page, "Manipulated variable (MV)").getByLabel(
        "Manipulated variable (MV) custom tag",
      ),
    ).toHaveValue("Duplicate.Loop.PY");
    await waitForDraftSave(page);

    // Leave the location state used by Duplicate this run before opening the form again;
    // otherwise a browser reload can legitimately retain that history state and bypass the
    // saved-draft hydration path being tested here.
    await page.goto("/runs");
    await page.goto("/runs/new");

    await waitForDraftHydration(page);
    await expect(page.getByLabel("Tag name")).toHaveValue("Duplicate.Loop.PV");
    await expect(
      mappingRow(page, "Manipulated variable (MV)").getByLabel(
        "Manipulated variable (MV) custom tag",
      ),
    ).toHaveValue("Duplicate.Loop.PY");
    await expect(page.getByLabel("Notes")).toHaveValue("");
    await expect(page.getByText("Loaded your saved Tune draft.")).toBeVisible();
  });

  test("persists Reset to defaults", async ({ page }) => {
    await page.goto("/runs/new");
    await waitForDraftHydration(page);
    await page
      .getByRole("combobox", { name: "Driver", exact: true })
      .selectOption("opcda");
    await expect(page.getByLabel("Bridge host")).toBeEnabled();
    await page.getByLabel("Poll interval (ms)").fill("123");
    await setCustomTag(page, "Manipulated variable (MV)", "Loop.PY");
    await waitForDraftSave(page);
    await expect(page.getByLabel("Poll interval (ms)")).toHaveValue("123");

    await page.getByRole("button", { name: "Reset to defaults" }).click();
    await waitForDraftSave(page);
    await page.reload();

    await expect(
      page.getByRole("combobox", { name: "Driver", exact: true }),
    ).toHaveValue("simulator");
    await expect(page.getByLabel("Poll interval (ms)")).toHaveValue("800");
    const mvRow = mappingRow(page, "Manipulated variable (MV)");
    await expect(
      mvRow.getByRole("button", { name: "Template tag", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      mvRow.getByText("Inactive for Simulator", { exact: true }),
    ).toBeVisible();
    await expect(page.getByLabel("Notes")).toHaveValue("");
  });

  test("preserves an explicitly cleared template", async ({
    page,
    request,
  }) => {
    const response = await request.put("/api/runs/draft", {
      data: { ...defaultDraft, template: null },
    });
    expect(response.ok()).toBeTruthy();

    await page.goto("/runs/new");

    await expect(
      page.getByRole("combobox", { name: "Template", exact: true }),
    ).toHaveValue("");
    await expect(page.getByText("Loaded your saved Tune draft.")).toBeVisible();
  });
});
