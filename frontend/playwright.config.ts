import { defineConfig, devices } from "@playwright/test";

/**
 * Distinct from the app's own documented default port (8787) and from `start-server.mjs`'s
 * own hardcoded copy of the same number -- kept as a literal here rather than imported,
 * since Playwright config files and the plain Node script it spawns as `webServer.command`
 * don't share a module graph worth wiring up for one constant.
 */
const PORT = 18787;
const BASE_URL = `http://127.0.0.1:${PORT}`;
const DEMO_PROXY_PORT = 18789;
const DEMO_BASE_URL = `https://127.0.0.1:${DEMO_PROXY_PORT}`;
const requestedMode = process.env.PLAYWRIGHT_MODE;

/**
 * `e2e-playwright`: drives a full tune through the real, built React SPA served by a real
 * `bhtune-server` binary running the in-process simulator driver -- no mocked HTTP layer,
 * no Vite dev server. A direct dividend of dropping Tauri (see AGENTS.md's "Web app
 * architecture"): `tauri-driver`/WebDriver would have been markedly more fragile in CI than
 * plain Playwright-against-a-browser.
 *
 * The Full and Demo projects each use an isolated launcher, database, port, and output
 * directory. By default Playwright starts both launchers for one config invocation;
 * `PLAYWRIGHT_MODE=full|demo` starts only the selected launcher. `testMatch` and `testIgnore`
 * keep the real Demo integration spec on the HTTPS Demo server. The route-mocked Demo
 * contract spec runs on the ordinary Full launcher because it replaces every API request and
 * needs only the shared SPA assets, avoiding an unnecessary second live Demo dependency.
 *
 * The launchers build nothing themselves -- they only run the already-compiled binary via
 * `e2e/start-server.mjs` or `e2e/start-demo-server.mjs` -- so both
 * `pnpm --filter bhtune-frontend run build` (produces `frontend/dist/`, which a debug
 * `bhtune-server` reads live off disk -- see that crate's `rust-embed` dependency comment)
 * and `cargo build -p bhtune-server` must have already run before this suite starts. CI's
 * `e2e` job does both explicitly; a local run needs the same two commands first, then
 * `pnpm run test:e2e` (or `pnpm run test:e2e -- --ui` while iterating on a spec file).
 */
export default defineConfig({
  testDir: "./e2e",
  // The suite shares one isolated server, including mutable global tuning configuration and
  // the app-wide New Tune draft. Keep workers serialized so one browser test cannot change
  // those shared values while another test is preparing a run.
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  // A run against a live relay-switching simulation is not inherently flaky, but CI
  // runners are noisier than a dev machine -- one retry absorbs an occasional slow-CI
  // timing hiccup without masking a real failure (a genuinely broken test still fails
  // twice).
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: BASE_URL,
    trace: "on-first-retry",
  },
  projects: [
    {
      name: "full",
      testIgnore: ["**/demo-real.spec.ts"],
      outputDir: "test-results/full",
      use: { ...devices["Desktop Chrome"] },
    },
    {
      name: "demo",
      testMatch: ["**/demo-real.spec.ts"],
      outputDir: "test-results/demo",
      use: {
        ...devices["Desktop Chrome"],
        baseURL: DEMO_BASE_URL,
        ignoreHTTPSErrors: true,
      },
    },
  ],
  webServer:
    requestedMode === "full"
      ? [
          {
            command: "node e2e/start-server.mjs",
            url: `${BASE_URL}/api/health`,
            // Reusing an already-running instance is a local-dev convenience only (e.g.
            // iterating on a Full spec without restarting the server) -- CI always starts
            // fresh, so a stale server from a previous failed run can never mask a regression.
            reuseExistingServer: !process.env.CI,
            timeout: 30_000,
            stdout: "pipe",
            stderr: "pipe",
          },
        ]
      : requestedMode === "demo"
        ? [
            {
              command: "node e2e/start-demo-server.mjs",
              // Probe the local backend directly. The Demo browser project exercises the
              // HTTPS proxy separately, whose intentionally self-signed certificate is
              // ignored by that project's browser context.
              url: `http://127.0.0.1:18788/api/health`,
              reuseExistingServer: !process.env.CI,
              timeout: 30_000,
              stdout: "pipe",
              stderr: "pipe",
            },
          ]
        : [
            {
              command: "node e2e/start-server.mjs",
              url: `${BASE_URL}/api/health`,
              reuseExistingServer: !process.env.CI,
              timeout: 30_000,
              stdout: "pipe",
              stderr: "pipe",
            },
            {
              command: "node e2e/start-demo-server.mjs",
              url: `http://127.0.0.1:18788/api/health`,
              reuseExistingServer: !process.env.CI,
              timeout: 30_000,
              stdout: "pipe",
              stderr: "pipe",
            },
          ],
});
