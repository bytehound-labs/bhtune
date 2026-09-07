import type { Page } from "@playwright/test";

const template = {
  name: "Yokogawa CentumVP",
  versions: ["R5", "R6"],
  description: "Yokogawa CentumVP tag conventions.",
  source: "CentumVP field mapping reference",
  process_variable_suffix: "PV",
  manipulated_variable_suffix: "OUT",
  setpoint_variable_suffix: "SP",
  upper_pv_range_suffix: "PV_HI",
  lower_pv_range_suffix: "PV_LO",
  upper_mv_range_suffix: "OUT_HI",
  lower_mv_range_suffix: "OUT_LO",
  controller_mode_suffix: "MODE",
  mode_manual_value: "MAN",
  mode_auto_value: "AUT",
  mode_attribute_suffix: "MODE_ATTR",
  mode_attribute_program_value: "PRG",
  controller_direction_suffix: "DIR",
  controller_action_direct_value: "1",
  proportional_constant_suffix: "P",
  integral_constant_suffix: "I",
  derivative_constant_suffix: "D",
  proportional_type: "gain",
  integral_type: "reset_time",
  derivative_type: "derivative_time",
  integral_unit: "minutes",
  derivative_unit: "minutes",
  revert_mode: true,
};

const userTemplate = {
  ...template,
  name: "Example Template",
  versions: ["1.0"],
  description: "A user-owned example template for documentation.",
  source: "Documentation fixture",
  origin: "user",
};

const templates = [
  { ...template, origin: "builtin" },
  {
    ...template,
    name: "Allen-Bradley PlantPAx",
    versions: ["3.0", "3.5", "4.0"],
    description: "Allen-Bradley PlantPAx tag conventions.",
    process_variable_suffix: "PV",
    origin: "builtin",
  },
  {
    ...template,
    name: "Honeywell Experion",
    versions: ["R400", "R410", "R430"],
    description: "Honeywell Experion tag conventions.",
    origin: "builtin",
  },
  userTemplate,
];

const capabilities = {
  mode: "full",
  demo: false,
  drivers: ["opcda", "simulator"],
  actions: {
    browse_opc: true,
    cancel_run: true,
    delete_run: true,
    edit_notes: true,
    export_run: true,
    list_history: true,
    manage_config: true,
    manage_templates: true,
    revert_pid: true,
    start_opcda_tune: true,
    start_simulator_tune: true,
    stream_run: true,
    write_pid: true,
  },
  security: {
    allowed_origin: "http://127.0.0.1:18787",
    exact_origin_required_for_mutations: false,
    https_required: false,
    loopback_http_allowed: true,
    trusted_proxy_configured: false,
    forwarded_client_ip_header: null,
    cookie: null,
  },
};

const demoCapabilities = {
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
  security: {
    allowed_origin: "https://127.0.0.1:18789",
    exact_origin_required_for_mutations: true,
    https_required: true,
    loopback_http_allowed: true,
    trusted_proxy_configured: false,
    forwarded_client_ip_header: null,
    cookie: {
      name: "__Host-bhtune_demo_session",
      path: "/",
      same_site: "Strict",
      max_age_secs: 86400,
      http_only: true,
      secure: true,
    },
  },
  restrictions: {
    simulator_only: true,
    built_in_templates_only: true,
    custom_tag_mappings_allowed: false,
    direction_must_match_process_gain: true,
    fixed_tag_name: true,
    notes_allowed: false,
    automatic_pid_write_allowed: false,
    post_run_pid_write_allowed: false,
  },
  quotas: {
    accepted_start_window_secs: 600,
    accepted_starts_per_client_ip: 6,
    accepted_starts_per_token: 6,
    max_active_runs_global: 8,
    max_active_runs_per_visitor: 1,
    max_runs_per_session: 10,
    max_json_body_bytes: 32768,
    max_sse_global: 32,
    max_sse_per_visitor: 2,
    max_tune_run_rows_global: 5000,
    ordinary_request_concurrency: 64,
    ordinary_request_timeout_secs: 10,
    retained_runs_per_visitor: 10,
    sse_lifetime_secs: 45,
  },
  demo_policy: {
    accepted_start_window_secs: 600,
    accepted_starts_per_client_ip: 6,
    accepted_starts_per_token: 6,
    cleanup_interval_secs: 300,
    max_active_runs_global: 8,
    max_active_runs_per_visitor: 1,
    max_runs_per_session: 10,
    max_json_body_bytes: 32768,
    max_sse_global: 32,
    max_sse_per_visitor: 2,
    max_tune_run_rows_global: 5000,
    ordinary_request_concurrency: 64,
    ordinary_request_timeout_secs: 10,
    poll_interval_ms: 200,
    retained_runs_per_visitor: 10,
    run_timeout_secs: 30,
    session_ttl_secs: 86400,
    sse_lifetime_secs: 45,
  },
  simulator: {
    template: "Yokogawa CentumVP",
    tag_name: "Simulator",
    templates: ["Yokogawa CentumVP"],
    process_types: ["flow", "pressure_line", "pressure_vessel", "level"],
    controller_types: ["p", "pi"],
    compatibility: [
      { process_type: "flow", controller_types: ["p", "pi"] },
      { process_type: "pressure_line", controller_types: ["p", "pi"] },
      { process_type: "pressure_vessel", controller_types: ["p", "pi"] },
      { process_type: "level", controller_types: ["p", "pi"] },
    ],
    defaults: {
      template: "Yokogawa CentumVP",
      tag_name: "Simulator",
      direction: "reverse",
      poll_interval_ms: 200,
      run_timeout_secs: 30,
      relay_amp: 10,
      cycles_skip: 1,
      cycles_count: 2,
      noise_protection_secs: 0,
      sim_gain: 1,
      sim_tau: 0.5,
      sim_dead_time: 1,
      sim_noise: 0,
      sim_seed: 0,
      sim_initial_pv: 50,
      sim_initial_mv: 50,
      pv_range: { min: 0, max: 100 },
      mv_range: { min: 0, max: 100 },
    },
    limits: {
      relay_amp: { min: 1, max: 20 },
      cycles_skip: { min: 0, max: 2 },
      cycles_count: { min: 1, max: 3 },
      noise_protection_secs: { min: 0, max: 3 },
      sim_gain: { min: -5, max: 5, absolute_min: 0.1 },
      sim_tau: { min: 0.05, max: 5 },
      sim_dead_time: { min: 0, max: 2 },
      sim_seed: { min: 0, max: 4294967295 },
      range_endpoint: { min: -1000, max: 1000 },
      range_span: { min: 1, max: 1000 },
      max_noise_fraction_of_pv_span: 0.05,
    },
  },
};

const config = {
  config_path: "/home/operator/.config/bhtune/bhtune.toml",
  backup_path: null,
  revision: "docs-fixture-revision",
  effective: {
    allow_uncertain_quality: true,
    retention_days: 90,
    tuning: {
      mrft_delay_secs: 5,
      poll_interval_ms: 800,
      timeout_secs: 3600,
      op_timeout_secs: 30,
      restore_timeout_secs: 30,
    },
  },
  source: {
    allow_uncertain_quality: "default",
    retention_days: "bhtune.toml",
    tuning: {
      mrft_delay_secs: "default",
      poll_interval_ms: "default",
      timeout_secs: "default",
      op_timeout_secs: "default",
      restore_timeout_secs: "default",
    },
  },
  toml: {
    allow_uncertain_quality: true,
    retention_days: 90,
    tuning: {
      mrft_delay_secs: null,
      poll_interval_ms: null,
      timeout_secs: null,
      op_timeout_secs: null,
      restore_timeout_secs: null,
    },
  },
};

const initialReadings = {
  controller_direction: "reverse",
  mode_attribute_raw: "PRG",
  mode_raw: "AUT",
  mv_ini: 50,
  mv_range_high: 100,
  mv_range_low: 0,
  pv_ini: 50,
  pv_range_high: 100,
  pv_range_low: 0,
  setpoint_ini: 50,
};

const configSnapshot = {
  controller_type: "pi",
  mrft_delay_secs: 5,
  noise_protection_secs: 0,
  num_cycles_count: 2,
  num_cycles_skip: 1,
  process_type: "flow",
  relay_amp_percent: 10,
};

const sample = (tickIndex: number, pv: number, mv: number) => ({
  tick_index: tickIndex,
  pv_quality: "good",
  sample: {
    pv,
    time: `2025-01-01T00:00:${String(tickIndex).padStart(2, "0")}.000Z`,
  },
  state: {
    counter_all_switches: tickIndex,
    cycles_completed: Math.floor(tickIndex / 2),
    cycles_remaining: Math.max(0, 2 - Math.floor(tickIndex / 2)),
    hysteresis: 1,
    mv_sign_next_step: tickIndex % 2 === 0 ? 1 : -1,
    mv_value_current: mv,
  },
});

const completedRun = {
  id: 4242,
  tag_name: "Area01.FIC101.PV",
  driver: "opcda",
  outcome: "completed",
  started_at: "2025-01-01T00:00:00.000Z",
  completed_at: "2025-01-01T00:02:00.000Z",
  config: configSnapshot,
  initial_readings: initialReadings,
  template_name: "Yokogawa CentumVP",
  template_origin: "builtin",
  allow_uncertain_quality: true,
  notes: "Review the moderate response before applying it to the loop.",
  opc_server: "Yokogawa.Example",
  bridge_host: "gateway.example:7600",
  pid_constant_tags: {
    proportional: "Area01.FIC101.P",
    integral: "Area01.FIC101.I",
    derivative: "Area01.FIC101.D",
  },
  pid_parameter_labels: {
    proportional: "Kp",
    integral: "Ti",
    derivative: "Td",
  },
  original_request: null,
  restore_status: "confirmed",
  restore_detail: "All original loop values restored and verified.",
  failure_reason: null,
  effective_tuning: {
    mrft_delay_secs: 5,
    poll_interval_ms: 800,
    timeout_secs: 3600,
    op_timeout_secs: 30,
    restore_timeout_secs: 30,
  },
  results: [
    {
      response_level: "aggressive",
      status: "valid",
      kp: 1.45,
      ti_minutes: 0.42,
      td_minutes: 0,
      proportional: 1.45,
      integral: 0.42,
      derivative: 0,
      invalid_reason: null,
    },
    {
      response_level: "moderate",
      status: "valid",
      kp: 1.12,
      ti_minutes: 0.58,
      td_minutes: 0,
      proportional: 1.12,
      integral: 0.58,
      derivative: 0,
      invalid_reason: null,
    },
    {
      response_level: "sluggish",
      status: "valid",
      kp: 0.84,
      ti_minutes: 0.82,
      td_minutes: 0,
      proportional: 0.84,
      integral: 0.82,
      derivative: 0,
      invalid_reason: null,
    },
  ],
  writes: [
    {
      id: 7,
      kind: "write",
      response_level: "moderate",
      written_at: "2025-01-01T00:03:00.000Z",
      success: true,
      error_message: null,
      proportional_written: 1.12,
      integral_written: 0.58,
      derivative_written: 0,
      proportional_readback: 1.12,
      integral_readback: 0.58,
      derivative_readback: 0,
      proportional_previous: 0.9,
      integral_previous: 0.7,
      derivative_previous: 0,
      rollback_state: null,
      rollback_error: null,
    },
  ],
  mv_actuations: [],
  timing_metrics: {
    basis: "live_monotonic",
    requested_interval_ms: 800,
    sample_gap_count: 14,
    mean_sample_gap_ms: 800,
    max_sample_gap_ms: 812,
    missed_poll_opportunity_count: 0,
    measured_oscillation_period_ms: 9600,
    approximate_samples_per_period: 12,
    sampling_adequacy: "adequate",
    poll_latency: {
      pv_read: { count: 15, mean_ms: 12, max_ms: 22 },
      mv_write: { count: 5, mean_ms: 9, max_ms: 15 },
      mv_verification: { count: 0, mean_ms: null, max_ms: null },
      sample_persist: { count: 15, mean_ms: 3, max_ms: 7 },
      tick_work: { count: 15, mean_ms: 18, max_ms: 31 },
    },
  },
  samples: [
    sample(0, 50, 50),
    sample(1, 51.2, 60),
    sample(2, 52.4, 40),
    sample(3, 53.1, 60),
    sample(4, 51.8, 40),
    sample(5, 50.4, 50),
  ],
};

const demoCompletedRun = {
  ...completedRun,
  driver: "simulator",
  tag_name: "Simulator",
  template_origin: "builtin",
  opc_server: null,
  bridge_host: null,
  pid_constant_tags: null,
  writes: [],
  mv_actuations: [],
  notes: null,
  original_request: null,
  timing_metrics: {
    ...completedRun.timing_metrics,
    basis: "simulated_fixed_step",
    requested_interval_ms: 200,
    mean_sample_gap_ms: 200,
    max_sample_gap_ms: 200,
  },
};

const liveRun = {
  ...completedRun,
  outcome: "running",
  completed_at: null,
  results: [],
  writes: [],
  restore_status: null,
  restore_detail: null,
  timing_metrics: null,
  samples: [sample(0, 50, 50), sample(1, 51, 60)],
};

const demoLiveRun = {
  ...demoCompletedRun,
  outcome: "running",
  completed_at: null,
  results: [],
  timing_metrics: null,
  samples: [sample(0, 50, 50), sample(1, 51, 60)],
};

type RunSummarySource = {
  id: number;
  tag_name: string;
  driver: string;
  outcome: string;
  config: { process_type: string };
  started_at: string;
  notes: string | null;
};

const runSummary = (run: RunSummarySource) => ({
  id: run.id,
  tag_name: run.tag_name,
  driver: run.driver,
  outcome: run.outcome,
  process_type: run.config.process_type,
  started_at: run.started_at,
  notes: run.notes,
});

const browseRoot = [
  { tag: "Area01", is_branch: true },
  { tag: "Area02", is_branch: true },
];

const browseArea = [
  { tag: "Area01.FIC101", is_branch: true },
  { tag: "Area01.PIC201", is_branch: true },
];

const browseLoop = [
  { tag: "Area01.FIC101.PV", is_branch: false },
  { tag: "Area01.FIC101.OUT", is_branch: false },
  { tag: "Area01.FIC101.SP", is_branch: false },
];

async function json(page: Page, pattern: string, value: unknown) {
  await page.route(pattern, (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(value),
    }),
  );
}

async function installCommonRoutes(page: Page, demo: boolean) {
  await json(page, "**/api/health", {
    status: "ok",
    version: "0.1.0-docs",
  });
  await json(
    page,
    "**/api/capabilities",
    demo ? demoCapabilities : capabilities,
  );
  await json(page, "**/api/templates", templates);
  await page.route("**/api/runs/draft", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(null),
    }),
  );
  await json(page, "**/api/runs/last-request", null);
  await page.route("**/api/templates/*", (route) => {
    const path = new URL(route.request().url()).pathname;
    const name = decodeURIComponent(path.slice(path.lastIndexOf("/") + 1));
    const selected =
      templates.find((item) => item.name === name) ??
      (name === "Example Template" ? userTemplate : template);
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(selected),
    });
  });
  await json(page, "**/api/runs/draft", null);
  await json(page, "**/api/runs/last-request", null);
  await json(page, "**/api/config", config);
  await json(page, "**/api/health", { status: "ok", version: "docs-fixture" });
  await json(page, "**/api/runs?*", {
    returned: 2,
    total: 2,
    runs: [
      runSummary(demo ? demoCompletedRun : completedRun),
      {
        ...runSummary(demo ? demoLiveRun : liveRun),
        id: 4243,
        tag_name: demo ? "Simulator" : "Area01.PIC201.PV",
        outcome: "running",
      },
    ],
  });
}

export async function installFullRoutes(page: Page) {
  await installCommonRoutes(page, false);
  await json(page, "**/api/opc/servers*", {
    servers: ["Yokogawa.Example", "Kepware.KEPServerEX.V6"],
  });
  await page.route("**/api/opc/browse*", (route) => {
    const url = new URL(route.request().url());
    const path = url.searchParams.get("path") ?? "";
    const nodes =
      path === "" ? browseRoot : path === "Area01" ? browseArea : browseLoop;
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ nodes }),
    });
  });
  await json(page, "**/api/opc/read*", {
    tag: "Area01.FIC101.OUT",
    value: "42.5",
    quality: "good",
    timestamp: null,
  });
  await page.route("**/api/runs/4242/stream", (route) =>
    route.fulfill({
      status: 200,
      headers: { "Content-Type": "text/event-stream" },
      body: [
        `event: initial\ndata: ${JSON.stringify(initialReadings)}\n\n`,
        `event: sample\ndata: ${JSON.stringify(sample(0, 50, 50))}\n\n`,
        `event: sample\ndata: ${JSON.stringify(sample(1, 51, 60))}\n\n`,
      ].join(""),
    }),
  );
  await json(page, "**/api/runs/4242", completedRun);
  await json(page, "**/api/runs/4243", liveRun);
}

export async function installDemoRoutes(page: Page) {
  await installCommonRoutes(page, true);
  await json(page, "**/api/runs/4242", demoCompletedRun);
  await json(page, "**/api/runs/4243", demoLiveRun);
  await page.route("**/api/runs/4242/stream", (route) =>
    route.fulfill({
      status: 200,
      headers: { "Content-Type": "text/event-stream" },
      body: [
        `event: initial\ndata: ${JSON.stringify(initialReadings)}\n\n`,
        `event: sample\ndata: ${JSON.stringify(sample(0, 50, 50))}\n\n`,
        `event: sample\ndata: ${JSON.stringify(sample(1, 51, 60))}\n\n`,
      ].join(""),
    }),
  );
}

export const runningFullRun = liveRun;
export const runningDemoRun = demoLiveRun;
