import { expect, test, type Locator, type Page } from "@playwright/test";
import type { RunDetailResponse } from "../src/api/runs";
import {
  expectAccessibilityInBothThemes,
  expectNoAccessibilityViolations,
  setTheme,
} from "./support/accessibility";

const RUN_ID = 4242;

function completedRun(withResults = true): RunDetailResponse {
  return {
    id: RUN_ID,
    tag_name: "Area.FIC101",
    outcome: "completed",
    driver: "opcda",
    template_name: "Yokogawa CentumVP",
    template_origin: "builtin",
    pid_rounding: { kind: "decimal_places", digits: 1 },
    started_at: "2026-01-01T12:00:00Z",
    completed_at: "2026-01-01T12:05:00Z",
    allow_uncertain_quality: true,
    bridge_host: "gateway.example:7600",
    opc_server: "Yokogawa.CSHIS_OPC.1",
    config: {
      process_type: "flow",
      controller_type: "pi",
      relay_amp_percent: 10,
      num_cycles_skip: 1,
      num_cycles_count: 2,
      noise_protection_secs: 0,
      mrft_delay_secs: 0,
    },
    initial_readings: {
      pv_ini: 50,
      mv_ini: 50,
      pv_range_low: 0,
      pv_range_high: 100,
      mv_range_low: 0,
      mv_range_high: 100,
      controller_direction: "reverse",
      setpoint_ini: 50,
      mode_raw: "MAN",
      mode_attribute_raw: "0",
    },
    pid_constant_tags: {
      proportional: "Area.FIC101.PB",
      integral: "Area.FIC101.RI",
      derivative: "Area.FIC101.D",
    },
    pid_parameter_labels: {
      proportional: "P",
      integral: "I",
      derivative: "D",
    },
    samples: [
      {
        tick_index: 0,
        pv_quality: "good",
        sample: {
          time: "2026-01-01T12:00:01Z",
          pv: 50,
        },
        state: {
          hysteresis: 0,
          mv_value_current: 50,
          mv_sign_next_step: 1,
          counter_all_switches: 0,
          cycles_completed: 0,
          cycles_remaining: 2,
        },
      },
    ],
    mv_actuations: [],
    results: withResults
      ? [
          {
            response_level: "aggressive",
            kp: 0.8,
            ti_minutes: 1.2,
            td_minutes: 0,
            proportional: 20.5,
            integral: 1.2,
            derivative: 0,
            status: "valid",
            invalid_reason: null,
            controller_values: {
              response_level: "aggressive",
              proportional: { value: 20.5, display: "20.5" },
              integral: { value: 1.2, display: "1.2" },
              derivative: { value: 0, display: "0.0" },
            },
            controller_target_error: null,
          },
          {
            response_level: "moderate",
            kp: 0.6,
            ti_minutes: 1.5,
            td_minutes: 0,
            proportional: 25,
            integral: 1.5,
            derivative: 0,
            status: "valid",
            invalid_reason: null,
            controller_values: {
              response_level: "moderate",
              proportional: { value: 25, display: "25.0" },
              integral: { value: 1.5, display: "1.5" },
              derivative: { value: 0, display: "0.0" },
            },
            controller_target_error: null,
          },
          {
            response_level: "sluggish",
            kp: 0.4,
            ti_minutes: 1.8,
            td_minutes: 0,
            proportional: 30,
            integral: 1.8,
            derivative: 0,
            status: "valid",
            invalid_reason: null,
            controller_values: {
              response_level: "sluggish",
              proportional: { value: 30, display: "30.0" },
              integral: { value: 1.8, display: "1.8" },
              derivative: { value: 0, display: "0.0" },
            },
            controller_target_error: null,
          },
        ]
      : [],
    writes: [
      {
        kind: "write",
        response_level: "moderate",
        written_at: "2026-01-01T12:04:00Z",
        success: true,
        allow_uncertain_quality: true,
        proportional_previous: 5,
        integral_previous: 6,
        derivative_previous: 7,
        proportional_written: 18,
        integral_written: 1.1,
        derivative_written: 0,
        proportional_readback: 18,
        integral_readback: 1.1,
        derivative_readback: 0,
        rollback_state: null,
        rollback_error: null,
        error_message: null,
      },
      {
        kind: "write",
        response_level: "sluggish",
        written_at: "2026-01-01T12:04:30Z",
        success: true,
        allow_uncertain_quality: true,
        proportional_previous: 11,
        integral_previous: 22,
        derivative_previous: 33,
        proportional_written: 30,
        integral_written: 1.8,
        derivative_written: 0,
        proportional_readback: 30,
        integral_readback: 1.8,
        derivative_readback: 0,
        rollback_state: null,
        rollback_error: null,
        error_message: null,
      },
    ],
  };
}

async function openRun(page: Page, run = completedRun()) {
  await page.route(`**/api/runs/${RUN_ID}`, async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(run),
      });
      return;
    }
    await route.continue();
  });

  await page.goto(`/runs/${RUN_ID}`);
  await expect(
    page.getByRole("heading", { name: "Calculated results", exact: true }),
  ).toBeVisible();
}

function completedRunWithInvalidAggressiveResult() {
  const run = completedRun();
  const aggressiveResult = run.results[0];
  if (!aggressiveResult) {
    throw new Error("The completed run is missing its aggressive result.");
  }
  run.results[0] = {
    ...aggressiveResult,
    kp: null,
    ti_minutes: null,
    td_minutes: null,
    proportional: null,
    integral: null,
    derivative: null,
    status: "invalid",
    invalid_reason: "non_positive_pv_amplitude",
    controller_values: null,
    controller_target_error: "The measured PV amplitude was zero or negative.",
  };
  return run;
}

function resultsSection(page: Page) {
  return detailSection(page, "Calculated results");
}

function detailSection(page: Page, title: string) {
  return page.locator("details").filter({
    has: page.locator("summary", { hasText: title }),
  });
}

async function expectCenteredInViewport(dialog: Locator) {
  const placement = await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return {
      bottom: rect.bottom,
      height: rect.height,
      innerHeight: window.innerHeight,
      innerWidth: window.innerWidth,
      left: rect.left,
      right: rect.right,
      top: rect.top,
      width: rect.width,
    };
  });

  expect(placement.top).toBeGreaterThanOrEqual(0);
  expect(placement.bottom).toBeLessThanOrEqual(placement.innerHeight);
  expect(placement.left).toBeGreaterThanOrEqual(0);
  expect(placement.right).toBeLessThanOrEqual(placement.innerWidth);
  expect(
    Math.abs(placement.left + placement.width / 2 - placement.innerWidth / 2),
  ).toBeLessThan(2);
  expect(
    Math.abs(placement.top + placement.height / 2 - placement.innerHeight / 2),
  ).toBeLessThan(2);
}

test.describe("post-tune PID actions", () => {
  test("promotes calculated results and supports safe review cancellation", async ({
    page,
  }) => {
    let writeRequestCount = 0;
    page.on("request", (request) => {
      if (request.url().endsWith(`/api/runs/${RUN_ID}/write`)) {
        writeRequestCount += 1;
      }
    });

    await openRun(page);
    await expect(
      page.locator('[role="status"][aria-live="polite"][aria-atomic="true"]'),
    ).toHaveText("Tune completed.");
    const chart = page.getByRole("figure");
    const chartCaption = chart.locator("figcaption");
    await expect(chart).toHaveCount(1);
    await expect(chart).toHaveAttribute("aria-describedby");
    await expect(chartCaption).toContainText("plotted points from");
    await expect(chartCaption).toContainText("PV ranged from");
    await expect(chartCaption).toContainText("MV ranged from");
    await expectAccessibilityInBothThemes(page);

    const headings = await page.locator("h2").allTextContents();
    expect(headings.indexOf("Calculated results")).toBe(0);
    expect(headings.indexOf("Trend")).toBeGreaterThan(0);
    await expect(resultsSection(page)).toHaveClass(/emerald/);
    await expect(
      page.getByText("Ready to review", { exact: true }),
    ).toBeVisible();

    const sectionTitles = await page
      .locator("details > summary > h2")
      .allTextContents();
    expect(sectionTitles).toEqual([
      "Calculated results",
      "Trend",
      "Summary",
      "Notes",
      "Test configuration",
      "Initial readings",
      "PID change history",
    ]);
    await Promise.all(
      sectionTitles.map((title) =>
        expect(detailSection(page, title)).toHaveAttribute("open", ""),
      ),
    );
    await expect(
      page.getByRole("heading", {
        name: "MV actuation verification",
        exact: true,
      }),
    ).toHaveCount(0);

    await detailSection(page, "Summary").locator("summary").click();
    await expect(detailSection(page, "Summary")).not.toHaveAttribute(
      "open",
      "",
    );
    await detailSection(page, "Summary").locator("summary").click();
    await expect(detailSection(page, "Summary")).toHaveAttribute("open", "");

    const reviewTrigger = resultsSection(page)
      .getByRole("button", { name: "Review & write" })
      .first();
    await reviewTrigger.click();

    const modal = page.getByRole("dialog");
    const close = modal.getByRole("button", { name: "Close" });
    const cancel = modal.getByRole("button", { name: "Cancel" });
    const confirm = modal.getByRole("button", { name: "Write PID settings" });
    await expect(
      modal.getByRole("heading", { name: "Review PID settings" }),
    ).toBeVisible();
    await expectCenteredInViewport(modal);
    await expect(modal).toContainText("Area.FIC101");
    await expect(modal).toContainText("Aggressive");
    await expect(modal).toContainText("Area.FIC101.PB");
    await expect(modal).toContainText("Area.FIC101.RI");
    await expect(modal).toContainText("Area.FIC101.D");
    await expect(modal).toContainText("20.5");
    await expect(modal).toContainText("1.2");
    await expect(modal).toContainText("This action changes a live controller.");
    await expect(cancel).toBeFocused();
    await expectNoAccessibilityViolations(page);
    await modal.evaluate((element) => {
      (element as HTMLDialogElement).focus();
    });
    await page.keyboard.press("Tab");
    await expect(close).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(confirm).toBeFocused();

    await cancel.click();
    await expect(page.getByRole("dialog")).not.toBeVisible();
    await expect(reviewTrigger).toBeFocused();
    expect(writeRequestCount).toBe(0);

    await setTheme(page, "dark");
    await reviewTrigger.click();
    await expect(modal).toBeVisible();
    await expectNoAccessibilityViolations(page);
    await cancel.click();
    await expect(reviewTrigger).toBeFocused();
  });

  test("keeps no-result panels in the lower layout position", async ({
    page,
  }) => {
    await openRun(page, completedRun(false));

    const headings = await page.locator("h2").allTextContents();
    expect(headings.indexOf("Trend")).toBeLessThan(
      headings.indexOf("Calculated results"),
    );
    await expect(resultsSection(page)).not.toHaveClass(/emerald/);
    await expect(
      page.getByText("No results were calculated for this tune.", {
        exact: true,
      }),
    ).toBeVisible();
  });

  test("shows invalid calculated results and disables only their write action", async ({
    page,
  }) => {
    await openRun(page, completedRunWithInvalidAggressiveResult());

    const rows = resultsSection(page).locator("tbody tr");
    const invalidRow = rows.filter({ hasText: "Aggressive" });
    await expect(invalidRow).toContainText("Invalid");
    await expect(invalidRow).toContainText(
      "The measured PV amplitude was zero or negative.",
    );
    await expect(invalidRow.locator("td").nth(1)).toHaveText("—");
    await expect(
      invalidRow.getByRole("button", { name: "Review & write" }),
    ).toBeDisabled();

    const validRow = rows.filter({ hasText: "Moderate" });
    await expect(
      validRow.getByRole("button", { name: "Review & write" }),
    ).toBeEnabled();
  });

  for (const kind of ["decimal_places", "significant_digits"] as const) {
    test(`uses canonical ${kind} values in both results and write review`, async ({
      page,
    }) => {
      const run = completedRun();
      const moderate = run.results[1];
      if (!moderate) throw new Error("The moderate fixture result is missing.");
      const significant = kind === "significant_digits";
      run.pid_rounding = { kind, digits: significant ? 3 : 1 };
      run.results = [
        {
          ...moderate,
          proportional: significant ? 0.004873 : 155.21378,
          integral: significant ? 9.999 : 2.482169,
          controller_values: {
            response_level: "moderate",
            proportional: significant
              ? { value: 0.00487, display: "0.00487" }
              : { value: 155.2, display: "155.2" },
            integral: significant
              ? { value: 10, display: "10.0" }
              : { value: 2.5, display: "2.5" },
            derivative: { value: 0, display: significant ? "0" : "0.0" },
          },
        },
      ];
      await openRun(page, run);
      const row = resultsSection(page)
        .locator("tbody tr")
        .filter({ hasText: "Moderate" });
      const expected = significant
        ? ["0.00487", "10.0", "0"]
        : ["155.2", "2.5", "0.0"];
      await Promise.all(
        expected.map((text, index) =>
          expect(row.locator("td").nth(index + 1)).toHaveText(text),
        ),
      );
      await row.getByRole("button", { name: "Review & write" }).click();
      await expect(
        page.getByRole("dialog").locator("tbody td:last-child"),
      ).toHaveText(expected);
      await page
        .getByRole("dialog")
        .getByRole("button", { name: "Cancel" })
        .click();
    });
  }

  test("keeps an otherwise valid result unwritable when precision erases an active term", async ({
    page,
  }) => {
    const run = completedRun();
    const moderate = run.results[1];
    if (!moderate) throw new Error("The moderate fixture result is missing.");
    run.results[1] = {
      ...moderate,
      integral: 0.049,
      controller_values: null,
      controller_target_error:
        "Template PID rounding would erase an active term to zero.",
    };
    await openRun(page, run);
    const row = resultsSection(page)
      .locator("tbody tr")
      .filter({ hasText: "Moderate" });
    await expect(row).toContainText("Unwritable");
    await expect(row).toContainText("erase an active term to zero");
    await expect(
      row.getByRole("button", { name: "Review & write" }),
    ).toBeDisabled();
    await expect(
      resultsSection(page)
        .locator("tbody tr")
        .filter({ hasText: "Aggressive" })
        .getByRole("button", { name: "Review & write" }),
    ).toBeEnabled();
  });

  test("restore review and audit preserve recorded precision rather than template rounding", async ({
    page,
  }) => {
    const run = completedRun();
    const lastWrite = run.writes.at(-1);
    if (!lastWrite) throw new Error("The last write fixture is missing.");
    lastWrite.proportional_previous = 11.123456;
    lastWrite.integral_previous = 22.765432;
    lastWrite.derivative_previous = 33.000123;
    await openRun(page, run);
    await expect(detailSection(page, "PID change history")).toContainText(
      "11.123456",
    );
    await page.getByRole("button", { name: "Restore previous values" }).click();
    await expect(
      page.getByRole("dialog").locator("tbody td:last-child"),
    ).toHaveText(["11.123456", "22.765432", "33.000123"]);
  });

  test("closes the write review modal immediately and stays silent after success", async ({
    page,
  }) => {
    await openRun(page);

    let releaseWrite: () => void = () => undefined;
    const writeGate = new Promise<void>((resolve) => {
      releaseWrite = resolve;
    });
    await page.route(`**/api/runs/${RUN_ID}/write`, async (route) => {
      expect(route.request().postDataJSON()).toEqual({
        response_level: "aggressive",
      });
      await writeGate;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          ...completedRun(),
          writes: [
            ...completedRun().writes,
            {
              kind: "write",
              response_level: "aggressive",
              written_at: "2026-01-01T12:05:30Z",
              success: true,
              allow_uncertain_quality: true,
              proportional_previous: 5,
              integral_previous: 6,
              derivative_previous: 7,
              proportional_written: 20.5,
              integral_written: 1.2,
              derivative_written: 0,
              proportional_readback: 20.5,
              integral_readback: 1.2,
              derivative_readback: 0,
              rollback_state: null,
              rollback_error: null,
              error_message: null,
            },
          ],
        }),
      });
    });

    await resultsSection(page)
      .getByRole("button", { name: "Review & write" })
      .first()
      .click();
    const modal = page.getByRole("dialog");
    const request = page.waitForRequest((candidate) =>
      candidate.url().endsWith(`/api/runs/${RUN_ID}/write`),
    );
    await modal.getByRole("button", { name: "Write PID settings" }).click();
    await request;

    await expect(page.getByRole("dialog")).not.toBeVisible();

    releaseWrite();
    await expect(
      page.getByRole("button", { name: "Review & write" }).first(),
    ).toBeVisible();
    await expect(page.getByRole("alert")).toHaveCount(0);
  });

  test("shows a failed request at the top after the write modal closes", async ({
    page,
  }) => {
    await openRun(page);
    await page.route(`**/api/runs/${RUN_ID}/write`, async (route) => {
      await route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({ error: "bridge unavailable" }),
      });
    });

    await resultsSection(page)
      .getByRole("button", { name: "Review & write" })
      .first()
      .click();
    const modal = page.getByRole("dialog");
    await modal.getByRole("button", { name: "Write PID settings" }).click();

    await expect(
      page.getByRole("alert").filter({
        hasText: "The server could not complete the request. Try again.",
      }),
    ).toBeVisible();
    await expect(page.getByRole("dialog")).not.toBeVisible();
  });

  test("shows a later readback failure at the top without reopening the modal", async ({
    page,
  }) => {
    await openRun(page);

    let releaseWrite: () => void = () => undefined;
    const writeGate = new Promise<void>((resolve) => {
      releaseWrite = resolve;
    });
    await page.route(`**/api/runs/${RUN_ID}/write`, async (route) => {
      await writeGate;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          ...completedRun(),
          writes: [
            ...completedRun().writes,
            {
              kind: "write",
              response_level: "aggressive",
              written_at: "2026-01-01T12:05:30Z",
              success: false,
              allow_uncertain_quality: true,
              proportional_previous: 5,
              integral_previous: null,
              derivative_previous: null,
              proportional_written: 20.5,
              integral_written: null,
              derivative_written: null,
              proportional_readback: null,
              integral_readback: null,
              derivative_readback: null,
              rollback_state: "failed",
              rollback_error: "rollback failed",
              error_message: "PID readback was outside tolerance",
            },
          ],
        }),
      });
    });

    await resultsSection(page)
      .getByRole("button", { name: "Review & write" })
      .first()
      .click();
    const modal = page.getByRole("dialog");
    const request = page.waitForRequest((candidate) =>
      candidate.url().endsWith(`/api/runs/${RUN_ID}/write`),
    );
    await modal.getByRole("button", { name: "Write PID settings" }).click();
    await request;

    await expect(page.getByRole("dialog")).not.toBeVisible();
    const writingButton = resultsSection(page).getByRole("button", {
      name: "Writing…",
    });
    await expect(writingButton).toBeDisabled();
    await expect(page.getByRole("alert")).toHaveCount(0);

    releaseWrite();
    const topAlert = page.getByRole("alert").first();
    await expect(topAlert).toContainText(
      "Aggressive: The PID settings could not be applied.",
    );
    await expect(topAlert).toBeVisible();
    await expect(page.getByRole("dialog")).not.toBeVisible();
  });

  test("closes the restore review modal immediately and stays silent after success", async ({
    page,
  }) => {
    await openRun(page);
    let releaseRevert: () => void = () => undefined;
    const revertGate = new Promise<void>((resolve) => {
      releaseRevert = resolve;
    });
    await page.route(`**/api/runs/${RUN_ID}/revert`, async (route) => {
      expect(route.request().method()).toBe("POST");
      expect(route.request().postData()).toBeNull();
      await revertGate;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          ...completedRun(),
          writes: [
            ...completedRun().writes,
            {
              kind: "revert",
              response_level: "sluggish",
              written_at: "2026-01-01T12:05:30Z",
              success: true,
              allow_uncertain_quality: true,
              proportional_previous: 30,
              integral_previous: 1.8,
              derivative_previous: 0,
              proportional_written: 11,
              integral_written: 22,
              derivative_written: 33,
              proportional_readback: 11,
              integral_readback: 22,
              derivative_readback: 33,
              rollback_state: null,
              rollback_error: null,
              error_message: null,
            },
          ],
        }),
      });
    });

    const restoreButtons = page.getByRole("button", {
      name: "Restore previous values",
    });
    await expect(restoreButtons).toHaveCount(1);
    await restoreButtons.click();

    const modal = page.getByRole("dialog");
    await expect(
      modal.getByRole("heading", { name: "Review PID restore" }),
    ).toBeVisible();
    await expect(modal).toContainText("Sluggish");
    await expect(modal).toContainText("11");
    await expect(modal).toContainText("22");
    await expect(modal).toContainText("33");
    await expect(modal).toContainText("Area.FIC101.PB");
    await expect(modal).toContainText("Area.FIC101.RI");
    await expect(modal).toContainText("Area.FIC101.D");

    const request = page.waitForRequest((candidate) =>
      candidate.url().endsWith(`/api/runs/${RUN_ID}/revert`),
    );
    await modal
      .getByRole("button", { name: "Restore previous values" })
      .click();
    await request;
    await expect(page.getByRole("dialog")).not.toBeVisible();
    const restoringButton = page.getByRole("button", {
      name: "Restoring…",
    });
    await expect(restoringButton).toBeDisabled();
    await expect(page.getByRole("alert")).toHaveCount(0);

    releaseRevert();
    await expect(page.getByRole("dialog")).not.toBeVisible();
    await expect(page.getByRole("alert")).toHaveCount(0);
  });

  test("shows a later restore readback failure at the top without reopening the modal", async ({
    page,
  }) => {
    await openRun(page);
    let releaseRevert: () => void = () => undefined;
    const revertGate = new Promise<void>((resolve) => {
      releaseRevert = resolve;
    });
    await page.route(`**/api/runs/${RUN_ID}/revert`, async (route) => {
      await revertGate;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          ...completedRun(),
          writes: [
            ...completedRun().writes,
            {
              kind: "revert",
              response_level: "sluggish",
              written_at: "2026-01-01T12:05:30Z",
              success: false,
              allow_uncertain_quality: true,
              proportional_previous: 30,
              integral_previous: null,
              derivative_previous: null,
              proportional_written: 11,
              integral_written: null,
              derivative_written: null,
              proportional_readback: null,
              integral_readback: null,
              derivative_readback: null,
              rollback_state: null,
              rollback_error: null,
              error_message: "PID restore readback was outside tolerance",
            },
          ],
        }),
      });
    });

    await page.getByRole("button", { name: "Restore previous values" }).click();
    const modal = page.getByRole("dialog");
    const request = page.waitForRequest((candidate) =>
      candidate.url().endsWith(`/api/runs/${RUN_ID}/revert`),
    );
    await modal
      .getByRole("button", { name: "Restore previous values" })
      .click();
    await request;
    await expect(page.getByRole("dialog")).not.toBeVisible();
    await expect(page.getByRole("alert")).toHaveCount(0);

    releaseRevert();
    const topAlert = page.getByRole("alert").first();
    await expect(topAlert).toContainText(
      "Sluggish: The previous PID values could not be restored.",
    );
    await expect(topAlert).toBeVisible();
    await expect(page.getByRole("dialog")).not.toBeVisible();
  });
});
