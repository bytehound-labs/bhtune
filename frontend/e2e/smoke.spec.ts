import { expect, test } from "@playwright/test";

/**
 * Basic navigation/rendering smoke coverage: the app shell loads on the Tune screen, the
 * health badge reaches a real driver, and the four built-in DCS/PLC templates that
 * `bhtune-db`'s `seed_builtin_templates` seeds into every fresh database (see
 * `db-seed-templates`) render in the Templates list. Deliberately no interaction with
 * tune/write-back flows here -- that's `tune.spec.ts`'s job.
 */
test.describe("app shell", () => {
  test("loads, reaches a healthy driver, and lands on the Tune screen", async ({
    page,
  }) => {
    await page.goto("/");

    // The index route redirects to /runs/new (see App.tsx's route table) -- starting a
    // tune is the app's default landing page (`ui-tune-nav`).
    await expect(page).toHaveURL(/\/runs\/new$/);
    await expect(
      page.locator("header").getByText("BHTune", { exact: true }),
    ).toBeVisible();

    await expect(page.getByRole("link", { name: "Tune" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Templates" })).toBeVisible();
    await expect(page.getByRole("link", { name: "History" })).toBeVisible();

    // The health badge polls a real `/api/health` -- confirms this isn't a static mock.
    await expect(page.getByText(/Server: ok/)).toBeVisible();
  });

  test("lists the seeded templates", async ({ page }) => {
    await page.goto("/templates");

    for (const name of [
      "Yokogawa CentumVP",
      "Honeywell Experion",
      "Schneider Modicon",
      "Allen-Bradley PlantPAx",
    ]) {
      await expect(page.getByText(name, { exact: true })).toBeVisible();
    }
  });

  test("navigates between Tune, Templates, and History via the header nav", async ({
    page,
  }) => {
    await page.goto("/templates");

    await page.getByRole("link", { name: "History" }).click();
    await expect(page).toHaveURL(/\/runs$/);
    await expect(page.getByRole("heading", { name: "History" })).toBeVisible();

    await page.getByRole("link", { name: "Templates" }).click();
    await expect(page).toHaveURL(/\/templates$/);

    await page.getByRole("link", { name: "Tune" }).click();
    await expect(page).toHaveURL(/\/runs\/new$/);
  });
});
