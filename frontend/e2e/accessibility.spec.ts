import { expect, test } from "@playwright/test";
import {
  expectAccessibilityInBothThemes,
  expectNoAccessibilityViolations,
  setTheme,
} from "./support/accessibility";

test("New Tune form has no WCAG accessibility violations in either theme", async ({
  page,
}) => {
  await page.goto("/runs/new");
  await expect(
    page.getByRole("heading", { name: "New tune", exact: true }),
  ).toBeVisible();
  await expectAccessibilityInBothThemes(page);
});

test("History has no WCAG accessibility violations in either theme", async ({
  page,
}) => {
  await page.goto("/runs");
  await expect(
    page.getByRole("heading", { name: "History", exact: true }),
  ).toBeVisible();
  await expectAccessibilityInBothThemes(page);
});

test("Templates has no WCAG accessibility violations in either theme", async ({
  page,
}) => {
  await page.goto("/templates");
  await expect(
    page.getByRole("heading", { name: "Templates", exact: true }),
  ).toBeVisible();
  await expectAccessibilityInBothThemes(page);
});

test("Configuration has no WCAG accessibility violations in either theme", async ({
  page,
}) => {
  await page.goto("/config");
  await expect(
    page.getByRole("heading", { name: "Configuration", exact: true }),
  ).toBeVisible();
  await expectAccessibilityInBothThemes(page);
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
