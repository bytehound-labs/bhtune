import type { NewRunDraft, StartRunRequest } from "../../api/runs";
import type { components } from "../../api/schema";
import type { SimulatorCapabilities } from "../../api/capabilities";
import { derivedTagPreview } from "../../lib/opcTags";
import {
  DEFAULT_TAG_MAPPING_SOURCES,
  DEFAULT_VALUE_MAPPING_SOURCES,
  EMPTY_TAG_OVERRIDES,
  EMPTY_VALUE_TAG_OVERRIDES,
  type ControllerDirection,
  type NumOrBlank,
  type TagOverrideFormState,
  type TagOverrideKey,
  type TagMappingSources,
  type ValueMappingKey,
  type ValueMappingSource,
  type ValueMappingSources,
  type ValueTagOverrideFormState,
} from "./mappingState";

export type TuneDriver = components["schemas"]["TuneDriver"];
export type ProcessType = components["schemas"]["ProcessType"];
export type ControllerType = components["schemas"]["ControllerType"];
export type ResponseLevel = components["schemas"]["ResponseLevel"];
type TagOverrides = components["schemas"]["TagOverrides"];
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

const TAG_OVERRIDE_KEYS: readonly TagOverrideKey[] = [
  "processVariable",
  "manipulatedVariable",
  "setpointVariable",
  "controllerMode",
  "modeAttribute",
  "proportionalConstant",
  "integralConstant",
  "derivativeConstant",
];

const VALUE_MAPPING_KEYS: readonly ValueMappingKey[] = [
  "direction",
  "pvRangeHigh",
  "pvRangeLow",
  "mvRangeHigh",
  "mvRangeLow",
];

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
    ? demoControllerTypesFor(capabilities, processType)[0]
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
    readonly absolute_min?: number | null;
  },
): value is number {
  return (
    value !== undefined &&
    value >= bounds.min &&
    value <= bounds.max &&
    (bounds.absolute_min == null || Math.abs(value) >= bounds.absolute_min)
  );
}

function demoNumber(
  request: Record<string, unknown>,
  key: string,
  bounds: {
    readonly min: number;
    readonly max: number;
    readonly absolute_min?: number | null;
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
      : controllerTypes[0];
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
    simDirection: simGain < 0 ? "direct" : "reverse",
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

function toOptional(value: NumOrBlank): number | undefined {
  return value === "" ? undefined : value;
}

function toNumOrBlank(value: number | null | undefined): NumOrBlank {
  return value ?? "";
}

function toNullable(value: NumOrBlank): number | null {
  return value === "" ? null : value;
}

type ProcessDefaultInputs = {
  readonly cyclesSkip: number | null | undefined;
  readonly cyclesCount: number | null | undefined;
  readonly noiseProtectionSecs: number | null | undefined;
};

type ProcessDefaultFormFields = Pick<
  FormState,
  "cyclesSkip" | "cyclesCount" | "noiseProtectionSecs"
>;

function processDefaultFields(
  processType: ProcessType,
  values: ProcessDefaultInputs,
): ProcessDefaultFormFields {
  const defaults = processDefaultsFor(processType);
  return {
    cyclesSkip: values.cyclesSkip ?? defaults.cyclesSkip,
    cyclesCount: values.cyclesCount ?? defaults.cyclesCount,
    noiseProtectionSecs:
      values.noiseProtectionSecs ?? defaults.noiseProtectionSecs,
  };
}

function formTagOverrides(
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

function formValueTagOverrides(
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

function tagOverridesFromForm(form: FormState): TagOverrides | undefined {
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

/**
 * Applies a new base tag and invalidates every custom mapping that could still point at the
 * previous loop. Fixed direction/range values are independent of the base tag and remain intact.
 */
export function applyTagNameChange(
  form: FormState,
  tagname: string,
): FormState {
  if (form.tagname === tagname) return form;

  const tagSources = { ...form.tagSources };
  for (const key of TAG_OVERRIDE_KEYS) {
    if (tagSources[key] === "custom") tagSources[key] = "template";
  }

  const valueSources = { ...form.valueSources };
  for (const key of VALUE_MAPPING_KEYS) {
    if (valueSources[key] === "custom") valueSources[key] = "tag";
  }

  return {
    ...form,
    tagname,
    tagSources,
    valueSources,
    tagOverrides: { ...EMPTY_TAG_OVERRIDES },
    valueTagOverrides: { ...EMPTY_VALUE_TAG_OVERRIDES },
  };
}

function inferTagSources(
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

function valueSource(
  hasFixedValue: boolean,
  customTag: string | null | undefined,
): ValueMappingSource {
  if (hasFixedValue) return "fixed";
  if (customTag?.trim()) return "custom";
  return "tag";
}

function inferRequestValueSources(
  request: StartRunRequest,
): ValueMappingSources {
  if (request.driver === "simulator") {
    return { ...DEFAULT_VALUE_MAPPING_SOURCES };
  }
  const overrides = request.tag_overrides;
  return {
    direction: valueSource(
      request.direction !== null && request.direction !== undefined,
      overrides?.controller_direction,
    ),
    pvRangeHigh: valueSource(
      request.pv_range_high !== null && request.pv_range_high !== undefined,
      overrides?.upper_pv_range,
    ),
    pvRangeLow: valueSource(
      request.pv_range_low !== null && request.pv_range_low !== undefined,
      overrides?.lower_pv_range,
    ),
    mvRangeHigh: valueSource(
      request.mv_range_high !== null && request.mv_range_high !== undefined,
      overrides?.upper_mv_range,
    ),
    mvRangeLow: valueSource(
      request.mv_range_low !== null && request.mv_range_low !== undefined,
      overrides?.lower_mv_range,
    ),
  };
}

function inferDraftValueSources(draft: NewRunDraft): ValueMappingSources {
  if (draft.value_sources) {
    return {
      direction: draft.value_sources.direction,
      pvRangeHigh: draft.value_sources.pv_range_high,
      pvRangeLow: draft.value_sources.pv_range_low,
      mvRangeHigh: draft.value_sources.mv_range_high,
      mvRangeLow: draft.value_sources.mv_range_low,
    };
  }

  const legacyOpc =
    draft.source_driver === "opcda" ||
    (draft.source_driver === undefined && draft.driver === "opcda");
  const overrides = draft.tag_overrides;
  return {
    direction: valueSource(
      legacyOpc && draft.direction !== null && draft.direction !== undefined,
      overrides?.controller_direction,
    ),
    pvRangeHigh: valueSource(
      legacyOpc &&
        draft.pv_range_high !== null &&
        draft.pv_range_high !== undefined,
      overrides?.upper_pv_range,
    ),
    pvRangeLow: valueSource(
      legacyOpc &&
        draft.pv_range_low !== null &&
        draft.pv_range_low !== undefined,
      overrides?.lower_pv_range,
    ),
    mvRangeHigh: valueSource(
      legacyOpc &&
        draft.mv_range_high !== null &&
        draft.mv_range_high !== undefined,
      overrides?.upper_mv_range,
    ),
    mvRangeLow: valueSource(
      legacyOpc &&
        draft.mv_range_low !== null &&
        draft.mv_range_low !== undefined,
      overrides?.lower_mv_range,
    ),
  };
}

function draftTagSources(draft: NewRunDraft): TagMappingSources {
  if (draft.tag_sources) {
    return {
      processVariable: draft.tag_sources.process_variable,
      manipulatedVariable: draft.tag_sources.manipulated_variable,
      setpointVariable: draft.tag_sources.setpoint_variable,
      controllerMode: draft.tag_sources.controller_mode,
      modeAttribute: draft.tag_sources.mode_attribute,
      proportionalConstant: draft.tag_sources.proportional_constant,
      integralConstant: draft.tag_sources.integral_constant,
      derivativeConstant: draft.tag_sources.derivative_constant,
    };
  }
  return inferTagSources(draft.tag_overrides);
}

function requestOpcDirection(
  request: StartRunRequest,
): "" | ControllerDirection {
  if (request.driver !== "opcda") return "";
  return request.direction ?? "";
}

function requestOpcNumber(
  request: StartRunRequest,
  value: number | null | undefined,
): NumOrBlank {
  if (request.driver !== "opcda") return "";
  return toNumOrBlank(value);
}

function requestSimulatorDirection(
  request: StartRunRequest,
): "" | ControllerDirection {
  if (request.driver !== "simulator") return initialForm.simDirection;
  return request.direction ?? initialForm.simDirection;
}

function requestSimulatorNumber(
  request: StartRunRequest,
  value: number | null | undefined,
  fallback: NumOrBlank,
): NumOrBlank {
  if (request.driver !== "simulator") return fallback;
  return toNumOrBlank(value);
}

/**
 * Converts a stored [`StartRunRequest`] into form state. Optional ranges and direction remain
 * blank when the original request omitted them; process defaults are shown when their values
 * were omitted.
 */
export function formFromRequest(request: StartRunRequest): FormState {
  const processDefaults = processDefaultFields(request.process_type, {
    cyclesSkip: request.cycles_skip,
    cyclesCount: request.cycles_count,
    noiseProtectionSecs: request.noise_protection_secs,
  });

  return {
    driver: request.driver,
    template: request.template,
    notes: "",
    tagname: request.tagname,
    server: request.server ?? "",
    bridgeHost: request.bridge_host ?? "",
    processType: request.process_type,
    controllerType: request.controller_type,
    relayAmp: request.relay_amp,
    ...processDefaults,
    tagSources: inferTagSources(request.tag_overrides),
    valueSources: inferRequestValueSources(request),
    valueTagOverrides: formValueTagOverrides(request.tag_overrides),
    opcDirection: requestOpcDirection(request),
    opcPvRangeHigh: requestOpcNumber(request, request.pv_range_high),
    opcPvRangeLow: requestOpcNumber(request, request.pv_range_low),
    opcMvRangeHigh: requestOpcNumber(request, request.mv_range_high),
    opcMvRangeLow: requestOpcNumber(request, request.mv_range_low),
    simDirection: requestSimulatorDirection(request),
    simPvRangeHigh: requestSimulatorNumber(
      request,
      request.pv_range_high,
      initialForm.simPvRangeHigh,
    ),
    simPvRangeLow: requestSimulatorNumber(
      request,
      request.pv_range_low,
      initialForm.simPvRangeLow,
    ),
    simMvRangeHigh: requestSimulatorNumber(
      request,
      request.mv_range_high,
      initialForm.simMvRangeHigh,
    ),
    simMvRangeLow: requestSimulatorNumber(
      request,
      request.mv_range_low,
      initialForm.simMvRangeLow,
    ),
    simGain: request.sim_gain ?? initialForm.simGain,
    simTau: request.sim_tau ?? initialForm.simTau,
    simDeadTime: request.sim_dead_time ?? initialForm.simDeadTime,
    simNoise: request.sim_noise ?? initialForm.simNoise,
    simSeed: request.sim_seed ?? initialForm.simSeed,
    simInitialPv: request.sim_initial_pv ?? initialForm.simInitialPv,
    simInitialMv: request.sim_initial_mv ?? initialForm.simInitialMv,
    tagOverrides: formTagOverrides(request.tag_overrides),
    writePid: request.write_pid ?? "",
    yes: request.yes ?? false,
  };
}

function draftText(value: string | null | undefined, fallback: string): string {
  if (value === undefined) return fallback;
  return value ?? "";
}

function draftNumber(
  value: number | null | undefined,
  fallback: NumOrBlank,
): NumOrBlank {
  if (value === undefined) return fallback;
  return toNumOrBlank(value);
}

function draftSimulatorDirection(
  sourceValue: ControllerDirection | "" | null | undefined,
  legacyValue: ControllerDirection | "" | null | undefined,
  legacyOpc: boolean,
  fallback: ControllerDirection | "",
): "" | ControllerDirection {
  if (sourceValue !== undefined) return sourceValue ?? "";
  if (legacyOpc) return fallback;
  return legacyValue ?? fallback;
}

function draftSimulatorNumber(
  sourceValue: number | null | undefined,
  legacyValue: number | null | undefined,
  legacyOpc: boolean,
  fallback: NumOrBlank,
): NumOrBlank {
  if (sourceValue !== undefined) return toNumOrBlank(sourceValue);
  if (legacyOpc) return fallback;
  return toNumOrBlank(legacyValue);
}

/**
 * Converts the mutable saved draft into form state. `undefined` means an older or partial
 * draft omitted a field and should use the built-in default; `null` means it was cleared,
 * except for process defaults, where it resolves to the selected process type's value.
 */
export function formFromDraft(draft: NewRunDraft): FormState {
  const driver = draft.driver ?? initialForm.driver;
  const processType = draft.process_type ?? initialForm.processType;
  const valueSources = inferDraftValueSources(draft);
  const legacyOpc =
    draft.source_driver === "opcda" ||
    (draft.source_driver === undefined && driver === "opcda");
  const simulatorValuesPresent =
    draft.source_direction !== undefined ||
    draft.source_pv_range_high !== undefined ||
    draft.source_pv_range_low !== undefined ||
    draft.source_mv_range_high !== undefined ||
    draft.source_mv_range_low !== undefined;
  const separatedMappingState =
    draft.source_driver !== undefined ||
    simulatorValuesPresent ||
    draft.tag_sources !== undefined ||
    draft.value_sources !== undefined;
  const restoreOpcValues =
    separatedMappingState || legacyOpc || driver === "opcda";
  const processDefaults = processDefaultFields(processType, {
    cyclesSkip: draft.cycles_skip,
    cyclesCount: draft.cycles_count,
    noiseProtectionSecs: draft.noise_protection_secs,
  });

  return {
    driver,
    template: draftText(draft.template, initialForm.template),
    notes: "",
    tagname: draftText(draft.tagname, initialForm.tagname),
    server: draft.server ?? "",
    bridgeHost: draft.bridge_host ?? "",
    processType,
    controllerType: draft.controller_type ?? initialForm.controllerType,
    relayAmp: toNumOrBlank(draft.relay_amp),
    ...processDefaults,
    tagSources: draftTagSources(draft),
    valueSources,
    valueTagOverrides: formValueTagOverrides(draft.tag_overrides),
    opcDirection: restoreOpcValues ? (draft.direction ?? "") : "",
    opcPvRangeHigh: restoreOpcValues ? toNumOrBlank(draft.pv_range_high) : "",
    opcPvRangeLow: restoreOpcValues ? toNumOrBlank(draft.pv_range_low) : "",
    opcMvRangeHigh: restoreOpcValues ? toNumOrBlank(draft.mv_range_high) : "",
    opcMvRangeLow: restoreOpcValues ? toNumOrBlank(draft.mv_range_low) : "",
    simDirection: draftSimulatorDirection(
      simulatorValuesPresent ? draft.source_direction : undefined,
      draft.direction,
      legacyOpc,
      initialForm.simDirection,
    ),
    simPvRangeHigh: draftSimulatorNumber(
      simulatorValuesPresent ? draft.source_pv_range_high : undefined,
      draft.pv_range_high,
      legacyOpc,
      initialForm.simPvRangeHigh,
    ),
    simPvRangeLow: draftSimulatorNumber(
      simulatorValuesPresent ? draft.source_pv_range_low : undefined,
      draft.pv_range_low,
      legacyOpc,
      initialForm.simPvRangeLow,
    ),
    simMvRangeHigh: draftSimulatorNumber(
      simulatorValuesPresent ? draft.source_mv_range_high : undefined,
      draft.mv_range_high,
      legacyOpc,
      initialForm.simMvRangeHigh,
    ),
    simMvRangeLow: draftSimulatorNumber(
      simulatorValuesPresent ? draft.source_mv_range_low : undefined,
      draft.mv_range_low,
      legacyOpc,
      initialForm.simMvRangeLow,
    ),
    simGain: draftNumber(draft.sim_gain, initialForm.simGain),
    simTau: draftNumber(draft.sim_tau, initialForm.simTau),
    simDeadTime: draftNumber(draft.sim_dead_time, initialForm.simDeadTime),
    simNoise: draftNumber(draft.sim_noise, initialForm.simNoise),
    simSeed: draftNumber(draft.sim_seed, initialForm.simSeed),
    simInitialPv: draftNumber(draft.sim_initial_pv, initialForm.simInitialPv),
    simInitialMv: draftNumber(draft.sim_initial_mv, initialForm.simInitialMv),
    tagOverrides: formTagOverrides(draft.tag_overrides),
    writePid: draft.write_pid ?? "",
    yes: draft.yes ?? initialForm.yes,
  };
}

/** Serializes every editable form field except Notes for the server-side draft. */
export function draftFromForm(form: FormState): NewRunDraft {
  return {
    driver: form.driver,
    template: form.template,
    tagname: form.tagname,
    server: form.server,
    bridge_host: form.bridgeHost,
    process_type: form.processType,
    controller_type: form.controllerType,
    relay_amp: toNullable(form.relayAmp),
    cycles_skip: toNullable(form.cyclesSkip),
    cycles_count: toNullable(form.cyclesCount),
    noise_protection_secs: toNullable(form.noiseProtectionSecs),
    direction: form.opcDirection || null,
    pv_range_high: toNullable(form.opcPvRangeHigh),
    pv_range_low: toNullable(form.opcPvRangeLow),
    mv_range_high: toNullable(form.opcMvRangeHigh),
    mv_range_low: toNullable(form.opcMvRangeLow),
    source_driver: form.driver,
    source_direction: form.simDirection || null,
    source_pv_range_high: toNullable(form.simPvRangeHigh),
    source_pv_range_low: toNullable(form.simPvRangeLow),
    source_mv_range_high: toNullable(form.simMvRangeHigh),
    source_mv_range_low: toNullable(form.simMvRangeLow),
    tag_sources: {
      process_variable: form.tagSources.processVariable,
      manipulated_variable: form.tagSources.manipulatedVariable,
      setpoint_variable: form.tagSources.setpointVariable,
      controller_mode: form.tagSources.controllerMode,
      mode_attribute: form.tagSources.modeAttribute,
      proportional_constant: form.tagSources.proportionalConstant,
      integral_constant: form.tagSources.integralConstant,
      derivative_constant: form.tagSources.derivativeConstant,
    },
    value_sources: {
      direction: form.valueSources.direction,
      pv_range_high: form.valueSources.pvRangeHigh,
      pv_range_low: form.valueSources.pvRangeLow,
      mv_range_high: form.valueSources.mvRangeHigh,
      mv_range_low: form.valueSources.mvRangeLow,
    },
    sim_gain: toNullable(form.simGain),
    sim_tau: toNullable(form.simTau),
    sim_dead_time: toNullable(form.simDeadTime),
    sim_noise: toNullable(form.simNoise),
    sim_seed: toNullable(form.simSeed),
    sim_initial_pv: toNullable(form.simInitialPv),
    sim_initial_mv: toNullable(form.simInitialMv),
    tag_overrides: tagOverridesFromForm(form) ?? null,
    write_pid: form.writePid || null,
    yes: form.yes,
  };
}

/** Serializes only fields visible and editable in Demo mode. */
export function demoDraftFromForm(form: FormState): NewRunDraft {
  return {
    template: form.template,
    process_type: form.processType,
    controller_type: form.controllerType,
    relay_amp: toNullable(form.relayAmp),
    cycles_skip: toNullable(form.cyclesSkip),
    cycles_count: toNullable(form.cyclesCount),
    noise_protection_secs: toNullable(form.noiseProtectionSecs),
    pv_range_high: toNullable(form.simPvRangeHigh),
    pv_range_low: toNullable(form.simPvRangeLow),
    mv_range_high: toNullable(form.simMvRangeHigh),
    mv_range_low: toNullable(form.simMvRangeLow),
    sim_gain: toNullable(form.simGain),
    sim_tau: toNullable(form.simTau),
    sim_dead_time: toNullable(form.simDeadTime),
    sim_noise: toNullable(form.simNoise),
    sim_seed: toNullable(form.simSeed),
    sim_initial_pv: toNullable(form.simInitialPv),
    sim_initial_mv: toNullable(form.simInitialMv),
  };
}

type MappingValidation = {
  readonly source: ValueMappingSource;
  readonly fixedValue: NumOrBlank | ControllerDirection;
  readonly customValue: string;
  readonly fixedMessage: string;
  readonly customMessage: string;
};

function validateSimulatorMappings(form: FormState): string | undefined {
  const requiredValues: readonly [NumOrBlank, string][] = [
    [
      form.simPvRangeHigh,
      "PV range high is required for the simulator driver (it has no range tags to read).",
    ],
    [
      form.simPvRangeLow,
      "PV range low is required for the simulator driver (it has no range tags to read).",
    ],
    [
      form.simMvRangeHigh,
      "MV range high is required for the simulator driver (it has no range tags to read).",
    ],
    [
      form.simMvRangeLow,
      "MV range low is required for the simulator driver (it has no range tags to read).",
    ],
  ];
  for (const [value, message] of requiredValues) {
    if (value === "") return message;
  }
  if (!form.simDirection) {
    return "Controller direction is required for the simulator driver (it has no direction tag to read).";
  }
  return undefined;
}

function validateMappingValue({
  source,
  fixedValue,
  customValue,
  fixedMessage,
  customMessage,
}: MappingValidation): string | undefined {
  if (source === "fixed" && fixedValue === "") return fixedMessage;
  if (source === "custom" && !customValue.trim()) return customMessage;
  return undefined;
}

function validateOpcMappings(form: FormState): string | undefined {
  const mappings: readonly MappingValidation[] = [
    {
      source: form.valueSources.direction,
      fixedValue: form.opcDirection,
      customValue: form.valueTagOverrides.direction,
      fixedMessage:
        "Controller direction is required when Fixed value is selected.",
      customMessage:
        "Controller direction read tag is required when Custom tag is selected.",
    },
    {
      source: form.valueSources.pvRangeHigh,
      fixedValue: form.opcPvRangeHigh,
      customValue: form.valueTagOverrides.pvRangeHigh,
      fixedMessage: "PV range high is required when Fixed value is selected.",
      customMessage:
        "PV range high read tag is required when Custom tag is selected.",
    },
    {
      source: form.valueSources.pvRangeLow,
      fixedValue: form.opcPvRangeLow,
      customValue: form.valueTagOverrides.pvRangeLow,
      fixedMessage: "PV range low is required when Fixed value is selected.",
      customMessage:
        "PV range low read tag is required when Custom tag is selected.",
    },
    {
      source: form.valueSources.mvRangeHigh,
      fixedValue: form.opcMvRangeHigh,
      customValue: form.valueTagOverrides.mvRangeHigh,
      fixedMessage: "MV range high is required when Fixed value is selected.",
      customMessage:
        "MV range high read tag is required when Custom tag is selected.",
    },
    {
      source: form.valueSources.mvRangeLow,
      fixedValue: form.opcMvRangeLow,
      customValue: form.valueTagOverrides.mvRangeLow,
      fixedMessage: "MV range low is required when Fixed value is selected.",
      customMessage:
        "MV range low read tag is required when Custom tag is selected.",
    },
  ];
  for (const mapping of mappings) {
    const error = validateMappingValue(mapping);
    if (error) return error;
  }
  return undefined;
}

function validateForm(form: FormState): string | undefined {
  if (!form.template) return "Choose a template.";
  if (form.driver !== "simulator" && !form.tagname.trim()) {
    return "Tag name is required.";
  }
  if (form.driver === "opcda" && !form.server.trim()) {
    return "OPC DA server ProgID is required for the opcda driver.";
  }
  if (form.relayAmp === "") return "Relay amplitude is required.";
  const processDefaults = requiredProcessDefaults(form);
  if (typeof processDefaults === "string") return processDefaults;
  const mappingError =
    form.driver === "simulator"
      ? validateSimulatorMappings(form)
      : validateOpcMappings(form);
  if (mappingError) return mappingError;
  if (form.writePid && !form.yes) {
    return "Enable Allow automatic PID write to apply PID settings without a prompt, or clear the automatic PID setting.";
  }
  return undefined;
}

function requiredProcessDefaults(form: FormState): ProcessDefaults | string {
  if (form.cyclesSkip === "") return "Cycles to skip is required.";
  if (form.cyclesCount === "") return "Cycles to count is required.";
  if (form.noiseProtectionSecs === "") {
    return "Noise protection is required.";
  }
  return {
    cyclesSkip: form.cyclesSkip,
    cyclesCount: form.cyclesCount,
    noiseProtectionSecs: form.noiseProtectionSecs,
  };
}

function mappedRangeValue(
  driver: TuneDriver,
  simulatorValue: NumOrBlank,
  source: ValueMappingSource,
  opcValue: NumOrBlank,
): number | undefined {
  if (driver === "simulator") return toOptional(simulatorValue);
  if (source === "fixed") return toOptional(opcValue);
  return undefined;
}

function mappedDirection(
  driver: TuneDriver,
  simulatorValue: "" | ControllerDirection,
  source: ValueMappingSource,
  opcValue: "" | ControllerDirection,
): ControllerDirection | undefined {
  if (driver === "simulator") return simulatorValue || undefined;
  if (source === "fixed") return opcValue || undefined;
  return undefined;
}

function requestServer(form: FormState): string | undefined {
  if (form.driver !== "opcda") return undefined;
  return form.server.trim();
}

function requestTagOverrides(form: FormState): TagOverrides | undefined {
  if (form.driver !== "opcda") return undefined;
  return tagOverridesFromForm(form);
}

/** Builds the request body, or returns a client-side validation message instead. */
export function buildRequest(form: FormState): StartRunRequest | string {
  const validationError = validateForm(form);
  if (validationError) return validationError;
  const relayAmp = form.relayAmp === "" ? undefined : form.relayAmp;
  if (relayAmp === undefined) return "Relay amplitude is required.";
  const processDefaults = requiredProcessDefaults(form);
  if (typeof processDefaults === "string") return processDefaults;

  return {
    tagname: form.tagname.trim(),
    template: form.template,
    process_type: form.processType,
    controller_type: form.controllerType,
    relay_amp: relayAmp,
    cycles_skip: processDefaults.cyclesSkip,
    cycles_count: processDefaults.cyclesCount,
    noise_protection_secs: processDefaults.noiseProtectionSecs,
    driver: form.driver,
    bridge_host: form.bridgeHost.trim() || undefined,
    server: requestServer(form),
    sim_gain: toOptional(form.simGain),
    sim_tau: toOptional(form.simTau),
    sim_dead_time: toOptional(form.simDeadTime),
    sim_noise: toOptional(form.simNoise),
    sim_seed: toOptional(form.simSeed),
    sim_initial_pv: toOptional(form.simInitialPv),
    sim_initial_mv: toOptional(form.simInitialMv),
    pv_range_high: mappedRangeValue(
      form.driver,
      form.simPvRangeHigh,
      form.valueSources.pvRangeHigh,
      form.opcPvRangeHigh,
    ),
    pv_range_low: mappedRangeValue(
      form.driver,
      form.simPvRangeLow,
      form.valueSources.pvRangeLow,
      form.opcPvRangeLow,
    ),
    mv_range_high: mappedRangeValue(
      form.driver,
      form.simMvRangeHigh,
      form.valueSources.mvRangeHigh,
      form.opcMvRangeHigh,
    ),
    mv_range_low: mappedRangeValue(
      form.driver,
      form.simMvRangeLow,
      form.valueSources.mvRangeLow,
      form.opcMvRangeLow,
    ),
    direction: mappedDirection(
      form.driver,
      form.simDirection,
      form.valueSources.direction,
      form.opcDirection,
    ),
    tag_overrides: requestTagOverrides(form),
    notes: form.notes.trim() || undefined,
    yes: form.yes,
    write_pid: form.writePid || undefined,
  };
}

function bounded(
  value: number,
  range: {
    readonly min: number;
    readonly max: number;
    readonly absolute_min?: number | null;
  },
  label: string,
): number | string {
  if (!Number.isFinite(value) || value < range.min || value > range.max) {
    return `${label} must be between ${range.min} and ${range.max}.`;
  }
  if (range.absolute_min != null && Math.abs(value) < range.absolute_min) {
    return `${label} must be between ${range.min} and -${range.absolute_min}, or between ${range.absolute_min} and ${range.max}.`;
  }
  return value;
}

/**
 * Produces the deliberately small request accepted by the public simulator. It does not
 * forward stale OPC mappings, notes, write-back flags, or a user-edited tag from Full mode.
 */
type DemoRangeLimit = {
  readonly min: number;
  readonly max: number;
  readonly absolute_min?: number | null;
};

type DemoNumericValues = {
  readonly relayAmp: number | undefined;
  readonly cyclesSkip: number | undefined;
  readonly cyclesCount: number | undefined;
  readonly noiseProtectionSecs: number | undefined;
  readonly simGain: number | undefined;
  readonly simTau: number | undefined;
  readonly simDeadTime: number | undefined;
  readonly simSeed: number | undefined;
  readonly simPvRangeLow: number | undefined;
  readonly simPvRangeHigh: number | undefined;
  readonly simMvRangeLow: number | undefined;
  readonly simMvRangeHigh: number | undefined;
  readonly simInitialPv: number | undefined;
  readonly simInitialMv: number | undefined;
  readonly simNoise: number | undefined;
};

type DemoRangeValues = {
  readonly pvRangeLow: number;
  readonly pvRangeHigh: number;
  readonly mvRangeLow: number;
  readonly mvRangeHigh: number;
};

function optionalNumber(value: NumOrBlank): number | undefined {
  return typeof value === "number" ? value : undefined;
}

function demoNumericValues(form: FormState): DemoNumericValues {
  return {
    relayAmp: optionalNumber(form.relayAmp),
    cyclesSkip: optionalNumber(form.cyclesSkip),
    cyclesCount: optionalNumber(form.cyclesCount),
    noiseProtectionSecs: optionalNumber(form.noiseProtectionSecs),
    simGain: optionalNumber(form.simGain),
    simTau: optionalNumber(form.simTau),
    simDeadTime: optionalNumber(form.simDeadTime),
    simSeed: optionalNumber(form.simSeed),
    simPvRangeLow: optionalNumber(form.simPvRangeLow),
    simPvRangeHigh: optionalNumber(form.simPvRangeHigh),
    simMvRangeLow: optionalNumber(form.simMvRangeLow),
    simMvRangeHigh: optionalNumber(form.simMvRangeHigh),
    simInitialPv: optionalNumber(form.simInitialPv),
    simInitialMv: optionalNumber(form.simInitialMv),
    simNoise: optionalNumber(form.simNoise),
  };
}

function validateDemoNumericValues(
  values: DemoNumericValues,
  capabilities: SimulatorCapabilities,
): string | undefined {
  const numbers: readonly [number | undefined, DemoRangeLimit, string][] = [
    [values.relayAmp, capabilities.limits.relay_amp, "Relay amplitude"],
    [values.cyclesSkip, capabilities.limits.cycles_skip, "Cycles to skip"],
    [values.cyclesCount, capabilities.limits.cycles_count, "Cycles to count"],
    [
      values.noiseProtectionSecs,
      capabilities.limits.noise_protection_secs,
      "Noise protection",
    ],
    [values.simGain, capabilities.limits.sim_gain, "Process gain"],
    [values.simTau, capabilities.limits.sim_tau, "Time constant"],
    [values.simDeadTime, capabilities.limits.sim_dead_time, "Dead time"],
    [values.simSeed, capabilities.limits.sim_seed, "RNG seed"],
    [values.simPvRangeLow, capabilities.limits.range_endpoint, "PV range low"],
    [
      values.simPvRangeHigh,
      capabilities.limits.range_endpoint,
      "PV range high",
    ],
    [values.simMvRangeLow, capabilities.limits.range_endpoint, "MV range low"],
    [
      values.simMvRangeHigh,
      capabilities.limits.range_endpoint,
      "MV range high",
    ],
  ];
  for (const [value, range, label] of numbers) {
    if (value === undefined) return `${label} is required.`;
    const error = bounded(value, range, label);
    if (typeof error === "string") return error;
  }

  for (const [value, label] of [
    [values.cyclesSkip, "Cycles to skip"],
    [values.cyclesCount, "Cycles to count"],
    [values.noiseProtectionSecs, "Noise protection"],
  ] as const) {
    if (!Number.isInteger(value)) {
      return `${label} must be a whole number.`;
    }
  }
  if (values.simSeed === undefined || !Number.isSafeInteger(values.simSeed)) {
    return "RNG seed must be a whole number.";
  }
  return undefined;
}

function validateDemoSelections(
  form: FormState,
  capabilities: SimulatorCapabilities,
): string | undefined {
  if (!capabilities.templates.includes(form.template)) {
    return "Choose a template supported by the Demo server.";
  }
  if (!capabilities.process_types.includes(form.processType)) {
    return "Choose a process type supported by the Demo server.";
  }
  const controllerTypes = demoControllerTypesFor(
    capabilities,
    form.processType,
  );
  if (!controllerTypes.includes(form.controllerType)) {
    return "Choose a controller type supported for this process.";
  }
  return undefined;
}

function requiredNumber(
  value: number | undefined,
  label: string,
): number | string {
  return value ?? `${label} is required.`;
}

function validateDemoRanges(
  values: DemoNumericValues,
  capabilities: SimulatorCapabilities,
): DemoRangeValues | string {
  const pvRangeLow = requiredNumber(values.simPvRangeLow, "PV range low");
  if (typeof pvRangeLow === "string") return pvRangeLow;
  const pvRangeHigh = requiredNumber(values.simPvRangeHigh, "PV range high");
  if (typeof pvRangeHigh === "string") return pvRangeHigh;
  const mvRangeLow = requiredNumber(values.simMvRangeLow, "MV range low");
  if (typeof mvRangeLow === "string") return mvRangeLow;
  const mvRangeHigh = requiredNumber(values.simMvRangeHigh, "MV range high");
  if (typeof mvRangeHigh === "string") return mvRangeHigh;

  for (const [low, high, label] of [
    [pvRangeLow, pvRangeHigh, "PV"],
    [mvRangeLow, mvRangeHigh, "MV"],
  ] as const) {
    const spanError = bounded(
      high - low,
      capabilities.limits.range_span,
      `${label} range span`,
    );
    if (typeof spanError === "string") return spanError;
  }
  return { pvRangeLow, pvRangeHigh, mvRangeLow, mvRangeHigh };
}

function validateDemoInitialValues(
  values: DemoNumericValues,
  ranges: DemoRangeValues,
  capabilities: SimulatorCapabilities,
): string | undefined {
  const initialPv = values.simInitialPv;
  if (initialPv === undefined) return "Initial PV is required.";
  if (initialPv < ranges.pvRangeLow || initialPv > ranges.pvRangeHigh) {
    return "Initial PV must be within the PV range.";
  }

  const initialMv = values.simInitialMv;
  if (initialMv === undefined) return "Initial MV is required.";
  if (initialMv < ranges.mvRangeLow || initialMv > ranges.mvRangeHigh) {
    return "Initial MV must be within the MV range.";
  }

  const simNoise = values.simNoise;
  if (simNoise === undefined) return "Measurement noise is required.";
  const maxNoise =
    (ranges.pvRangeHigh - ranges.pvRangeLow) *
    capabilities.limits.max_noise_fraction_of_pv_span;
  if (!Number.isFinite(simNoise) || simNoise < 0 || simNoise > maxNoise) {
    return `Measurement noise must be between 0 and ${maxNoise} (${capabilities.limits.max_noise_fraction_of_pv_span * 100}% of the PV span).`;
  }
  return undefined;
}

function demoRequest(
  form: FormState,
  values: DemoNumericValues,
  ranges: DemoRangeValues,
  capabilities: SimulatorCapabilities,
): DemoStartRunRequest {
  return {
    driver: "simulator",
    template: form.template,
    tagname: capabilities.tag_name,
    process_type: form.processType,
    controller_type: form.controllerType,
    relay_amp: values.relayAmp!,
    cycles_skip: values.cyclesSkip,
    cycles_count: values.cyclesCount,
    noise_protection_secs: values.noiseProtectionSecs,
    direction: values.simGain! < 0 ? "direct" : "reverse",
    pv_range_high: ranges.pvRangeHigh,
    pv_range_low: ranges.pvRangeLow,
    mv_range_high: ranges.mvRangeHigh,
    mv_range_low: ranges.mvRangeLow,
    sim_gain: values.simGain!,
    sim_tau: values.simTau!,
    sim_dead_time: values.simDeadTime!,
    sim_noise: values.simNoise!,
    sim_seed: values.simSeed!,
    sim_initial_pv: values.simInitialPv!,
    sim_initial_mv: values.simInitialMv!,
  } satisfies DemoStartRunRequest;
}

export function normalizeSimulatorRequest(
  form: FormState,
  capabilities: SimulatorCapabilities,
): DemoStartRunRequest | string {
  const values = demoNumericValues(form);
  const numericError = validateDemoNumericValues(values, capabilities);
  if (numericError) return numericError;

  const selectionError = validateDemoSelections(form, capabilities);
  if (selectionError) return selectionError;

  const ranges = validateDemoRanges(values, capabilities);
  if (typeof ranges === "string") return ranges;

  const initialValueError = validateDemoInitialValues(
    values,
    ranges,
    capabilities,
  );
  if (initialValueError) return initialValueError;

  return demoRequest(form, values, ranges, capabilities);
}
