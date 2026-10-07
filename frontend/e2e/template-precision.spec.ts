import { expect, test } from "@playwright/test";

test("custom template precision validates and survives creation and editing", async ({
  page,
}) => {
  const name = `Precision template ${Date.now()}`;
  await page.goto("/templates/new");
  await expect(page.getByLabel("PID rounding", { exact: true })).toHaveValue(
    "significant_digits",
  );
  await expect(page.getByLabel("PID precision", { exact: true })).toHaveValue(
    "3",
  );
  await page.getByLabel("Name", { exact: true }).fill(name);
  await page.getByLabel("Process variable", { exact: true }).fill("PV");
  await page.getByLabel("Manipulated variable", { exact: true }).fill("MV");
  await page.getByLabel("PID precision", { exact: true }).fill("0");
  await expect(
    page.getByText("PID precision must be a whole number from 1 to 7."),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Create template", exact: true }),
  ).toBeDisabled();
  await page
    .getByLabel("PID rounding", { exact: true })
    .selectOption("decimal_places");
  await expect(
    page.getByRole("button", { name: "Create template", exact: true }),
  ).toBeEnabled();
  await page.getByLabel("PID precision", { exact: true }).fill("1");
  const created = page.waitForRequest(
    (request) =>
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/templates",
  );
  await page
    .getByRole("button", { name: "Create template", exact: true })
    .click();
  expect((await created).postDataJSON().pid_rounding).toEqual({
    kind: "decimal_places",
    digits: 1,
  });
  await expect(page.getByRole("heading", { name, exact: true })).toBeVisible();
  await expect(
    page.getByText("Decimal places: 1", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await expect(page.getByLabel("PID precision", { exact: true })).toHaveValue(
    "1",
  );
  await page
    .getByLabel("PID rounding", { exact: true })
    .selectOption("significant_digits");
  await page.getByLabel("PID precision", { exact: true }).fill("7");
  await page.getByRole("button", { name: "Save changes", exact: true }).click();
  await expect(page.getByRole("heading", { name, exact: true })).toBeVisible();
  await expect(
    page.getByText("Significant digits: 7", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Delete template", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Templates", exact: true }),
  ).toBeVisible();
});
