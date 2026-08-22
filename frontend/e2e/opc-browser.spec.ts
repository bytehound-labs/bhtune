import { expect, test } from "@playwright/test";

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
 * the HTTP responses, keeping the selection and template-specific PV-tag transformation
 * covered without requiring a second permanent gateway service.
 */
test.describe("OPC DA server discovery and tag browser (no gateway present)", () => {
  test.beforeEach(async ({ page }) => {
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
    await page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox")
      .selectOption("Yokogawa CentumVP");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");

    await page.route("**/api/opc/browse**", async (route) => {
      const url = new URL(route.request().url());
      const path = url.searchParams.get("path");
      const nodes =
        path === "Simulink.Device1._System"
          ? [
              {
                tag: "Simulink.Device1._System._DemandPoll",
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
    const mappingDetails = page.locator("details");
    await expect(mappingDetails).not.toHaveAttribute("open", "");
    await mappingDetails.locator("summary").click();
    await expect(mappingDetails.locator("ul")).toBeVisible();
    await mappingDetails.locator("summary").click();
    await expect(mappingDetails).not.toHaveAttribute("open", "");

    await page.getByRole("button", { name: "Expand" }).click();
    await page
      .getByRole("button", {
        name: "Simulink.Device1._System._DemandPoll",
      })
      .click();

    await expect(
      page.getByText("PV tag: Simulink.Device1._System.PV"),
    ).toBeVisible();
    await page.getByRole("button", { name: "Select tag" }).click();

    await expect(page.getByLabel("Tag name")).toHaveValue(
      "Simulink.Device1._System.PV",
    );

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
  });
});
