import { expect, test } from "@playwright/test";
import {
  OPC_BROWSER_SUITE,
  openOpcDaRunForm,
  mappingRow,
  browseNode,
  browsePage,
} from "./support/opcBrowser";
import {
  expectNoAccessibilityViolations,
  setTheme,
} from "./support/accessibility";

/**
 * Tag-tree selection coverage: template-specific PV selection, paged and
 * branch-and-item browsing with browse-session cleanup, and the non-Good quality
 * confirmation.
 */
test.describe(OPC_BROWSER_SUITE, () => {
  test.beforeEach(async ({ page }) => {
    await openOpcDaRunForm(page);
  });

  test("uses the active template PV suffix when a browse leaf is selected", async ({
    page,
  }) => {
    const originalTag = "Simulink.Device1._System._DemandPoll";
    const readTags: string[] = [];
    await page
      .getByRole("combobox", { name: "Template" })
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
      const parentNodeKey = url.searchParams.get("parent_node_key");
      const nodes =
        parentNodeKey === "system"
          ? [browseNode("demand-poll", originalTag, "item", originalTag)]
          : [browseNode("system", "Simulink.Device1._System", "branch")];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage(nodes)),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    const tree = page.getByRole("tree", { name: "OPC tag hierarchy" });
    const systemNode = page.getByRole("treeitem", {
      name: "Simulink.Device1._System",
      exact: true,
    });
    const dialog = page.getByRole("dialog", {
      name: "Browse tags on Yokogawa.CSHIS_OPC.1",
    });
    const closeButton = dialog.getByRole("button", { name: "Close" });
    await expect(tree).toBeVisible();
    await expect(systemNode).toHaveAttribute("aria-expanded", "false");
    await expect(
      page.getByText("Select a tag to test its live value and quality."),
    ).toBeVisible();
    await expect(closeButton).toBeFocused();
    await expectNoAccessibilityViolations(page);
    await dialog.evaluate((element) => {
      (element as HTMLDialogElement).focus();
    });
    await page.keyboard.press("Tab");
    await expect(closeButton).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(dialog.locator(":focus")).toHaveCount(1);

    const tagNode = page.getByRole("treeitem", { name: originalTag });
    await systemNode.focus();
    await page.keyboard.press("ArrowRight");
    await expect(systemNode).toHaveAttribute("aria-expanded", "true");
    await expect(tagNode).toBeVisible();
    await page.keyboard.press("ArrowDown");
    await expect(tagNode).toBeFocused();
    await page.keyboard.press("ArrowUp");
    await expect(systemNode).toBeFocused();
    await page.keyboard.press("End");
    await expect(tagNode).toBeFocused();
    await page.keyboard.press("Home");
    await expect(systemNode).toBeFocused();
    await page.keyboard.press("End");
    await expect(tagNode).toBeFocused();
    await page.keyboard.press("Space");
    await expect(tagNode).toHaveAttribute("aria-selected", "true");
    await page.evaluate(() => {
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: {
          writeText: async (value: string) => {
            window.localStorage.setItem("copied-item-id", value);
          },
        },
      });
    });
    await page.getByRole("button", { name: "Copy ItemID" }).click();
    await expect(
      page.getByText("ItemID copied to the clipboard.", { exact: true }),
    ).toBeVisible();
    expect(
      await page.evaluate(() => localStorage.getItem("copied-item-id")),
    ).toBe(originalTag);
    await tagNode.focus();
    await page.keyboard.press("Enter");
    await page.keyboard.press("ArrowLeft");
    await expect(systemNode).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await expect(systemNode).toHaveAttribute("aria-expanded", "false");
    await page.keyboard.press("ArrowRight");
    await expect(tagNode).toBeVisible();
    await page.keyboard.press("ArrowRight");
    await expect(tagNode).toBeFocused();

    await page.getByRole("button", { name: "Select tag" }).click();

    await expect(page.getByLabel("Tag name")).toHaveValue(
      "Simulink.Device1._System.PV",
    );
    await expect(
      mappingRow(page, "Manipulated variable (MV)").getByRole("button", {
        name: "Template tag",
        exact: true,
      }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(readTags).toEqual([originalTag]);

    await expect(
      page.getByRole("button", { name: "Browse tags" }),
    ).toBeFocused();
    await setTheme(page, "light");
    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(
      page.getByRole("treeitem", { name: "Simulink.Device1._System" }),
    ).toBeVisible();
    await expectNoAccessibilityViolations(page);

    await page
      .getByRole("treeitem", {
        name: "Simulink.Device1._System",
      })
      .dblclick();
    await expect(
      page.getByRole("treeitem", {
        name: "Simulink.Device1._System._DemandPoll",
      }),
    ).toBeVisible();
    await page
      .getByRole("treeitem", {
        name: "Simulink.Device1._System._DemandPoll",
      })
      .dblclick();

    await expect(page.getByLabel("Tag name")).toHaveValue(
      "Simulink.Device1._System.PV",
    );
    expect(readTags).toEqual([originalTag, originalTag]);
  });

  test("loads additional pages, expands branch-and-item nodes, and closes the session", async ({
    page,
  }) => {
    const selectedItemId = "Unit1.LIC101.PV";
    const readTags: string[] = [];
    const closePaths: string[] = [];
    await page
      .getByRole("combobox", { name: "Template" })
      .selectOption("Yokogawa CentumVP");
    await page
      .getByLabel("OPC DA server ProgID")
      .fill("Matrikon.OPC.Simulation");
    await page.getByLabel("Tag name").fill("");

    await page.route("**/api/opc/read**", async (route) => {
      const url = new URL(route.request().url());
      readTags.push(url.searchParams.get("tag") ?? "");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          tag: selectedItemId,
          value: "42.0",
          quality: "good",
          timestamp: null,
        }),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      if (route.request().method() === "DELETE") {
        const url = new URL(route.request().url());
        closePaths.push(url.pathname);
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ closed: true }),
        });
        return;
      }
      const url = new URL(route.request().url());
      const parentNodeKey = url.searchParams.get("parent_node_key");
      const pageToken = url.searchParams.get("page_token");
      const nodes =
        parentNodeKey === "loop"
          ? [browseNode("sv", "SV", "item", "Unit1.LIC101.SV")]
          : pageToken === "root-next"
            ? [
                browseNode(
                  "loop",
                  "Unit1.LIC101",
                  "branch_and_item",
                  selectedItemId,
                ),
              ]
            : [browseNode("first", "First", "item", "First.PV")];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          browsePage(nodes, {
            nextPageToken: pageToken ? null : "root-next",
            complete: Boolean(pageToken || parentNodeKey),
          }),
        ),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(page.getByRole("treeitem", { name: "First" })).toBeVisible();
    await page.getByRole("button", { name: "Load more" }).click();
    await page.getByRole("treeitem", { name: "Unit1.LIC101" }).dblclick();
    await expect(page.getByText(`Selected: ${selectedItemId}`)).toBeVisible();
    await expect(
      page.getByRole("treeitem", { name: "Unit1.LIC101" }),
    ).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByRole("treeitem", { name: "SV" })).toBeVisible();
    await page.getByRole("button", { name: "Select tag" }).click();

    await expect(page.getByLabel("Tag name")).toHaveValue(selectedItemId);
    expect(readTags).toEqual([selectedItemId]);
    await expect
      .poll(() => closePaths)
      .toEqual(["/api/opc/browse/sessions/session-1"]);
  });

  test("warns before selecting a tag whose OPC quality is not Good", async ({
    page,
  }) => {
    const originalTag = "Simulink.Device1._System._DemandPoll";
    const readTags: string[] = [];
    await page
      .getByRole("combobox", { name: "Template" })
      .selectOption("Yokogawa CentumVP");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByLabel("Tag name").fill("Sim.Loop1.PV");

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
      const parentNodeKey = url.searchParams.get("parent_node_key");
      const nodes =
        parentNodeKey === "system"
          ? [browseNode("demand-poll", originalTag, "item", originalTag)]
          : [browseNode("system", "Simulink.Device1._System", "branch")];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage(nodes)),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(
      page.getByRole("treeitem", { name: "Simulink.Device1._System" }),
    ).toBeVisible();
    const systemNode = page.getByRole("treeitem", {
      name: "Simulink.Device1._System",
    });
    await systemNode.focus();
    await page.keyboard.press("ArrowRight");
    await page.getByRole("treeitem", { name: originalTag }).click();
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
