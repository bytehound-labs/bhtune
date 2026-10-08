import { readFile } from "node:fs/promises";
import { expect, test, type Page } from "@playwright/test";

/**
 * Locates the outcome badge specifically (`RunDetailPage`'s "Outcome" field value), scoped
 * to `<dd>` (value) elements and matched on exact full text. This disambiguates it from the
 * unrelated "Completed" field *label* (a `<dt>`, for the completion-timestamp field) which
 * renders the identical text now that outcomes use friendly, capitalized labels
 * (`ui-friendly-process-names`) instead of the raw lowercase wire value.
 */
function outcomeBadge(page: Page, outcome: "Completed" | "Aborted") {
  return page.locator("dd").filter({ hasText: new RegExp(`^${outcome}$`) });
}

/**
 * Clicks "Start tune" and waits for navigation to the new run's detail page.
 *
 * Independent tune tasks may run concurrently. The retry loop remains useful for transient
 * request failures without encoding any server-wide single-run assumption.
 */
async function startTune(page: Page) {
  const startButton = page.getByRole("button", { name: "Start tune" });
  const startError = page.getByText("Unable to start the tune.");
  // oxlint-disable no-await-in-loop -- Each retry depends on the preceding submit and UI response.
  for (let attempt = 0; attempt < 20; attempt++) {
    await startButton.click();
    const navigated = page
      .waitForURL(/\/runs\/\d+$/, { timeout: 500 })
      .then(() => true)
      .catch(() => false);
    if (await navigated) {
      return;
    }
    if (await startError.isVisible()) {
      await expect(startButton).toBeEnabled();
      continue;
    }
    // Neither navigated nor showed the generic start error -- a real failure. Let the
    // caller's own `toHaveURL` assertion below report it with a normal Playwright error.
    return;
  }
  // oxlint-enable no-await-in-loop
}

/**
 * Drives a full MRFT tune end-to-end through the real browser UI -- the scenario
 * `e2e-playwright` exists for. Fills in the New tune form, submits it against a real
 * `bhtune-server` running the in-process simulator driver, and asserts the *rendered*
 * results are sane and correctly ordered, not just that the page didn't crash.
 *
 * Mirrors `crates/bhtune-cli/tests/e2e_simulator.rs`'s own "flow / PI / reverse" matrix
 * case and its millisecond-scale simulator parameters (`sim_tau`/`sim_dead_time`). The
 * Playwright server starts with a temporary global `[tuning]` configuration using a 5 ms
 * poll interval and a 30 s whole-run timeout, since those values are installation-wide
 * settings rather than New Tune form fields. `direction=reverse` is likewise required --
 * confirmed (see that Rust test's own comment) to be the only direction that produces a
 * genuine relay oscillation against this fixed simulator configuration; it's already the
 * form's default whenever `driver=simulator`, so it isn't set explicitly below.
 */
test.describe("running a tune", () => {
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
    await page.route("**/api/runs/last-request", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: "null",
      }),
    );
  });

  for (const template of ["Yokogawa CentumVP", "Honeywell Experion"]) {
    test(`completes a full ${template} simulator tune with controller previews`, async ({
      page,
    }) => {
      test.setTimeout(45_000);

      await page.goto("/runs/new");

      await page
        .getByRole("combobox", { name: "Template", exact: true })
        .selectOption(template);
      await page.getByLabel("Cycles to skip").fill("1");
      await page.getByLabel("Cycles to count").fill("2");
      await page.getByLabel("Noise protection (s)").fill("0");
      await page.getByLabel("Time constant τ (s)").fill("0.01");
      await page.getByLabel("Dead time (s)").fill("0.025");

      await startTune(page);

      await expect(page).toHaveURL(/\/runs\/\d+$/);

      // The SSE-driven live banner (`frontend-live-stream`) invalidates the run query the
      // instant its `done` event arrives, so this resolves close to real completion time
      // rather than waiting for `useRun`'s 5s polling fallback -- see `api/runs.ts`'s
      // `useRunStream` doc comment.
      await expect(outcomeBadge(page, "Completed")).toBeVisible({
        timeout: 30_000,
      });

      const resultsSection = page.locator("details").filter({
        has: page.getByRole("heading", { name: "Calculated results" }),
      });
      const rows = resultsSection.locator("tbody tr");
      await expect(rows).toHaveCount(3);

      await expect(
        resultsSection.getByRole("columnheader", {
          name: "Response level",
          exact: true,
        }),
      ).toBeVisible();
      const headers =
        template === "Yokogawa CentumVP" ? ["P", "I", "D"] : ["K", "T1", "T2"];
      await Promise.all(
        headers.map((name) =>
          expect(
            resultsSection.getByRole("columnheader", { name, exact: true }),
          ).toBeVisible(),
        ),
      );
      await expect(
        resultsSection.getByRole("columnheader", { name: "Kp", exact: true }),
      ).toHaveCount(0);
      await expect(
        resultsSection.getByRole("columnheader", {
          name: "Ti (min)",
          exact: true,
        }),
      ).toHaveCount(0);
      await expect(
        resultsSection.getByRole("columnheader", {
          name: "Td (min)",
          exact: true,
        }),
      ).toHaveCount(0);

      async function proportional(
        level: "aggressive" | "moderate" | "sluggish",
      ) {
        const row = rows.filter({ hasText: level });
        await expect(row).toHaveCount(1);
        const proportionalText = await row.locator("td").nth(1).innerText();
        return Number.parseFloat(proportionalText);
      }
      async function integral(level: "aggressive" | "moderate" | "sluggish") {
        const row = rows.filter({ hasText: level });
        const integralText = await row.locator("td").nth(2).innerText();
        return Number.parseFloat(integralText);
      }
      async function derivative(level: "aggressive" | "moderate" | "sluggish") {
        const row = rows.filter({ hasText: level });
        const derivativeText = await row.locator("td").nth(3).innerText();
        return Number.parseFloat(derivativeText);
      }

      if (template === "Yokogawa CentumVP") {
        await expect(
          resultsSection.getByText("Unwritable", { exact: true }),
        ).toHaveCount(3);
        await expect(
          resultsSection.getByText(
            /rounding would erase an active term to zero/,
          ),
        ).toHaveCount(3);
      } else {
        const aggressiveProportional = await proportional("aggressive");
        const moderateProportional = await proportional("moderate");
        const sluggishProportional = await proportional("sluggish");

        expect(aggressiveProportional).toBeGreaterThan(0);
        expect(moderateProportional).toBeGreaterThan(0);
        expect(sluggishProportional).toBeGreaterThan(0);
        expect(aggressiveProportional).toBeGreaterThan(moderateProportional);
        expect(moderateProportional).toBeGreaterThan(sluggishProportional);

        // The regression `e2e_simulator.rs` was written to catch a sub-second relay-period
        // truncation bug silently zeroing the integral result for every controller type. This
        // run's controller type is "pi" (the form's own default), so the template-specific I
        // value must be genuinely nonzero and the always-visible D value must be exactly zero.
        expect(await integral("aggressive")).toBeGreaterThan(0);
        expect(await derivative("aggressive")).toBe(0);
      }

      await expect(
        page.getByRole("heading", { name: "Timing", exact: true }),
      ).toHaveCount(0);
      await expect(
        page.getByText("Timing warning:", { exact: false }),
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

      await expect(
        page.getByText(/\d+ measurements were recorded/),
      ).toHaveCount(0);

      const runId = page.url().match(/\/runs\/(\d+)$/)?.[1];
      expect(runId).toBeTruthy();
      const runResponse = await page.request.get(`/api/runs/${runId}`);
      expect(runResponse.ok()).toBe(true);
      const run = await runResponse.json();
      expect(run.pid_rounding).toEqual(
        template === "Yokogawa CentumVP"
          ? { kind: "decimal_places", digits: 1 }
          : { kind: "significant_digits", digits: 3 },
      );
      expect(run.results).toHaveLength(3);
      for (const result of run.results) {
        expect(result.status).toBe("valid");
        expect(result.proportional).toBeGreaterThan(0);
        expect(result.integral).toBeGreaterThan(0);
        if (template === "Yokogawa CentumVP") {
          expect(result.controller_values).toBeNull();
          expect(result.controller_target_error).toContain(
            "rounding would erase an active term to zero",
          );
        } else {
          expect(result.controller_values).not.toBeNull();
          expect(result.controller_target_error).toBeNull();
        }
      }
      expect(run.timing_metrics).toMatchObject({
        basis: "simulated_fixed_step",
        requested_interval_ms: 5,
        mean_sample_gap_ms: 5,
        max_sample_gap_ms: 5,
        missed_poll_opportunity_count: 0,
        sampling_adequacy: "adequate",
      });
      expect(
        run.timing_metrics.approximate_samples_per_period,
      ).toBeGreaterThanOrEqual(6);
    });
  }

  test("exports a completed run's samples as CSV and JSON downloads", async ({
    page,
  }) => {
    test.setTimeout(45_000);

    await page.goto("/runs/new");
    await page
      .getByRole("combobox", { name: "Template", exact: true })
      .selectOption("Yokogawa CentumVP");
    await page.getByLabel("Cycles to skip").fill("1");
    await page.getByLabel("Cycles to count").fill("2");
    await page.getByLabel("Noise protection (s)").fill("0");
    await page.getByLabel("Time constant τ (s)").fill("0.01");
    await page.getByLabel("Dead time (s)").fill("0.025");

    await startTune(page);
    await expect(page).toHaveURL(/\/runs\/\d+$/);
    await expect(outcomeBadge(page, "Completed")).toBeVisible({
      timeout: 30_000,
    });

    const [csvDownload] = await Promise.all([
      page.waitForEvent("download"),
      page.getByRole("link", { name: "Export CSV" }).click(),
    ]);
    expect(csvDownload.suggestedFilename()).toMatch(/^run-\d+\.csv$/);
    const csvPath = await csvDownload.path();
    const csvContents = await readFile(csvPath, "utf-8");
    expect(csvContents.split("\n")[0]).toBe(
      "tick,time,pv,pv_quality,hysteresis,mv_value_current,mv_sign_next_step,counter_all_switches,cycles_completed,cycles_remaining",
    );

    const [jsonDownload] = await Promise.all([
      page.waitForEvent("download"),
      page.getByRole("link", { name: "Export JSON" }).click(),
    ]);
    expect(jsonDownload.suggestedFilename()).toMatch(/^run-\d+\.json$/);
  });

  test("deletes a completed run from its detail page", async ({ page }) => {
    test.setTimeout(45_000);

    await page.goto("/runs/new");
    await page
      .getByRole("combobox", { name: "Template", exact: true })
      .selectOption("Yokogawa CentumVP");
    await page.getByLabel("Cycles to skip").fill("1");
    await page.getByLabel("Cycles to count").fill("2");
    await page.getByLabel("Noise protection (s)").fill("0");
    await page.getByLabel("Time constant τ (s)").fill("0.01");
    await page.getByLabel("Dead time (s)").fill("0.025");

    await startTune(page);
    await expect(page).toHaveURL(/\/runs\/\d+$/);
    await expect(outcomeBadge(page, "Completed")).toBeVisible({
      timeout: 30_000,
    });

    const runUrl = page.url();
    const runId = runUrl.match(/\/runs\/(\d+)$/)?.[1];
    expect(runId).toBeTruthy();

    await page.getByRole("button", { name: "Delete tune" }).click();
    const deleteDialog = page.getByRole("dialog", { name: "Delete tune?" });
    await expect(deleteDialog).toBeVisible();
    await expect(
      deleteDialog.getByText(
        "Its recorded measurements, calculated results, and PID write history will be removed.",
      ),
    ).toBeVisible();
    await deleteDialog
      .getByRole("button", { name: "Delete tune", exact: true })
      .click();

    await expect(page).toHaveURL(/\/runs$/);

    // Navigating straight back to the deleted run's own URL now 404s -- proves the row is
    // really gone, not just removed from the list view.
    await page.goto(`/runs/${runId}`);
    await expect(page.getByText("Unable to load tune details.")).toBeVisible();
  });

  test("cancels a running tune from the run detail page", async ({ page }) => {
    test.setTimeout(45_000);

    await page.goto("/runs/new");

    await page
      .getByRole("combobox", { name: "Template", exact: true })
      .selectOption("Yokogawa CentumVP");
    // Deliberately slower than the completion test above -- reliably leaves a multi-second
    // window to click "Cancel tune" before the tune would otherwise finish on its own.
    await page.getByLabel("Time constant τ (s)").fill("1");
    await page.getByLabel("Dead time (s)").fill("1");

    await startTune(page);
    await expect(page).toHaveURL(/\/runs\/\d+$/);

    const cancelButton = page.getByRole("button", { name: "Cancel tune" });
    await expect(cancelButton).toBeVisible();
    await cancelButton.click();

    await expect(outcomeBadge(page, "Aborted")).toBeVisible({
      timeout: 30_000,
    });
    await expect(
      page.getByRole("button", { name: "Cancel tune" }),
    ).not.toBeVisible();
  });
});
