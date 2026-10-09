import { expect, test, type Page } from "@playwright/test";
import type { OpcSearchIndexStatusResponse } from "../src/api/opc";
import {
  OPC_BROWSER_SUITE,
  openOpcDaRunForm,
  browseNode,
  browsePage,
  searchIndexStatus,
  indexedSearchResponse,
} from "./support/opcBrowser";

async function mockBrowserIndex(
  page: Page,
  status: OpcSearchIndexStatusResponse,
) {
  const requests: URL[] = [];
  const index = { status, requests };
  await page.unroute("**/api/opc/search-index/status**");
  await page.route("**/api/opc/search-index/status**", async (route) => {
    requests.push(new URL(route.request().url()));
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(index.status),
    });
  });
  await page.route("**/api/opc/browse**", async (route) => {
    if (route.request().method() === "DELETE") {
      await route.fulfill({ status: 204 });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(browsePage([])),
    });
  });
  await page.getByLabel("Tag name").fill("");
  await page.getByLabel("OPC DA server ProgID").fill("Test.Server");
  return index;
}

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
    await expect(
      page.getByText("Next refresh: in 7 days 2 hours"),
    ).toHaveAttribute(
      "title",
      await page.evaluate(
        (timestamp) => new Date(Number(timestamp)).toLocaleString(),
        status.scheduler.next_refresh_at,
      ),
    );

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

  for (const schedule of [
    { name: "null", nextRefreshAt: null },
    { name: "omitted", nextRefreshAt: undefined },
  ]) {
    test(`reports unscheduled auto-refresh with a ${schedule.name} schedule`, async ({
      page,
    }) => {
      const ready = searchIndexStatus("ready");
      const index = await mockBrowserIndex(page, {
        ...ready,
        scheduler: {
          ...ready.scheduler,
          next_refresh_at: schedule.nextRefreshAt,
        },
      });
      const toggles: boolean[] = [];
      const refreshRequests: string[] = [];
      page.on("request", (request) => {
        if (
          new URL(request.url()).pathname === "/api/opc/search-index/refresh"
        ) {
          refreshRequests.push(request.method());
        }
      });
      await page.route(
        "**/api/opc/search-index/auto-refresh**",
        async (route) => {
          const enabled =
            new URL(route.request().url()).searchParams.get("enabled") ===
            "true";
          toggles.push(enabled);
          index.status = {
            ...index.status,
            auto_refresh_enabled: enabled,
            scheduler: {
              ...index.status.scheduler,
              next_refresh_at: enabled
                ? schedule.nextRefreshAt
                : ready.scheduler.next_refresh_at,
            },
          };
          await route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify(index.status),
          });
        },
      );

      await page.getByRole("button", { name: "Browse tags" }).click();
      const unscheduled = page.getByText("Auto-refresh: not scheduled", {
        exact: true,
      });
      await expect(
        page.getByText("Index: ready", { exact: true }),
      ).toBeVisible();
      await expect(unscheduled).toBeVisible();
      await expect(unscheduled).toHaveAttribute(
        "title",
        "This server is enabled, but the gateway has not reported a scheduled refresh.",
      );
      await expect(page.getByText("Auto-refresh: enabled")).toHaveCount(0);
      await expect(page.getByText("Next refresh:")).toHaveCount(0);
      await expect(page.getByLabel("Search OPC tags")).toBeEnabled();
      await expect(
        page.getByRole("button", { name: "Refresh index", exact: true }),
      ).toBeEnabled();

      await page
        .getByRole("button", { name: "Disable server preference", exact: true })
        .click();
      await expect(page.getByText("Auto-refresh: disabled")).toBeVisible();
      await expect(page.getByText("Next refresh:")).toHaveCount(0);
      await page
        .getByRole("button", { name: "Enable auto-refresh", exact: true })
        .click();
      await expect(unscheduled).toBeVisible();
      await expect(page.getByText("Next refresh:")).toHaveCount(0);
      expect(index.status.state).toBe("ready");
      expect(index.status.active_generation).toBe(ready.active_generation);
      expect(toggles).toEqual([false, true]);
      expect(refreshRequests).toEqual([]);
    });
  }

  for (const policy of ["disabled", "paused"] as const) {
    for (const enabled of [true, false]) {
      test(`reports ${policy} gateway policy with server preference ${enabled}`, async ({
        page,
      }) => {
        const ready = searchIndexStatus("ready", enabled);
        const index = await mockBrowserIndex(page, {
          ...ready,
          scheduler: { ...ready.scheduler, auto_refresh_policy: policy },
        });
        const toggles: boolean[] = [];
        const refreshes: string[] = [];
        page.on("request", (request) => {
          if (
            new URL(request.url()).pathname === "/api/opc/search-index/refresh"
          ) {
            refreshes.push(request.method());
          }
        });
        await page.route(
          "**/api/opc/search-index/auto-refresh**",
          async (route) => {
            const requestedEnabled =
              new URL(route.request().url()).searchParams.get("enabled") ===
              "true";
            toggles.push(requestedEnabled);
            index.status = {
              ...index.status,
              auto_refresh_enabled: requestedEnabled,
            };
            await route.fulfill({
              status: 200,
              contentType: "application/json",
              body: JSON.stringify(index.status),
            });
          },
        );

        await page.getByRole("button", { name: "Browse tags" }).click();
        await expect(
          page.getByText("Index: ready", { exact: true }),
        ).toBeVisible();
        await expect(
          page.getByText(
            policy === "disabled"
              ? "Automatic refresh is blocked by gateway configuration (index.enabled = false). Cached search and manual refresh remain available."
              : "Automatic refresh is paused by gateway configuration (index.paused = true). Cached search and manual refresh remain available.",
          ),
        ).toBeVisible();
        await expect(page.getByText("Next refresh:")).toHaveCount(0);
        await expect(page.getByLabel("Search OPC tags")).toBeEnabled();
        await expect(
          page.getByRole("button", { name: "Refresh index", exact: true }),
        ).toBeEnabled();
        await expect(
          page.getByRole("button", {
            name: "Disable auto-refresh",
            exact: true,
          }),
        ).toHaveCount(0);

        if (enabled) {
          await expect(
            page.getByText("Auto-refresh: blocked by gateway", { exact: true }),
          ).toBeVisible();
          await expect(
            page.getByText("Server preference: enabled", { exact: true }),
          ).toBeVisible();
          await page
            .getByRole("button", {
              name: "Disable server preference",
              exact: true,
            })
            .click();
        }
        await expect(
          page.getByText("Auto-refresh: disabled", { exact: true }),
        ).toBeVisible();
        await expect(
          page.getByText("Server preference: disabled", { exact: true }),
        ).toBeVisible();
        await expect(
          page.getByRole("button", {
            name: "Enable auto-refresh",
            exact: true,
          }),
        ).toBeDisabled();
        await expect(
          page.getByRole("button", {
            name: "Enable auto-refresh",
            exact: true,
          }),
        ).toHaveAttribute("aria-describedby", "opc-auto-refresh-policy");
        expect(index.status.active_generation).toBe(ready.active_generation);
        expect(toggles).toEqual(enabled ? [false] : []);
        expect(refreshes).toEqual([]);
      });
    }
  }

  for (const policy of [null, undefined]) {
    test(`keeps an unreported ${policy === null ? "null" : "omitted"} gateway policy unknown`, async ({
      page,
    }) => {
      const ready = searchIndexStatus("ready");
      await mockBrowserIndex(page, {
        ...ready,
        scheduler: { ...ready.scheduler, auto_refresh_policy: policy },
      });
      await page.getByRole("button", { name: "Browse tags" }).click();
      await expect(
        page.getByText("Auto-refresh: policy unavailable", { exact: true }),
      ).toBeVisible();
      await expect(
        page.getByText("Server preference: enabled", { exact: true }),
      ).toBeVisible();
      await expect(
        page.getByText(
          "Controls only save this server's auto-refresh preference.",
        ),
      ).toBeVisible();
      await expect(
        page.getByRole("button", {
          name: "Disable server preference",
          exact: true,
        }),
      ).toBeEnabled();
      await expect(page.getByText("Next refresh:")).toHaveCount(0);
      await expect(page.getByLabel("Search OPC tags")).toBeEnabled();
    });
  }

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

  test("dismisses a cancelled refresh error immediately when Browse reopens", async ({
    page,
  }) => {
    const diagnostic = "inventory stream ended before completion";
    await page.clock.install();
    const index = await mockBrowserIndex(
      page,
      searchIndexStatus("ready", true, null, 7),
    );
    let cancellations = 0;
    await page.route("**/api/opc/search-index/refresh**", async (route) => {
      index.status = searchIndexStatus("refreshing", true, null, 7);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(index.status),
      });
    });
    await page.route("**/api/opc/search-index/control**", async (route) => {
      expect(new URL(route.request().url()).searchParams.get("action")).toBe(
        "cancel",
      );
      cancellations += 1;
      index.status = searchIndexStatus("failed", true, diagnostic, 7);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(index.status),
      });
    });

    const open = page.getByRole("button", { name: "Browse tags" });
    await open.click();
    await page.getByRole("button", { name: "Refresh index" }).click();
    const cancelledStatus = page.waitForResponse(
      "**/api/opc/search-index/status**",
    );
    await page.getByRole("button", { name: "Cancel build" }).click();
    await cancelledStatus;
    await expect(page.getByText(`Index error: ${diagnostic}`)).toBeVisible();
    await expect(
      page.getByText("Index: failed", { exact: true }),
    ).toBeVisible();
    await expect(page.getByLabel("Search OPC tags")).toBeEnabled();
    const requestsBeforeClose = index.requests.length;
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await open.click();

    await expect(page.getByRole("dialog")).toBeVisible();
    await expect(
      page.getByText("Index: failed", { exact: true }),
    ).toBeVisible();
    await expect(page.getByText(`Index error: ${diagnostic}`)).toHaveCount(0);
    await expect(page.getByLabel("Search OPC tags")).toBeEnabled();
    expect(index.requests).toHaveLength(requestsBeforeClose);
    expect(index.status.last_error).toBe(diagnostic);
    expect(index.status.active_generation).toBe(7);
    expect(cancellations).toBe(1);

    await page.getByRole("button", { name: "Close", exact: true }).click();
    await page.clock.fastForward(5_001);
    await open.click();
    await expect
      .poll(() => index.requests.length)
      .toBeGreaterThan(requestsBeforeClose);
    await expect(page.getByText(`Index error: ${diagnostic}`)).toHaveCount(0);

    await page.reload();
    await page.getByLabel("Driver").selectOption("opcda");
    await page.getByLabel("Tag name").fill("");
    await page.getByLabel("OPC DA server ProgID").fill("Test.Server");
    await open.click();
    await expect(page.getByText(`Index error: ${diagnostic}`)).toBeVisible();
  });

  test("shows a later same-text failure that finishes while Browse is closed", async ({
    page,
  }) => {
    await page.clock.install();
    const diagnostic = "the inventory database is unavailable";
    const index = await mockBrowserIndex(
      page,
      searchIndexStatus("failed", true, diagnostic, 7),
    );
    const open = page.getByRole("button", { name: "Browse tags" });
    const error = page.getByText(`Index error: ${diagnostic}`);
    await open.click();
    await expect(error).toBeVisible();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    index.status = {
      ...index.status,
      completed_at: "2024-01-16T10:23:45Z",
      scheduler: {
        ...index.status.scheduler,
        last_attempt_at: "2024-01-16T10:00:00Z",
      },
    };
    await page.clock.fastForward(5_001);
    await open.click();
    await expect(error).toBeVisible();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await open.click();
    await expect(error).toHaveCount(0);
  });

  test("does not acknowledge an unseen failure after closing during a new build", async ({
    page,
  }) => {
    const diagnostic = "inventory stream ended before completion";
    const initialStatus = searchIndexStatus("failed", true, diagnostic, 7);
    const index = await mockBrowserIndex(page, {
      ...initialStatus,
      completed_at: null,
      scheduler: { ...initialStatus.scheduler, last_attempt_at: null },
    });
    await page.route("**/api/opc/search-index/refresh**", async (route) => {
      index.status = { ...index.status, state: "refreshing", last_error: null };
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(index.status),
      });
    });
    const open = page.getByRole("button", { name: "Browse tags" });
    await open.click();
    await expect(page.getByText(`Index error: ${diagnostic}`)).toBeVisible();
    await page.getByRole("button", { name: "Refresh index" }).click();
    await expect(page.getByText("Index: refreshing")).toBeVisible();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    index.status = {
      ...index.status,
      state: "failed",
      last_error: diagnostic,
    };
    await open.click();
    await expect(
      page.getByRole("button", { name: "Cancel build" }),
    ).toBeVisible();
    await expect(page.getByText(`Index error: ${diagnostic}`)).toBeVisible();
    await expect(page.getByLabel("Search OPC tags")).toBeEnabled();
  });

  test("re-arms accepted refreshes even without an observable build or attempt timestamps", async ({
    page,
  }) => {
    const diagnostic = "the inventory database is unavailable";
    const initialStatus = searchIndexStatus("failed", true, diagnostic, 7);
    const index = await mockBrowserIndex(page, {
      ...initialStatus,
      completed_at: null,
      scheduler: { ...initialStatus.scheduler, last_attempt_at: null },
    });
    await page.route("**/api/opc/search-index/refresh**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(index.status),
      });
    });
    const open = page.getByRole("button", { name: "Browse tags" });
    const error = page.getByText(`Index error: ${diagnostic}`);
    await open.click();
    await expect(error).toBeVisible();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await open.click();
    await expect(error).toHaveCount(0);
    await page.getByRole("button", { name: "Refresh index" }).click();
    await expect(error).toBeVisible();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await open.click();
    await expect(error).toHaveCount(0);
  });

  test("re-arms externally observed builds when attempt timestamps are absent", async ({
    page,
  }) => {
    await page.clock.install();
    const diagnostic = "the inventory database is unavailable";
    const initialStatus = searchIndexStatus("failed", true, diagnostic, 7);
    const index = await mockBrowserIndex(page, {
      ...initialStatus,
      completed_at: null,
      scheduler: { ...initialStatus.scheduler, last_attempt_at: null },
    });
    const open = page.getByRole("button", { name: "Browse tags" });
    const error = page.getByText(`Index error: ${diagnostic}`);
    await open.click();
    await expect(error).toBeVisible();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    index.status = { ...index.status, state: "refreshing", last_error: null };
    await page.clock.fastForward(5_001);
    await open.click();
    await expect(page.getByText("Index: refreshing")).toBeVisible();
    index.status = { ...index.status, state: "failed", last_error: diagnostic };
    await expect(error).toBeVisible();
  });

  test("keeps acknowledgements local to each bridge and OPC server", async ({
    page,
  }) => {
    const diagnostic = "the inventory database is unavailable";
    await mockBrowserIndex(
      page,
      searchIndexStatus("failed", true, diagnostic, 7),
    );
    const open = page.getByRole("button", { name: "Browse tags" });
    const error = page.getByText(`Index error: ${diagnostic}`);
    const acknowledgeOnConnection = async (bridge: string, server: string) => {
      await page.getByLabel("Bridge host").fill(bridge);
      await page.getByLabel("OPC DA server ProgID").fill(server);
      await open.click();
      await expect(error).toBeVisible();
      await page.getByRole("button", { name: "Close", exact: true }).click();
    };
    await acknowledgeOnConnection("bridge-one:7600", "Test.Server");
    await acknowledgeOnConnection("bridge-one:7600", "Other.Server");
    await acknowledgeOnConnection("bridge-two:7600", "Test.Server");
    await page.getByLabel("Bridge host").fill("bridge-one:7600");
    await open.click();
    await expect(error).toHaveCount(0);
  });

  test("does not acknowledge an index error covered by a quality warning", async ({
    page,
  }) => {
    const diagnostic = "inventory stream ended before completion";
    const index = await mockBrowserIndex(
      page,
      searchIndexStatus("refreshing", true, null, 7),
    );
    await page.route("**/api/opc/browse**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          browsePage([browseNode("pv", "PV", "item", "Test.Loop.PV")]),
        ),
      });
    });
    await page.route("**/api/opc/read**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          tag: "Test.Loop.PV",
          value: "42",
          quality: "bad",
          timestamp: null,
        }),
      });
    });
    const open = page.getByRole("button", { name: "Browse tags" });
    await open.click();
    await page.getByRole("button", { name: "Select tag", exact: true }).click();
    await expect(
      page.getByRole("dialog", { name: "OPC quality warning" }),
    ).toBeVisible();
    const failedStatus = page.waitForResponse(
      "**/api/opc/search-index/status**",
    );
    index.status = searchIndexStatus("failed", true, diagnostic, 7);
    await failedStatus;
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
    );
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await open.click();
    await expect(page.getByText(`Index error: ${diagnostic}`)).toBeVisible();
  });

  for (const closePath of ["Escape", "backdrop", "Cancel", "selection"]) {
    test(`acknowledges a seen index error through ${closePath}`, async ({
      page,
    }) => {
      const diagnostic = "the inventory database is unavailable";
      await mockBrowserIndex(
        page,
        searchIndexStatus("failed", true, diagnostic, 7),
      );
      await page
        .getByRole("combobox", { name: "Template" })
        .selectOption("Yokogawa CentumVP");
      await page.route("**/api/opc/browse**", async (route) => {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(
            browsePage([browseNode("pv", "PV", "item", "Test.Loop.PV")]),
          ),
        });
      });
      await page.route("**/api/opc/read**", async (route) => {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            tag: "Test.Loop.PV",
            value: "42",
            quality: "good",
            timestamp: null,
          }),
        });
      });
      const open = page.getByRole("button", { name: "Browse tags" });
      const browser = page.getByRole("dialog", {
        name: "Browse tags on Test.Server",
      });
      const error = page.getByText(`Index error: ${diagnostic}`);
      await open.click();
      await expect(error).toBeVisible();
      await page
        .getByRole("button", { name: "Delete index", exact: true })
        .click();
      await page
        .getByRole("dialog", { name: "Delete tag index?" })
        .getByRole("button", { name: "Cancel", exact: true })
        .click();
      await expect(error).toBeVisible();
      if (closePath === "Escape") {
        await browser.getByRole("button", { name: "Close" }).press("Escape");
      } else if (closePath === "backdrop") {
        await page
          .getByRole("button", { name: "Dismiss modal backdrop" })
          .click({ position: { x: 2, y: 2 } });
      } else {
        await browser
          .getByRole("button", {
            name: closePath === "selection" ? "Select tag" : "Cancel",
            exact: true,
          })
          .click();
      }
      await expect(page.getByRole("dialog")).toHaveCount(0);
      if (closePath === "selection") {
        await expect(page.getByLabel("Tag name")).toHaveValue("Test.Loop.PV");
        await page.getByLabel("Tag name").fill("");
      }
      await open.click();
      await expect(error).toHaveCount(0);
      await expect(
        page.getByText("Index: failed", { exact: true }),
      ).toBeVisible();
    });
  }

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
