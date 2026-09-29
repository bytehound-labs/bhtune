import type { SimulatorCapabilities } from "../../api/capabilities";
import type { StartRunRequest } from "../../api/runs";
import type { components } from "../../api/schema";
import { derivedTagPreview } from "../../lib/opcTags";
import {
  DEFAULT_TAG_MAPPING_SOURCES,
  DEFAULT_VALUE_MAPPING_SOURCES,
  EMPTY_TAG_OVERRIDES,
  EMPTY_VALUE_TAG_OVERRIDES,
  type ControllerDirection,
  type NumOrBlank,
  type TagMappingSources,
  type TagOverrideFormState,
  type TagOverrideKey,
  type ValueMappingKey,
  type ValueMappingSource,
  type ValueMappingSources,
  type ValueTagOverrideFormState,
} from "./mappingState";

export type TuneDriver = components["schemas"]["TuneDriver"];
export type ProcessType = components["schemas"]["ProcessType"];
export type ControllerType = components["schemas"]["ControllerType"];
export type ResponseLevel = components["schemas"]["ResponseLevel"];
export type TagOverrides = components["schemas"]["TagOverrides"];
export type TemplateResponse = components["schemas"]["TemplateResponse"];
export type DemoStartRunRequest = Pick<
  StartRunRequest,
  | "driver"
  | "template"
  | "tagname"
  | "process_type"
  | "controller_type"
  | "relay_amp"
  | "cycles_skip"
  | "cycles_count"
  | "noise_protection_secs"
  | "direction"
  | "pv_range_high"
  | "pv_range_low"
  | "mv_range_high"
  | "mv_range_low"
  | "sim_gain"
  | "sim_tau"
  | "sim_dead_time"
  | "sim_noise"
  | "sim_seed"
  | "sim_initial_pv"
  | "sim_initial_mv"
>;

export const DRIVERS: readonly TuneDriver[] = ["simulator", "opcda"];
export const PROCESS_TYPES: readonly ProcessType[] = [
  "flow",
  "pressure_line",
  "pressure_vessel",
  "level",
  "temperature_mixing",
  "temperature_heat_exchange",
];
export const CONTROLLER_TYPES: readonly ControllerType[] = ["p", "pi", "pid"];
export const RESPONSE_LEVELS: readonly ResponseLevel[] = [
  "aggressive",
  "moderate",
  "sluggish",
];

export type ProcessDefaults = {
  readonly cyclesSkip: number;
  readonly cyclesCount: number;
  readonly noiseProtectionSecs: number;
};

/** Mirrors the authoritative defaults from `bhtune_core::ProcessType`. */
const PROCESS_DEFAULTS = {
  flow: { cyclesSkip: 1, cyclesCount: 2, noiseProtectionSecs: 3 },
  pressure_line: { cyclesSkip: 1, cyclesCount: 2, noiseProtectionSecs: 3 },
  pressure_vessel: { cyclesSkip: 1, cyclesCount: 1, noiseProtectionSecs: 10 },
  level: { cyclesSkip: 1, cyclesCount: 1, noiseProtectionSecs: 10 },
  temperature_mixing: {
    cyclesSkip: 1,
    cyclesCount: 1,
    noiseProtectionSecs: 20,
  },
  temperature_heat_exchange: {
    cyclesSkip: 1,
    cyclesCount: 1,
    noiseProtectionSecs: 20,
  },
} satisfies Record<ProcessType, ProcessDefaults>;

export function processDefaultsFor(processType: ProcessType): ProcessDefaults {
  return PROCESS_DEFAULTS[processType];
}

/** Mirrors `bhtune_core::ProcessType::allows_pid`. */
export const TEMPERATURE_PROCESS_TYPES = new Set<ProcessType>([
  "temperature_mixing",
  "temperature_heat_exchange",
]);

export function demoProcessDefaultsFor(
  capabilities: SimulatorCapabilities,
  _processType: ProcessType,
): ProcessDefaults {
  return {
    cyclesSkip: capabilities.defaults.cycles_skip,
    cyclesCount: capabilities.defaults.cycles_count,
    noiseProtectionSecs: capabilities.defaults.noise_protection_secs,
  };
}

export function demoControllerTypesFor(
  capabilities: SimulatorCapabilities,
  processType: ProcessType,
): readonly ControllerType[] {
  return (
    capabilities.compatibility.find((item) => item.process_type === processType)
      ?.controller_types ?? []
  );
}

export function demoDefaultControllerTypeFor(
  capabilities: SimulatorCapabilities,
  processType: ProcessType,
): ControllerType {
  const controllerTypes = demoControllerTypesFor(capabilities, processType);
  if (controllerTypes.includes(capabilities.defaults.controller_type)) {
    return capabilities.defaults.controller_type;
  }
  const fallback = controllerTypes[0];
  if (fallback === undefined) {
    throw new Error(
      `The server did not provide controller types for ${processType}.`,
    );
  }
  return fallback;
}

const TAG_PREVIEW_LABELS: Record<TagOverrideKey, string> = {
  processVariable: "Process variable (PV)",
  manipulatedVariable: "Manipulated variable (MV)",
  setpointVariable: "Setpoint",
  controllerMode: "Controller mode",
  modeAttribute: "Mode attribute",
  proportionalConstant: "Proportional constant",
  integralConstant: "Integral constant",
  derivativeConstant: "Derivative constant",
};

const VALUE_PREVIEW_LABELS: Record<ValueMappingKey, string> = {
  direction: "Controller direction",
  pvRangeHigh: "PV range high",
  pvRangeLow: "PV range low",
  mvRangeHigh: "MV range high",
  mvRangeLow: "MV range low",
};

export function templateTagFor(
  template: TemplateResponse | undefined,
  key: TagOverrideKey,
  tagname: string,
): string {
  return (
    (template &&
      derivedTagPreview(tagname, template).find(
        (row) => row.label === TAG_PREVIEW_LABELS[key],
      )?.tag) ??
    ""
  );
}

export function templateValueTagFor(
  template: TemplateResponse | undefined,
  key: ValueMappingKey,
  tagname: string,
): string {
  return (
    (template &&
      derivedTagPreview(tagname, template).find(
        (row) => row.label === VALUE_PREVIEW_LABELS[key],
      )?.tag) ??
    ""
  );
}

export type FormState = {
  driver: TuneDriver;
  template: string;
  notes: string;
  tagname: string;
  server: string;
  bridgeHost: string;
  processType: ProcessType;
  controllerType: ControllerType;
  relayAmp: NumOrBlank;
  cyclesSkip: NumOrBlank;
  cyclesCount: NumOrBlank;
  noiseProtectionSecs: NumOrBlank;
  tagSources: TagMappingSources;
  valueSources: ValueMappingSources;
  valueTagOverrides: ValueTagOverrideFormState;
  opcDirection: "" | ControllerDirection;
  opcPvRangeHigh: NumOrBlank;
  opcPvRangeLow: NumOrBlank;
  opcMvRangeHigh: NumOrBlank;
  opcMvRangeLow: NumOrBlank;
  simDirection: "" | ControllerDirection;
  simPvRangeHigh: NumOrBlank;
  simPvRangeLow: NumOrBlank;
  simMvRangeHigh: NumOrBlank;
  simMvRangeLow: NumOrBlank;
  simGain: NumOrBlank;
  simTau: NumOrBlank;
  simDeadTime: NumOrBlank;
  simNoise: NumOrBlank;
  simSeed: NumOrBlank;
  simInitialPv: NumOrBlank;
  simInitialMv: NumOrBlank;
  tagOverrides: TagOverrideFormState;
  writePid: "" | ResponseLevel;
  yes: boolean;
};

/**
 * Every default here matches `StartRunRequest`'s server defaults or `bhtune-cli`'s
 * simulator defaults, field-for-field.
 */
export const initialForm: FormState = {
  driver: "simulator",
  template: "",
  notes: "",
  tagname: "Sim.Loop1.PV",
  server: "",
  bridgeHost: "",
  processType: "flow",
  controllerType: "pi",
  relayAmp: 10,
  ...processDefaultsFor("flow"),
  tagSources: { ...DEFAULT_TAG_MAPPING_SOURCES },
  valueSources: { ...DEFAULT_VALUE_MAPPING_SOURCES },
  opcDirection: "",
  opcPvRangeHigh: "",
  opcPvRangeLow: "",
  opcMvRangeHigh: "",
  opcMvRangeLow: "",
  simDirection: "reverse",
  simPvRangeHigh: 100,
  simPvRangeLow: 0,
  simMvRangeHigh: 100,
  simMvRangeLow: 0,
  simGain: 1,
  simTau: 2,
  simDeadTime: 5,
  simNoise: 0,
  simSeed: 0,
  simInitialPv: 50,
  simInitialMv: 50,
  tagOverrides: { ...EMPTY_TAG_OVERRIDES },
  valueTagOverrides: { ...EMPTY_VALUE_TAG_OVERRIDES },
  writePid: "",
  yes: false,
};

/** Builds Demo form state exclusively from the server capability contract. */
export function formFromDemoCapabilities(
  capabilities: SimulatorCapabilities,
): FormState {
  const processType = capabilities.process_types[0];
  const controllerType = processType
    ? demoDefaultControllerTypeFor(capabilities, processType)
    : undefined;
  const processDefaults = processType
    ? demoProcessDefaultsFor(capabilities, processType)
    : undefined;
  if (!processType || !controllerType || !processDefaults) {
    throw new Error(
      "The server did not provide a complete Demo process/controller contract.",
    );
  }

  return {
    ...initialForm,
    driver: "simulator",
    template: capabilities.template,
    tagname: capabilities.tag_name,
    processType,
    controllerType,
    relayAmp: capabilities.defaults.relay_amp,
    ...processDefaults,
    simDirection: capabilities.defaults.direction,
    simPvRangeHigh: capabilities.defaults.pv_range.max,
    simPvRangeLow: capabilities.defaults.pv_range.min,
    simMvRangeHigh: capabilities.defaults.mv_range.max,
    simMvRangeLow: capabilities.defaults.mv_range.min,
    simGain: capabilities.defaults.sim_gain,
    simTau: capabilities.defaults.sim_tau,
    simDeadTime: capabilities.defaults.sim_dead_time,
    simNoise: capabilities.defaults.sim_noise,
    simSeed: capabilities.defaults.sim_seed,
    simInitialPv: capabilities.defaults.sim_initial_pv,
    simInitialMv: capabilities.defaults.sim_initial_mv,
  };
}

function recordRequest(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return {};
  }
  return value as Record<string, unknown>;
}

function requestNumber(
  request: Record<string, unknown>,
  key: string,
): number | undefined {
  const value = request[key];
  return typeof value === "number" && Number.isFinite(value)
    ? value
    : undefined;
}

function requestInteger(
  request: Record<string, unknown>,
  key: string,
): number | undefined {
  const value = requestNumber(request, key);
  return value !== undefined && Number.isSafeInteger(value) ? value : undefined;
}

function within(
  value: number | undefined,
  bounds: {
    readonly min: number;
    readonly max: number;
  },
): value is number {
  return value !== undefined && value >= bounds.min && value <= bounds.max;
}

function demoNumber(
  request: Record<string, unknown>,
  key: string,
  bounds: {
    readonly min: number;
    readonly max: number;
  },
  fallback: number,
): number {
  const value = requestNumber(request, key);
  return within(value, bounds) ? value : fallback;
}

function demoInteger(
  request: Record<string, unknown>,
  key: string,
  bounds: {
    readonly min: number;
    readonly max: number;
  },
  fallback: number,
): number {
  const value = requestInteger(request, key);
  return value !== undefined && value >= bounds.min && value <= bounds.max
    ? value
    : fallback;
}

function demoRange(
  request: Record<string, unknown>,
  lowKey: string,
  highKey: string,
  defaults: { readonly min: number; readonly max: number },
  endpointBounds: { readonly min: number; readonly max: number },
  spanBounds: { readonly min: number; readonly max: number },
): { readonly min: number; readonly max: number } {
  const low = requestNumber(request, lowKey);
  const high = requestNumber(request, highKey);
  if (
    within(low, endpointBounds) &&
    within(high, endpointBounds) &&
    high > low &&
    within(high - low, spanBounds)
  ) {
    return { min: low, max: high };
  }
  return defaults;
}

function demoInitialValue(
  request: Record<string, unknown>,
  key: string,
  range: { readonly min: number; readonly max: number },
  fallback: number,
): number {
  const value = requestNumber(request, key);
  if (value !== undefined && value >= range.min && value <= range.max) {
    return value;
  }
  return Math.min(range.max, Math.max(range.min, fallback));
}

/**
 * Converts an untrusted payload into safe Demo form state. Demo hydration is deliberately a
 * form operation, not a request shortcut: every value is checked against the server
 * capability contract and the final submit still passes through `normalizeSimulatorRequest`.
 */
function formFromDemoInput(
  request: unknown,
  capabilities: SimulatorCapabilities,
): FormState {
  const source = recordRequest(request);
  const defaults = formFromDemoCapabilities(capabilities);
  const template =
    typeof source.template === "string" &&
    capabilities.templates.includes(source.template)
      ? source.template
      : defaults.template;
  const processType =
    typeof source.process_type === "string" &&
    capabilities.process_types.includes(source.process_type as ProcessType)
      ? (source.process_type as ProcessType)
      : defaults.processType;
  const controllerTypes = demoControllerTypesFor(capabilities, processType);
  const controllerType =
    typeof source.controller_type === "string" &&
    controllerTypes.includes(source.controller_type as ControllerType)
      ? (source.controller_type as ControllerType)
      : demoDefaultControllerTypeFor(capabilities, processType);
  const endpointBounds = capabilities.limits.range_endpoint;
  const spanBounds = capabilities.limits.range_span;
  const pvRange = demoRange(
    source,
    "pv_range_low",
    "pv_range_high",
    {
      min: capabilities.defaults.pv_range.min,
      max: capabilities.defaults.pv_range.max,
    },
    endpointBounds,
    spanBounds,
  );
  const mvRange = demoRange(
    source,
    "mv_range_low",
    "mv_range_high",
    {
      min: capabilities.defaults.mv_range.min,
      max: capabilities.defaults.mv_range.max,
    },
    endpointBounds,
    spanBounds,
  );
  const simGain = demoNumber(
    source,
    "sim_gain",
    capabilities.limits.sim_gain,
    capabilities.defaults.sim_gain,
  );
  const simNoise = Math.min(
    demoNumber(
      source,
      "sim_noise",
      {
        min: 0,
        max:
          (pvRange.max - pvRange.min) *
          capabilities.limits.max_noise_fraction_of_pv_span,
      },
      capabilities.defaults.sim_noise,
    ),
    (pvRange.max - pvRange.min) *
      capabilities.limits.max_noise_fraction_of_pv_span,
  );
  const processDefaults = demoProcessDefaultsFor(capabilities, processType);
  const direction =
    source.direction === "direct" || source.direction === "reverse"
      ? source.direction
      : defaults.simDirection;

  return {
    ...defaults,
    template,
    processType,
    controllerType,
    relayAmp: demoNumber(
      source,
      "relay_amp",
      capabilities.limits.relay_amp,
      capabilities.defaults.relay_amp,
    ),
    ...processDefaults,
    cyclesSkip: demoInteger(
      source,
      "cycles_skip",
      capabilities.limits.cycles_skip,
      capabilities.defaults.cycles_skip,
    ),
    cyclesCount: demoInteger(
      source,
      "cycles_count",
      capabilities.limits.cycles_count,
      capabilities.defaults.cycles_count,
    ),
    noiseProtectionSecs: demoInteger(
      source,
      "noise_protection_secs",
      capabilities.limits.noise_protection_secs,
      capabilities.defaults.noise_protection_secs,
    ),
    simDirection: direction,
    simPvRangeLow: pvRange.min,
    simPvRangeHigh: pvRange.max,
    simMvRangeLow: mvRange.min,
    simMvRangeHigh: mvRange.max,
    simGain,
    simTau: demoNumber(
      source,
      "sim_tau",
      capabilities.limits.sim_tau,
      capabilities.defaults.sim_tau,
    ),
    simDeadTime: demoNumber(
      source,
      "sim_dead_time",
      capabilities.limits.sim_dead_time,
      capabilities.defaults.sim_dead_time,
    ),
    simNoise,
    simSeed: demoInteger(
      source,
      "sim_seed",
      capabilities.limits.sim_seed,
      capabilities.defaults.sim_seed,
    ),
    simInitialPv: demoInitialValue(
      source,
      "sim_initial_pv",
      pvRange,
      capabilities.defaults.sim_initial_pv,
    ),
    simInitialMv: demoInitialValue(
      source,
      "sim_initial_mv",
      mvRange,
      capabilities.defaults.sim_initial_mv,
    ),
  };
}

/**
 * Converts a saved browser-local Demo draft into form state. Current drafts keep simulator
 * ranges and direction under `source_*`; older drafts used the generic fields, which remain a
 * fallback unless the draft explicitly came from the OPC DA driver.
 */
export function formFromDemoDraft(
  draft: unknown,
  capabilities: SimulatorCapabilities,
): FormState {
  const source = recordRequest(draft);
  const legacyOpc =
    source.source_driver === "opcda" ||
    (source.source_driver === undefined && source.driver === "opcda");
  const simulatorDraftValue = (
    sourceKey: string,
    legacyKey: string,
  ): unknown => {
    if (source[sourceKey] !== undefined) return source[sourceKey];
    return legacyOpc ? undefined : source[legacyKey];
  };
  const simulatorScalarValue = (key: string): unknown =>
    legacyOpc ? undefined : source[key];

  return formFromDemoInput(
    {
      ...source,
      direction: simulatorDraftValue("source_direction", "direction"),
      pv_range_low: simulatorDraftValue("source_pv_range_low", "pv_range_low"),
      pv_range_high: simulatorDraftValue(
        "source_pv_range_high",
        "pv_range_high",
      ),
      mv_range_low: simulatorDraftValue("source_mv_range_low", "mv_range_low"),
      mv_range_high: simulatorDraftValue(
        "source_mv_range_high",
        "mv_range_high",
      ),
      sim_gain: simulatorScalarValue("sim_gain"),
      sim_tau: simulatorScalarValue("sim_tau"),
      sim_dead_time: simulatorScalarValue("sim_dead_time"),
      sim_noise: simulatorScalarValue("sim_noise"),
      sim_seed: simulatorScalarValue("sim_seed"),
      sim_initial_pv: simulatorScalarValue("sim_initial_pv"),
      sim_initial_mv: simulatorScalarValue("sim_initial_mv"),
    },
    capabilities,
  );
}

/**
 * Converts an untrusted duplicate payload into safe Demo form state. Demo duplication is
 * deliberately a form operation, not a request shortcut: every value is checked against the
 * server capability contract and the final submit still passes through
 * `normalizeSimulatorRequest`.
 */
export function formFromDemoDuplicate(
  request: unknown,
  capabilities: SimulatorCapabilities,
): FormState {
  return formFromDemoInput(request, capabilities);
}

export function toOptional(value: NumOrBlank): number | undefined {
  return value === "" ? undefined : value;
}

export function toNumOrBlank(value: number | null | undefined): NumOrBlank {
  return value ?? "";
}

export function toNullable(value: NumOrBlank): number | null {
  return value === "" ? null : value;
}

export function processDefaultFields(
  processType: ProcessType,
  values: {
    readonly cyclesSkip: number | null | undefined;
    readonly cyclesCount: number | null | undefined;
    readonly noiseProtectionSecs: number | null | undefined;
  },
): Pick<FormState, "cyclesSkip" | "cyclesCount" | "noiseProtectionSecs"> {
  const defaults = processDefaultsFor(processType);
  return {
    cyclesSkip: values.cyclesSkip ?? defaults.cyclesSkip,
    cyclesCount: values.cyclesCount ?? defaults.cyclesCount,
    noiseProtectionSecs:
      values.noiseProtectionSecs ?? defaults.noiseProtectionSecs,
  };
}

export function formTagOverrides(
  overrides: TagOverrides | null | undefined,
): TagOverrideFormState {
  return {
    processVariable: overrides?.process_variable ?? "",
    manipulatedVariable: overrides?.manipulated_variable ?? "",
    setpointVariable: overrides?.setpoint_variable ?? "",
    controllerMode: overrides?.controller_mode ?? "",
    modeAttribute: overrides?.mode_attribute ?? "",
    proportionalConstant: overrides?.proportional_constant ?? "",
    integralConstant: overrides?.integral_constant ?? "",
    derivativeConstant: overrides?.derivative_constant ?? "",
  };
}

export function formValueTagOverrides(
  overrides: TagOverrides | null | undefined,
): ValueTagOverrideFormState {
  return {
    direction: overrides?.controller_direction ?? "",
    pvRangeHigh: overrides?.upper_pv_range ?? "",
    pvRangeLow: overrides?.lower_pv_range ?? "",
    mvRangeHigh: overrides?.upper_mv_range ?? "",
    mvRangeLow: overrides?.lower_mv_range ?? "",
  };
}

export function tagOverridesFromForm(
  form: FormState,
): TagOverrides | undefined {
  const overrides: TagOverrides = {
    process_variable:
      form.tagSources.processVariable === "custom"
        ? form.tagOverrides.processVariable.trim() || undefined
        : undefined,
    manipulated_variable:
      form.tagSources.manipulatedVariable === "custom"
        ? form.tagOverrides.manipulatedVariable.trim() || undefined
        : undefined,
    setpoint_variable:
      form.tagSources.setpointVariable === "custom"
        ? form.tagOverrides.setpointVariable.trim() || undefined
        : undefined,
    controller_mode:
      form.tagSources.controllerMode === "custom"
        ? form.tagOverrides.controllerMode.trim() || undefined
        : undefined,
    mode_attribute:
      form.tagSources.modeAttribute === "custom"
        ? form.tagOverrides.modeAttribute.trim() || undefined
        : undefined,
    proportional_constant:
      form.tagSources.proportionalConstant === "custom"
        ? form.tagOverrides.proportionalConstant.trim() || undefined
        : undefined,
    integral_constant:
      form.tagSources.integralConstant === "custom"
        ? form.tagOverrides.integralConstant.trim() || undefined
        : undefined,
    derivative_constant:
      form.tagSources.derivativeConstant === "custom"
        ? form.tagOverrides.derivativeConstant.trim() || undefined
        : undefined,
    controller_direction:
      form.valueSources.direction === "custom"
        ? form.valueTagOverrides.direction.trim() || undefined
        : undefined,
    upper_pv_range:
      form.valueSources.pvRangeHigh === "custom"
        ? form.valueTagOverrides.pvRangeHigh.trim() || undefined
        : undefined,
    lower_pv_range:
      form.valueSources.pvRangeLow === "custom"
        ? form.valueTagOverrides.pvRangeLow.trim() || undefined
        : undefined,
    upper_mv_range:
      form.valueSources.mvRangeHigh === "custom"
        ? form.valueTagOverrides.mvRangeHigh.trim() || undefined
        : undefined,
    lower_mv_range:
      form.valueSources.mvRangeLow === "custom"
        ? form.valueTagOverrides.mvRangeLow.trim() || undefined
        : undefined,
  };
  return Object.values(overrides).some((value) => value !== undefined)
    ? overrides
    : undefined;
}

const TAG_SOURCE_FIELDS: readonly [TagOverrideKey, keyof TagOverrides][] = [
  ["processVariable", "process_variable"],
  ["manipulatedVariable", "manipulated_variable"],
  ["setpointVariable", "setpoint_variable"],
  ["controllerMode", "controller_mode"],
  ["modeAttribute", "mode_attribute"],
  ["proportionalConstant", "proportional_constant"],
  ["integralConstant", "integral_constant"],
  ["derivativeConstant", "derivative_constant"],
];

export function inferTagSources(
  overrides: TagOverrides | null | undefined,
): TagMappingSources {
  const sources = { ...DEFAULT_TAG_MAPPING_SOURCES };
  for (const [formKey, apiKey] of TAG_SOURCE_FIELDS) {
    if (overrides?.[apiKey]?.trim()) {
      sources[formKey] = "custom";
    }
  }
  return sources;
}

export function valueSource(
  hasFixedValue: boolean,
  customTag: string | null | undefined,
): ValueMappingSource {
  if (hasFixedValue) return "fixed";
  if (customTag?.trim()) return "custom";
  return "tag";
}
