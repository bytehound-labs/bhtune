import { expect, test, type Page } from "@playwright/test";

/**
 * Regression coverage for the OPC DA server-discovery and tag-browser affordances
 * (`ui-opc-browser`) that doesn't require a live OPC DA gateway. This suite's shared
 * `bhtune-server` instance (`playwright.config.ts`'s `webServer`) never starts an
 * `opcda-bridge-gateway`, so every action below fails at the connection step against the
 * default `localhost:7600` bridge host -- confirmed by hand to fail near-instantly
 * (`ECONNREFUSED`, no gateway listening), well inside `api/opc.rs`'s own 30s
 * `OPC_QUERY_TIMEOUT_SECS` budget, so this suite is fast and deterministic without needing
 * one.
 *
 * A connection failure is still genuinely useful regression coverage: it exercises the
 * driver-switch visibility of the OPC-only fields, the real `GET /api/opc/servers` and
 * `GET /api/opc/browse` request wiring behind "Browse servers"/"Browse tags", the modal
 * opening/closing, and that a failure renders as a visible error rather than a silent no-op
 * or an unhandled exception. The populated-tree cases below use Playwright route fixtures for
 * the HTTP responses, keeping selection and template-specific PV-tag transformation covered
 * without requiring a second permanent gateway service. The main form's collapsed mapping
 * section covers the default/effective tag preview and per-tune overrides.
 */
function loopMapping(page: Page) {
  return page
    .locator("details")
    .filter({ has: page.locator("summary", { hasText: "Loop mapping" }) });
}

function mappingRow(page: Page, label: string) {
  return loopMapping(page).getByRole("group", { name: label, exact: true });
}

test.describe("OPC DA server discovery and tag browser (no gateway present)", () => {
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
    await page.goto("/runs/new");
    await page.getByLabel("Driver").selectOption("opcda");
  });

  test("orders the OPC DA connection fields before notes", async ({ page }) => {
    const fieldOrder = await page
      .locator("form label > span:first-child")
      .allTextContents();
    const indexOfField = (fieldName: string) =>
      fieldOrder.findIndex((label) => label.trim().startsWith(fieldName));

    expect(indexOfField("Bridge host")).toBeLessThan(
      indexOfField("OPC DA server ProgID"),
    );
    expect(indexOfField("OPC DA server ProgID")).toBeLessThan(
      indexOfField("Tag name"),
    );
    expect(indexOfField("Tag name")).toBeLessThan(indexOfField("Notes"));
  });

  test("updates a PV tag suffix when the template changes", async ({
    page,
  }) => {
    const templateField = page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox");
    const tagField = page.getByLabel("Tag name");

    await templateField.selectOption("Allen-Bradley PlantPAx");
    await tagField.fill("Simulink.Device1._System.Inp_PV");
    await templateField.selectOption("Yokogawa CentumVP");

    await expect(tagField).toHaveValue("Simulink.Device1._System.PV");
  });

  test("replaces an incorrect existing suffix when the template changes", async ({
    page,
  }) => {
    const templateField = page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox");
    const tagField = page.getByLabel("Tag name");

    await templateField.selectOption("Yokogawa CentumVP");
    await tagField.fill("Simulink.Device1.Python.MV");
    await templateField.selectOption("Allen-Bradley PlantPAx");

    await expect(tagField).toHaveValue("Simulink.Device1.Python.Inp_PV");
  });

  test("shows template defaults and applies a per-tune MV tag override", async ({
    page,
  }) => {
    const templateField = page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox");
    await templateField.selectOption("Yokogawa CentumVP");
    await page.getByLabel("Tag name").fill("Loop101.PV");

    const mapping = loopMapping(page);
    await expect(mapping).toHaveAttribute("open", "");

    const mvRow = mappingRow(page, "Manipulated variable (MV)");
    await expect(mvRow.getByText("Loop101.MV", { exact: true })).toBeVisible();
    await mvRow
      .getByRole("button", { name: "Custom tag", exact: true })
      .click();
    await mvRow
      .getByLabel("Manipulated variable (MV) custom tag")
      .fill("Loop101.PY");
    await expect(
      mvRow.getByLabel("Manipulated variable (MV) custom tag"),
    ).toHaveValue("Loop101.PY");
    await expect(
      mvRow.getByRole("button", { name: "Custom tag", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
  });

  test("keeps custom tags pinned while the template and base tag change", async ({
    page,
  }) => {
    const templateField = page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox");
    const mvRow = mappingRow(page, "Manipulated variable (MV)");

    await templateField.selectOption("Yokogawa CentumVP");
    await page.getByLabel("Tag name").fill("Loop101.PV");
    await mvRow
      .getByRole("button", { name: "Custom tag", exact: true })
      .click();
    await mvRow
      .getByLabel("Manipulated variable (MV) custom tag")
      .fill("Loop101.PY");

    await page.getByLabel("Tag name").fill("Loop202.PV");
    await templateField.selectOption("Allen-Bradley PlantPAx");

    await expect(page.getByLabel("Tag name")).toHaveValue("Loop202.Inp_PV");
    await expect(
      mvRow.getByLabel("Manipulated variable (MV) custom tag"),
    ).toHaveValue("Loop101.PY");
  });

  test("resets one mapping row or all mapping overrides", async ({ page }) => {
    const templateField = page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox");
    await templateField.selectOption("Yokogawa CentumVP");
    await page.getByLabel("Tag name").fill("Loop101.PV");

    const mapping = loopMapping(page);
    const mvRow = mappingRow(page, "Manipulated variable (MV)");
    const setpointRow = mappingRow(page, "Setpoint");
    await mvRow
      .getByRole("button", { name: "Custom tag", exact: true })
      .click();
    await mvRow
      .getByLabel("Manipulated variable (MV) custom tag")
      .fill("Loop101.PY");
    await setpointRow
      .getByRole("button", { name: "Custom tag", exact: true })
      .click();
    await setpointRow
      .getByLabel("Setpoint custom tag")
      .fill("Loop101.SP_CUSTOM");

    await mvRow.getByRole("button", { name: "Reset", exact: true }).click();
    await expect(
      mvRow.getByRole("button", { name: "Template tag", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(mvRow.getByText("Loop101.MV", { exact: true })).toBeVisible();
    await expect(
      mvRow.getByLabel("Manipulated variable (MV) custom tag"),
    ).toHaveCount(0);
    await expect(setpointRow.getByLabel("Setpoint custom tag")).toHaveValue(
      "Loop101.SP_CUSTOM",
    );

    await mapping
      .getByRole("button", { name: "Reset all mapping overrides" })
      .click();
    await expect(
      setpointRow.getByRole("button", { name: "Template tag", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      setpointRow.getByText("Loop101.SV", { exact: true }),
    ).toBeVisible();
  });

  test("keeps fixed direction and ranges separate from simulator values", async ({
    page,
  }) => {
    const templateField = page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox");
    await templateField.selectOption("Yokogawa CentumVP");

    const directionRow = mappingRow(page, "Controller direction");
    await directionRow
      .getByRole("button", { name: "Fixed value", exact: true })
      .click();
    await directionRow
      .getByLabel("Controller direction fixed value")
      .selectOption("direct");

    const pvHighRow = mappingRow(page, "PV range high");
    await pvHighRow
      .getByRole("button", { name: "Fixed value", exact: true })
      .click();
    await pvHighRow.getByLabel("PV range high fixed value").fill("90");

    await page
      .getByRole("combobox", { name: "Driver", exact: true })
      .selectOption("simulator");
    await expect(
      directionRow.getByRole("button", { name: "Template tag", exact: true }),
    ).toBeDisabled();
    await expect(
      directionRow.getByRole("button", { name: "Fixed value", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      page.getByLabel("Controller direction fixed value"),
    ).toHaveValue("reverse");
    await expect(page.getByLabel("PV range high fixed value")).toHaveValue(
      "100",
    );

    await page
      .getByRole("combobox", { name: "Driver", exact: true })
      .selectOption("opcda");
    await expect(
      directionRow.getByRole("button", { name: "Fixed value", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      directionRow.getByLabel("Controller direction fixed value"),
    ).toHaveValue("direct");
    await expect(pvHighRow.getByLabel("PV range high fixed value")).toHaveValue(
      "90",
    );
  });

  test("submits custom read tags and fixed values as active OPC overrides", async ({
    page,
  }) => {
    const templateField = page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox");
    await templateField.selectOption("Yokogawa CentumVP");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByLabel("Tag name").fill("Loop101.PV");

    const directionRow = mappingRow(page, "Controller direction");
    await directionRow
      .getByRole("button", { name: "Custom tag", exact: true })
      .click();
    await directionRow
      .getByLabel("Controller direction custom read tag")
      .fill("Loop101.ACTION");

    const pvHighRow = mappingRow(page, "PV range high");
    await pvHighRow
      .getByRole("button", { name: "Fixed value", exact: true })
      .click();
    await pvHighRow.getByLabel("PV range high fixed value").fill("90");

    await page.route("**/api/runs", async (route) => {
      if (route.request().method() !== "POST") {
        await route.continue();
        return;
      }
      await route.fulfill({
        status: 400,
        contentType: "application/json",
        body: JSON.stringify({ error: "request captured by the test" }),
      });
    });

    const requestPromise = page.waitForRequest(
      (request) =>
        request.url().endsWith("/api/runs") && request.method() === "POST",
    );
    await page.getByRole("button", { name: "Start tune" }).click();
    const request = await requestPromise;
    const body = request.postDataJSON() as {
      direction?: string;
      pv_range_high?: number;
      pv_range_low?: number;
      tag_overrides?: Record<string, string>;
    };

    expect(body.direction).toBeUndefined();
    expect(body.pv_range_high).toBe(90);
    expect(body.pv_range_low).toBeUndefined();
    expect(body.tag_overrides).toEqual({
      controller_direction: "Loop101.ACTION",
    });
    await expect(
      page.getByText("Unable to start the tune.", { exact: true }),
    ).toBeVisible();
  });

  test("Browse tags button stays disabled until a ProgID is entered", async ({
    page,
  }) => {
    const browseButton = page.getByRole("button", { name: "Browse tags" });
    await expect(browseButton).toBeDisabled();

    await page
      .getByLabel("OPC DA server ProgID")
      .fill("Matrikon.OPC.Simulation");
    await expect(browseButton).toBeEnabled();
  });

  test("opens the server picker and fills the ProgID from a discovered server", async ({
    page,
  }) => {
    await page.route("**/api/opc/servers**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          servers: ["Kepware.KEPServerEX.V5", "Yokogawa.CSHIS_OPC.1"],
        }),
      });
    });

    const serverField = page.getByLabel("OPC DA server ProgID");
    await page.getByRole("button", { name: "Browse servers" }).click();

    await expect(
      page.getByRole("heading", { name: "Browse OPC DA servers" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Yokogawa.CSHIS_OPC.1" }),
    ).toBeVisible();

    await page.getByRole("button", { name: "Yokogawa.CSHIS_OPC.1" }).click();
    await expect(serverField).toHaveValue("Yokogawa.CSHIS_OPC.1");
    await expect(
      page.getByRole("heading", { name: "Browse OPC DA servers" }),
    ).not.toBeVisible();
  });

  test("shows a connection error when browsing servers with no gateway present", async ({
    page,
  }) => {
    await page.getByRole("button", { name: "Browse servers" }).click();

    await expect(
      page.getByText("Unable to browse OPC DA servers."),
    ).toBeVisible();
  });

  test("opens the tag browser modal and shows a connection error at the root level, then closes", async ({
    page,
  }) => {
    await page
      .getByLabel("OPC DA server ProgID")
      .fill("Matrikon.OPC.Simulation");
    await page.getByRole("button", { name: "Browse tags" }).click();

    await expect(
      page.getByRole("heading", {
        name: "Browse tags on Matrikon.OPC.Simulation",
      }),
    ).toBeVisible();
    await expect(
      page.getByText("Unable to load tags at this level."),
    ).toBeVisible();

    await page.getByRole("button", { name: "Close" }).click();
    await expect(
      page.getByRole("heading", {
        name: "Browse tags on Matrikon.OPC.Simulation",
      }),
    ).not.toBeVisible();
  });

  test("closes the tag browser modal via Escape", async ({ page }) => {
    await page
      .getByLabel("OPC DA server ProgID")
      .fill("Matrikon.OPC.Simulation");
    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(
      page.getByRole("heading", {
        name: "Browse tags on Matrikon.OPC.Simulation",
      }),
    ).toBeVisible();

    await page.keyboard.press("Escape");
    await expect(
      page.getByRole("heading", {
        name: "Browse tags on Matrikon.OPC.Simulation",
      }),
    ).not.toBeVisible();
  });

  test("uses the active template PV suffix when a browse leaf is selected", async ({
    page,
  }) => {
    const originalTag = "Simulink.Device1._System._DemandPoll";
    const readTags: string[] = [];
    await page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox")
      .selectOption("Yokogawa CentumVP");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");

    await page.route("**/api/opc/read**", async (route) => {
      const url = new URL(route.request().url());
      readTags.push(url.searchParams.get("tag") ?? "");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          tag: originalTag,
          value: "42.0",
          quality: "good",
          timestamp: null,
        }),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      const url = new URL(route.request().url());
      const path = url.searchParams.get("path");
      const nodes =
        path === "Simulink.Device1._System"
          ? [
              {
                tag: originalTag,
                is_branch: false,
              },
            ]
          : [
              {
                tag: "Simulink.Device1._System",
                is_branch: true,
              },
            ];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ nodes }),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(
      page.getByRole("button", { name: "Simulink.Device1._System" }),
    ).toBeVisible();
    await expect(
      page.getByText("Selected: Simulink.Device1._System"),
    ).toBeVisible();

    await page.getByRole("button", { name: "Expand" }).click();
    await page
      .getByRole("button", {
        name: "Simulink.Device1._System._DemandPoll",
      })
      .click();

    await page.getByRole("button", { name: "Select tag" }).click();

    await expect(page.getByLabel("Tag name")).toHaveValue(
      "Simulink.Device1._System.PV",
    );
    expect(readTags).toEqual([originalTag]);

    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(
      page.getByRole("button", { name: "Simulink.Device1._System" }),
    ).toBeVisible();

    await page
      .getByRole("button", {
        name: "Simulink.Device1._System",
      })
      .dblclick();
    await expect(
      page.getByRole("button", {
        name: "Simulink.Device1._System._DemandPoll",
      }),
    ).toBeVisible();
    await page
      .getByRole("button", {
        name: "Simulink.Device1._System._DemandPoll",
      })
      .dblclick();

    await expect(page.getByLabel("Tag name")).toHaveValue(
      "Simulink.Device1._System.PV",
    );
    expect(readTags).toEqual([originalTag, originalTag]);
  });

  test("reopens the browser at the previously selected tag", async ({
    page,
  }) => {
    const originalTag = "Simulink._Statistics.Inp_PV";
    await page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox")
      .selectOption("Allen-Bradley PlantPAx");
    await page
      .getByLabel("OPC DA server ProgID")
      .fill("Kepware.KEPServerEX.V6");
    await page.getByLabel("Tag name").fill(originalTag);

    await page.route("**/api/opc/read**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          tag: originalTag,
          value: "42.0",
          quality: "good",
          timestamp: null,
        }),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      const url = new URL(route.request().url());
      const path = url.searchParams.get("path");
      const fillerNodes = Array.from({ length: 30 }, (_, index) => ({
        tag: `Filler${index}`,
        is_branch: false,
      }));
      const nodes =
        path === "Simulink"
          ? [{ tag: "Simulink._Statistics", is_branch: true }]
          : path === "Simulink._Statistics"
            ? [{ tag: originalTag, is_branch: false }]
            : [...fillerNodes, { tag: "Simulink", is_branch: true }];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ nodes }),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(page.getByRole("button", { name: originalTag })).toBeVisible();
    await expect(page.getByText(`Selected: ${originalTag}`)).toBeVisible();
    const treeViewport = page.locator("div.max-h-64").first();
    await expect
      .poll(() => treeViewport.evaluate((element) => element.scrollTop))
      .toBeGreaterThan(0);
    await page.getByRole("button", { name: originalTag }).click();
    await page.getByRole("button", { name: "Select tag" }).click();

    await expect(page.getByLabel("Tag name")).toHaveValue(originalTag);

    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(page.getByRole("button", { name: originalTag })).toBeVisible();
    await expect(page.getByText(`Selected: ${originalTag}`)).toBeVisible();
    await expect
      .poll(() => treeViewport.evaluate((element) => element.scrollTop))
      .toBeGreaterThan(0);
    await expect(page.getByRole("button", { name: "Collapse" })).toHaveCount(2);
  });

  test("warns before selecting a tag whose OPC quality is not Good", async ({
    page,
  }) => {
    const originalTag = "Simulink.Device1._System._DemandPoll";
    const readTags: string[] = [];
    await page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox")
      .selectOption("Yokogawa CentumVP");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");

    await page.route("**/api/opc/read**", async (route) => {
      const url = new URL(route.request().url());
      readTags.push(url.searchParams.get("tag") ?? "");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          tag: originalTag,
          value: "42.0",
          quality: "uncertain",
          timestamp: null,
        }),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      const url = new URL(route.request().url());
      const path = url.searchParams.get("path");
      const nodes =
        path === "Simulink.Device1._System"
          ? [{ tag: originalTag, is_branch: false }]
          : [{ tag: "Simulink.Device1._System", is_branch: true }];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ nodes }),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(
      page.getByRole("button", { name: "Simulink.Device1._System" }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Expand" }).click();
    await page.getByRole("button", { name: originalTag }).click();
    await page.getByRole("button", { name: "Select tag" }).click();

    await expect(
      page.getByRole("heading", { name: "OPC quality warning" }),
    ).toBeVisible();
    await expect(page.getByText("Uncertain", { exact: true })).toBeVisible();
    await expect(page.getByLabel("Tag name")).toHaveValue("Sim.Loop1.PV");
    expect(readTags).toEqual([originalTag]);

    await page.getByRole("button", { name: "Choose a different tag" }).click();
    await expect(
      page.getByRole("heading", {
        name: "Browse tags on Yokogawa.CSHIS_OPC.1",
      }),
    ).toBeVisible();

    await page.getByRole("button", { name: "Select tag" }).click();
    await expect(
      page.getByRole("heading", { name: "OPC quality warning" }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Proceed anyway" }).click();

    await expect(page.getByLabel("Tag name")).toHaveValue(
      "Simulink.Device1._System.PV",
    );
    expect(readTags).toEqual([originalTag, originalTag]);
  });
});
