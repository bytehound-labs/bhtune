import {
  expect,
  test,
  type APIResponse,
  type Browser,
  type BrowserContext,
  type Download,
  type Page,
  type TestInfo,
} from "@playwright/test";

const DEMO_COOKIE_NAME = "__Host-bhtune_demo_session";

type DemoCapabilities = {
  readonly mode: "demo";
  readonly demo: true;
  readonly actions: {
    readonly browse_opc: boolean;
    readonly edit_notes: boolean;
    readonly manage_config: boolean;
    readonly manage_templates: boolean;
    readonly start_opcda_tune: boolean;
    readonly start_simulator_tune: boolean;
    readonly write_pid: boolean;
  };
  readonly quotas: {
    readonly accepted_starts_per_client_ip: number;
    readonly retained_runs_per_visitor: number;
  };
  readonly security: {
    readonly allowed_origin: string;
    readonly exact_origin_required_for_mutations: boolean;
    readonly forwarded_client_ip_header: string | null;
    readonly https_required: boolean;
    readonly trusted_proxy_configured: boolean;
    readonly cookie: {
      readonly name: string;
      readonly path: string;
      readonly max_age_secs: number;
      readonly http_only: boolean;
      readonly secure: boolean;
      readonly same_site: string;
    };
  };
};

type RunListResponse = {
  readonly runs: ReadonlyArray<{ readonly id: number }>;
  readonly returned: number;
  readonly total: number;
};

type SseRecorder = {
  source: EventSource;
  initial: number;
  samples: number[];
  done: string[];
  errors: number;
};

type DemoWindow = Window & {
  __bhtuneDemoSse?: SseRecorder;
};

type DemoTuneValues = {
  readonly template?: string;
  readonly processType?: string;
  readonly controllerType?: string;
  readonly relayAmp?: string;
  readonly cyclesSkip?: string;
  readonly cyclesCount?: string;
  readonly noiseProtection?: string;
  readonly gain?: string;
  readonly tau?: string;
  readonly deadTime?: string;
  readonly seed?: string;
};

function demoOrigin(testInfo: TestInfo): string {
  const baseURL = testInfo.project.use.baseURL;
  if (typeof baseURL !== "string") {
    throw new Error("The Demo Playwright project must define a baseURL.");
  }
  return baseURL;
}

async function newDemoContext(
  browser: Browser,
  baseURL: string,
  extraHTTPHeaders?: Record<string, string>,
) {
  const context = await browser.newContext({
    baseURL,
    ignoreHTTPSErrors: true,
    extraHTTPHeaders,
  });
  const response = await context.request.get("/api/capabilities");
  expect(response.status()).toBe(200);
  const capabilities = (await response.json()) as DemoCapabilities;
  expect(capabilities.mode).toBe("demo");
  expect(capabilities.demo).toBe(true);
  return { context, response, capabilities };
}

function expectSecurityHeaders(response: APIResponse) {
  const headers = response.headers();
  expect(headers["cache-control"]).toBe("no-store");
  expect(headers.vary?.split(",").map((value) => value.trim())).toContain(
    "Cookie",
  );
  expect(headers["x-robots-tag"]).toBe("noindex, nofollow, noarchive");
  expect(headers["x-frame-options"]).toBe("DENY");
  expect(headers["x-content-type-options"]).toBe("nosniff");
  expect(headers["referrer-policy"]).toBe("no-referrer");
  expect(headers["cross-origin-resource-policy"]).toBe("same-origin");
  expect(headers["cross-origin-opener-policy"]).toBe("same-origin");
  expect(headers["permissions-policy"]).toContain("camera=()");
  expect(headers["access-control-allow-origin"]).toBeUndefined();
  expect(headers["access-control-allow-credentials"]).toBeUndefined();

  const csp = headers["content-security-policy"];
  for (const directive of [
    "default-src 'self'",
    "script-src 'self'",
    "style-src 'self'",
    "style-src-attr 'unsafe-inline'",
    "connect-src 'self'",
    "object-src 'none'",
    "base-uri 'self'",
    "frame-ancestors 'none'",
    "form-action 'self'",
  ]) {
    expect(csp).toContain(directive);
  }
}

function outcomeBadge(page: Page, outcome: "Completed" | "Aborted") {
  return page.locator("dd").filter({ hasText: new RegExp(`^${outcome}$`) });
}

async function prepareDemoTune(page: Page, values: DemoTuneValues = {}) {
  await page.goto("/runs/new");
  await expect(
    page.getByRole("heading", { name: "BHTune Simulator Demo" }),
  ).toBeVisible();

  if (values.template) {
    await page.getByLabel("Template").selectOption(values.template);
  }
  if (values.processType) {
    await page
      .getByRole("combobox", { name: "Process type", exact: true })
      .selectOption(values.processType);
  }
  if (values.controllerType) {
    await page
      .getByRole("combobox", { name: "Controller type", exact: true })
      .selectOption(values.controllerType);
  }

  const fields: ReadonlyArray<[string, string | undefined]> = [
    ["Relay amplitude (%)", values.relayAmp],
    ["Cycles to skip", values.cyclesSkip],
    ["Cycles to count", values.cyclesCount],
    ["Noise protection (s)", values.noiseProtection],
    ["Process gain", values.gain],
    ["Time constant τ (s)", values.tau],
    ["Dead time (s)", values.deadTime],
    ["RNG seed", values.seed],
  ];
  for (const [label, value] of fields) {
    if (value !== undefined) {
      await page.getByLabel(label).fill(value);
    }
  }
}

async function startPreparedTune(page: Page): Promise<number> {
  await Promise.all([
    page.waitForURL(/\/runs\/\d+$/),
    page.getByRole("button", { name: "Start tune" }).click(),
  ]);
  const runId = Number(page.url().match(/\/runs\/(\d+)$/)?.[1]);
  expect(Number.isSafeInteger(runId)).toBe(true);
  return runId;
}

async function listRuns(context: BrowserContext): Promise<RunListResponse> {
  const response = await context.request.get("/api/runs");
  expect(response.status()).toBe(200);
  return (await response.json()) as RunListResponse;
}

async function expectCrossSessionNotFound(
  context: BrowserContext,
  runId: number,
  origin: string,
) {
  const responses = await Promise.all([
    context.request.get(`/api/runs/${runId}`),
    context.request.get(`/api/runs/${runId}/stream`),
    context.request.post(`/api/runs/${runId}/cancel`, {
      headers: { Origin: origin },
    }),
    context.request.get(`/api/runs/${runId}/export?format=json`),
    context.request.delete(`/api/runs/${runId}`, {
      headers: { Origin: origin },
    }),
  ]);
  expect(responses.map((response) => response.status())).toEqual([
    404, 404, 404, 404, 404,
  ]);
}

async function startSseRecorder(page: Page, runId: number) {
  await page.evaluate((id) => {
    const demoWindow = window as DemoWindow;
    const recorder: SseRecorder = {
      source: new EventSource(`/api/runs/${id}/stream`, {
        withCredentials: true,
      }),
      initial: 0,
      samples: [],
      done: [],
      errors: 0,
    };
    demoWindow.__bhtuneDemoSse = recorder;
    recorder.source.addEventListener("initial", () => {
      recorder.initial += 1;
    });
    recorder.source.addEventListener("sample", (event) => {
      const data = JSON.parse((event as MessageEvent<string>).data) as {
        tick_index: number;
      };
      recorder.samples.push(data.tick_index);
    });
    recorder.source.addEventListener("done", (event) => {
      const data = JSON.parse((event as MessageEvent<string>).data) as {
        outcome: string;
      };
      recorder.done.push(data.outcome);
      recorder.source.close();
    });
    recorder.source.addEventListener("error", () => {
      recorder.errors += 1;
    });
  }, runId);
}

async function sseSnapshot(page: Page) {
  return page.evaluate(() => {
    const recorder = (window as DemoWindow).__bhtuneDemoSse;
    return recorder
      ? {
          initial: recorder.initial,
          samples: [...recorder.samples],
          done: [...recorder.done],
          errors: recorder.errors,
        }
      : { initial: 0, samples: [], done: [], errors: 0 };
  });
}

async function downloadText(download: Download): Promise<string> {
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return Buffer.concat(chunks).toString("utf8");
}

async function discardRun(
  context: BrowserContext,
  runId: number | undefined,
  origin: string,
) {
  if (runId === undefined) return;
  try {
    await context.request.post(`/api/runs/${runId}/cancel`, {
      headers: { Origin: origin },
    });
    for (let attempt = 0; attempt < 30; attempt += 1) {
      const detail = await context.request.get(`/api/runs/${runId}`);
      if (detail.status() === 404) return;
      if (detail.ok()) {
        const run = (await detail.json()) as { outcome: string };
        if (run.outcome !== "running") break;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    await context.request.delete(`/api/runs/${runId}`, {
      headers: { Origin: origin },
    });
  } catch {
    // Best-effort cleanup must not hide the test's original failure.
  }
}

test.describe("real HTTPS Demo mode", () => {
  test("issues isolated secure sessions and exposes only the hardened Demo surface", async ({
    browser,
  }, testInfo) => {
    const origin = demoOrigin(testInfo);
    const first = await newDemoContext(browser, origin);
    const second = await newDemoContext(browser, origin);

    try {
      expectSecurityHeaders(first.response);
      const documentResponse = await first.context.request.get("/");
      expect(documentResponse.status()).toBe(200);
      expectSecurityHeaders(documentResponse);

      expect(first.capabilities.actions).toMatchObject({
        browse_opc: false,
        edit_notes: false,
        manage_config: false,
        manage_templates: false,
        start_opcda_tune: false,
        start_simulator_tune: true,
        write_pid: false,
      });
      expect(first.capabilities.security).toMatchObject({
        allowed_origin: origin,
        exact_origin_required_for_mutations: true,
        forwarded_client_ip_header: "X-BHTune-Client-IP",
        https_required: true,
        trusted_proxy_configured: true,
        cookie: {
          name: DEMO_COOKIE_NAME,
          path: "/",
          max_age_secs: 86_400,
          http_only: true,
          secure: true,
          same_site: "Strict",
        },
      });
      expect(first.capabilities.quotas.accepted_starts_per_client_ip).toBe(6);

      const setCookie = first.response
        .headersArray()
        .find((header) => header.name.toLowerCase() === "set-cookie")?.value;
      expect(setCookie).toMatch(
        /^__Host-bhtune_demo_session=[0-9a-f]{64}; Path=\/; Max-Age=86400; HttpOnly; SameSite=Strict; Secure$/,
      );

      const firstCookie = (await first.context.cookies(origin)).find(
        (cookie) => cookie.name === DEMO_COOKIE_NAME,
      );
      const secondCookie = (await second.context.cookies(origin)).find(
        (cookie) => cookie.name === DEMO_COOKIE_NAME,
      );
      expect(firstCookie).toMatchObject({
        httpOnly: true,
        path: "/",
        sameSite: "Strict",
        secure: true,
      });
      expect(secondCookie).toMatchObject({
        httpOnly: true,
        path: "/",
        sameSite: "Strict",
        secure: true,
      });
      expect(firstCookie?.value).toMatch(/^[0-9a-f]{64}$/);
      expect(secondCookie?.value).toMatch(/^[0-9a-f]{64}$/);
      expect(firstCookie?.value).not.toBe(secondCookie?.value);
      expect(firstCookie?.expires ?? 0).toBeGreaterThan(Date.now() / 1000);

      const missingOrigin = await first.context.request.post(
        "/api/runs/999999/cancel",
      );
      expect(missingOrigin.status()).toBe(403);
      expect(await missingOrigin.json()).toEqual({
        error: "cross-origin request rejected",
      });
      expectSecurityHeaders(missingOrigin);

      for (const rejectedOrigin of [
        `${origin}/`,
        origin.replace("https://", "http://"),
        `${origin}.attacker.invalid`,
      ]) {
        const response = await first.context.request.post(
          "/api/runs/999999/cancel",
          { headers: { Origin: rejectedOrigin } },
        );
        expect(response.status()).toBe(403);
      }
      const exactOrigin = await first.context.request.post(
        "/api/runs/999999/cancel",
        { headers: { Origin: origin } },
      );
      expect(exactOrigin.status()).toBe(404);

      for (const path of [
        "/api/openapi.json",
        "/api/docs",
        "/api/config",
        "/api/opc/servers",
      ]) {
        const response = await first.context.request.get(path);
        expect(response.status(), path).toBe(404);
        expect(await response.json()).toEqual({
          error: "API route is not available in Demo mode",
        });
      }
      // The dynamic `/api/runs/{id}` route owns this path before the Demo catch-all,
      // so Axum rejects the non-numeric id. It still proves the Full-only draft API is
      // unavailable and, importantly, does not return another visitor's persisted state.
      expect(
        (await first.context.request.get("/api/runs/draft")).status(),
      ).toBe(400);
      const forbiddenMutation = await first.context.request.post(
        "/api/runs/999999/write",
        {
          headers: { Origin: origin },
          data: { response_level: "moderate" },
        },
      );
      expect(forbiddenMutation.status()).toBe(404);

      const page = await first.context.newPage();
      await page.goto("/templates");
      await expect(page).toHaveURL(/\/runs\/new$/);
      await expect(
        page.getByText("Demo mode — Simulator only.", { exact: false }),
      ).toBeVisible();
      await expect(page.getByRole("link", { name: "Tune" })).toBeVisible();
      await expect(page.getByRole("link", { name: "History" })).toBeVisible();
      await expect(page.getByRole("link", { name: "Templates" })).toHaveCount(
        0,
      );
      await expect(page.getByRole("link", { name: "Config" })).toHaveCount(0);
      await expect(page.getByLabel("OPC DA server ProgID")).toHaveCount(0);
      await expect(page.getByLabel("Notes")).toHaveCount(0);
      await expect(page.getByText("Loop mapping")).toHaveCount(0);
      await expect(page.getByText("Automatic PID settings")).toHaveCount(0);
      await expect(
        page.getByRole("button", { name: "Reset to defaults" }),
      ).toHaveCount(0);
    } finally {
      await first.context.close();
      await second.context.close();
    }
  });

  test("runs two visitors concurrently with private histories, cross-session 404s, cancellation, and incremental SSE", async ({
    browser,
  }, testInfo) => {
    test.setTimeout(45_000);
    const origin = demoOrigin(testInfo);
    const first = await newDemoContext(browser, origin, {
      "X-BHTune-Client-IP": "203.0.113.10",
      "X-Forwarded-For": "203.0.113.11",
      "X-Real-IP": "203.0.113.12",
    });
    const second = await newDemoContext(browser, origin, {
      "X-BHTune-Client-IP": "198.51.100.20",
      "X-Forwarded-For": "198.51.100.21",
      "X-Real-IP": "198.51.100.22",
    });
    const firstPage = await first.context.newPage();
    const secondPage = await second.context.newPage();
    let firstRunId: number | undefined;
    let secondRunId: number | undefined;

    try {
      await Promise.all([
        prepareDemoTune(firstPage, {
          cyclesSkip: "0",
          cyclesCount: "3",
          tau: "5",
          deadTime: "2",
          seed: "101",
        }),
        prepareDemoTune(secondPage, {
          cyclesSkip: "0",
          cyclesCount: "1",
          tau: "0.5",
          deadTime: "0.2",
          seed: "202",
        }),
      ]);

      [firstRunId, secondRunId] = await Promise.all([
        startPreparedTune(firstPage),
        startPreparedTune(secondPage),
      ]);
      expect(firstRunId).not.toBe(secondRunId);

      await startSseRecorder(secondPage, secondRunId);
      await expect
        .poll(async () => (await sseSnapshot(secondPage)).samples.length, {
          timeout: 10_000,
        })
        .toBeGreaterThan(0);
      const firstSampleCount = (await sseSnapshot(secondPage)).samples.length;

      await expectCrossSessionNotFound(second.context, firstRunId, origin);
      await expectCrossSessionNotFound(first.context, secondRunId, origin);

      const [firstHistory, secondHistory] = await Promise.all([
        listRuns(first.context),
        listRuns(second.context),
      ]);
      expect(firstHistory).toMatchObject({ returned: 1, total: 1 });
      expect(secondHistory).toMatchObject({ returned: 1, total: 1 });
      expect(firstHistory.runs.map((run) => run.id)).toEqual([firstRunId]);
      expect(secondHistory.runs.map((run) => run.id)).toEqual([secondRunId]);

      await expect
        .poll(async () => (await sseSnapshot(secondPage)).samples.length, {
          timeout: 10_000,
        })
        .toBeGreaterThan(firstSampleCount);

      await firstPage.getByRole("button", { name: "Cancel tune" }).click();
      await expect(outcomeBadge(firstPage, "Aborted")).toBeVisible({
        timeout: 30_000,
      });
      await expect(outcomeBadge(secondPage, "Completed")).toBeVisible({
        timeout: 30_000,
      });
      await expect
        .poll(async () => (await sseSnapshot(secondPage)).done, {
          timeout: 5_000,
        })
        .toEqual(["completed"]);

      const stream = await sseSnapshot(secondPage);
      expect(stream.initial).toBe(1);
      expect(stream.samples.length).toBeGreaterThan(firstSampleCount);
      expect(stream.samples).toEqual(
        [...stream.samples].sort((left, right) => left - right),
      );
      expect(new Set(stream.samples).size).toBe(stream.samples.length);
      expect(stream.done).toEqual(["completed"]);
      expect(stream.errors).toBe(0);

      await Promise.all([firstPage.goto("/runs"), secondPage.goto("/runs")]);
      await expect(
        firstPage.getByRole("link", { name: `#${firstRunId}` }),
      ).toBeVisible();
      await expect(
        firstPage.getByRole("link", { name: `#${secondRunId}` }),
      ).toHaveCount(0);
      await expect(
        secondPage.getByRole("link", { name: `#${secondRunId}` }),
      ).toBeVisible();
      await expect(
        secondPage.getByRole("link", { name: `#${firstRunId}` }),
      ).toHaveCount(0);
    } finally {
      await discardRun(first.context, firstRunId, origin);
      await discardRun(second.context, secondRunId, origin);
      await first.context.close();
      await second.context.close();
    }
  });

  test("exports, duplicates, and deletes a completed synthetic run", async ({
    browser,
  }, testInfo) => {
    test.setTimeout(45_000);
    const origin = demoOrigin(testInfo);
    const demo = await newDemoContext(browser, origin);
    const page = await demo.context.newPage();
    let runId: number | undefined;

    try {
      await prepareDemoTune(page, {
        template: "Honeywell Experion",
        processType: "temperature_mixing",
        controllerType: "pid",
        relayAmp: "8",
        cyclesSkip: "0",
        cyclesCount: "1",
        gain: "1.5",
        tau: "0.1",
        deadTime: "0.25",
        seed: "303",
      });
      runId = await startPreparedTune(page);
      await expect(outcomeBadge(page, "Completed")).toBeVisible({
        timeout: 30_000,
      });

      await expect(
        page.getByRole("button", { name: "Review & write" }),
      ).toHaveCount(0);
      await expect(
        page.getByRole("heading", { name: "Notes", exact: true }),
      ).toHaveCount(0);
      await expect(
        page.getByRole("heading", {
          name: "PID change history",
          exact: true,
        }),
      ).toHaveCount(0);

      const [csvDownload] = await Promise.all([
        page.waitForEvent("download"),
        page.getByRole("link", { name: "Export CSV" }).click(),
      ]);
      expect(csvDownload.suggestedFilename()).toBe(`demo-run-${runId}.csv`);
      const csv = await downloadText(csvDownload);
      expect(csv.split(/\r?\n/, 1)[0]).toBe(
        "tick,time,pv,pv_quality,hysteresis,mv_value_current,mv_sign_next_step,counter_all_switches,cycles_completed,cycles_remaining",
      );

      const [jsonDownload] = await Promise.all([
        page.waitForEvent("download"),
        page.getByRole("link", { name: "Export JSON" }).click(),
      ]);
      expect(jsonDownload.suggestedFilename()).toBe(`demo-run-${runId}.json`);
      const exportedSamples = JSON.parse(
        await downloadText(jsonDownload),
      ) as unknown[];
      expect(exportedSamples.length).toBeGreaterThan(0);

      await page.getByRole("button", { name: "Duplicate this run" }).click();
      await expect(page).toHaveURL(/\/runs\/new$/);
      await expect(page.getByLabel("Template")).toHaveValue(
        "Honeywell Experion",
      );
      await expect(
        page.getByRole("combobox", { name: "Process type", exact: true }),
      ).toHaveValue("temperature_mixing");
      await expect(
        page.getByRole("combobox", { name: "Controller type", exact: true }),
      ).toHaveValue("pid");
      await expect(page.getByLabel("Relay amplitude (%)")).toHaveValue("8");
      await expect(page.getByLabel("Cycles to skip")).toHaveValue("0");
      await expect(page.getByLabel("Cycles to count")).toHaveValue("1");
      await expect(page.getByLabel("Process gain")).toHaveValue("1.5");
      await expect(page.getByLabel("Time constant τ (s)")).toHaveValue("0.1");
      await expect(page.getByLabel("Dead time (s)")).toHaveValue("0.25");
      await expect(page.getByLabel("RNG seed")).toHaveValue("303");

      await page.goto(`/runs/${runId}`);
      page.once("dialog", (dialog) => void dialog.accept());
      await page.getByRole("button", { name: "Delete tune" }).click();
      await expect(page).toHaveURL(/\/runs$/);
      await expect(
        page.getByText("No tunes match this filter.", { exact: true }),
      ).toBeVisible();
      expect(
        (await demo.context.request.get(`/api/runs/${runId}`)).status(),
      ).toBe(404);
      runId = undefined;
    } finally {
      await discardRun(demo.context, runId, origin);
      await demo.context.close();
    }
  });
});
