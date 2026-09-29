import type { NewRunDraft } from "../../api/runs";
import type {
  ControllerDirection,
  NumOrBlank,
  TagMappingSources,
  ValueMappingSources,
} from "./mappingState";
import {
  formTagOverrides,
  formValueTagOverrides,
  inferTagSources,
  initialForm,
  processDefaultFields,
  tagOverridesFromForm,
  toNullable,
  toNumOrBlank,
  valueSource,
  type FormState,
} from "./newRunFormState";

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
 * except for process defaults and the required relay amplitude, which resolve to their
 * built-in defaults.
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
    relayAmp: draft.relay_amp ?? initialForm.relayAmp,
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

const DRAFT_SAVE_DELAY_MS = 400;

export function scheduleDemoDraftSave(
  form: FormState,
  snapshot: { current: string },
  save: (payload: NewRunDraft, savedAt?: number) => boolean,
): (() => void) | undefined {
  const payload = demoDraftFromForm(form);
  const serialized = JSON.stringify(payload);
  if (serialized === snapshot.current) return undefined;
  const timer = window.setTimeout(() => {
    if (save(payload)) snapshot.current = serialized;
  }, DRAFT_SAVE_DELAY_MS);
  return () => window.clearTimeout(timer);
}

export function scheduleFullDraftSave(
  form: FormState,
  chain: { current: Promise<void> },
  save: (payload: NewRunDraft) => Promise<unknown>,
  onSaved: () => void,
  onError: (error: unknown) => void,
): () => void {
  const payload = draftFromForm(form);
  const timer = window.setTimeout(() => {
    chain.current = chain.current
      .catch(() => undefined)
      .then(() => save(payload))
      .then(() => {
        onSaved();
        return undefined;
      })
      .catch((error: unknown) => {
        onError(error);
      });
  }, DRAFT_SAVE_DELAY_MS);
  return () => window.clearTimeout(timer);
}
