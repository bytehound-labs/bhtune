import { expect, type Locator, type Page } from "@playwright/test";

/**
 * Shared route fixtures and helpers for the `opc-browser*.spec.ts` regression coverage of
 * the OPC DA server-discovery and tag-browser affordances (`ui-opc-browser`) that doesn't
 * require a live OPC DA gateway. This suite's shared `bhtune-server` instance
 * (`playwright.config.ts`'s `webServer`) never starts an `opcda-bridge-gateway`, so every
 * action without a Playwright route fixture fails at the connection step against the
 * default `localhost:7600` bridge host -- confirmed by hand to fail near-instantly
 * (`ECONNREFUSED`, no gateway listening), well inside
 * `crates/bhtune-server/src/routes/opc.rs`'s own 30s `OPC_QUERY_TIMEOUT_SECS` budget, so
 * this suite is fast and deterministic without needing one.
 *
 * A connection failure is still genuinely useful regression coverage: it exercises the
 * driver-switch visibility of the OPC-only fields, the real `GET /api/opc/servers` and
 * `GET /api/opc/browse` request wiring behind "Browse servers"/"Browse tags", the modal
 * opening/closing, and that a failure renders as a visible error rather than a silent no-op
 * or an unhandled exception. The populated-tree cases use Playwright route fixtures for the
 * HTTP responses, keeping selection and template-specific PV-tag transformation covered
 * without requiring a second permanent gateway service. The main form's collapsed mapping
 * section covers the default/effective tag preview and per-tune overrides.
 */

/** Shared `test.describe` title, so every split spec keeps the suite's full test titles. */
export const OPC_BROWSER_SUITE =
  "OPC DA server discovery and tag browser (no gateway present)";

/**
 * Registers the draft and default search-index-status route fixtures every OPC browser test
 * relies on, then opens the New Run form with the OPC DA driver selected. Tests that need a
 * different index status unroute the default status fixture before registering their own.
 */
export async function openOpcDaRunForm(page: Page): Promise<void> {
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
}

export function loopMapping(page: Page) {
  return page
    .locator("details")
    .filter({ has: page.locator("summary", { hasText: "Loop mapping" }) });
}

export function mappingRow(page: Page, label: string) {
  return loopMapping(page).getByRole("group", { name: label, exact: true });
}

export async function expectTreeNodeVisible(
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

export function browseNode(
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

export function browsePage(
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

export function searchIndexStatus(
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

export function indexedSearchResponse(
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
