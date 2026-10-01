import { expect, test } from "@playwright/test";
import { OPC_BROWSER_SUITE, openOpcDaRunForm } from "./support/opcBrowser";
import {
  expectNoAccessibilityViolations,
  setTheme,
} from "./support/accessibility";

/**
 * Server discovery and tag-browser modal coverage: the ProgID gate on Browse tags, the
 * server picker, pending and connection-error feedback against the gateway-less
 * `bhtune-server`, and closing the modal.
 */
test.describe(OPC_BROWSER_SUITE, () => {
  test.beforeEach(async ({ page }) => {
    await openOpcDaRunForm(page);
  });

  test("Browse tags button stays disabled until a ProgID is entered", async ({
    page,
  }) => {
    const browseButton = page.getByRole("button", { name: "Browse tags" });
    await expect(browseButton).toBeDisabled();

    await page
      .getByLabel("OPC DA server ProgID")
      .fill("Matrikon.OPC.Simulation");
    await expect(browseButton).toBeEnabled();
  });

  test("opens the server picker and fills the ProgID from a discovered server", async ({
    page,
  }) => {
    await page.route("**/api/opc/servers**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          servers: ["Kepware.KEPServerEX.V5", "Yokogawa.CSHIS_OPC.1"],
        }),
      });
    });

    const serverField = page.getByLabel("OPC DA server ProgID");
    const browseTrigger = page.getByRole("button", { name: "Browse servers" });
    await browseTrigger.click();

    await expect(
      page.getByRole("heading", { name: "Browse OPC DA servers" }),
    ).toBeVisible();
    await expect(
      page.getByRole("dialog", { name: "Browse OPC DA servers" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Yokogawa.CSHIS_OPC.1" }),
    ).toBeVisible();
    const dialog = page.getByRole("dialog", {
      name: "Browse OPC DA servers",
    });
    const close = dialog.getByRole("button", { name: "Close" });
    const lastServer = dialog.getByRole("button", {
      name: "Yokogawa.CSHIS_OPC.1",
    });
    await expect(close).toBeFocused();
    await expectNoAccessibilityViolations(page);
    await page.keyboard.press("Shift+Tab");
    await expect(lastServer).toBeFocused();

    await lastServer.click();
    await expect(serverField).toHaveValue("Yokogawa.CSHIS_OPC.1");
    await expect(browseTrigger).toBeFocused();
    await expect(
      page.getByRole("heading", { name: "Browse OPC DA servers" }),
    ).not.toBeVisible();

    await setTheme(page, "light");
    await browseTrigger.click();
    await expect(lastServer).toBeVisible();
    await expectNoAccessibilityViolations(page);
    await page.keyboard.press("Escape");
    await expect(dialog).not.toBeVisible();
    await expect(browseTrigger).toBeFocused();
  });

  test("shows shared loading feedback while server discovery is pending", async ({
    page,
  }) => {
    let releaseServers: () => void = () => undefined;
    const serversGate = new Promise<void>((resolve) => {
      releaseServers = resolve;
    });
    await page.route("**/api/opc/servers**", async (route) => {
      await serversGate;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ servers: ["Yokogawa.CSHIS_OPC.1"] }),
      });
    });

    const browseButton = page.getByRole("button", { name: "Browse servers" });
    await browseButton.click();

    const dialog = page.getByRole("dialog", {
      name: "Browse OPC DA servers",
    });
    await expect(dialog.getByRole("status")).toContainText("Connecting…");
    await expect(browseButton).toHaveAttribute("aria-busy", "true");
    await expect(browseButton).toBeDisabled();

    releaseServers();
    await expect(
      dialog.getByRole("button", { name: "Yokogawa.CSHIS_OPC.1" }),
    ).toBeVisible();
    await expect(browseButton).not.toHaveAttribute("aria-busy");
  });

  test("restores focus when the server picker closes during discovery", async ({
    page,
  }) => {
    let releaseServers: () => void = () => undefined;
    const serversGate = new Promise<void>((resolve) => {
      releaseServers = resolve;
    });
    await page.route("**/api/opc/servers**", async (route) => {
      await serversGate;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ servers: ["Yokogawa.CSHIS_OPC.1"] }),
      });
    });

    const browseButton = page.getByRole("button", { name: "Browse servers" });
    await browseButton.click();
    const dialog = page.getByRole("dialog", {
      name: "Browse OPC DA servers",
    });
    await expect(dialog.getByRole("status")).toContainText("Connecting…");
    await page.keyboard.press("Escape");
    await expect(dialog).not.toBeVisible();
    await expect(browseButton).toBeEnabled();
    await expect(browseButton).toBeFocused();

    releaseServers();
  });

  test("shows a connection error when browsing servers with no gateway present", async ({
    page,
  }) => {
    await page.getByRole("button", { name: "Browse servers" }).click();

    await expect(
      page.getByText("Unable to browse OPC DA servers."),
    ).toBeVisible();
  });

  test("opens the tag browser modal and shows a connection error at the root level, then closes", async ({
    page,
  }) => {
    await page
      .getByLabel("OPC DA server ProgID")
      .fill("Matrikon.OPC.Simulation");
    await page.getByRole("button", { name: "Browse tags" }).click();

    await expect(
      page.getByRole("heading", {
        name: "Browse tags on Matrikon.OPC.Simulation",
      }),
    ).toBeVisible();
    await expect(
      page.getByRole("dialog", {
        name: "Browse tags on Matrikon.OPC.Simulation",
      }),
    ).toBeVisible();
    await expect(
      page.getByText("Unable to load tags at this level."),
    ).toBeVisible();
    await expect(
      page
        .getByRole("dialog", {
          name: "Browse tags on Matrikon.OPC.Simulation",
        })
        .getByRole("status"),
    ).not.toBeVisible();

    await page.getByRole("button", { name: "Close" }).click();
    await expect(
      page.getByRole("heading", {
        name: "Browse tags on Matrikon.OPC.Simulation",
      }),
    ).not.toBeVisible();
  });

  test("closes the tag browser modal via Escape", async ({ page }) => {
    await page
      .getByLabel("OPC DA server ProgID")
      .fill("Matrikon.OPC.Simulation");
    await page.getByRole("button", { name: "Browse tags" }).click();
    await expect(
      page.getByRole("heading", {
        name: "Browse tags on Matrikon.OPC.Simulation",
      }),
    ).toBeVisible();

    await page.keyboard.press("Escape");
    await expect(
      page.getByRole("heading", {
        name: "Browse tags on Matrikon.OPC.Simulation",
      }),
    ).not.toBeVisible();
  });
});
