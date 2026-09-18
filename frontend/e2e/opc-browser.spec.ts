import { expect, test, type Locator, type Page } from "@playwright/test";

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

async function expectTreeNodeVisible(
  node: Locator,
  treeViewport: Locator,
): Promise<void> {
  await expect
    .poll(() =>
      node.evaluate((element) => {
        const viewport = element.closest<HTMLElement>(
          '[data-testid="opc-tag-tree-viewport"]',
        );
        if (!viewport) return false;
        const nodeRect = element.getBoundingClientRect();
        const viewportRect = viewport.getBoundingClientRect();
        const top = viewportRect.top + viewport.clientTop;
        const bottom = top + viewport.clientHeight;
        return nodeRect.top >= top && nodeRect.bottom <= bottom;
      }),
    )
    .toBe(true);
  await expect(treeViewport).toBeVisible();
}

function browseNode(
  nodeKey: string,
  displayName: string,
  kind: "branch" | "item" | "branch_and_item",
  itemId?: string,
) {
  return {
    node_key: nodeKey,
    display_name: displayName,
    kind,
    item_id: itemId ?? null,
  };
}

function browsePage(
  nodes: ReturnType<typeof browseNode>[],
  options: { nextPageToken?: string | null; complete?: boolean } = {},
) {
  return {
    session_id: "session-1",
    nodes,
    next_page_token: options.nextPageToken ?? null,
    complete: options.complete ?? true,
    organization: "hierarchical",
    source: "da2",
    warning: null,
  };
}

function searchIndexStatus(
  state:
    | "not_indexed"
    | "partial"
    | "ready"
    | "stale"
    | "refreshing"
    | "deleting"
    | "failed" = "ready",
  autoRefreshEnabled = true,
  lastError: string | null = null,
  activeGeneration?: number,
) {
  return {
    server: "Test.Server",
    state,
    auto_refresh_enabled: autoRefreshEnabled,
    active_generation:
      activeGeneration ??
      (state === "not_indexed" || state === "deleting" || state === "failed"
        ? 0
        : 1),
    entry_count: state === "deleting" ? 0 : 2,
    unique_item_count: state === "deleting" ? 0 : 2,
    started_at: null,
    completed_at: "2024-01-15T10:23:45Z",
    last_error: lastError,
    database_bytes: 1024,
    organization: "hierarchical",
    source: "da2",
    progress: null,
    scheduler: {
      next_refresh_at: autoRefreshEnabled
        ? String(Date.now() + 7 * 24 * 60 * 60 * 1000 + 2 * 60 * 60 * 1000)
        : null,
      last_attempt_at: "2024-01-15T10:23:45Z",
      last_success_at: "2024-01-15T10:23:45Z",
      last_success_duration_ms: 1234,
      retry_after: null,
      consecutive_failures: 0,
      circuit_open: false,
    },
  };
}

function indexedSearchResponse(
  matches: {
    item_id: string;
    display_name: string;
    kind: "item" | "branch" | "branch_and_item";
    breadcrumbs: string[];
  }[],
  options: {
    hasMore?: boolean;
    state?: Parameters<typeof searchIndexStatus>[0];
  } = {},
) {
  return {
    matches,
    has_more: options.hasMore ?? false,
    status: searchIndexStatus(options.state),
  };
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
    await page.route("**/api/opc/search-index/status**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(searchIndexStatus()),
      });
    });

    await page.goto("/runs/new");
    await page.getByLabel("Driver").selectOption("opcda");
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

  test("resets custom mappings when the base tag changes but keeps fixed values", async ({
    page,
  }) => {
    const templateField = page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox");
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
      page.getByRole("dialog", { name: "Browse OPC DA servers" }),
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

  test("shows shared loading feedback while server discovery is pending", async ({
    page,
  }) => {
    let releaseServers: () => void = () => undefined;
    const serversGate = new Promise<void>((resolve) => {
      releaseServers = resolve;
    });
    await page.route("**/api/opc/servers**", async (route) => {
      await serversGate;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ servers: ["Yokogawa.CSHIS_OPC.1"] }),
      });
    });

    const browseButton = page.getByRole("button", { name: "Browse servers" });
    await browseButton.click();

    const dialog = page.getByRole("dialog", {
      name: "Browse OPC DA servers",
    });
    await expect(dialog.getByRole("status")).toContainText("Connecting…");
    await expect(browseButton).toHaveAttribute("aria-busy", "true");
    await expect(browseButton).toBeDisabled();

    releaseServers();
    await expect(
      dialog.getByRole("button", { name: "Yokogawa.CSHIS_OPC.1" }),
    ).toBeVisible();
    await expect(browseButton).not.toHaveAttribute("aria-busy");
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
      page.getByRole("dialog", {
        name: "Browse tags on Matrikon.OPC.Simulation",
      }),
    ).toBeVisible();
    await expect(
      page.getByText("Unable to load tags at this level."),
    ).toBeVisible();
    await expect(
      page
        .getByRole("dialog", {
          name: "Browse tags on Matrikon.OPC.Simulation",
        })
        .getByRole("status"),
    ).not.toBeVisible();

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
      page.getByRole("button", { name: "Simulink.Device1._System" }),
    ).toBeVisible();
    await expect(
      page.getByText("Select a tag to test its live value and quality."),
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
    await expect(
      mappingRow(page, "Manipulated variable (MV)").getByRole("button", {
        name: "Template tag",
        exact: true,
      }),
    ).toHaveAttribute("aria-pressed", "true");
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

  test("loads additional pages, expands branch-and-item nodes, and closes the session", async ({
    page,
  }) => {
    const selectedItemId = "Unit1.LIC101.PV";
    const readTags: string[] = [];
    const closePaths: string[] = [];
    await page
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox")
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
    await expect(page.getByRole("button", { name: "First" })).toBeVisible();
    await page.getByRole("button", { name: "Load more" }).click();
    await page.getByRole("button", { name: "Unit1.LIC101" }).dblclick();
    await expect(page.getByText(`Selected: ${selectedItemId}`)).toBeVisible();
    await expect(page.getByRole("button", { name: "Collapse" })).toBeVisible();
    await expect(page.getByRole("button", { name: "SV" })).toBeVisible();
    await page.getByRole("button", { name: "Select tag" }).click();

    await expect(page.getByLabel("Tag name")).toHaveValue(selectedItemId);
    expect(readTags).toEqual([selectedItemId]);
    await expect
      .poll(() => closePaths)
      .toEqual(["/api/opc/browse/sessions/session-1"]);
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

  test("provides debounced indexed search with keyboard selection and exact ItemIDs", async ({
    page,
  }) => {
    const queries: Array<{ query: string; mode: string }> = [];
    const selectedItemId = "FCS0202!204FI00510.PV";
    const readTags: string[] = [];

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
      .locator("label")
      .filter({ hasText: /^Template/ })
      .getByRole("combobox")
      .selectOption("Yokogawa CentumVP");
    await page.getByLabel("Tag name").fill("");
    await page.getByLabel("OPC DA server ProgID").fill("Yokogawa.CSHIS_OPC.1");
    await page.getByRole("button", { name: "Browse tags" }).click();

    await expect(page.getByLabel("Search OPC tags")).toBeDisabled();
    await expect(page.getByRole("button", { name: "FCS0201" })).toBeVisible();
    await page.getByRole("button", { name: "Expand" }).click();
    await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
    await page.getByRole("button", { name: "Retry" }).click();
    await expect(
      page.getByRole("button", { name: selectedItemId }),
    ).toBeVisible();

    await page.getByRole("button", { name: selectedItemId }).click();
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
