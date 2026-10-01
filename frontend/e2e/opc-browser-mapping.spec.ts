import { expect, test } from "@playwright/test";
import {
  OPC_BROWSER_SUITE,
  openOpcDaRunForm,
  loopMapping,
  mappingRow,
} from "./support/opcBrowser";

/**
 * Loop-mapping coverage for the OPC DA New Run form: field order, template-driven PV
 * suffix replacement, per-tune tag overrides, mapping resets, fixed direction and range
 * values, and the override request submitted with a run.
 */
test.describe(OPC_BROWSER_SUITE, () => {
  test.beforeEach(async ({ page }) => {
    await openOpcDaRunForm(page);
  });

  test("orders the OPC DA connection fields before notes", async ({ page }) => {
    const fieldOrder = await page.locator("form label").allTextContents();
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
    const templateField = page.getByRole("combobox", { name: "Template" });
    const tagField = page.getByLabel("Tag name");

    await templateField.selectOption("Allen-Bradley PlantPAx");
    await tagField.fill("Simulink.Device1._System.Inp_PV");
    await templateField.selectOption("Yokogawa CentumVP");

    await expect(tagField).toHaveValue("Simulink.Device1._System.PV");
  });

  test("replaces an incorrect existing suffix when the template changes", async ({
    page,
  }) => {
    const templateField = page.getByRole("combobox", { name: "Template" });
    const tagField = page.getByLabel("Tag name");

    await templateField.selectOption("Yokogawa CentumVP");
    await tagField.fill("Simulink.Device1.Python.MV");
    const mvRow = mappingRow(page, "Manipulated variable (MV)");
    await mvRow
      .getByRole("button", { name: "Custom tag", exact: true })
      .click();
    await mvRow
      .getByLabel("Manipulated variable (MV) custom tag")
      .fill("Simulink.Device1.Python.PY");
    await templateField.selectOption("Allen-Bradley PlantPAx");

    await expect(tagField).toHaveValue("Simulink.Device1.Python.Inp_PV");
    await expect(
      mvRow.getByRole("button", { name: "Template tag", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      mvRow.getByLabel("Manipulated variable (MV) custom tag"),
    ).toHaveCount(0);
  });

  test("shows template defaults and applies a per-tune MV tag override", async ({
    page,
  }) => {
    const templateField = page.getByRole("combobox", { name: "Template" });
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

  test("resets custom mappings when the base tag changes but keeps fixed values", async ({
    page,
  }) => {
    const templateField = page.getByRole("combobox", { name: "Template" });
    const mvRow = mappingRow(page, "Manipulated variable (MV)");
    const directionRow = mappingRow(page, "Controller direction");
    const pvHighRow = mappingRow(page, "PV range high");
    const pvLowRow = mappingRow(page, "PV range low");

    await templateField.selectOption("Yokogawa CentumVP");
    await page.getByLabel("Tag name").fill("Loop101.PV");
    await mvRow
      .getByRole("button", { name: "Custom tag", exact: true })
      .click();
    await mvRow
      .getByLabel("Manipulated variable (MV) custom tag")
      .fill("Loop101.PY");
    await directionRow
      .getByRole("button", { name: "Custom tag", exact: true })
      .click();
    await directionRow
      .getByLabel("Controller direction custom read tag")
      .fill("Loop101.ACTION");
    await pvLowRow
      .getByRole("button", { name: "Custom tag", exact: true })
      .click();
    await pvLowRow
      .getByLabel("PV range low custom read tag")
      .fill("Loop101.PVLOW");
    await pvHighRow
      .getByRole("button", { name: "Fixed value", exact: true })
      .click();
    await pvHighRow.getByLabel("PV range high fixed value").fill("90");

    await page.getByLabel("Tag name").fill("Loop202.PV");

    await expect(page.getByLabel("Tag name")).toHaveValue("Loop202.PV");
    await expect(
      mvRow.getByRole("button", { name: "Template tag", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      mvRow.getByLabel("Manipulated variable (MV) custom tag"),
    ).toHaveCount(0);
    await expect(
      directionRow.getByRole("button", { name: "Template tag", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      directionRow.getByLabel("Controller direction custom read tag"),
    ).toHaveCount(0);
    await expect(
      pvLowRow.getByRole("button", { name: "Template tag", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      pvLowRow.getByLabel("PV range low custom read tag"),
    ).toHaveCount(0);
    await expect(
      pvHighRow.getByRole("button", { name: "Fixed value", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(pvHighRow.getByLabel("PV range high fixed value")).toHaveValue(
      "90",
    );
  });

  test("resets one mapping row or all mapping overrides", async ({ page }) => {
    const templateField = page.getByRole("combobox", { name: "Template" });
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
    const templateField = page.getByRole("combobox", { name: "Template" });
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
    const templateField = page.getByRole("combobox", { name: "Template" });
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
});
