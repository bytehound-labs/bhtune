import { expect, test } from "@playwright/test";

/**
 * Regression coverage for the OPC DA server-discovery and tag-browser affordances
 * (`ui-opc-browser`) that doesn't require a live OPC DA gateway. This suite's shared
 * `bhtune-server` instance (`playwright.config.ts`'s `webServer`) never starts an
 * `opcda-bridge-gateway`, so every action below fails at the connection step against the
 * default `localhost:7600` bridge host -- confirmed by hand to fail near-instantly
 * (`ECONNREFUSED`, no gateway listening), well inside `api/opc.rs`'s own 30s
 * `OPC_QUERY_TIMEOUT_SECS` budget, so this suite is fast and deterministic without needing
 * one.
 *
 * A connection failure is still genuinely useful regression coverage: it exercises the
 * driver-switch visibility of the OPC-only fields, the real `GET /api/opc/servers` and
 * `GET /api/opc/browse` request wiring behind "Discover servers"/"Browse tags…", the modal
 * opening/closing, and that a failure renders as a visible error rather than a silent no-op
 * or an unhandled exception -- none of which any other spec in this suite touches. The
 * populated-tree happy path (expand/select/derived-tag preview/"Test read"/"Use this tag")
 * was verified once by hand against a temporary mock gRPC gateway -- see AGENTS.md's
 * `ui-opc-browser` section -- and is deliberately not re-proven here: standing up a second,
 * permanent mock gRPC service just for this suite would be disproportionate to what it
 * would additionally prove over that manual pass.
 */
test.describe("OPC DA server discovery and tag browser (no gateway present)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/runs/new");
    await page.getByLabel("Driver").selectOption("opcda");
  });

  test("Browse tags button stays disabled until a ProgID is entered", async ({
    page,
  }) => {
    const browseButton = page.getByRole("button", { name: "Browse tags…" });
    await expect(browseButton).toBeDisabled();

    await page
      .getByLabel("OPC DA server ProgID")
      .fill("Matrikon.OPC.Simulation");
    await expect(browseButton).toBeEnabled();
  });

  test("shows a connection error when discovering servers with no gateway present", async ({
    page,
  }) => {
    await page
      .getByLabel("OPC DA server ProgID")
      .fill("Matrikon.OPC.Simulation");
    await page.getByRole("button", { name: "Discover servers" }).click();

    await expect(page.getByText(/failed to connect/)).toBeVisible();
  });

  test("opens the tag browser modal and shows a connection error at the root level, then closes", async ({
    page,
  }) => {
    await page
      .getByLabel("OPC DA server ProgID")
      .fill("Matrikon.OPC.Simulation");
    await page.getByRole("button", { name: "Browse tags…" }).click();

    await expect(
      page.getByRole("heading", {
        name: "Browse tags on Matrikon.OPC.Simulation",
      }),
    ).toBeVisible();
    await expect(page.getByText(/failed to connect/)).toBeVisible();

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
    await page.getByRole("button", { name: "Browse tags…" }).click();
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
