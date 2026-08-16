import { expect, test } from "@playwright/test";

/**
 * Basic navigation/rendering smoke coverage: the app shell loads, the health badge reaches
 * a real backend, and the four built-in DCS/PLC templates that `bhtune-db`'s
 * `seed_builtin_templates` seeds into every fresh database (see `db-seed-templates`) render
 * in the Templates list. Deliberately no interaction with tune/write-back flows here --
 * that's `tune.spec.ts`'s job.
 */
test.describe("app shell", () => {
  test("loads, reaches a healthy backend, and lists the seeded templates", async ({
    page,
  }) => {
    await page.goto("/");

    // The index route redirects to /templates (see App.tsx's route table).
    await expect(page).toHaveURL(/\/templates$/);
    await expect(
      page.locator("header").getByText("BHTune", { exact: true }),
    ).toBeVisible();

    await expect(page.getByRole("link", { name: "Templates" })).toBeVisible();
    await expect(page.getByRole("link", { name: "History" })).toBeVisible();

    // The health badge polls a real `/api/health` -- confirms this isn't a static mock.
    await expect(page.getByText(/Server: ok/)).toBeVisible();

    for (const name of [
      "Yokogawa CentumVP",
      "Honeywell Experion",
      "Schneider Modicon",
      "Allen-Bradley PlantPAx",
    ]) {
      await expect(page.getByText(name, { exact: true })).toBeVisible();
    }
  });

  test("navigates to History and back via the header nav", async ({ page }) => {
    await page.goto("/templates");

    await page.getByRole("link", { name: "History" }).click();
    await expect(page).toHaveURL(/\/runs$/);
    await expect(page.getByRole("heading", { name: "History" })).toBeVisible();

    await page.getByRole("link", { name: "Templates" }).click();
    await expect(page).toHaveURL(/\/templates$/);
  });
});
