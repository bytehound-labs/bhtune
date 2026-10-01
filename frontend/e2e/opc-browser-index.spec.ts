import { expect, test } from "@playwright/test";
import {
  OPC_BROWSER_SUITE,
  openOpcDaRunForm,
  browseNode,
  browsePage,
  searchIndexStatus,
  indexedSearchResponse,
} from "./support/opcBrowser";

/**
 * Persistent search-index coverage: building, disabling, and deleting an index,
 * debounced indexed search, refresh, index-state diagnostics, and lazy browsing without
 * a usable index.
 */
test.describe(OPC_BROWSER_SUITE, () => {
  test.beforeEach(async ({ page }) => {
    await openOpcDaRunForm(page);
  });

  test("builds, disables, and deletes a server index", async ({ page }) => {
    let status = searchIndexStatus("not_indexed", false);
    let deletionPolls = 0;
    await page.getByLabel("OPC DA server ProgID").fill("Test.Server");

    await page.unroute("**/api/opc/search-index/status**");
    await page.route("**/api/opc/search-index/status**", async (route) => {
      if (status.state === "deleting") {
        deletionPolls += 1;
        if (deletionPolls >= 2) {
          status = searchIndexStatus("not_indexed", false);
        }
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(status),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage([])),
      });
    });
    await page.route("**/api/opc/search-index/refresh**", async (route) => {
      status = searchIndexStatus("ready", true);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(status),
      });
    });
    await page.route(
      "**/api/opc/search-index/auto-refresh**",
      async (route) => {
        const url = new URL(route.request().url());
        const enabled = url.searchParams.get("enabled") === "true";
        status = searchIndexStatus("ready", enabled);
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(status),
        });
      },
    );
    await page.route("**/api/opc/search-index**", async (route) => {
      if (route.request().method() !== "DELETE") {
        await route.fallback();
        return;
      }
      deletionPolls = 0;
      status = searchIndexStatus("deleting", false);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(status),
      });
    });
    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(
      page.getByRole("button", { name: "Build index", exact: true }),
    ).toBeVisible();

    await page
      .getByRole("button", { name: "Build index", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "Refresh index", exact: true }),
    ).toBeVisible();
    await expect(page.getByText("Auto-refresh: enabled")).toBeVisible();
    await expect(
      page.getByText("Next refresh: in 7 days 2 hours"),
    ).toBeVisible();

    await page
      .getByRole("button", { name: "Disable auto-refresh", exact: true })
      .click();
    await expect(
      page.getByRole("button", {
        name: "Enable auto-refresh",
        exact: true,
      }),
    ).toBeVisible();
    await expect(page.getByText("Auto-refresh: disabled")).toBeVisible();
    await expect(page.getByText("Next refresh:")).toHaveCount(0);

    await page
      .getByRole("button", { name: "Delete index", exact: true })
      .click();
    const deleteDialog = page.getByRole("dialog", {
      name: "Delete tag index?",
    });
    await expect(deleteDialog).toBeVisible();
    await expect(
      deleteDialog.getByText(
        "Indexed search data and this server's index enrollment will be removed.",
      ),
    ).toBeVisible();
    await deleteDialog
      .getByRole("button", { name: "Delete index", exact: true })
      .click();
    await expect(page.getByText("Index: deleting")).toBeVisible();
    await expect(
      page.getByText(
        "Deleting the tag index… browse and direct reads remain available.",
      ),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Build index", exact: true }),
    ).toBeDisabled();
    await expect(
      page.getByRole("button", { name: "Build index", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Build index", exact: true }),
    ).toBeEnabled();
    await expect(
      page.getByRole("button", { name: "Delete index", exact: true }),
    ).toHaveCount(0);
  });

  test("provides debounced indexed search with keyboard selection and exact ItemIDs", async ({
    page,
  }) => {
    const queries: Array<{ query: string; mode: string }> = [];
    const selectedItemId = "FCS0202!204FI00510.PV";
    const readTags: string[] = [];

    await page
      .getByRole("combobox", { name: "Template" })
      .selectOption("Allen-Bradley PlantPAx");
    await page.getByLabel("Tag name").fill("");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.route("**/api/opc/browse**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          browsePage([browseNode("root", "FCS0201", "branch")]),
        ),
      });
    });
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
    await page.route("**/api/opc/search-index/search**", async (route) => {
      const url = new URL(route.request().url());
      const query = url.searchParams.get("query") ?? "";
      queries.push({
        query,
        mode: url.searchParams.get("match_mode") ?? "",
      });
      if (query === "fc") await slowPrefixSearch;
      const matches =
        query === "fcs"
          ? [
              {
                item_id: "FCS0201!204FI00510.PV",
                display_name: "204FI00510.PV",
                kind: "item" as const,
                breadcrumbs: ["FCS0201"],
              },
              {
                item_id: selectedItemId,
                display_name: "204FI00510.PV",
                kind: "item" as const,
                breadcrumbs: ["FCS0202"],
              },
            ]
          : [
              {
                item_id: "FCS0201!204FI00510.PV",
                display_name: "204FI00510.PV",
                kind: "item" as const,
                breadcrumbs: ["FCS0201"],
              },
            ];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(indexedSearchResponse(matches)),
      });
    });

    let releaseSlowPrefixSearch: (() => void) | undefined;
    const slowPrefixSearch = new Promise<void>((resolve) => {
      releaseSlowPrefixSearch = resolve;
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    const search = page.getByLabel("Search OPC tags");
    await expect(page.getByText("Smart contains search")).toHaveCount(0);
    await expect(page.getByText("results stay on the gateway")).toHaveCount(0);
    await search.fill("f");
    await expect.poll(() => queries.length).toBe(0);

    await search.fill("fc");
    await expect
      .poll(() => queries.some(({ query }) => query === "fc"))
      .toBe(true);
    await search.fill("fcs");
    await expect(page.getByRole("listbox").getByRole("option")).toHaveCount(2);
    expect(queries).toEqual(
      expect.arrayContaining([
        { query: "fc", mode: "prefix" },
        { query: "fcs", mode: "contains" },
      ]),
    );
    expect(queries.some(({ query }) => query === "f")).toBe(false);

    await search.press("ArrowDown");
    await search.press("Enter");
    await expect(
      page.getByText(`Selected: ${selectedItemId}`, { exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Select tag" }).click();

    await expect(page.getByLabel("Tag name")).toHaveValue(
      "FCS0202!204FI00510.Inp_PV",
    );
    expect(readTags).toEqual([selectedItemId]);
    releaseSlowPrefixSearch?.();
  });

  test("confirms an indexed search result on double-click", async ({
    page,
  }) => {
    const selectedItemId = "FCS0202!204FI00510.PV";
    const readTags: string[] = [];

    await page
      .getByRole("combobox", { name: "Template" })
      .selectOption("Allen-Bradley PlantPAx");
    await page.getByLabel("Tag name").fill("");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.route("**/api/opc/browse**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage([])),
      });
    });
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
    await page.route("**/api/opc/search-index/search**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          indexedSearchResponse([
            {
              item_id: selectedItemId,
              display_name: "204FI00510.PV",
              kind: "item",
              breadcrumbs: ["FCS0202"],
            },
          ]),
        ),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    const search = page.getByLabel("Search OPC tags");
    await search.fill("fcs");
    const result = page.locator(`button[title="${selectedItemId}"]`);
    await expect(result).toBeVisible();
    await result.dblclick();

    await expect(page.getByLabel("Tag name")).toHaveValue(
      "FCS0202!204FI00510.Inp_PV",
    );
    await expect(
      page.getByRole("heading", {
        name: "Browse tags on Yokogawa.CSHIS_OPC.1",
      }),
    ).toHaveCount(0);
    expect(readTags).toEqual([selectedItemId]);
  });

  test("refreshes the indexed namespace without turning status into an error", async ({
    page,
  }) => {
    let statusCalls = 0;
    await page.unroute("**/api/opc/search-index/status**");
    await page.route("**/api/opc/search-index/status**", async (route) => {
      statusCalls += 1;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          searchIndexStatus(
            statusCalls < 2
              ? "ready"
              : statusCalls === 2
                ? "refreshing"
                : "ready",
          ),
        ),
      });
    });
    await page.getByLabel("Tag name").fill("");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.route("**/api/opc/browse**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage([])),
      });
    });
    await page.route(/search-index\/refresh/, async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(searchIndexStatus("refreshing")),
      });
    });

    await page.getByRole("button", { name: "Browse tags" }).click();
    await page.getByRole("button", { name: "Refresh index" }).click();
    await expect(page.getByText("Index: refreshing")).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Refresh index" }),
    ).toBeDisabled();
    await expect(page.getByText("Index: ready")).toBeVisible({
      timeout: 5_000,
    });
    await expect(
      page.getByRole("button", { name: "Refresh index" }),
    ).toBeEnabled();
    await expect(
      page.getByText("Unable to refresh the tag index."),
    ).not.toBeVisible();
  });

  test("hides recovered inventory diagnostics when the index is ready", async ({
    page,
  }) => {
    const diagnostic =
      "1,779 non-navigable DA2 branches were skipped during inventory.";
    await page.unroute("**/api/opc/search-index/status**");
    await page.route("**/api/opc/search-index/status**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(searchIndexStatus("ready", true, diagnostic)),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage([])),
      });
    });

    await page.getByLabel("Tag name").fill("");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByRole("button", { name: "Browse tags" }).click();

    await expect(page.getByText("Index: ready")).toBeVisible();
    await expect(
      page.getByText(`Index warning: ${diagnostic}`, { exact: true }),
    ).not.toBeVisible();
  });

  test("shows a diagnostic when the index has failed", async ({ page }) => {
    const diagnostic = "the inventory database is unavailable";
    await page.unroute("**/api/opc/search-index/status**");
    await page.route("**/api/opc/search-index/status**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(searchIndexStatus("failed", true, diagnostic)),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage([])),
      });
    });

    await page.getByLabel("Tag name").fill("");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByRole("button", { name: "Browse tags" }).click();

    const indexError = page.getByText(`Index error: ${diagnostic}`, {
      exact: true,
    });
    const unavailableMessage = page.getByText(
      "Global search is unavailable because the gateway has no complete index. Lazy browse and direct ItemID entry remain available.",
      { exact: true },
    );
    await expect(indexError).toBeVisible();
    await expect(unavailableMessage).toBeVisible();
    await expect(indexError).toHaveClass(/block/);
    await expect(unavailableMessage).toHaveClass(/block/);

    const [errorBox, unavailableBox] = await Promise.all([
      indexError.boundingBox(),
      unavailableMessage.boundingBox(),
    ]);
    expect(errorBox).not.toBeNull();
    expect(unavailableBox).not.toBeNull();
    expect(unavailableBox!.y).toBeGreaterThan(errorBox!.y + errorBox!.height);
  });

  test("offers a first build when the server has no index", async ({
    page,
  }) => {
    await page.unroute("**/api/opc/search-index/status**");
    await page.route("**/api/opc/search-index/status**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(searchIndexStatus("not_indexed", false)),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage([])),
      });
    });

    await page.getByLabel("Tag name").fill("");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByRole("button", { name: "Browse tags" }).click();

    await expect(
      page.getByText(
        "Global search is unavailable until the gateway has a complete index. Lazy browse and direct ItemID entry remain available.",
        { exact: true },
      ),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Build index" }),
    ).toBeEnabled();
  });

  test("keeps lazy browse usable without an index and retries a failed level", async ({
    page,
  }) => {
    const selectedItemId = "FCS0201!204FI00510.PV";
    let childBrowseAttempts = 0;
    let indexedSearchRequests = 0;

    await page.unroute("**/api/opc/search-index/status**");
    await page.route("**/api/opc/search-index/status**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(searchIndexStatus("not_indexed", false)),
      });
    });
    page.on("request", (request) => {
      if (request.url().includes("/api/opc/search-index/search")) {
        indexedSearchRequests += 1;
      }
    });
    await page.route("**/api/opc/read**", async (route) => {
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
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ closed: true }),
        });
        return;
      }
      const url = new URL(route.request().url());
      const parentNodeKey = url.searchParams.get("parent_node_key");
      if (parentNodeKey === "fcs0201") {
        childBrowseAttempts += 1;
        if (childBrowseAttempts === 1) {
          await route.fulfill({
            status: 503,
            contentType: "application/json",
            body: JSON.stringify({ error: "temporary browse failure" }),
          });
          return;
        }
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(
            browsePage([
              browseNode("pv", selectedItemId, "item", selectedItemId),
            ]),
          ),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          browsePage([browseNode("fcs0201", "FCS0201", "branch")]),
        ),
      });
    });

    await page
      .getByRole("combobox", { name: "Template" })
      .selectOption("Yokogawa CentumVP");
    await page.getByLabel("Tag name").fill("");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByRole("button", { name: "Browse tags" }).click();

    await expect(page.getByLabel("Search OPC tags")).toBeDisabled();
    const rootNode = page.getByRole("treeitem", { name: "FCS0201" });
    await expect(rootNode).toBeVisible();
    await rootNode.focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
    await page.getByRole("button", { name: "Retry" }).click();
    await expect(
      page.getByRole("treeitem", { name: selectedItemId }),
    ).toBeVisible();

    await page.getByRole("treeitem", { name: selectedItemId }).click();
    await page.getByRole("button", { name: "Select tag" }).click();
    await expect(page.getByLabel("Tag name")).toHaveValue(selectedItemId);
    expect(indexedSearchRequests).toBe(0);
  });

  test("surfaces a refresh failure without relying on gateway error text", async ({
    page,
  }) => {
    await page.unroute("**/api/opc/search-index/status**");
    await page.route("**/api/opc/search-index/status**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(searchIndexStatus("ready")),
      });
    });
    await page.route("**/api/opc/browse**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(browsePage([])),
      });
    });
    await page.route(/search-index\/refresh/, async (route) => {
      await route.fulfill({
        status: 400,
        contentType: "application/json",
        body: JSON.stringify({
          error:
            "refresh the OPC namespace index: the server is not enrolled for indexing",
        }),
      });
    });

    await page.getByLabel("Tag name").fill("");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByRole("button", { name: "Browse tags" }).click();
    await page.getByRole("button", { name: "Refresh index" }).click();

    await expect(
      page.getByText("Unable to refresh the tag index.", { exact: true }),
    ).toBeVisible();
  });
});
