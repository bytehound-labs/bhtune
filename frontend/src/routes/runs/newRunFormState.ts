import type { NewRunDraft, StartRunRequest } from "../../api/runs";
import type { components } from "../../api/schema";
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
  mrftDelay: NumOrBlank;
  pollIntervalMs: NumOrBlank;
  timeoutSecs: NumOrBlank;
  opTimeoutSecs: NumOrBlank;
  restoreTimeoutSecs: NumOrBlank;
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
  cyclesSkip: "",
  cyclesCount: "",
  noiseProtectionSecs: "",
  mrftDelay: 0,
  pollIntervalMs: 800,
  timeoutSecs: 3600,
  opTimeoutSecs: 30,
  restoreTimeoutSecs: 30,
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

function toOptional(value: NumOrBlank): number | undefined {
  return value === "" ? undefined : value;
}

function toNumOrBlank(value: number | null | undefined): NumOrBlank {
  return value ?? "";
}

function toNullable(value: NumOrBlank): number | null {
  return value === "" ? null : value;
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
 * blank when the original request omitted them; resolved server defaults are copied through.
 */
export function formFromRequest(request: StartRunRequest): FormState {
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
    cyclesSkip: toNumOrBlank(request.cycles_skip),
    cyclesCount: toNumOrBlank(request.cycles_count),
    noiseProtectionSecs: toNumOrBlank(request.noise_protection_secs),
    mrftDelay: request.mrft_delay ?? initialForm.mrftDelay,
    pollIntervalMs: request.poll_interval_ms ?? initialForm.pollIntervalMs,
    timeoutSecs: request.timeout_secs ?? initialForm.timeoutSecs,
    opTimeoutSecs: request.op_timeout_secs ?? initialForm.opTimeoutSecs,
    restoreTimeoutSecs:
      request.restore_timeout_secs ?? initialForm.restoreTimeoutSecs,
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
 * draft omitted a field and should use the built-in default; `null` means it was cleared.
 */
export function formFromDraft(draft: NewRunDraft): FormState {
  const driver = draft.driver ?? initialForm.driver;
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

  return {
    driver,
    template: draftText(draft.template, initialForm.template),
    notes: "",
    tagname: draftText(draft.tagname, initialForm.tagname),
    server: draft.server ?? "",
    bridgeHost: draft.bridge_host ?? "",
    processType: draft.process_type ?? initialForm.processType,
    controllerType: draft.controller_type ?? initialForm.controllerType,
    relayAmp: toNumOrBlank(draft.relay_amp),
    cyclesSkip: toNumOrBlank(draft.cycles_skip),
    cyclesCount: toNumOrBlank(draft.cycles_count),
    noiseProtectionSecs: toNumOrBlank(draft.noise_protection_secs),
    mrftDelay: draftNumber(draft.mrft_delay, initialForm.mrftDelay),
    pollIntervalMs: draftNumber(
      draft.poll_interval_ms,
      initialForm.pollIntervalMs,
    ),
    timeoutSecs: draftNumber(draft.timeout_secs, initialForm.timeoutSecs),
    opTimeoutSecs: draftNumber(
      draft.op_timeout_secs,
      initialForm.opTimeoutSecs,
    ),
    restoreTimeoutSecs: draftNumber(
      draft.restore_timeout_secs,
      initialForm.restoreTimeoutSecs,
    ),
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
    mrft_delay: toNullable(form.mrftDelay),
    poll_interval_ms: toNullable(form.pollIntervalMs),
    timeout_secs: toNullable(form.timeoutSecs),
    op_timeout_secs: toNullable(form.opTimeoutSecs),
    restore_timeout_secs: toNullable(form.restoreTimeoutSecs),
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
  if (
    form.driver === "opcda" &&
    form.restoreTimeoutSecs !== "" &&
    form.restoreTimeoutSecs < 4
  ) {
    return "Restore timeout must be at least 4 seconds for OPC DA MV confirmation.";
  }
  if (form.relayAmp === "") return "Relay amplitude is required.";
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

  return {
    tagname: form.tagname.trim(),
    template: form.template,
    process_type: form.processType,
    controller_type: form.controllerType,
    relay_amp: relayAmp,
    cycles_skip: toOptional(form.cyclesSkip),
    cycles_count: toOptional(form.cyclesCount),
    noise_protection_secs: toOptional(form.noiseProtectionSecs),
    mrft_delay: toOptional(form.mrftDelay),
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
    poll_interval_ms: toOptional(form.pollIntervalMs),
    timeout_secs: toOptional(form.timeoutSecs),
    notes: form.notes.trim() || undefined,
    yes: form.yes,
    write_pid: form.writePid || undefined,
    op_timeout_secs: toOptional(form.opTimeoutSecs),
    restore_timeout_secs: toOptional(form.restoreTimeoutSecs),
  };
}
