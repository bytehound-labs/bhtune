import {
  expect,
  test,
  type BrowserContext,
  type Route,
} from "@playwright/test";
import type { RunDetailResponse, StartRunRequest } from "../src/api/runs";

const compatibility = [
  { process_type: "flow", controller_types: ["p", "pi"] },
  { process_type: "pressure_line", controller_types: ["p", "pi"] },
  { process_type: "pressure_vessel", controller_types: ["p", "pi"] },
  { process_type: "level", controller_types: ["p", "pi"] },
  {
    process_type: "temperature_mixing",
    controller_types: ["p", "pi", "pid"],
  },
  {
    process_type: "temperature_heat_exchange",
    controller_types: ["p", "pi", "pid"],
  },
];

const capabilities = {
  mode: "demo",
  demo: true,
  drivers: ["simulator"],
  actions: {
    browse_opc: false,
    cancel_run: true,
    delete_run: true,
    edit_notes: false,
    export_run: true,
    list_history: true,
    manage_config: false,
    manage_templates: false,
    revert_pid: false,
    start_opcda_tune: false,
    start_simulator_tune: true,
    stream_run: true,
    write_pid: false,
  },
  demo_policy: {
    session_ttl_secs: 86_400,
    poll_interval_ms: 50,
    run_timeout_secs: 30,
    max_active_runs_global: 8,
    max_active_runs_per_visitor: 1,
    accepted_starts_per_token: 6,
    accepted_starts_per_client_ip: 6,
    accepted_start_window_secs: 600,
    retained_runs_per_visitor: 10,
    max_tune_run_rows_global: 5000,
    max_json_body_bytes: 32_768,
    max_sse_per_visitor: 2,
    max_sse_global: 32,
    sse_lifetime_secs: 45,
    ordinary_request_concurrency: 64,
    ordinary_request_timeout_secs: 10,
    cleanup_interval_secs: 300,
  },
  simulator: {
    template: "Yokogawa CentumVP",
    templates: [
      "Yokogawa CentumVP",
      "Honeywell Experion",
      "Schneider Modicon",
      "Allen-Bradley PlantPAx",
    ],
    tag_name: "Simulator demo",
    process_types: compatibility.map((item) => item.process_type),
    controller_types: ["p", "pi", "pid"],
    compatibility,
    defaults: {
      tag_name: "Simulator demo",
      template: "Yokogawa CentumVP",
      direction: "reverse",
      pv_range: { min: 0, max: 100, absolute_min: null },
      mv_range: { min: 0, max: 100, absolute_min: null },
      poll_interval_ms: 50,
      run_timeout_secs: 30,
      relay_amp: 10,
      cycles_skip: 1,
      cycles_count: 2,
      noise_protection_secs: 0,
      sim_gain: 1,
      sim_tau: 0.1,
      sim_dead_time: 0.25,
      sim_noise: 0,
      sim_seed: 0,
      sim_initial_pv: 50,
      sim_initial_mv: 50,
    },
    limits: {
      relay_amp: { min: 1, max: 20, absolute_min: null },
      cycles_skip: { min: 0, max: 2 },
      cycles_count: { min: 1, max: 3 },
      noise_protection_secs: { min: 0, max: 3 },
      sim_gain: { min: -5, max: 5, absolute_min: 0.1 },
      sim_tau: { min: 0.05, max: 5, absolute_min: null },
      sim_dead_time: { min: 0, max: 2, absolute_min: null },
      sim_seed: { min: 0, max: 2_147_483_647 },
      range_endpoint: { min: -1000, max: 1000, absolute_min: null },
      range_span: { min: 1, max: 1000, absolute_min: null },
      max_noise_fraction_of_pv_span: 0.05,
    },
  },
  restrictions: {
    simulator_only: true,
    built_in_templates_only: true,
    fixed_tag_name: true,
    direction_must_match_process_gain: true,
    custom_tag_mappings_allowed: false,
    notes_allowed: false,
    automatic_pid_write_allowed: false,
    post_run_pid_write_allowed: false,
  },
  quotas: {
    max_active_runs_global: 8,
    max_active_runs_per_visitor: 1,
    accepted_starts_per_token: 6,
    accepted_starts_per_client_ip: 6,
    accepted_start_window_secs: 600,
    retained_runs_per_visitor: 10,
    max_tune_run_rows_global: 5000,
    max_json_body_bytes: 32_768,
    max_sse_per_visitor: 2,
    max_sse_global: 32,
    sse_lifetime_secs: 45,
    ordinary_request_concurrency: 64,
    ordinary_request_timeout_secs: 10,
  },
  security: {
    allowed_origin: "https://demo.example.test",
    exact_origin_required_for_mutations: true,
    https_required: true,
    loopback_http_allowed: true,
    trusted_proxy_configured: true,
    forwarded_client_ip_header: "X-BHTune-Client-IP",
    cookie: {
      name: "__Host-bhtune_demo_session",
      path: "/",
      max_age_secs: 86_400,
      http_only: true,
      secure: true,
      same_site: "Strict",
    },
  },
};

function json(route: Route, body: unknown, status = 200) {
  return route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
}

function demoRun(id: number, request: StartRunRequest): RunDetailResponse {
  return {
    id,
    tag_name: request.tagname,
    outcome: "completed",
    driver: "simulator",
    template_name: request.template,
    template_origin: "builtin",
    started_at: "2026-09-01T12:00:00Z",
    completed_at: "2026-09-01T12:00:01Z",
    allow_uncertain_quality: true,
    config: {
      process_type: request.process_type,
      controller_type: request.controller_type,
      relay_amp_percent: request.relay_amp,
      num_cycles_skip: request.cycles_skip ?? 1,
      num_cycles_count: request.cycles_count ?? 1,
      noise_protection_secs: request.noise_protection_secs ?? 0,
      mrft_delay_secs: 0,
    },
    initial_readings: null,
    pid_constant_tags: null,
    pid_parameter_labels: {
      proportional: "P",
      integral: "I",
      derivative: "D",
    },
    samples: [],
    mv_actuations: [],
    results: [],
    writes: [],
    original_request: request,
  };
}

type DemoApi = {
  readonly starts: StartRunRequest[];
  readonly unexpectedPaths: string[];
};

async function installDemoApi(
  context: BrowserContext,
  firstRunId: number,
  startStatus?: 403 | 429 | 503,
): Promise<DemoApi> {
  const starts: StartRunRequest[] = [];
  const runs: RunDetailResponse[] = [];
  const unexpectedPaths: string[] = [];

  await context.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());

    if (url.pathname === "/api/capabilities") {
      await json(route, capabilities);
      return;
    }
    if (url.pathname === "/api/health") {
      await json(route, { status: "ok", version: "0.1.0" });
      return;
    }
    if (url.pathname === "/api/runs") {
      if (request.method() === "GET") {
        await json(route, {
          runs: runs.map((run) => ({
            id: run.id,
            tag_name: run.tag_name,
            process_type: run.config.process_type,
            controller_type: run.config.controller_type,
            outcome: run.outcome,
            driver: run.driver,
            started_at: run.started_at,
            completed_at: run.completed_at,
          })),
          returned: runs.length,
          total: runs.length,
        });
        return;
      }
      if (request.method() === "POST") {
        const body = request.postDataJSON() as StartRunRequest;
        starts.push(body);
        if (startStatus) {
          await json(
            route,
            { error: "demo capacity unavailable" },
            startStatus,
          );
          return;
        }
        const run = demoRun(firstRunId + runs.length, body);
        runs.push(run);
        await json(route, run, 201);
        return;
      }
    }

    const runMatch = url.pathname.match(/^\/api\/runs\/(\d+)$/);
    if (runMatch && request.method() === "GET") {
      const run = runs.find(
        (candidate) => candidate.id === Number(runMatch[1]),
      );
      await json(
        route,
        run ?? { error: `no demo run with id ${runMatch[1]}` },
        run ? 200 : 404,
      );
      return;
    }

    unexpectedPaths.push(`${request.method()} ${url.pathname}`);
    await json(route, { error: "unexpected test request" }, 404);
  });

  return { starts, unexpectedPaths };
}

async function seedDemoDraft(
  page: import("@playwright/test").Page,
  draft: unknown,
  savedAt = Date.now(),
) {
  await page.addInitScript(
    ({ draft: initialDraft, savedAt: initialSavedAt }) => {
      window.localStorage.setItem(
        "bhtune.demo.new-run-draft",
        JSON.stringify({
          saved_at: initialSavedAt,
          draft: initialDraft,
        }),
      );
    },
    { draft, savedAt },
  );
}

test.describe("Demo mode contract", () => {
  test("waits for capabilities and restricts routes", async ({ page }) => {
    let releaseCapabilities: (() => void) | undefined;
    const requests: string[] = [];

    await page.route("**/api/**", async (route) => {
      requests.push(
        `${route.request().method()} ${new URL(route.request().url()).pathname}`,
      );
      await json(route, { error: "unexpected request" }, 404);
    });
    await page.route("**/api/capabilities", async (route) => {
      await new Promise<void>((resolve) => {
        releaseCapabilities = resolve;
      });
      await json(route, capabilities);
    });
    await page.route("**/api/health", (route) =>
      json(route, { status: "ok", version: "0.1.0" }),
    );

    await page.goto("/templates");
    await expect(
      page.getByText("Loading BHTune capabilities…", { exact: true }),
    ).toBeVisible();
    expect(requests).toEqual([]);

    releaseCapabilities?.();
    await expect(page).toHaveURL(/\/runs\/new$/);
    await expect(
      page.getByRole("heading", { name: "BHTune Simulator Demo" }),
    ).toBeVisible();
    await expect(
      page.getByText(
        "Choose a built-in template and bounded simulator settings, then watch a synthetic MRFT tune.",
        { exact: true },
      ),
    ).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Demo tune settings" }),
    ).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Simulator parameters" }),
    ).toBeVisible();
    await expect(
      page.getByText("Demo mode — Simulator only.", { exact: false }),
    ).toBeVisible();
    await expect(page.getByRole("link", { name: "Tune" })).toBeVisible();
    await expect(page.getByRole("link", { name: "History" })).toBeVisible();
    await expect(page.getByRole("alert")).toContainText(
      "State-changing Demo actions are blocked at this address.",
    );
    await expect(
      page.getByRole("link", { name: "https://demo.example.test" }),
    ).toHaveAttribute("href", "https://demo.example.test");
    await expect(page.getByRole("link", { name: "Templates" })).toHaveCount(0);
    await expect(page.getByRole("link", { name: "Config" })).toHaveCount(0);
  });

  test("uses capability defaults and posts only normalized simulator inputs", async ({
    page,
  }) => {
    const api = await installDemoApi(page.context(), 1001);
    await page.goto("/runs/new");

    await expect(page.getByLabel("Template")).toHaveValue("Yokogawa CentumVP");
    await expect(page.getByLabel("Relay amplitude (%)")).toHaveValue("10");
    await expect(page.getByLabel("Cycles to skip")).toHaveValue("1");
    await expect(page.getByLabel("Cycles to count")).toHaveValue("2");
    await expect(page.getByLabel("Noise protection (s)")).toHaveValue("0");
    await expect(page.getByLabel("Process gain")).toHaveValue("1");
    await expect(page.getByLabel("Time constant τ (s)")).toHaveValue("0.1");
    await expect(page.getByLabel("Dead time (s)")).toHaveValue("0.25");
    await expect(page.getByLabel("Initial PV")).toHaveValue("50");
    await expect(page.getByLabel("Initial MV")).toHaveValue("50");
    await expect(page.getByLabel("OPC DA server ProgID")).toHaveCount(0);
    await expect(page.getByLabel("Notes")).toHaveCount(0);
    await expect(page.getByText("Loop mapping")).toHaveCount(0);
    await expect(page.getByText("Automatic PID settings")).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "Reset to defaults" }),
    ).toHaveCount(0);

    await page.getByLabel("Template").selectOption("Honeywell Experion");
    await page
      .getByRole("combobox", { name: "Process type", exact: true })
      .selectOption("temperature_mixing");
    await page
      .getByRole("combobox", { name: "Controller type", exact: true })
      .selectOption("pid");
    await page.getByLabel("Process gain").fill("-2.5");
    await page.getByRole("button", { name: "Start tune" }).click();
    await expect(page).toHaveURL(/\/runs\/1001$/);

    expect(Object.keys(api.starts[0]).sort()).toEqual(
      [
        "controller_type",
        "cycles_count",
        "cycles_skip",
        "direction",
        "driver",
        "mv_range_high",
        "mv_range_low",
        "noise_protection_secs",
        "process_type",
        "pv_range_high",
        "pv_range_low",
        "relay_amp",
        "sim_dead_time",
        "sim_gain",
        "sim_initial_mv",
        "sim_initial_pv",
        "sim_noise",
        "sim_seed",
        "sim_tau",
        "tagname",
        "template",
      ].sort(),
    );
    expect(api.starts[0]).toMatchObject({
      driver: "simulator",
      template: "Honeywell Experion",
      tagname: "Simulator demo",
      process_type: "temperature_mixing",
      controller_type: "pid",
      cycles_skip: 1,
      cycles_count: 2,
      noise_protection_secs: 0,
      direction: "direct",
      pv_range_high: 100,
      pv_range_low: 0,
      mv_range_high: 100,
      mv_range_low: 0,
      sim_gain: -2.5,
    });
    expect(api.unexpectedPaths).toEqual([]);

    await expect(
      page.getByText("Simulator demo", { exact: true }).first(),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Duplicate this run" }),
    ).toBeVisible();

    // A saved browser-local draft must not override the explicit duplicate action.
    await page.evaluate(() => {
      window.localStorage.setItem(
        "bhtune.demo.new-run-draft",
        JSON.stringify({
          saved_at: Date.now(),
          draft: {
            driver: "simulator",
            template: "Schneider Modicon",
            process_type: "flow",
            controller_type: "p",
            relay_amp: 2,
            cycles_skip: 0,
            cycles_count: 1,
            noise_protection_secs: 0,
            source_driver: "simulator",
            source_direction: "reverse",
            source_pv_range_high: 100,
            source_pv_range_low: 0,
            source_mv_range_high: 100,
            source_mv_range_low: 0,
            sim_gain: 1,
            sim_tau: 0.2,
            sim_dead_time: 0,
            sim_noise: 0,
            sim_seed: 0,
            sim_initial_pv: 50,
            sim_initial_mv: 50,
          },
        }),
      );
    });
    await page.getByRole("button", { name: "Duplicate this run" }).click();
    await expect(page).toHaveURL(/\/runs\/new$/);
    await expect(page.getByLabel("Template")).toHaveValue("Honeywell Experion");
    await expect(
      page.getByRole("combobox", { name: "Process type", exact: true }),
    ).toHaveValue("temperature_mixing");
    await expect(
      page.getByRole("combobox", { name: "Controller type", exact: true }),
    ).toHaveValue("pid");
    await expect(page.getByLabel("Process gain")).toHaveValue("-2.5");
    await expect(page.getByLabel("Time constant τ (s)")).toHaveValue("0.1");
    await expect(page.getByLabel("Dead time (s)")).toHaveValue("0.25");
    await expect(page.getByLabel("Initial PV")).toHaveValue("50");
    await expect(page.getByLabel("Initial MV")).toHaveValue("50");
    await expect(page.getByLabel("OPC DA server ProgID")).toHaveCount(0);
    await expect(page.getByLabel("Notes")).toHaveCount(0);
    await page.getByRole("button", { name: "Start tune" }).click();
    await expect(page).toHaveURL(/\/runs\/1002$/);
    expect(api.starts[1]).toMatchObject({
      driver: "simulator",
      template: "Honeywell Experion",
      tagname: "Simulator demo",
      process_type: "temperature_mixing",
      controller_type: "pid",
      direction: "direct",
      sim_gain: -2.5,
    });
    expect(Object.keys(api.starts[1]).sort()).toEqual(
      [
        "controller_type",
        "cycles_count",
        "cycles_skip",
        "direction",
        "driver",
        "mv_range_high",
        "mv_range_low",
        "noise_protection_secs",
        "process_type",
        "pv_range_high",
        "pv_range_low",
        "relay_amp",
        "sim_dead_time",
        "sim_gain",
        "sim_initial_mv",
        "sim_initial_pv",
        "sim_noise",
        "sim_seed",
        "sim_tau",
        "tagname",
        "template",
      ].sort(),
    );
    await page.getByRole("link", { name: "History", exact: true }).click();
    await expect(page.getByRole("link", { name: "#1001" })).toBeVisible();
    await expect(
      page.getByRole("link", { name: "Simulator demo" }),
    ).toHaveCount(2);
  });

  test("sanitizes invalid stored duplicate settings back to capability defaults", async ({
    page,
  }) => {
    const invalidRequest = {
      driver: "opcda",
      template: "not-a-template",
      tagname: "untrusted user text",
      process_type: "flow",
      controller_type: "pid",
      relay_amp: 999,
      cycles_skip: 99,
      cycles_count: 0,
      noise_protection_secs: -1,
      direction: "direct",
      pv_range_low: 900,
      pv_range_high: 900,
      mv_range_low: -2000,
      mv_range_high: 2000,
      sim_gain: 0,
      sim_tau: 999,
      sim_dead_time: -1,
      sim_noise: 999,
      sim_seed: -1,
      sim_initial_pv: 999,
      sim_initial_mv: -999,
      server: "forbidden-server",
      bridge_host: "forbidden-host",
      notes: "forbidden notes",
      write_pid: "aggressive",
      yes: true,
    } as unknown as StartRunRequest;
    const run = demoRun(4001, invalidRequest);

    await page.route("**/api/capabilities", (route) =>
      json(route, capabilities),
    );
    await page.route("**/api/health", (route) =>
      json(route, { status: "ok", version: "0.1.0" }),
    );
    await page.route("**/api/runs/4001", (route) => json(route, run));

    await page.goto("/runs/4001");
    await expect(
      page.getByRole("button", { name: "Duplicate this run" }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Duplicate this run" }).click();
    await expect(page).toHaveURL(/\/runs\/new$/);

    await expect(page.getByLabel("Template")).toHaveValue("Yokogawa CentumVP");
    await expect(
      page.getByRole("combobox", { name: "Process type", exact: true }),
    ).toHaveValue("flow");
    await expect(
      page.getByRole("combobox", { name: "Controller type", exact: true }),
    ).toHaveValue("p");
    await expect(page.getByLabel("Relay amplitude (%)")).toHaveValue("10");
    await expect(page.getByLabel("Cycles to skip")).toHaveValue("1");
    await expect(page.getByLabel("Cycles to count")).toHaveValue("2");
    await expect(page.getByLabel("Noise protection (s)")).toHaveValue("0");
    await expect(page.getByLabel("Process gain")).toHaveValue("1");
    await expect(page.getByLabel("Time constant τ (s)")).toHaveValue("0.1");
    await expect(page.getByLabel("Dead time (s)")).toHaveValue("0.25");
    await expect(page.getByLabel("Measurement noise")).toHaveValue("0");
    await expect(page.getByLabel("RNG seed")).toHaveValue("0");
    await expect(page.getByLabel("Initial PV")).toHaveValue("50");
    await expect(page.getByLabel("Initial MV")).toHaveValue("50");
    await expect(page.getByLabel("PV range low")).toHaveValue("0");
    await expect(page.getByLabel("PV range high")).toHaveValue("100");
    await expect(page.getByLabel("MV range low")).toHaveValue("0");
    await expect(page.getByLabel("MV range high")).toHaveValue("100");

    const api = await installDemoApi(page.context(), 4001);
    await page.getByRole("button", { name: "Start tune" }).click();
    await expect(page).toHaveURL(/\/runs\/4001$/);
    expect(api.starts[0]).toMatchObject({
      driver: "simulator",
      template: "Yokogawa CentumVP",
      tagname: "Simulator demo",
      process_type: "flow",
      controller_type: "p",
      relay_amp: 10,
      cycles_skip: 1,
      cycles_count: 2,
      noise_protection_secs: 0,
      direction: "reverse",
      pv_range_low: 0,
      pv_range_high: 100,
      mv_range_low: 0,
      mv_range_high: 100,
      sim_gain: 1,
      sim_tau: 0.1,
      sim_dead_time: 0.25,
      sim_noise: 0,
      sim_seed: 0,
      sim_initial_pv: 50,
      sim_initial_mv: 50,
    });
  });

  test("hydrates valid Demo drafts from simulator-specific values", async ({
    page,
  }) => {
    const savedAt = Date.now() - 60_000;
    await seedDemoDraft(
      page,
      {
        driver: "opcda",
        source_driver: "simulator",
        template: "Allen-Bradley PlantPAx",
        process_type: "temperature_mixing",
        controller_type: "pid",
        relay_amp: 12,
        cycles_skip: 2,
        cycles_count: 3,
        noise_protection_secs: 2,
        source_direction: "reverse",
        source_pv_range_low: -20,
        source_pv_range_high: 80,
        source_mv_range_low: 10,
        source_mv_range_high: 90,
        sim_gain: -2,
        sim_tau: 1.25,
        sim_dead_time: 0.75,
        sim_noise: 2,
        sim_seed: 42,
        sim_initial_pv: 20,
        sim_initial_mv: 50,
        tagname: "untrusted tag",
        server: "untrusted server",
        bridge_host: "untrusted bridge",
        notes: "untrusted notes",
        write_pid: "aggressive",
        yes: true,
        direction: "reverse",
        pv_range_low: 0,
        pv_range_high: 100,
        mv_range_low: 0,
        mv_range_high: 100,
      },
      savedAt,
    );
    const api = await installDemoApi(page.context(), 5001);
    await page.goto("/runs/new");

    await expect(page.getByLabel("Template")).toHaveValue(
      "Allen-Bradley PlantPAx",
    );
    await expect(
      page.getByRole("combobox", { name: "Process type", exact: true }),
    ).toHaveValue("temperature_mixing");
    await expect(
      page.getByRole("combobox", { name: "Controller type", exact: true }),
    ).toHaveValue("pid");
    await expect(page.getByLabel("Relay amplitude (%)")).toHaveValue("12");
    await expect(page.getByLabel("Cycles to skip")).toHaveValue("2");
    await expect(page.getByLabel("Cycles to count")).toHaveValue("3");
    await expect(page.getByLabel("Noise protection (s)")).toHaveValue("2");
    await expect(page.getByLabel("Process gain")).toHaveValue("-2");
    await expect(page.getByLabel("Time constant τ (s)")).toHaveValue("1.25");
    await expect(page.getByLabel("Dead time (s)")).toHaveValue("0.75");
    await expect(page.getByLabel("Measurement noise")).toHaveValue("2");
    await expect(page.getByLabel("RNG seed")).toHaveValue("42");
    await expect(page.getByLabel("Initial PV")).toHaveValue("20");
    await expect(page.getByLabel("Initial MV")).toHaveValue("50");
    await expect(page.getByLabel("PV range low")).toHaveValue("-20");
    await expect(page.getByLabel("PV range high")).toHaveValue("80");
    await expect(page.getByLabel("MV range low")).toHaveValue("10");
    await expect(page.getByLabel("MV range high")).toHaveValue("90");
    await expect
      .poll(() =>
        page.evaluate(() => {
          const stored = JSON.parse(
            window.localStorage.getItem("bhtune.demo.new-run-draft") ?? "null",
          );
          return stored;
        }),
      )
      .toMatchObject({
        saved_at: savedAt,
        draft: {
          template: "Allen-Bradley PlantPAx",
          process_type: "temperature_mixing",
          controller_type: "pid",
          sim_gain: -2,
        },
      });
    const sanitizedDraft = await page.evaluate(() => {
      const stored = JSON.parse(
        window.localStorage.getItem("bhtune.demo.new-run-draft") ?? "null",
      );
      return stored?.draft;
    });
    for (const forbidden of [
      "driver",
      "source_driver",
      "source_direction",
      "tagname",
      "server",
      "bridge_host",
      "notes",
      "write_pid",
      "yes",
      "tag_overrides",
    ]) {
      expect(sanitizedDraft).not.toHaveProperty(forbidden);
    }

    await page.getByRole("button", { name: "Start tune" }).click();
    await expect(page).toHaveURL(/\/runs\/5001$/);
    expect(api.starts[0]).toMatchObject({
      driver: "simulator",
      template: "Allen-Bradley PlantPAx",
      tagname: "Simulator demo",
      process_type: "temperature_mixing",
      controller_type: "pid",
      relay_amp: 12,
      cycles_skip: 2,
      cycles_count: 3,
      noise_protection_secs: 2,
      direction: "direct",
      pv_range_low: -20,
      pv_range_high: 80,
      mv_range_low: 10,
      mv_range_high: 90,
      sim_gain: -2,
      sim_tau: 1.25,
      sim_dead_time: 0.75,
      sim_noise: 2,
      sim_seed: 42,
      sim_initial_pv: 20,
      sim_initial_mv: 50,
    });
    expect(api.starts[0]).not.toHaveProperty("server");
    expect(api.starts[0]).not.toHaveProperty("bridge_host");
    expect(api.starts[0]).not.toHaveProperty("notes");
    expect(api.unexpectedPaths).toEqual([]);
  });

  test("replaces partial, stale, and legacy Full-mode drafts with Demo-safe values", async ({
    page,
  }) => {
    await seedDemoDraft(page, {
      driver: "opcda",
      template: "not-a-template",
      process_type: "not-a-process",
      controller_type: "pid",
      relay_amp: 999,
      cycles_skip: 99,
      cycles_count: 0,
      noise_protection_secs: 20,
      direction: "direct",
      pv_range_low: 900,
      pv_range_high: 900,
      mv_range_low: -2000,
      mv_range_high: 2000,
      sim_gain: 0,
      sim_tau: 2,
      sim_dead_time: 5,
      sim_noise: 999,
      sim_seed: -1,
      sim_initial_pv: 999,
      sim_initial_mv: -999,
      server: "stale server",
      bridge_host: "stale bridge",
      tagname: "stale tag",
      notes: "stale notes",
      write_pid: "aggressive",
      yes: true,
    });
    const api = await installDemoApi(page.context(), 6001);
    await page.goto("/runs/new");

    await expect(page.getByLabel("Template")).toHaveValue("Yokogawa CentumVP");
    await expect(
      page.getByRole("combobox", { name: "Process type", exact: true }),
    ).toHaveValue("flow");
    await expect(
      page.getByRole("combobox", { name: "Controller type", exact: true }),
    ).toHaveValue("p");
    await expect(page.getByLabel("Relay amplitude (%)")).toHaveValue("10");
    await expect(page.getByLabel("Cycles to skip")).toHaveValue("1");
    await expect(page.getByLabel("Cycles to count")).toHaveValue("2");
    await expect(page.getByLabel("Noise protection (s)")).toHaveValue("0");
    await expect(page.getByLabel("Process gain")).toHaveValue("1");
    await expect(page.getByLabel("Time constant τ (s)")).toHaveValue("0.1");
    await expect(page.getByLabel("Dead time (s)")).toHaveValue("0.25");
    await expect(page.getByLabel("Measurement noise")).toHaveValue("0");
    await expect(page.getByLabel("RNG seed")).toHaveValue("0");
    await expect(page.getByLabel("Initial PV")).toHaveValue("50");
    await expect(page.getByLabel("Initial MV")).toHaveValue("50");
    await expect(page.getByLabel("PV range low")).toHaveValue("0");
    await expect(page.getByLabel("PV range high")).toHaveValue("100");
    await expect(page.getByLabel("MV range low")).toHaveValue("0");
    await expect(page.getByLabel("MV range high")).toHaveValue("100");

    await page.getByRole("button", { name: "Start tune" }).click();
    await expect(page).toHaveURL(/\/runs\/6001$/);
    expect(api.starts[0]).toMatchObject({
      driver: "simulator",
      template: "Yokogawa CentumVP",
      tagname: "Simulator demo",
      process_type: "flow",
      controller_type: "p",
      relay_amp: 10,
      cycles_skip: 1,
      cycles_count: 2,
      noise_protection_secs: 0,
      direction: "reverse",
      pv_range_low: 0,
      pv_range_high: 100,
      mv_range_low: 0,
      mv_range_high: 100,
      sim_gain: 1,
      sim_tau: 0.1,
      sim_dead_time: 0.25,
      sim_noise: 0,
      sim_seed: 0,
      sim_initial_pv: 50,
      sim_initial_mv: 50,
    });
    expect(api.unexpectedPaths).toEqual([]);
  });

  test("ignores malformed and expired Demo draft storage", async ({ page }) => {
    await page.addInitScript(() => {
      if (!window.sessionStorage.getItem("seeded-malformed-demo-draft")) {
        window.sessionStorage.setItem("seeded-malformed-demo-draft", "true");
        window.localStorage.setItem(
          "bhtune.demo.new-run-draft",
          "{not valid JSON",
        );
      }
    });
    await installDemoApi(page.context(), 7001);
    await page.goto("/runs/new");

    await expect(page.getByLabel("Template")).toHaveValue("Yokogawa CentumVP");
    await expect(page.getByLabel("Time constant τ (s)")).toHaveValue("0.1");
    await expect(page.getByLabel("Dead time (s)")).toHaveValue("0.25");
    await expect(page.getByLabel("Noise protection (s)")).toHaveValue("0");
    expect(
      await page.evaluate(() =>
        window.localStorage.getItem("bhtune.demo.new-run-draft"),
      ),
    ).toBeNull();

    await page.evaluate(() => {
      window.localStorage.setItem(
        "bhtune.demo.new-run-draft",
        JSON.stringify({
          saved_at: Date.now() - 25 * 60 * 60 * 1000,
          draft: {
            template: "Honeywell Experion",
            sim_tau: 4,
            sim_dead_time: 1,
            noise_protection_secs: 3,
          },
        }),
      );
    });
    await page.reload();
    await expect(page.getByLabel("Template")).toHaveValue("Yokogawa CentumVP");
    await expect(page.getByLabel("Time constant τ (s)")).toHaveValue("0.1");
    await expect(page.getByLabel("Dead time (s)")).toHaveValue("0.25");
    await expect(page.getByLabel("Noise protection (s)")).toHaveValue("0");
    expect(
      await page.evaluate(() =>
        window.localStorage.getItem("bhtune.demo.new-run-draft"),
      ),
    ).toBeNull();
  });

  test("expires Demo draft storage while an open form remains usable", async ({
    page,
  }) => {
    const savedAt = new Date("2026-09-02T12:00:00Z");
    await page.clock.install({ time: savedAt });
    await seedDemoDraft(
      page,
      {
        template: "Honeywell Experion",
        process_type: "flow",
        controller_type: "pi",
        sim_tau: 4,
      },
      savedAt.getTime(),
    );
    await installDemoApi(page.context(), 7002);
    await page.goto("/runs/new");

    await expect(page.getByLabel("Template")).toHaveValue("Honeywell Experion");
    await expect(page.getByLabel("Time constant τ (s)")).toHaveValue("4");

    await page.clock.fastForward(24 * 60 * 60 * 1000);

    expect(
      await page.evaluate(() =>
        window.localStorage.getItem("bhtune.demo.new-run-draft"),
      ),
    ).toBeNull();
    await expect(page.getByLabel("Time constant τ (s)")).toHaveValue("4");
  });

  for (const status of [429, 503] as const) {
    test(`shows actionable retry guidance for HTTP ${status}`, async ({
      page,
    }) => {
      await installDemoApi(page.context(), 2001, status);
      await page.goto("/runs/new");
      await page.getByRole("button", { name: "Start tune" }).click();

      await expect(
        page.getByText(
          status === 429
            ? "Demo usage limit reached. Wait a few minutes before retrying; repeated attempts will not reset the limit."
            : "The demo service is temporarily unavailable. Wait a moment and retry; if it continues, come back later.",
          { exact: true },
        ),
      ).toBeVisible();
    });
  }

  test("explains an origin rejection instead of showing a generic start failure", async ({
    page,
  }) => {
    await installDemoApi(page.context(), 2501, 403);
    await page.goto("/runs/new");
    await page.getByRole("button", { name: "Start tune" }).click();

    await expect(
      page.getByText(
        "This Demo page is not using the configured browser URL. Open the Demo through its configured browser origin and try again.",
        { exact: true },
      ),
    ).toBeVisible();
  });

  test("rejects out-of-contract integer, range, and gain values before POST", async ({
    page,
  }) => {
    const api = await installDemoApi(page.context(), 3001);
    await page.goto("/runs/new");

    await page.getByLabel("Cycles to count").fill("1.5");
    await page.getByRole("button", { name: "Start tune" }).click();
    await expect(
      page.getByText("Cycles to count must be a whole number.", {
        exact: true,
      }),
    ).toBeVisible();

    await page.getByLabel("Cycles to count").fill("1");
    await page.getByLabel("PV range high").fill("0.5");
    await page.getByRole("button", { name: "Start tune" }).click();
    await expect(
      page.getByText("PV range span must be between 1 and 1000.", {
        exact: true,
      }),
    ).toBeVisible();

    await page.getByLabel("PV range high").fill("100");
    await page.getByLabel("Process gain").fill("0");
    await page.getByRole("button", { name: "Start tune" }).click();
    await expect(
      page.getByText(
        "Process gain must be between -5 and -0.1, or between 0.1 and 5.",
        { exact: true },
      ),
    ).toBeVisible();
    expect(api.starts).toEqual([]);
  });

  test("fails closed when Demo restrictions are incomplete", async ({
    page,
  }) => {
    const modeSensitiveRequests: string[] = [];
    await page.route("**/api/**", async (route) => {
      modeSensitiveRequests.push(new URL(route.request().url()).pathname);
      await json(route, { error: "unexpected request" }, 404);
    });
    await page.route("**/api/capabilities", (route) =>
      json(route, { ...capabilities, restrictions: undefined }),
    );

    await page.goto("/runs/new");
    await expect(
      page.getByText(
        "Unable to determine which BHTune features are available.",
        { exact: true },
      ),
    ).toBeVisible();
    expect(modeSensitiveRequests).toEqual([]);
  });

  test("keeps Full navigation and APIs available", async ({ page }) => {
    await page.route("**/api/capabilities", (route) =>
      json(route, {
        mode: "full",
        demo: false,
        drivers: ["opcda", "simulator"],
        actions: {
          ...capabilities.actions,
          browse_opc: true,
          edit_notes: true,
          manage_config: true,
          manage_templates: true,
          revert_pid: true,
          start_opcda_tune: true,
          write_pid: true,
        },
        demo_policy: null,
        simulator: null,
        restrictions: null,
        quotas: null,
        security: {
          allowed_origin: "",
          exact_origin_required_for_mutations: false,
          https_required: false,
          loopback_http_allowed: false,
          trusted_proxy_configured: false,
          forwarded_client_ip_header: null,
          cookie: null,
        },
      }),
    );
    await page.route("**/api/templates", (route) => json(route, []));
    await page.route("**/api/health", (route) =>
      json(route, { status: "ok", version: "0.1.0" }),
    );

    await page.goto("/templates");
    await expect(page).toHaveURL(/\/templates$/);
    await expect(page.getByRole("link", { name: "Templates" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Config" })).toBeVisible();
  });
});
