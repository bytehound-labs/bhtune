import { expect, test, type Locator, type Page } from "@playwright/test";

const RUN_ID = 4242;

function completedRun(withResults = true) {
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
          },
          {
            response_level: "moderate",
            kp: 0.6,
            ti_minutes: 1.5,
            td_minutes: 0,
            proportional: 25,
            integral: 1.5,
            derivative: 0,
          },
          {
            response_level: "sluggish",
            kp: 0.4,
            ti_minutes: 1.8,
            td_minutes: 0,
            proportional: 30,
            integral: 1.8,
            derivative: 0,
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
      "MV actuation verification",
    ]);
    for (const title of sectionTitles.slice(0, -1)) {
      await expect(detailSection(page, title)).toHaveAttribute("open", "");
    }
    await expect(
      detailSection(page, "MV actuation verification"),
    ).not.toHaveAttribute("open", "");

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

  test("locks the review modal while writing and closes after HTTP success", async ({
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
        body: JSON.stringify(completedRun()),
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

    await expect(modal.getByRole("status")).toContainText(
      "Writing and verifying",
    );
    await expect(modal.getByRole("button", { name: "Cancel" })).toBeDisabled();
    await expect(modal.getByRole("button", { name: "Close" })).toBeDisabled();
    await expect(
      modal.getByRole("button", { name: /Writing and verifying/ }),
    ).toBeDisabled();

    await page.keyboard.press("Escape");
    await expect(modal).toBeVisible();

    releaseWrite();
    await expect(page.getByRole("dialog")).not.toBeVisible();
  });

  test("shows a failed request inside the review modal", async ({ page }) => {
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
      modal.getByText("The server could not complete the request. Try again.", {
        exact: true,
      }),
    ).toBeVisible();
    await expect(modal).toBeVisible();
  });

  test("reviews the newest successful write before restoring its previous values", async ({
    page,
  }) => {
    await openRun(page);
    await page.route(`**/api/runs/${RUN_ID}/revert`, async (route) => {
      expect(route.request().method()).toBe("POST");
      expect(route.request().postData()).toBeNull();
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(completedRun()),
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
  });
});
