import { expect, test, type Locator, type Page } from "@playwright/test";
import type { RunDetailResponse } from "../src/api/runs";

const RUN_ID = 4242;

function completedRun(withResults = true): RunDetailResponse {
  return {
    id: RUN_ID,
    tag_name: "Area.FIC101",
    outcome: "completed",
    driver: "opcda",
    template_name: "Yokogawa CentumVP",
    template_origin: "builtin",
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
  run.results[0] = {
    ...run.results[0],
    kp: null,
    ti_minutes: null,
    td_minutes: null,
    proportional: null,
    integral: null,
    derivative: null,
    status: "invalid",
    invalid_reason: "non_positive_pv_amplitude",
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
    for (const title of sectionTitles) {
      await expect(detailSection(page, title)).toHaveAttribute("open", "");
    }
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

    await resultsSection(page)
      .getByRole("button", { name: "Review & write" })
      .first()
      .click();

    const modal = page.getByRole("dialog");
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
    await expect(modal.getByRole("button", { name: "Cancel" })).toBeFocused();

    await modal.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByRole("dialog")).not.toBeVisible();
    expect(writeRequestCount).toBe(0);
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
