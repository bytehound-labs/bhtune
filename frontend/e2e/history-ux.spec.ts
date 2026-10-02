import { expect, test, type Page } from "@playwright/test";
import { installFullRoutes } from "./docs-screenshots/fixtures";
import { expectAccessibilityInBothThemes } from "./support/accessibility";

const historyRows = Array.from({ length: 68 }, (_, index) => ({
  id: index === 50 ? 4242 : 6000 + index,
  tag_name: `Area01.FIC${index}.PV`,
  process_type: index < 55 ? "flow" : "level",
  driver: index < 55 ? "simulator" : "opcda",
  outcome: index < 55 ? "completed" : "failed",
  started_at: new Date(Date.UTC(2025, 0, 1, 0, 0, index)).toISOString(),
  notes: null,
}));

async function installHistoryList(page: Page) {
  const requests: URL[] = [];
  await page.route("**/api/runs?*", async (route) => {
    if (route.request().method() !== "GET") {
      await route.fallback();
      return;
    }

    const url = new URL(route.request().url());
    requests.push(url);
    const processType = url.searchParams.get("process_type");
    const outcome = url.searchParams.get("outcome");
    const driver = url.searchParams.get("driver");
    const filteredRows = historyRows.filter(
      (run) =>
        (!processType || run.process_type === processType) &&
        (!outcome || run.outcome === outcome) &&
        (!driver || run.driver === driver),
    );
    const limit = Number(url.searchParams.get("limit") ?? 50);
    const offset = Number(url.searchParams.get("offset") ?? 0);
    const runs = filteredRows.slice(offset, offset + limit);

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        runs,
        returned: runs.length,
        total: filteredRows.length,
      }),
    });
  });
  return requests;
}

async function expectPageFitsWidth(
  page: Page,
  width: number,
  path: string,
  heading: string,
) {
  await page.setViewportSize({ width, height: 900 });
  await page.goto(path);
  await expect(page.getByRole("heading", { name: heading })).toBeVisible();
  const dimensions = await page.evaluate(() => ({
    viewport: window.innerWidth,
    document: document.documentElement.scrollWidth,
  }));
  expect(dimensions.document).toBeLessThanOrEqual(dimensions.viewport);
}

test.beforeEach(async ({ page }) => {
  await installFullRoutes(page);
});

test("history URL state survives reload, browser navigation, and run detail", async ({
  page,
}) => {
  const requests = await installHistoryList(page);
  const query =
    "process_type=flow&outcome=completed&driver=simulator&offset=50";
  await page.goto(`/runs?${query}`);

  await expect(page.getByLabel("Filter by process type")).toHaveValue("flow");
  await expect(page.getByLabel("Filter by outcome")).toHaveValue("completed");
  await expect(page.getByLabel("Filter by driver")).toHaveValue("simulator");
  await expect(page.getByText("Showing 51–55 of 55")).toBeVisible();

  await page.reload();
  await expect(page).toHaveURL(new RegExp(`/runs\\?${query}$`));
  await expect(page.getByText("Showing 51–55 of 55")).toBeVisible();
  await expect.poll(() => requests.length).toBeGreaterThan(1);

  await page.getByRole("button", { name: "Previous" }).click();
  await expect(page).toHaveURL(
    /\/runs\?process_type=flow&outcome=completed&driver=simulator$/,
  );
  await expect(page.getByText("Showing 1–50 of 55")).toBeVisible();

  await page.goBack();
  await expect(page).toHaveURL(new RegExp(`/runs\\?${query}$`));
  await page.goForward();
  await expect(page).toHaveURL(
    /\/runs\?process_type=flow&outcome=completed&driver=simulator$/,
  );
  await page.goBack();

  await page.getByRole("link", { name: "#4242" }).click();
  await expect(page).toHaveURL(/\/runs\/4242$/);
  await expect(page.getByRole("button", { name: "Copy run ID" })).toBeVisible();
  await page.evaluate(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (value: string) => {
          window.localStorage.setItem("copied-run-id", value);
        },
      },
    });
  });
  await page.getByRole("button", { name: "Copy run ID" }).click();
  await expect(
    page.getByText("run ID copied to the clipboard.", { exact: true }),
  ).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem("copied-run-id"))).toBe(
    "4242",
  );

  await page.getByRole("link", { name: "Back to tune history" }).click();
  await expect(page).toHaveURL(new RegExp(`/runs\\?${query}$`));
  await expect(page.getByText("Showing 51–55 of 55")).toBeVisible();
});

test("invalid history query values are reset with an explanation", async ({
  page,
}) => {
  await page.goto("/runs?process_type=unknown&offset=12&driver=opcda");

  await expect(page).toHaveURL(/\/runs\?driver=opcda$/);
  await expect(
    page.getByText("Invalid history filter or page URL values were reset.", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(page.getByLabel("Filter by process type")).toHaveValue("");
  await expect(page.getByLabel("Filter by driver")).toHaveValue("opcda");
});

test("trend legend, raw values, and cursor are keyboard accessible in both themes", async ({
  page,
}) => {
  await page.goto("/runs/4242");

  const legend = page.getByRole("group", { name: "Trend series legend" });
  await expect(legend).toContainText("PV (raw tag units)");
  await expect(legend).toContainText("Commanded MV (raw tag units)");
  await expect(
    page.getByText(/Recorded run tag: Area01\.FIC101\.PV/),
  ).toBeVisible();
  await expect(
    page.getByText(
      "Recorded run tag: Area01.FIC101.PV. PV and commanded MV are shown as raw values; engineering units are not recorded.",
      { exact: true },
    ),
  ).toBeVisible();

  const slider = page.getByRole("slider", { name: "Inspect trend points" });
  const cursor = page.locator(".u-cursor-x");
  await expect(slider).toHaveAttribute("aria-valuetext", /Point 8 of 8/);
  const lastPointCursor = await cursor.evaluate(
    (element) => getComputedStyle(element).transform,
  );
  await slider.press("Home");
  await expect(cursor).toBeVisible();
  await expect(slider).toHaveAttribute("aria-valuetext", /Point 1 of 8/);
  await expect
    .poll(() =>
      cursor.evaluate((element) => getComputedStyle(element).transform),
    )
    .not.toBe(lastPointCursor);
  await expect(page.getByText(/PV 50 raw tag units/)).toBeVisible();
  await expectAccessibilityInBothThemes(page);
});

test("history and tune configuration do not overflow at 1024px or mobile width", async ({
  page,
}) => {
  await installHistoryList(page);

  await expectPageFitsWidth(page, 1024, "/runs", "History");
  await expectPageFitsWidth(page, 1024, "/runs/new", "New tune");
  await expectPageFitsWidth(page, 768, "/runs", "History");
  await expectPageFitsWidth(page, 768, "/runs/new", "New tune");
  await expectPageFitsWidth(page, 390, "/runs", "History");
  await expectPageFitsWidth(page, 390, "/runs/new", "New tune");
});
