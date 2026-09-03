import { useQuery } from "@tanstack/react-query";
import { ApiError, apiErrorMessage } from "./errors";
import type { components } from "./schema";

export type AppMode = "demo" | "full";
type CapabilitiesResponse = components["schemas"]["CapabilitiesResponse"];
export type CapabilityActions = components["schemas"]["CapabilityActions"];
export type SimulatorCapabilities =
  components["schemas"]["DemoSimulatorCapabilities"];
type DemoRestrictions = components["schemas"]["DemoRestrictions"];
type DemoQuotas = components["schemas"]["DemoQuotas"];
type SecurityCapabilities = components["schemas"]["SecurityCapabilities"];

export interface AppCapabilities {
  readonly mode: AppMode;
  readonly demo: boolean;
  readonly drivers: readonly string[];
  readonly actions: CapabilityActions;
  readonly demoPolicy: components["schemas"]["DemoPolicy"] | null;
  readonly simulator: SimulatorCapabilities | null;
  readonly restrictions: DemoRestrictions | null;
  readonly quotas: DemoQuotas | null;
  readonly security: SecurityCapabilities;
}

const capabilitiesKey = ["capabilities"] as const;
const PROCESS_TYPES = new Set([
  "flow",
  "pressure_line",
  "pressure_vessel",
  "level",
  "temperature_mixing",
  "temperature_heat_exchange",
]);
const CONTROLLER_TYPES = new Set(["p", "pi", "pid"]);
const ACTION_KEYS = [
  "browse_opc",
  "cancel_run",
  "delete_run",
  "edit_notes",
  "export_run",
  "list_history",
  "manage_config",
  "manage_templates",
  "revert_pid",
  "start_opcda_tune",
  "start_simulator_tune",
  "stream_run",
  "write_pid",
] as const;
const RESTRICTION_KEYS = [
  "automatic_pid_write_allowed",
  "built_in_templates_only",
  "custom_tag_mappings_allowed",
  "direction_must_match_process_gain",
  "fixed_tag_name",
  "notes_allowed",
  "post_run_pid_write_allowed",
  "simulator_only",
] as const;
const QUOTA_KEYS = [
  "accepted_start_window_secs",
  "accepted_starts_per_client_ip",
  "accepted_starts_per_token",
  "max_active_runs_global",
  "max_active_runs_per_visitor",
  "max_json_body_bytes",
  "max_sse_global",
  "max_sse_per_visitor",
  "max_tune_run_rows_global",
  "ordinary_request_concurrency",
  "ordinary_request_timeout_secs",
  "retained_runs_per_visitor",
  "sse_lifetime_secs",
] as const;
const POLICY_KEYS = [
  "accepted_start_window_secs",
  "accepted_starts_per_client_ip",
  "accepted_starts_per_token",
  "cleanup_interval_secs",
  "max_active_runs_global",
  "max_active_runs_per_visitor",
  "max_json_body_bytes",
  "max_sse_global",
  "max_sse_per_visitor",
  "max_tune_run_rows_global",
  "ordinary_request_concurrency",
  "ordinary_request_timeout_secs",
  "poll_interval_ms",
  "retained_runs_per_visitor",
  "run_timeout_secs",
  "session_ttl_secs",
  "sse_lifetime_secs",
] as const;

function record(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`The server returned incomplete ${label} capabilities.`);
  }
  return value as Record<string, unknown>;
}

function bool(source: Record<string, unknown>, key: string, label: string) {
  if (typeof source[key] !== "boolean") {
    throw new TypeError(`The server omitted the ${label}.${key} capability.`);
  }
  return source[key] as boolean;
}

function text(source: Record<string, unknown>, key: string, label: string) {
  if (typeof source[key] !== "string") {
    throw new TypeError(`The server omitted the ${label}.${key} capability.`);
  }
  return source[key] as string;
}

function finiteNumber(
  source: Record<string, unknown>,
  key: string,
  label: string,
) {
  const value = source[key];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError(`The server omitted the ${label}.${key} capability.`);
  }
  return value;
}

function positiveInteger(
  source: Record<string, unknown>,
  key: string,
  label: string,
) {
  const value = finiteNumber(source, key, label);
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`The server returned an invalid ${label}.${key} value.`);
  }
  return value;
}

function nonNegativeInteger(
  source: Record<string, unknown>,
  key: string,
  label: string,
) {
  const value = finiteNumber(source, key, label);
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`The server returned an invalid ${label}.${key} value.`);
  }
  return value;
}

function stringList(
  source: Record<string, unknown>,
  key: string,
  label: string,
) {
  const value = source[key];
  if (
    !Array.isArray(value) ||
    value.length === 0 ||
    !value.every((item) => typeof item === "string" && item.length > 0)
  ) {
    throw new Error(`The server omitted the ${label}.${key} capability.`);
  }
  return value as string[];
}

function enumList(
  source: Record<string, unknown>,
  key: string,
  label: string,
  allowed: ReadonlySet<string>,
) {
  const values = stringList(source, key, label);
  if (!values.every((value) => allowed.has(value))) {
    throw new Error(`The server returned an invalid ${label}.${key} value.`);
  }
  return values;
}

function validateFloatBounds(value: unknown, label: string) {
  const bounds = record(value, label);
  const min = finiteNumber(bounds, "min", label);
  const max = finiteNumber(bounds, "max", label);
  if (min > max) {
    throw new Error(`The server returned invalid ${label} bounds.`);
  }
  if (
    bounds.absolute_min !== null &&
    bounds.absolute_min !== undefined &&
    (typeof bounds.absolute_min !== "number" ||
      !Number.isFinite(bounds.absolute_min) ||
      bounds.absolute_min <= 0)
  ) {
    throw new Error(`The server returned an invalid ${label}.absolute_min.`);
  }
  return {
    min,
    max,
    absoluteMin:
      typeof bounds.absolute_min === "number" ? bounds.absolute_min : null,
  };
}

function validateIntegerBounds(value: unknown, label: string) {
  const bounds = record(value, label);
  const min = finiteNumber(bounds, "min", label);
  const max = finiteNumber(bounds, "max", label);
  if (
    !Number.isSafeInteger(min) ||
    !Number.isSafeInteger(max) ||
    min < 0 ||
    min > max
  ) {
    throw new Error(`The server returned invalid ${label} bounds.`);
  }
  return { min, max };
}

function requireWithin(
  value: number,
  bounds: { readonly min: number; readonly max: number },
  label: string,
) {
  if (value < bounds.min || value > bounds.max) {
    throw new Error(`The server returned an out-of-bounds ${label} default.`);
  }
}

function validateActions(value: unknown) {
  const actions = record(value, "actions");
  for (const key of ACTION_KEYS) bool(actions, key, "actions");
  return actions as unknown as CapabilityActions;
}

function validateSecurity(value: unknown, demo: boolean) {
  const security = record(value, "security");
  text(security, "allowed_origin", "security");
  bool(security, "exact_origin_required_for_mutations", "security");
  bool(security, "https_required", "security");
  bool(security, "loopback_http_allowed", "security");
  bool(security, "trusted_proxy_configured", "security");
  if (
    security.forwarded_client_ip_header !== null &&
    security.forwarded_client_ip_header !== undefined &&
    typeof security.forwarded_client_ip_header !== "string"
  ) {
    throw new Error(
      "The server returned an invalid security.forwarded_client_ip_header capability.",
    );
  }
  if (demo) {
    const cookie = record(security.cookie, "security.cookie");
    const cookieName = text(cookie, "name", "security.cookie");
    text(cookie, "path", "security.cookie");
    text(cookie, "same_site", "security.cookie");
    positiveInteger(cookie, "max_age_secs", "security.cookie");
    if (
      !bool(cookie, "http_only", "security.cookie") ||
      !bool(cookie, "secure", "security.cookie") ||
      cookieName !== "__Host-bhtune_demo_session" ||
      cookie.path !== "/" ||
      cookie.same_site !== "Strict" ||
      security.allowed_origin === "" ||
      !bool(security, "exact_origin_required_for_mutations", "security") ||
      !bool(security, "https_required", "security")
    ) {
      throw new Error("The server returned unsafe Demo security capabilities.");
    }
    if (
      (security.trusted_proxy_configured === true &&
        security.forwarded_client_ip_header !== "X-BHTune-Client-IP") ||
      (security.trusted_proxy_configured === false &&
        security.forwarded_client_ip_header != null)
    ) {
      throw new Error(
        "The server returned contradictory trusted-proxy capabilities.",
      );
    }
  }
  return security as unknown as SecurityCapabilities;
}

function validateDemoSimulator(value: unknown) {
  const simulator = record(value, "simulator");
  const template = text(simulator, "template", "simulator");
  const tagName = text(simulator, "tag_name", "simulator");
  const templates = stringList(simulator, "templates", "simulator");
  const processTypes = enumList(
    simulator,
    "process_types",
    "simulator",
    PROCESS_TYPES,
  );
  const controllerTypes = enumList(
    simulator,
    "controller_types",
    "simulator",
    CONTROLLER_TYPES,
  );
  if (!templates.includes(template)) {
    throw new Error(
      "The Demo default template is not supported by the server.",
    );
  }

  const compatibility = simulator.compatibility;
  if (!Array.isArray(compatibility) || compatibility.length === 0) {
    throw new Error(
      "The server omitted the simulator.compatibility capability.",
    );
  }
  const compatibleProcesses = new Set<string>();
  for (const [index, item] of compatibility.entries()) {
    const entry = record(item, `simulator.compatibility[${index}]`);
    const processType = text(
      entry,
      "process_type",
      `simulator.compatibility[${index}]`,
    );
    const controllers = enumList(
      entry,
      "controller_types",
      `simulator.compatibility[${index}]`,
      CONTROLLER_TYPES,
    );
    if (
      !processTypes.includes(processType) ||
      !controllers.every((controller) => controllerTypes.includes(controller))
    ) {
      throw new Error(
        "The server returned contradictory simulator compatibility data.",
      );
    }
    compatibleProcesses.add(processType);
  }

  const defaults = record(simulator.defaults, "simulator.defaults");
  if (
    text(defaults, "template", "simulator.defaults") !== template ||
    text(defaults, "tag_name", "simulator.defaults") !== tagName ||
    (defaults.direction !== "direct" && defaults.direction !== "reverse")
  ) {
    throw new Error("The server returned contradictory simulator defaults.");
  }
  positiveInteger(defaults, "poll_interval_ms", "simulator.defaults");
  positiveInteger(defaults, "run_timeout_secs", "simulator.defaults");
  const relayAmp = finiteNumber(defaults, "relay_amp", "simulator.defaults");
  const cyclesSkip = nonNegativeInteger(
    defaults,
    "cycles_skip",
    "simulator.defaults",
  );
  const cyclesCount = positiveInteger(
    defaults,
    "cycles_count",
    "simulator.defaults",
  );
  const noiseProtectionSecs = nonNegativeInteger(
    defaults,
    "noise_protection_secs",
    "simulator.defaults",
  );
  const simGain = finiteNumber(defaults, "sim_gain", "simulator.defaults");
  const simTau = finiteNumber(defaults, "sim_tau", "simulator.defaults");
  const simDeadTime = finiteNumber(
    defaults,
    "sim_dead_time",
    "simulator.defaults",
  );
  const simNoise = finiteNumber(defaults, "sim_noise", "simulator.defaults");
  const simSeed = nonNegativeInteger(
    defaults,
    "sim_seed",
    "simulator.defaults",
  );
  const simInitialPv = finiteNumber(
    defaults,
    "sim_initial_pv",
    "simulator.defaults",
  );
  const simInitialMv = finiteNumber(
    defaults,
    "sim_initial_mv",
    "simulator.defaults",
  );
  const defaultPvRange = validateFloatBounds(
    defaults.pv_range,
    "simulator.defaults.pv_range",
  );
  const defaultMvRange = validateFloatBounds(
    defaults.mv_range,
    "simulator.defaults.mv_range",
  );

  if (
    processTypes.some((processType) => !compatibleProcesses.has(processType))
  ) {
    throw new Error(
      "The server omitted Demo compatibility for a process type.",
    );
  }

  const limits = record(simulator.limits, "simulator.limits");
  const relayBounds = validateFloatBounds(
    limits.relay_amp,
    "simulator.limits.relay_amp",
  );
  const cyclesSkipBounds = validateIntegerBounds(
    limits.cycles_skip,
    "simulator.limits.cycles_skip",
  );
  const cyclesCountBounds = validateIntegerBounds(
    limits.cycles_count,
    "simulator.limits.cycles_count",
  );
  const noiseProtectionBounds = validateIntegerBounds(
    limits.noise_protection_secs,
    "simulator.limits.noise_protection_secs",
  );
  const gainBounds = validateFloatBounds(
    limits.sim_gain,
    "simulator.limits.sim_gain",
  );
  const tauBounds = validateFloatBounds(
    limits.sim_tau,
    "simulator.limits.sim_tau",
  );
  const deadTimeBounds = validateFloatBounds(
    limits.sim_dead_time,
    "simulator.limits.sim_dead_time",
  );
  const seedBounds = validateIntegerBounds(
    limits.sim_seed,
    "simulator.limits.sim_seed",
  );
  const endpointBounds = validateFloatBounds(
    limits.range_endpoint,
    "simulator.limits.range_endpoint",
  );
  const spanBounds = validateFloatBounds(
    limits.range_span,
    "simulator.limits.range_span",
  );
  const maxNoiseFraction = finiteNumber(
    limits,
    "max_noise_fraction_of_pv_span",
    "simulator.limits",
  );
  if (maxNoiseFraction <= 0 || maxNoiseFraction > 1) {
    throw new Error(
      "The server returned an invalid simulator.limits.max_noise_fraction_of_pv_span value.",
    );
  }

  requireWithin(relayAmp, relayBounds, "relay_amp");
  requireWithin(cyclesSkip, cyclesSkipBounds, "cycles_skip");
  requireWithin(cyclesCount, cyclesCountBounds, "cycles_count");
  requireWithin(
    noiseProtectionSecs,
    noiseProtectionBounds,
    "noise_protection_secs",
  );
  requireWithin(simGain, gainBounds, "sim_gain");
  if (
    gainBounds.absoluteMin === null ||
    Math.abs(simGain) < gainBounds.absoluteMin ||
    (simGain > 0 && defaults.direction !== "reverse") ||
    (simGain < 0 && defaults.direction !== "direct")
  ) {
    throw new Error("The server returned unsafe simulator gain defaults.");
  }
  requireWithin(simTau, tauBounds, "sim_tau");
  requireWithin(simDeadTime, deadTimeBounds, "sim_dead_time");
  requireWithin(simSeed, seedBounds, "sim_seed");
  for (const [range, label] of [
    [defaultPvRange, "PV"],
    [defaultMvRange, "MV"],
  ] as const) {
    requireWithin(range.min, endpointBounds, `${label} range minimum`);
    requireWithin(range.max, endpointBounds, `${label} range maximum`);
    requireWithin(range.max - range.min, spanBounds, `${label} range span`);
  }
  requireWithin(simInitialPv, defaultPvRange, "sim_initial_pv");
  requireWithin(simInitialMv, defaultMvRange, "sim_initial_mv");
  if (
    simNoise < 0 ||
    simNoise > (defaultPvRange.max - defaultPvRange.min) * maxNoiseFraction
  ) {
    throw new Error("The server returned unsafe simulator noise defaults.");
  }
  return simulator as unknown as SimulatorCapabilities;
}

function validateDemoContract(
  payload: Record<string, unknown>,
  response: CapabilitiesResponse,
) {
  const simulator = validateDemoSimulator(payload.simulator);
  const restrictions = record(payload.restrictions, "restrictions");
  for (const key of RESTRICTION_KEYS) bool(restrictions, key, "restrictions");
  if (
    !restrictions.simulator_only ||
    !restrictions.built_in_templates_only ||
    !restrictions.fixed_tag_name ||
    !restrictions.direction_must_match_process_gain ||
    restrictions.custom_tag_mappings_allowed ||
    restrictions.notes_allowed ||
    restrictions.automatic_pid_write_allowed ||
    restrictions.post_run_pid_write_allowed
  ) {
    throw new Error("The server returned unsafe Demo restrictions.");
  }

  const quotas = record(payload.quotas, "quotas");
  for (const key of QUOTA_KEYS) positiveInteger(quotas, key, "quotas");
  const policy = record(payload.demo_policy, "demo_policy");
  for (const key of POLICY_KEYS) {
    positiveInteger(policy, key, "demo_policy");
  }
  for (const key of QUOTA_KEYS) {
    if (quotas[key] !== policy[key]) {
      throw new Error(
        `The server returned contradictory Demo quota data for ${key}.`,
      );
    }
  }
  if (
    simulator.defaults.poll_interval_ms !== policy.poll_interval_ms ||
    simulator.defaults.run_timeout_secs !== policy.run_timeout_secs
  ) {
    throw new Error("The server returned contradictory Demo timing defaults.");
  }
  const securityCookie = response.security.cookie;
  if (
    !securityCookie ||
    securityCookie.max_age_secs !== policy.session_ttl_secs
  ) {
    throw new Error(
      "The server returned contradictory Demo session lifetime data.",
    );
  }

  if (
    response.drivers.length !== 1 ||
    response.drivers[0] !== "simulator" ||
    !response.actions.start_simulator_tune ||
    response.actions.start_opcda_tune ||
    response.actions.edit_notes ||
    response.actions.write_pid ||
    response.actions.revert_pid ||
    response.actions.manage_templates ||
    response.actions.manage_config ||
    response.actions.browse_opc
  ) {
    throw new Error("The server returned unsafe Demo actions.");
  }

  return {
    simulator,
    restrictions: restrictions as unknown as DemoRestrictions,
    quotas: quotas as unknown as DemoQuotas,
    demoPolicy: response.demo_policy ?? null,
  };
}

function parseCapabilities(payload: unknown): AppCapabilities {
  const raw = record(payload, "application");
  if (raw.mode !== "demo" && raw.mode !== "full") {
    throw new Error("The server did not declare a valid application mode.");
  }
  if (raw.demo !== (raw.mode === "demo")) {
    throw new Error(
      "The server returned contradictory application capabilities.",
    );
  }
  const drivers = stringList(raw, "drivers", "application");
  const actions = validateActions(raw.actions);
  const security = validateSecurity(raw.security, raw.mode === "demo");
  const response = {
    ...raw,
    drivers,
    actions,
    security,
  } as unknown as CapabilitiesResponse;

  if (raw.mode === "demo") {
    const demo = validateDemoContract(raw, response);
    return {
      mode: "demo",
      demo: true,
      drivers,
      actions,
      security,
      ...demo,
    };
  }

  return {
    mode: "full",
    demo: false,
    drivers,
    actions,
    demoPolicy: null,
    simulator: null,
    restrictions: null,
    quotas: null,
    security,
  };
}

async function requestCapabilities(): Promise<AppCapabilities> {
  const response = await fetch("/api/capabilities", {
    credentials: "same-origin",
    headers: { Accept: "application/json" },
  });
  let payload: unknown;
  try {
    payload = await response.json();
  } catch {
    payload = undefined;
  }
  if (!response.ok) {
    throw new ApiError(apiErrorMessage(payload), response.status);
  }
  return parseCapabilities(payload);
}

export function useCapabilities() {
  return useQuery({
    queryKey: capabilitiesKey,
    queryFn: requestCapabilities,
    staleTime: Infinity,
    retry: false,
  });
}
