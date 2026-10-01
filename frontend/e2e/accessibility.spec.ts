import { expect, test } from "@playwright/test";
import {
  expectAccessibilityInBothThemes,
  expectNoAccessibilityViolations,
  setTheme,
} from "./support/accessibility";

const accessibleScreens = [
  { name: "New Tune form", path: "/runs/new", heading: "New tune" },
  { name: "History", path: "/runs", heading: "History" },
  { name: "Templates", path: "/templates", heading: "Templates" },
  { name: "Configuration", path: "/config", heading: "Configuration" },
] as const;

test("key screens have no WCAG accessibility violations in either theme", async ({
  page,
}) => {
  await accessibleScreens.reduce(
    (checks, screen) =>
      checks.then(() =>
        test.step(screen.name, async () => {
          await page.goto(screen.path);
          await expect(
            page.getByRole("heading", { name: screen.heading, exact: true }),
          ).toBeVisible();
          await expectAccessibilityInBothThemes(page);
        }),
      ),
    Promise.resolve(),
  );
});

test("shared confirmation dialogs trap and restore focus in both themes", async ({
  page,
}) => {
  await page.goto("/templates");
  const deleteTrigger = page
    .getByRole("button", { name: "Delete", exact: true })
    .first();
  await expect(deleteTrigger).toBeVisible();
  await deleteTrigger.click();

  const dialog = page.getByRole("dialog", { name: "Delete template?" });
  const cancel = dialog.getByRole("button", { name: "Cancel" });
  const close = dialog.getByRole("button", { name: "Close" });
  const confirm = dialog.getByRole("button", { name: "Delete template" });
  await expect(cancel).toBeFocused();
  await expectNoAccessibilityViolations(page);

  await page.keyboard.press("Shift+Tab");
  await expect(close).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(confirm).toBeFocused();
  await dialog.evaluate((element) => {
    (element as HTMLDialogElement).focus();
  });
  await page.keyboard.press("Tab");
  await expect(close).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(confirm).toBeFocused();

  await cancel.click();
  await expect(dialog).not.toBeVisible();
  await expect(deleteTrigger).toBeFocused();

  await setTheme(page, "light");
  await deleteTrigger.click();
  await expect(dialog).toBeVisible();
  await expectNoAccessibilityViolations(page);
  await cancel.click();
  await expect(deleteTrigger).toBeFocused();
});
