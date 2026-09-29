import { expect, test } from "@playwright/test";
import {
  OPC_BROWSER_SUITE,
  openOpcDaRunForm,
  expectTreeNodeVisible,
  browseNode,
  browsePage,
  searchIndexStatus,
  indexedSearchResponse,
} from "./support/opcBrowser";

/**
 * Saved-tag restoration coverage: reopening the tag browser at the previously selected
 * tag through indexed or bounded live search, the loading overlay until the exact row is
 * visible, cancellation, and the root-level fallback.
 */
test.describe(OPC_BROWSER_SUITE, () => {
  test.beforeEach(async ({ page }) => {
    await openOpcDaRunForm(page);
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
    await page.route("**/api/opc/search-index/search**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          indexedSearchResponse([
            {
              item_id: originalTag,
              display_name: originalTag,
              kind: "item",
              breadcrumbs: ["Simulink", "Simulink._Statistics"],
            },
          ]),
        ),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      const url = new URL(route.request().url());
      const parentNodeKey = url.searchParams.get("parent_node_key");
      const fillerNodes = Array.from({ length: 30 }, (_, index) =>
        browseNode(
          `filler-${index}`,
          `Filler${index}`,
          "item",
          `Filler${index}`,
        ),
      );
      const nodes =
        parentNodeKey === "simulink"
          ? [browseNode("statistics", "Simulink._Statistics", "branch")]
          : parentNodeKey === "statistics"
            ? [browseNode("pv", originalTag, "item", originalTag)]
            : [...fillerNodes, browseNode("simulink", "Simulink", "branch")];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage(nodes)),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(page.getByRole("button", { name: originalTag })).toBeVisible();
    await expect(page.getByText(`Selected: ${originalTag}`)).toBeVisible();
    const treeViewport = page.getByTestId("opc-tag-tree-viewport");
    await expectTreeNodeVisible(
      page.getByRole("button", { name: originalTag, exact: true }),
      treeViewport,
    );
    await page.getByRole("button", { name: originalTag }).click();
    await page.getByRole("button", { name: "Select tag" }).click();

    await expect(page.getByLabel("Tag name")).toHaveValue(originalTag);

    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(page.getByRole("button", { name: originalTag })).toBeVisible();
    await expect(page.getByText(`Selected: ${originalTag}`)).toBeVisible();
    await expectTreeNodeVisible(
      page.getByRole("button", { name: originalTag, exact: true }),
      treeViewport,
    );
    await expect(page.getByRole("button", { name: "Collapse" })).toHaveCount(2);
  });

  test("keeps saved-tag restoration covered until the exact row is visible", async ({
    page,
  }) => {
    const originalTag = "FCS0217!204FC03010.PV";
    let rootRequestSeen = false;
    let searchRequestSeen = false;
    let releaseSearch: () => void = () => undefined;
    const searchGate = new Promise<void>((resolve) => {
      releaseSearch = resolve;
    });
    let releaseLeaf: () => void = () => undefined;
    const leafGate = new Promise<void>((resolve) => {
      releaseLeaf = resolve;
    });

    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByLabel("Tag name").fill(originalTag);
    await page.route("**/api/opc/search-index/search**", async (route) => {
      searchRequestSeen = true;
      await searchGate;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          indexedSearchResponse([
            {
              item_id: originalTag,
              display_name: "PV",
              kind: "item",
              breadcrumbs: ["FCS0217", "204FC03010"],
            },
          ]),
        ),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      if (route.request().method() === "DELETE") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ closed: true }),
        });
        return;
      }

      const url = new URL(route.request().url());
      const parentNodeKey = url.searchParams.get("parent_node_key");
      if (!parentNodeKey) rootRequestSeen = true;
      const nodes =
        parentNodeKey === "fcs0217"
          ? [browseNode("loop", "204FC03010", "branch")]
          : parentNodeKey === "loop"
            ? (() => {
                return [browseNode("pv", "PV", "item", originalTag)];
              })()
            : [
                ...Array.from({ length: 30 }, (_, index) =>
                  browseNode(
                    `filler-${index}`,
                    `Filler${index}`,
                    "item",
                    `Filler${index}`,
                  ),
                ),
                browseNode("fcs0217", "FCS0217", "branch"),
              ];
      if (parentNodeKey === "loop") {
        await leafGate;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage(nodes)),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    const dialog = page.getByRole("dialog", {
      name: "Browse tags on Yokogawa.CSHIS_OPC.1",
    });
    const restoring = dialog
      .getByRole("status")
      .filter({ hasText: "Locating saved tag…" });
    await expect(restoring).toBeVisible();
    const globalSearch = dialog.getByLabel("Search OPC tags");
    await expect(globalSearch).toBeVisible();
    await expect(globalSearch).toBeEnabled();
    await expect(globalSearch).not.toHaveAttribute("aria-hidden", "true");
    await expect(
      dialog.locator('div[aria-hidden="true"]').first(),
    ).toHaveAttribute("aria-hidden", "true");
    await expect(dialog.getByRole("button", { name: "Close" })).toBeEnabled();

    await expect.poll(() => rootRequestSeen).toBe(true);
    await expect.poll(() => searchRequestSeen).toBe(true);
    releaseSearch();
    await expect(
      dialog.locator("button").filter({ hasText: /^FCS0217$/ }),
    ).toHaveCount(1);
    await expect(
      dialog.locator("button").filter({ hasText: /^204FC03010$/ }),
    ).toHaveCount(1);
    await expect(restoring).toBeVisible();
    await expect(dialog.getByRole("button", { name: "PV" })).toHaveCount(0);

    releaseLeaf();
    const selected = dialog.getByRole("button", { name: "PV", exact: true });
    await expect(selected).toBeVisible();
    await expect(dialog.getByText(`Selected: ${originalTag}`)).toBeVisible();
    await expectTreeNodeVisible(
      selected,
      dialog.getByTestId("opc-tag-tree-viewport"),
    );
    await expect(restoring).not.toBeVisible();
  });

  test("keeps the tag-browser loading overlay dismissible during cancellation", async ({
    page,
  }) => {
    let rootRequestSeen = false;
    let releaseRoot: () => void = () => undefined;
    const rootGate = new Promise<void>((resolve) => {
      releaseRoot = resolve;
    });
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByLabel("Tag name").fill("FCS0217!204FC03010.PV");
    await page.route("**/api/opc/browse**", async (route) => {
      if (route.request().method() === "DELETE") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ closed: true }),
        });
        return;
      }
      rootRequestSeen = true;
      await rootGate;
      await route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({ error: "temporary browse failure" }),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    const dialog = page.getByRole("dialog", {
      name: "Browse tags on Yokogawa.CSHIS_OPC.1",
    });
    await expect(dialog.getByRole("status")).toContainText(
      "Locating saved tag…",
    );
    await expect.poll(() => rootRequestSeen).toBe(true);
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).not.toBeVisible();
    releaseRoot();
  });

  test("reopens a saved tag with bounded live search when no index is available", async ({
    page,
  }) => {
    const originalTag = "FCS0217!204FC03010.PV";
    let liveSearchRequests = 0;
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByLabel("Tag name").fill(originalTag);

    await page.unroute("**/api/opc/search-index/status**");
    await page.route("**/api/opc/search-index/status**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(searchIndexStatus("not_indexed", false)),
      });
    });
    await page.route("**/api/opc/search**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname !== "/api/opc/search") {
        await route.fallback();
        return;
      }
      liveSearchRequests += 1;
      expect(url.searchParams.get("session_id")).toBe("session-1");
      expect(url.searchParams.get("scope_node_key")).toBe("fcs0217");
      expect(url.searchParams.get("query")).toBe(originalTag);
      expect(url.searchParams.get("match_mode")).toBe("exact");
      await route.fulfill({
        status: 200,
        headers: { "content-type": "text/event-stream" },
        body: [
          "event: match\n",
          `data: ${JSON.stringify({
            node: {
              node_key: "pv",
              display_name: "PV",
              kind: "item",
              item_id: originalTag,
            },
            breadcrumbs: [
              { node_key: "fcs0217", display_name: "" },
              { node_key: "loop", display_name: "204FC03010" },
            ],
          })}\n\n`,
          'event: completed\ndata: {"complete":true,"cancelled":false,"truncated":false,"warning":null}\n\n',
        ].join(""),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      const url = new URL(route.request().url());
      const parentNodeKey = url.searchParams.get("parent_node_key");
      const nodes =
        parentNodeKey === "fcs0217"
          ? [browseNode("loop", "204FC03010", "branch")]
          : parentNodeKey === "loop"
            ? [browseNode("pv", "PV", "item", originalTag)]
            : [
                browseNode("unrelated", "SCS0130", "item", "SCS0130.PV"),
                browseNode("fcs0217", "FCS0217", "branch_and_item", "FCS0217"),
              ];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage(nodes)),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(page.getByText(`Selected: ${originalTag}`)).toBeVisible();
    await expect(page.getByRole("button", { name: "FCS0217" })).toBeVisible();
    await expect(
      page.getByRole("button", { name: "204FC03010" }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: "PV" })).toBeVisible();
    expect(liveSearchRequests).toBe(1);
  });

  test("falls back to the root only when indexed and live search fail", async ({
    page,
  }) => {
    const originalTag = "FCS0217!204FC03010.PV";
    let indexedSearchRequests = 0;
    let liveSearchRequests = 0;

    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByLabel("Tag name").fill(originalTag);

    await page.unroute("**/api/opc/search-index/status**");
    await page.route("**/api/opc/search-index/status**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          searchIndexStatus(
            "failed",
            true,
            "inventory stream ended before completion",
            1,
          ),
        ),
      });
    });
    await page.route("**/api/opc/search-index/search**", async (route) => {
      indexedSearchRequests += 1;
      await route.fulfill({
        status: 400,
        contentType: "application/json",
        body: JSON.stringify({
          error: "persistent index search is unavailable",
        }),
      });
    });
    await page.route("**/api/opc/search**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname !== "/api/opc/search") {
        await route.fallback();
        return;
      }
      liveSearchRequests += 1;
      expect(url.searchParams.get("scope_node_key")).toBe("fcs0217");
      await route.fulfill({
        status: 400,
        contentType: "application/json",
        body: JSON.stringify({
          error: "live namespace search is unavailable",
        }),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      const url = new URL(route.request().url());
      const parentNodeKey = url.searchParams.get("parent_node_key");
      const nodes =
        parentNodeKey === "fcs0217"
          ? [browseNode("loop", "204FC03010", "branch")]
          : parentNodeKey === "loop"
            ? [browseNode("pv", "PV", "item", originalTag)]
            : [
                browseNode("unrelated", "SCS0130", "item", "SCS0130.PV"),
                browseNode("fcs0217", "FCS0217", "branch_and_item", "FCS0217"),
              ];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage(nodes)),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(page.getByText("Index: failed")).toBeVisible();
    await expect(page.getByText("Selected: SCS0130.PV")).toBeVisible();
    await expect(page.getByRole("button", { name: "FCS0217" })).toBeVisible();
    await expect(page.getByRole("button", { name: "204FC03010" })).toHaveCount(
      0,
    );
    expect(indexedSearchRequests).toBe(1);
    expect(liveSearchRequests).toBe(1);
  });
});
