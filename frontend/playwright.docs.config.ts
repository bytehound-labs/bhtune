import { defineConfig, devices } from "@playwright/test";

const screenshotDir =
  process.env.DOCS_SCREENSHOT_DIR ?? "test-results/docs-screenshots";
const fullPort = 18787;
const demoBackendPort = 18788;
const demoProxyPort = 18789;
const requestedMode = process.env.DOCS_MODE;

export default defineConfig({
  testDir: "./e2e/docs-screenshots",
  testMatch: "**/*.spec.ts",
  outputDir: "test-results/docs-screenshots",
  snapshotDir: "test-results/docs-screenshots/snapshots",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: 1,
  reporter: process.env.CI ? [["line"], ["html"]] : "list",
  use: {
    ...devices["Desktop Chrome"],
    baseURL:
      process.env.DOCS_BASE_URL ??
      (process.env.DOCS_MODE === "demo"
        ? `https://127.0.0.1:${demoProxyPort}`
        : `http://127.0.0.1:${fullPort}`),
    locale: "en-CA",
    timezoneId: "UTC",
    colorScheme: "dark",
    deviceScaleFactor: 1,
    viewport: { width: 1440, height: 1100 },
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    video: "off",
    ignoreHTTPSErrors: true,
  },
  metadata: {
    screenshotDir,
  },
  projects:
    requestedMode === "full"
      ? [
          {
            name: "full",
            testMatch: "**/full.spec.ts",
            use: {
              ...devices["Desktop Chrome"],
              baseURL:
                process.env.DOCS_BASE_URL ?? `http://127.0.0.1:${fullPort}`,
              colorScheme: "dark",
              deviceScaleFactor: 1,
              locale: "en-CA",
              timezoneId: "UTC",
              viewport: { width: 1440, height: 1100 },
            },
          },
        ]
      : requestedMode === "demo"
        ? [
            {
              name: "demo",
              testMatch: "**/demo.spec.ts",
              use: {
                ...devices["Desktop Chrome"],
                baseURL:
                  process.env.DOCS_BASE_URL ??
                  `https://127.0.0.1:${demoProxyPort}`,
                colorScheme: "dark",
                deviceScaleFactor: 1,
                ignoreHTTPSErrors: true,
                locale: "en-CA",
                timezoneId: "UTC",
                viewport: { width: 1440, height: 1100 },
              },
            },
          ]
        : [
            {
              name: "full",
              testMatch: "**/full.spec.ts",
              use: {
                ...devices["Desktop Chrome"],
                baseURL:
                  process.env.DOCS_BASE_URL ?? `http://127.0.0.1:${fullPort}`,
                colorScheme: "dark",
                deviceScaleFactor: 1,
                locale: "en-CA",
                timezoneId: "UTC",
                viewport: { width: 1440, height: 1100 },
              },
            },
            {
              name: "demo",
              testMatch: "**/demo.spec.ts",
              use: {
                ...devices["Desktop Chrome"],
                baseURL:
                  process.env.DOCS_BASE_URL ??
                  `https://127.0.0.1:${demoProxyPort}`,
                colorScheme: "dark",
                deviceScaleFactor: 1,
                ignoreHTTPSErrors: true,
                locale: "en-CA",
                timezoneId: "UTC",
                viewport: { width: 1440, height: 1100 },
              },
            },
          ],
  webServer:
    requestedMode === "full"
      ? [
          {
            command: "node e2e/start-server.mjs",
            url: `http://127.0.0.1:${fullPort}/api/health`,
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
              url: `http://127.0.0.1:${demoBackendPort}/api/health`,
              reuseExistingServer: !process.env.CI,
              timeout: 30_000,
              stdout: "pipe",
              stderr: "pipe",
            },
          ]
        : [
            {
              command: "node e2e/start-server.mjs",
              url: `http://127.0.0.1:${fullPort}/api/health`,
              reuseExistingServer: !process.env.CI,
              timeout: 30_000,
              stdout: "pipe",
              stderr: "pipe",
            },
            {
              command: "node e2e/start-demo-server.mjs",
              url: `http://127.0.0.1:${demoBackendPort}/api/health`,
              reuseExistingServer: !process.env.CI,
              timeout: 30_000,
              stdout: "pipe",
              stderr: "pipe",
            },
          ],
});
