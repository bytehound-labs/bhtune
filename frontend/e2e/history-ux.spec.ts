import { expect, test, type Page } from "@playwright/test";
import {
  completedDemoRun,
  completedFullRun,
  installDemoRoutes,
  installFullRoutes,
  runningDemoRun,
  runningFullRun,
} from "./docs-screenshots/fixtures";
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

for (const mode of ["full", "demo"] as const) {
  for (const outcome of ["running", "completed"] as const) {
    test(`${mode} ${outcome} trend keeps concise labels without an inspection panel`, async ({
      page,
      baseURL,
    }) => {
      if (mode === "demo") {
        if (!baseURL) {
          throw new Error("The Demo fixture must use the configured origin.");
        }
        await installDemoRoutes(page, new URL(baseURL).origin);
      }
      const tagName =
        outcome === "running"
          ? "FCS0001!EXAMPLE_FIC101.PV"
          : "Area01.FIC101.PV";
      if (outcome === "running") {
        const run = mode === "demo" ? runningDemoRun : runningFullRun;
        await page.route("**/api/runs/4242", (route) =>
          route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({ ...run, tag_name: tagName }),
          }),
        );
      }
      await page.goto("/runs/4242");

      if (outcome === "running") {
        await expect(
          page.getByText("Tune in progress — collecting live measurements."),
        ).toBeVisible();
        await expect(
          page.getByText(
            "Tick 1: PV 51, MV 60, cycles 0 completed / 2 remaining",
            { exact: true },
          ),
        ).toBeVisible();
      }

      const chart = page.getByRole("figure");
      await expect(chart).toBeVisible();
      await expect(
        page.getByText(
          /^\d+ measurements (recorded so far|were recorded) for this tune\.$/,
        ),
      ).toHaveCount(0);
      await expect(
        page.getByRole("heading", {
          name: "Sampling diagnostics",
          exact: true,
        }),
      ).toHaveCount(0);
      await expect(chart.locator('input[type="range"]')).toHaveCount(0);
      await expect(page.getByText("Inspect trend points")).toHaveCount(0);
      await expect(chart.getByText(/^Point \d+ of \d+/)).toHaveCount(0);
      await expect(chart.getByText(/Recorded run tag:/)).toHaveCount(0);
      await expect(chart.getByText(/raw tag units/)).toHaveCount(0);

      const legend = page.getByRole("group", {
        name: "Trend series legend",
      });
      await expect(legend.getByText("PV", { exact: true })).toBeVisible();
      await expect(
        legend.getByText("Commanded MV", { exact: true }),
      ).toBeVisible();
      if (mode === "full") {
        await expect(page.getByText(tagName, { exact: true })).toBeVisible();
      }

      const description = chart.locator("figcaption");
      const descriptionId = await chart.getAttribute("aria-describedby");
      if (!descriptionId) {
        throw new Error("The trend figure must reference its description.");
      }
      await expect(description).toHaveAttribute("id", descriptionId);
      await expect(description).toContainText("plotted points from");
      await expect(description).toContainText("PV ranged from");
      await expect(description).toContainText("commanded MV ranged from");
      expect(
        await description.evaluate((element) => {
          const rect = element.getBoundingClientRect();
          const styles = getComputedStyle(element);
          return {
            width: rect.width,
            height: rect.height,
            position: styles.position,
            overflow: styles.overflow,
            clipPath: styles.clipPath,
            tabIndex: element.tabIndex,
          };
        }),
      ).toEqual({
        width: 1,
        height: 1,
        position: "absolute",
        overflow: "hidden",
        clipPath: "inset(50%)",
        tabIndex: -1,
      });
      await expectAccessibilityInBothThemes(page);
      await chart.locator(".u-over").hover({ position: { x: 20, y: 20 } });
      await expect(chart.locator(".u-cursor-x")).toBeVisible();
    });
  }
}

for (const mode of ["full", "demo"] as const) {
  for (const sampling of [
    "adequate",
    "marginal",
    "not_assessed",
    "legacy",
  ] as const) {
    test(`${mode} ${sampling} sampling diagnostics are absent from run detail`, async ({
      page,
      baseURL,
    }) => {
      if (mode === "demo") {
        if (!baseURL) {
          throw new Error("The Demo fixture must use the configured origin.");
        }
        await installDemoRoutes(page, new URL(baseURL).origin);
      }
      const run = mode === "demo" ? completedDemoRun : completedFullRun;
      const samplesPerPeriod =
        sampling === "not_assessed"
          ? null
          : sampling === "marginal"
            ? 5.28
            : 12;
      const timing =
        sampling === "legacy"
          ? null
          : {
              ...run.timing_metrics,
              sampling_adequacy: sampling,
              approximate_samples_per_period: samplesPerPeriod,
              measured_oscillation_period_ms:
                samplesPerPeriod === null
                  ? null
                  : samplesPerPeriod * run.timing_metrics.mean_sample_gap_ms,
            };
      await page.route("**/api/runs/4242", (route) =>
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ ...run, timing_metrics: timing }),
        }),
      );
      await page.goto("/runs/4242");
      await expect(
        page.getByRole("heading", { name: "Calculated results", exact: true }),
      ).toBeVisible();
      await expect(
        page.getByText(
          `${run.samples.length} measurements were recorded for this tune.`,
          { exact: true },
        ),
      ).toHaveCount(0);
      await expect(
        page.getByRole("heading", {
          name: "Sampling diagnostics",
          exact: true,
        }),
      ).toHaveCount(0);
      await expect(
        page.locator('[aria-label="Sampling adequacy advisory"]'),
      ).toHaveCount(0);
      await expect(page.getByText("Marginal", { exact: true })).toHaveCount(0);
      await expect(
        page.getByText(/calculated values may be less reliable/),
      ).toHaveCount(0);
    });
  }
}

test("failure and incomplete restore evidence remains without sampling diagnostics", async ({
  page,
}) => {
  await page.route("**/api/runs/4242", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        ...completedFullRun,
        outcome: "failed",
        results: [],
        failure_reason: "The PV quality was Bad.",
        restore_status: "incomplete",
        restore_detail: "The original MV could not be confirmed.",
        timing_metrics: {
          ...completedFullRun.timing_metrics,
          sampling_adequacy: "marginal",
          measured_oscillation_period_ms: 4224,
          approximate_samples_per_period: 5.28,
        },
      }),
    }),
  );
  await page.goto("/runs/4242");
  await expect(
    page.getByText("The PV quality was Bad.", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("incomplete", { exact: true })).toBeVisible();
  await expect(
    page.getByText("The original MV could not be confirmed.", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText(
      /^\d+ measurements (recorded so far|were recorded) for this tune\.$/,
    ),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", {
      name: "Sampling diagnostics",
      exact: true,
    }),
  ).toHaveCount(0);
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
