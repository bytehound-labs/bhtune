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
 * Clicks "Start tune" and waits for navigation to the new run's detail page, retrying the
 * click if the server still reports the *previous* run as active.
 *
 * `bhtune-server` only frees its single-active-run slot (`ActiveRun::release`) once a run's
 * background task fully returns from `drive()` -- one `await` *after* the same task's
 * `drive()` call already persisted the completed/aborted outcome that the UI's SSE stream
 * reacts to (see `routes::runs::start_run`). That gap is normally sub-millisecond, but it is
 * real: a client that submits a new run the instant it observes the previous one finish can
 * still land in the brief window before `release()` itself has run, and gets rejected with
 * "run N is already active" even though the UI already shows it done. A real user pressing
 * the button a moment later would never notice; this retries rather than papering over it
 * with an arbitrary fixed sleep.
 */
async function startTune(page: Page) {
  const startButton = page.getByRole("button", { name: "Start tune" });
  const alreadyActiveError = page.getByText(/is already active/);
  for (let attempt = 0; attempt < 20; attempt++) {
    await startButton.click();
    const navigated = page
      .waitForURL(/\/runs\/\d+$/, { timeout: 500 })
      .then(() => true)
      .catch(() => false);
    if (await navigated) {
      return;
    }
    if (await alreadyActiveError.isVisible()) {
      await page.waitForTimeout(250);
      continue;
    }
    // Neither navigated nor showed the known transient error -- a real failure. Let the
    // caller's own `toHaveURL` assertion below report it with a normal Playwright error.
    return;
  }
}

/**
 * Drives a full MRFT tune end-to-end through the real browser UI -- the scenario
 * `e2e-playwright` exists for. Fills in the New Run form, submits it against a real
 * `bhtune-server` running the in-process simulator driver, and asserts the *rendered*
 * results are sane and correctly ordered, not just that the page didn't crash.
 *
 * Mirrors `crates/bhtune-cli/tests/e2e_simulator.rs`'s own "flow / PI / reverse" matrix
 * case and its millisecond-scale simulator parameters (`sim_tau`/`sim_dead_time`/
 * `poll_interval_ms`), for the same reason that test uses them: the form's human-oriented
 * defaults (`sim_tau=2`, `poll_interval_ms=800`) are realistic for an actual plant loop but
 * would make this test take minutes. `direction=reverse` is likewise required -- confirmed
 * (see that Rust test's own comment) to be the only direction that produces a genuine relay
 * oscillation against this fixed simulator configuration; it's already the form's default
 * whenever `driver=simulator`, so it isn't set explicitly below.
 */
test.describe("running a tune from the browser", () => {
  // `bhtune-server` allows exactly one active run at a time (`ActiveRun`, `server-start-
  // tune-api`) -- matching the real constraint that only one physical loop can be under
  // test through a given server at once. Both tests below start a real run against the
  // one shared `webServer` instance this whole suite runs against, so they must never
  // execute concurrently (Playwright's default is parallel workers *across* tests in a
  // file unless told otherwise) or the second one to start fails with a 409-equivalent
  // "run N is already active" error instead of the scenario it's meant to test.
  test.describe.configure({ mode: "serial" });

  test("completes a full simulator tune and renders sane, ordered results", async ({
    page,
  }) => {
    test.setTimeout(45_000);

    await page.goto("/runs/new");

    await page.getByLabel("Template").selectOption("Yokogawa CentumVP");
    await page.getByLabel("Cycles to skip").fill("1");
    await page.getByLabel("Cycles to count").fill("2");
    await page.getByLabel("Noise protection (s)").fill("0");
    await page.getByLabel("Poll interval (ms)").fill("5");
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

    const resultsSection = page.locator("section").filter({
      has: page.getByRole("heading", { name: "Calculated results" }),
    });
    const rows = resultsSection.locator("tbody tr");
    await expect(rows).toHaveCount(3);

    async function kp(level: "aggressive" | "moderate" | "sluggish") {
      const row = rows.filter({ hasText: level });
      await expect(row).toHaveCount(1);
      const kpText = await row.locator("td").nth(1).innerText();
      return Number.parseFloat(kpText);
    }
    async function tiMinutes(level: "aggressive" | "moderate" | "sluggish") {
      const row = rows.filter({ hasText: level });
      const tiText = await row.locator("td").nth(2).innerText();
      return Number.parseFloat(tiText);
    }
    async function tdMinutes(level: "aggressive" | "moderate" | "sluggish") {
      const row = rows.filter({ hasText: level });
      const tdText = await row.locator("td").nth(3).innerText();
      return Number.parseFloat(tdText);
    }

    const aggressiveKp = await kp("aggressive");
    const moderateKp = await kp("moderate");
    const sluggishKp = await kp("sluggish");

    expect(aggressiveKp).toBeGreaterThan(0);
    expect(moderateKp).toBeGreaterThan(0);
    expect(sluggishKp).toBeGreaterThan(0);
    expect(aggressiveKp).toBeGreaterThan(moderateKp);
    expect(moderateKp).toBeGreaterThan(sluggishKp);

    // The regression `e2e_simulator.rs` was written to catch (a sub-second relay-period
    // truncation bug silently zeroing ti_minutes/td_minutes for every controller type):
    // re-asserted here through the real rendered UI. This run's controller type is "pi"
    // (the form's own default), so ti_minutes must be genuinely nonzero and td_minutes
    // must be exactly zero (PI has no derivative term).
    expect(await tiMinutes("aggressive")).toBeGreaterThan(0);
    expect(await tdMinutes("aggressive")).toBe(0);

    await expect(
      page.getByText(/\d+ per-tick samples were recorded/),
    ).toBeVisible();
  });

  test("exports a completed run's samples as CSV and JSON downloads", async ({
    page,
  }) => {
    test.setTimeout(45_000);

    await page.goto("/runs/new");
    await page.getByLabel("Template").selectOption("Yokogawa CentumVP");
    await page.getByLabel("Cycles to skip").fill("1");
    await page.getByLabel("Cycles to count").fill("2");
    await page.getByLabel("Noise protection (s)").fill("0");
    await page.getByLabel("Poll interval (ms)").fill("5");
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
    await page.getByLabel("Template").selectOption("Yokogawa CentumVP");
    await page.getByLabel("Cycles to skip").fill("1");
    await page.getByLabel("Cycles to count").fill("2");
    await page.getByLabel("Noise protection (s)").fill("0");
    await page.getByLabel("Poll interval (ms)").fill("5");
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

    page.once("dialog", (dialog) => void dialog.accept());
    await page.getByRole("button", { name: "Delete run" }).click();

    await expect(page).toHaveURL(/\/runs$/);

    // Navigating straight back to the deleted run's own URL now 404s -- proves the row is
    // really gone, not just removed from the list view.
    await page.goto(`/runs/${runId}`);
    await expect(page.getByText(`no run with id ${runId}`)).toBeVisible();
  });

  test("cancels a running tune from the run detail page", async ({ page }) => {
    test.setTimeout(45_000);

    await page.goto("/runs/new");

    await page.getByLabel("Template").selectOption("Yokogawa CentumVP");
    // Deliberately slower than the completion test above (but still far faster than the
    // form's human-oriented defaults) -- reliably leaves a multi-second window to click
    // "Cancel run" before the tune would otherwise finish on its own, without wasting CI
    // time waiting on the form's real ~minutes-scale defaults.
    await page.getByLabel("Poll interval (ms)").fill("200");
    await page.getByLabel("Time constant τ (s)").fill("1");
    await page.getByLabel("Dead time (s)").fill("1");

    await startTune(page);
    await expect(page).toHaveURL(/\/runs\/\d+$/);

    const cancelButton = page.getByRole("button", { name: "Cancel run" });
    await expect(cancelButton).toBeVisible();
    await cancelButton.click();

    await expect(outcomeBadge(page, "Aborted")).toBeVisible({
      timeout: 30_000,
    });
    await expect(
      page.getByRole("button", { name: "Cancel run" }),
    ).not.toBeVisible();
  });
});
