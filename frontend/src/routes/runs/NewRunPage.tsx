import { useEffect, useRef, useState } from "react";
import { Link, useLocation, useNavigate } from "react-router";
import {
  useLastRunRequest,
  useRunDraft,
  useSaveRunDraft,
  useStartRun,
} from "../../api/runs";
import type { NewRunDraft, StartRunRequest } from "../../api/runs";
import { useTemplates } from "../../api/templates";
import { userFacingErrorMessage } from "../../api/errors";
import type { components } from "../../api/schema";
import {
  CONTROLLER_TYPE_LABELS,
  DRIVER_LABELS,
  PROCESS_TYPE_LABELS,
  RESPONSE_LEVEL_LABELS,
} from "../../lib/enumLabels";
import { OpcServerDiscovery } from "../../components/OpcServerDiscovery";
import { OpcTagBrowserModal } from "../../components/OpcTagBrowserModal";
import { derivedTagPreview, replaceTagSuffix } from "../../lib/opcTags";
import {
  Button,
  CheckboxField,
  ErrorBanner,
  FormSection,
  NumberField,
  PageHeading,
  SelectField,
  TextAreaField,
  TextField,
} from "../../components/ui";
import { LoopMappingEditor } from "./LoopMappingEditor";
import {
  DEFAULT_TAG_MAPPING_SOURCES,
  DEFAULT_VALUE_MAPPING_SOURCES,
  EMPTY_TAG_OVERRIDES,
  EMPTY_VALUE_TAG_OVERRIDES,
  type NumOrBlank,
  type TagMappingSource,
  type TagOverrideFormState,
  type TagOverrideKey,
  type TagMappingSources,
  type ValueMappingKey,
  type ValueMappingSource,
  type ValueMappingSources,
  type ValueTagOverrideFormState,
} from "./mappingState";

type TuneDriver = components["schemas"]["TuneDriver"];
type ProcessType = components["schemas"]["ProcessType"];
type ControllerType = components["schemas"]["ControllerType"];
type ControllerDirection = components["schemas"]["ControllerDirection"];
type ResponseLevel = components["schemas"]["ResponseLevel"];
type TagOverrides = components["schemas"]["TagOverrides"];

const DRIVERS: readonly TuneDriver[] = ["simulator", "opcda"];
const PROCESS_TYPES: readonly ProcessType[] = [
  "flow",
  "pressure_line",
  "pressure_vessel",
  "level",
  "temperature_mixing",
  "temperature_heat_exchange",
];
const CONTROLLER_TYPES: readonly ControllerType[] = ["p", "pi", "pid"];
const RESPONSE_LEVELS: readonly ResponseLevel[] = [
  "aggressive",
  "moderate",
  "sluggish",
];

/** Mirrors `bhtune_core::ProcessType::allows_pid` — PID is only offered for the two
 * Temperature process types, exactly like the legacy app's dynamic controller-type list. */
const TEMPERATURE_PROCESS_TYPES = new Set<ProcessType>([
  "temperature_mixing",
  "temperature_heat_exchange",
]);

type FormState = {
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
 * Every default here matches `StartRunRequest`'s own `#[serde(default = ...)]` values (see
 * `bhtune-server`'s `routes::runs`) or `bhtune-cli`'s `SimulateArgs` defaults, field-for-field
 * — so a first-time visitor who changes nothing except picking a template gets a working
 * simulator-backed run, and every other pre-filled number is exactly what omitting the
 * matching CLI flag would produce.
 */
const initialForm: FormState = {
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
  // The simulator has no range/direction tags at all, so these five are hard-required
  // whenever `driver` is "simulator" (see `build_loop_tags` in `bhtune-cli`). Defaulted to
  // exactly the same 0-100% span and direction `bhtune simulate`'s CLI convenience path
  // uses, matching the default `driver: "simulator"` above so a first-time visitor can
  // submit immediately.
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

function inferRequestValueSources(
  request: StartRunRequest,
): ValueMappingSources {
  if (request.driver === "simulator") {
    return { ...DEFAULT_VALUE_MAPPING_SOURCES };
  }
  return {
    direction:
      request.direction !== undefined
        ? "fixed"
        : request.tag_overrides?.controller_direction?.trim()
          ? "custom"
          : "tag",
    pvRangeHigh:
      request.pv_range_high !== undefined
        ? "fixed"
        : request.tag_overrides?.upper_pv_range?.trim()
          ? "custom"
          : "tag",
    pvRangeLow:
      request.pv_range_low !== undefined
        ? "fixed"
        : request.tag_overrides?.lower_pv_range?.trim()
          ? "custom"
          : "tag",
    mvRangeHigh:
      request.mv_range_high !== undefined
        ? "fixed"
        : request.tag_overrides?.upper_mv_range?.trim()
          ? "custom"
          : "tag",
    mvRangeLow:
      request.mv_range_low !== undefined
        ? "fixed"
        : request.tag_overrides?.lower_mv_range?.trim()
          ? "custom"
          : "tag",
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
  return {
    direction:
      legacyOpc && draft.direction !== null && draft.direction !== undefined
        ? "fixed"
        : draft.tag_overrides?.controller_direction?.trim()
          ? "custom"
          : "tag",
    pvRangeHigh:
      legacyOpc &&
      draft.pv_range_high !== null &&
      draft.pv_range_high !== undefined
        ? "fixed"
        : draft.tag_overrides?.upper_pv_range?.trim()
          ? "custom"
          : "tag",
    pvRangeLow:
      legacyOpc &&
      draft.pv_range_low !== null &&
      draft.pv_range_low !== undefined
        ? "fixed"
        : draft.tag_overrides?.lower_pv_range?.trim()
          ? "custom"
          : "tag",
    mvRangeHigh:
      legacyOpc &&
      draft.mv_range_high !== null &&
      draft.mv_range_high !== undefined
        ? "fixed"
        : draft.tag_overrides?.upper_mv_range?.trim()
          ? "custom"
          : "tag",
    mvRangeLow:
      legacyOpc &&
      draft.mv_range_low !== null &&
      draft.mv_range_low !== undefined
        ? "fixed"
        : draft.tag_overrides?.lower_mv_range?.trim()
          ? "custom"
          : "tag",
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

/**
 * Converts a stored [`StartRunRequest`] (from a specific run's `original_request`, or the
 * newest-run fallback at `GET /api/runs/last-request`) into `FormState`.
 *
 * `bhtune-cli`'s `RequestSnapshot` (what actually populates `request_json`) always resolves
 * the fields that carry a CLI/server default — `mrft_delay`, `poll_interval_ms`, every
 * timeout, every `sim_*` field, and `yes` — to a concrete value before
 * it's stored, so those are simply copied across; the `?? initialForm...` fallback only ever
 * matters for a foreign/pre-`db-run-request-snapshot` row that somehow lacks the field. Every
 * other optional field (cycles, ranges, direction, connection overrides) is
 * shown *blank* when absent rather than substituting today's hardcoded default — an absent
 * value there specifically means "the engineer relied on a default last time", which is
 * exactly what should be shown again, not silently overwritten. Notes are intentionally
 * excluded from remembered settings, so every new or duplicate tune starts with an empty
 * notes field for fresh operator context.
 */
function formFromRequest(request: StartRunRequest): FormState {
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
    opcDirection: request.driver === "opcda" ? (request.direction ?? "") : "",
    opcPvRangeHigh:
      request.driver === "opcda" ? toNumOrBlank(request.pv_range_high) : "",
    opcPvRangeLow:
      request.driver === "opcda" ? toNumOrBlank(request.pv_range_low) : "",
    opcMvRangeHigh:
      request.driver === "opcda" ? toNumOrBlank(request.mv_range_high) : "",
    opcMvRangeLow:
      request.driver === "opcda" ? toNumOrBlank(request.mv_range_low) : "",
    simDirection:
      request.driver === "simulator"
        ? (request.direction ?? initialForm.simDirection)
        : initialForm.simDirection,
    simPvRangeHigh:
      request.driver === "simulator"
        ? toNumOrBlank(request.pv_range_high)
        : initialForm.simPvRangeHigh,
    simPvRangeLow:
      request.driver === "simulator"
        ? toNumOrBlank(request.pv_range_low)
        : initialForm.simPvRangeLow,
    simMvRangeHigh:
      request.driver === "simulator"
        ? toNumOrBlank(request.mv_range_high)
        : initialForm.simMvRangeHigh,
    simMvRangeLow:
      request.driver === "simulator"
        ? toNumOrBlank(request.mv_range_low)
        : initialForm.simMvRangeLow,
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

/**
 * Converts the mutable saved draft into the form state. `undefined` means an older or
 * hand-written partial draft omitted a field and should use the built-in default; `null` is
 * the explicit representation of a field the engineer cleared while editing.
 */
function formFromDraft(draft: NewRunDraft): FormState {
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
    template:
      draft.template === undefined
        ? initialForm.template
        : (draft.template ?? ""),
    notes: "",
    tagname:
      draft.tagname === undefined ? initialForm.tagname : (draft.tagname ?? ""),
    server: draft.server ?? "",
    bridgeHost: draft.bridge_host ?? "",
    processType: draft.process_type ?? initialForm.processType,
    controllerType: draft.controller_type ?? initialForm.controllerType,
    relayAmp: toNumOrBlank(draft.relay_amp),
    cyclesSkip: toNumOrBlank(draft.cycles_skip),
    cyclesCount: toNumOrBlank(draft.cycles_count),
    noiseProtectionSecs: toNumOrBlank(draft.noise_protection_secs),
    mrftDelay:
      draft.mrft_delay === undefined
        ? initialForm.mrftDelay
        : toNumOrBlank(draft.mrft_delay),
    pollIntervalMs:
      draft.poll_interval_ms === undefined
        ? initialForm.pollIntervalMs
        : toNumOrBlank(draft.poll_interval_ms),
    timeoutSecs:
      draft.timeout_secs === undefined
        ? initialForm.timeoutSecs
        : toNumOrBlank(draft.timeout_secs),
    opTimeoutSecs:
      draft.op_timeout_secs === undefined
        ? initialForm.opTimeoutSecs
        : toNumOrBlank(draft.op_timeout_secs),
    restoreTimeoutSecs:
      draft.restore_timeout_secs === undefined
        ? initialForm.restoreTimeoutSecs
        : toNumOrBlank(draft.restore_timeout_secs),
    tagSources: draftTagSources(draft),
    valueSources,
    valueTagOverrides: formValueTagOverrides(draft.tag_overrides),
    opcDirection: restoreOpcValues ? (draft.direction ?? "") : "",
    opcPvRangeHigh: restoreOpcValues ? toNumOrBlank(draft.pv_range_high) : "",
    opcPvRangeLow: restoreOpcValues ? toNumOrBlank(draft.pv_range_low) : "",
    opcMvRangeHigh: restoreOpcValues ? toNumOrBlank(draft.mv_range_high) : "",
    opcMvRangeLow: restoreOpcValues ? toNumOrBlank(draft.mv_range_low) : "",
    simDirection: simulatorValuesPresent
      ? (draft.source_direction ?? "")
      : legacyOpc
        ? initialForm.simDirection
        : (draft.direction ?? initialForm.simDirection),
    simPvRangeHigh: simulatorValuesPresent
      ? toNumOrBlank(draft.source_pv_range_high)
      : legacyOpc
        ? initialForm.simPvRangeHigh
        : toNumOrBlank(draft.pv_range_high),
    simPvRangeLow: simulatorValuesPresent
      ? toNumOrBlank(draft.source_pv_range_low)
      : legacyOpc
        ? initialForm.simPvRangeLow
        : toNumOrBlank(draft.pv_range_low),
    simMvRangeHigh: simulatorValuesPresent
      ? toNumOrBlank(draft.source_mv_range_high)
      : legacyOpc
        ? initialForm.simMvRangeHigh
        : toNumOrBlank(draft.mv_range_high),
    simMvRangeLow: simulatorValuesPresent
      ? toNumOrBlank(draft.source_mv_range_low)
      : legacyOpc
        ? initialForm.simMvRangeLow
        : toNumOrBlank(draft.mv_range_low),
    simGain:
      draft.sim_gain === undefined
        ? initialForm.simGain
        : toNumOrBlank(draft.sim_gain),
    simTau:
      draft.sim_tau === undefined
        ? initialForm.simTau
        : toNumOrBlank(draft.sim_tau),
    simDeadTime:
      draft.sim_dead_time === undefined
        ? initialForm.simDeadTime
        : toNumOrBlank(draft.sim_dead_time),
    simNoise:
      draft.sim_noise === undefined
        ? initialForm.simNoise
        : toNumOrBlank(draft.sim_noise),
    simSeed:
      draft.sim_seed === undefined
        ? initialForm.simSeed
        : toNumOrBlank(draft.sim_seed),
    simInitialPv:
      draft.sim_initial_pv === undefined
        ? initialForm.simInitialPv
        : toNumOrBlank(draft.sim_initial_pv),
    simInitialMv:
      draft.sim_initial_mv === undefined
        ? initialForm.simInitialMv
        : toNumOrBlank(draft.sim_initial_mv),
    tagOverrides: formTagOverrides(draft.tag_overrides),
    writePid: draft.write_pid ?? "",
    yes: draft.yes ?? initialForm.yes,
  };
}

/** Serializes every editable form field except Notes for the server-side draft. */
function draftFromForm(form: FormState): NewRunDraft {
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

/**
 * Router `state` shape for navigating to this page to duplicate a specific historical run
 * (`RunDetailPage`'s "Duplicate this run" button) — as opposed to a plain visit to
 * `/runs/new`, which uses the saved draft and falls back to
 * `GET /api/runs/last-request` (the *newest* run). Exported so `RunDetailPage` constructs it
 * with type safety rather than an untyped object literal that could silently drift from what
 * this page reads.
 */
export interface DuplicateRunState {
  duplicateRequest: StartRunRequest;
  duplicateFromRunId: number;
}

/** Builds the request body, or returns a client-side validation message instead. Mirrors
 * `StartRunRequest::into_tune_args`'s own checks so most mistakes are caught before the round
 * trip — the server re-validates everything regardless, this is purely for fast feedback. */
function buildRequest(form: FormState): StartRunRequest | string {
  if (!form.template) return "Choose a template.";
  // Tag name is disabled (and therefore excluded from validation) for the simulator driver,
  // which hardcodes its own PV/MV tags and ignores this field entirely.
  if (form.driver !== "simulator" && !form.tagname.trim()) {
    return "Tag name is required.";
  }
  if (form.driver === "opcda" && !form.server.trim()) {
    return "OPC DA server ProgID is required for the opcda driver.";
  }
  if (form.relayAmp === "") return "Relay amplitude is required.";
  if (form.driver === "simulator") {
    if (form.simPvRangeHigh === "") {
      return "PV range high is required for the simulator driver (it has no range tags to read).";
    }
    if (form.simPvRangeLow === "") {
      return "PV range low is required for the simulator driver (it has no range tags to read).";
    }
    if (form.simMvRangeHigh === "") {
      return "MV range high is required for the simulator driver (it has no range tags to read).";
    }
    if (form.simMvRangeLow === "") {
      return "MV range low is required for the simulator driver (it has no range tags to read).";
    }
    if (!form.simDirection) {
      return "Controller direction is required for the simulator driver (it has no direction tag to read).";
    }
  } else {
    if (form.valueSources.direction === "fixed" && !form.opcDirection) {
      return "Controller direction is required when Fixed value is selected.";
    }
    if (
      form.valueSources.direction === "custom" &&
      !form.valueTagOverrides.direction.trim()
    ) {
      return "Controller direction read tag is required when Custom tag is selected.";
    }
    if (
      form.valueSources.pvRangeHigh === "fixed" &&
      form.opcPvRangeHigh === ""
    ) {
      return "PV range high is required when Fixed value is selected.";
    }
    if (form.valueSources.pvRangeLow === "fixed" && form.opcPvRangeLow === "") {
      return "PV range low is required when Fixed value is selected.";
    }
    if (
      form.valueSources.pvRangeHigh === "custom" &&
      !form.valueTagOverrides.pvRangeHigh.trim()
    ) {
      return "PV range high read tag is required when Custom tag is selected.";
    }
    if (
      form.valueSources.pvRangeLow === "custom" &&
      !form.valueTagOverrides.pvRangeLow.trim()
    ) {
      return "PV range low read tag is required when Custom tag is selected.";
    }
    if (
      form.valueSources.mvRangeHigh === "fixed" &&
      form.opcMvRangeHigh === ""
    ) {
      return "MV range high is required when Fixed value is selected.";
    }
    if (form.valueSources.mvRangeLow === "fixed" && form.opcMvRangeLow === "") {
      return "MV range low is required when Fixed value is selected.";
    }
    if (
      form.valueSources.mvRangeHigh === "custom" &&
      !form.valueTagOverrides.mvRangeHigh.trim()
    ) {
      return "MV range high read tag is required when Custom tag is selected.";
    }
    if (
      form.valueSources.mvRangeLow === "custom" &&
      !form.valueTagOverrides.mvRangeLow.trim()
    ) {
      return "MV range low read tag is required when Custom tag is selected.";
    }
  }
  if (form.writePid && !form.yes) {
    return "Enable Allow automatic PID write to apply PID settings without a prompt, or clear the automatic PID setting.";
  }

  return {
    tagname: form.tagname.trim(),
    template: form.template,
    process_type: form.processType,
    controller_type: form.controllerType,
    relay_amp: form.relayAmp,
    cycles_skip: toOptional(form.cyclesSkip),
    cycles_count: toOptional(form.cyclesCount),
    noise_protection_secs: toOptional(form.noiseProtectionSecs),
    mrft_delay: toOptional(form.mrftDelay),
    driver: form.driver,
    bridge_host: form.bridgeHost.trim() || undefined,
    server: form.driver === "opcda" ? form.server.trim() : undefined,
    sim_gain: toOptional(form.simGain),
    sim_tau: toOptional(form.simTau),
    sim_dead_time: toOptional(form.simDeadTime),
    sim_noise: toOptional(form.simNoise),
    sim_seed: toOptional(form.simSeed),
    sim_initial_pv: toOptional(form.simInitialPv),
    sim_initial_mv: toOptional(form.simInitialMv),
    pv_range_high:
      form.driver === "simulator"
        ? toOptional(form.simPvRangeHigh)
        : form.valueSources.pvRangeHigh === "fixed"
          ? toOptional(form.opcPvRangeHigh)
          : undefined,
    pv_range_low:
      form.driver === "simulator"
        ? toOptional(form.simPvRangeLow)
        : form.valueSources.pvRangeLow === "fixed"
          ? toOptional(form.opcPvRangeLow)
          : undefined,
    mv_range_high:
      form.driver === "simulator"
        ? toOptional(form.simMvRangeHigh)
        : form.valueSources.mvRangeHigh === "fixed"
          ? toOptional(form.opcMvRangeHigh)
          : undefined,
    mv_range_low:
      form.driver === "simulator"
        ? toOptional(form.simMvRangeLow)
        : form.valueSources.mvRangeLow === "fixed"
          ? toOptional(form.opcMvRangeLow)
          : undefined,
    direction:
      form.driver === "simulator"
        ? form.simDirection || undefined
        : form.valueSources.direction === "fixed"
          ? form.opcDirection || undefined
          : undefined,
    tag_overrides:
      form.driver === "opcda" ? tagOverridesFromForm(form) : undefined,
    poll_interval_ms: toOptional(form.pollIntervalMs),
    timeout_secs: toOptional(form.timeoutSecs),
    notes: form.notes.trim() || undefined,
    yes: form.yes,
    write_pid: form.writePid || undefined,
    op_timeout_secs: toOptional(form.opTimeoutSecs),
    restore_timeout_secs: toOptional(form.restoreTimeoutSecs),
  };
}

export function NewRunPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const templates = useTemplates();
  const startRun = useStartRun();
  const lastRunRequest = useLastRunRequest();
  const runDraft = useRunDraft();
  const saveRunDraft = useSaveRunDraft();

  // Set only by `RunDetailPage`'s "Duplicate this run" button -- a plain visit to
  // `/runs/new` has no location state and uses the saved-draft/newest-run fallback below.
  const duplicateState = location.state as DuplicateRunState | null | undefined;

  const [form, setForm] = useState<FormState>(() =>
    duplicateState
      ? formFromRequest(duplicateState.duplicateRequest)
      : initialForm,
  );
  const [validationError, setValidationError] = useState<string | null>(null);
  // Tracks *why* the form currently looks the way it does, purely to show an explanatory
  // note -- not read anywhere else. `null` means "still the hardcoded defaults".
  const [prefillSource, setPrefillSource] = useState<
    | { kind: "duplicate"; runId: number }
    | { kind: "draft" }
    | { kind: "last-run" }
    | null
  >(() =>
    duplicateState
      ? { kind: "duplicate", runId: duplicateState.duplicateFromRunId }
      : null,
  );
  const [hydrated, setHydrated] = useState(Boolean(duplicateState));
  const hydratedRef = useRef(Boolean(duplicateState));
  const preserveBlankTemplateRef = useRef(false);
  const [draftLoadError, setDraftLoadError] = useState<string | null>(null);
  const [draftSaveError, setDraftSaveError] = useState<string | null>(null);
  const draftSaveChainRef = useRef(Promise.resolve());
  const saveDraftAsync = saveRunDraft.mutateAsync;
  const [tagBrowserOpen, setTagBrowserOpen] = useState(false);
  const activeTemplate = templates.data?.find((t) => t.name === form.template);

  // Hydrate exactly once. A duplicate is already present in the lazy state initializer;
  // otherwise prefer the mutable draft and only use the newest run's immutable snapshot as a
  // one-time fallback for installations that predate draft persistence.
  useEffect(() => {
    if (hydratedRef.current || duplicateState || runDraft.isPending) return;
    if (runDraft.data === undefined && !runDraft.isError) return;
    if (
      (runDraft.data === null || runDraft.isError) &&
      lastRunRequest.isPending
    ) {
      return;
    }

    hydratedRef.current = true;
    setHydrated(true);
    if (runDraft.isError) {
      setDraftLoadError(
        userFacingErrorMessage(
          runDraft.error,
          "Unable to load the saved Tune draft; using the available fallback.",
        ),
      );
    }
    if (runDraft.data) {
      preserveBlankTemplateRef.current = runDraft.data.template === null;
      setForm(formFromDraft(runDraft.data));
      setPrefillSource({ kind: "draft" });
    } else if (lastRunRequest.data) {
      preserveBlankTemplateRef.current = false;
      setForm(formFromRequest(lastRunRequest.data));
      setPrefillSource({ kind: "last-run" });
    } else {
      preserveBlankTemplateRef.current = false;
    }
  }, [
    duplicateState,
    lastRunRequest.data,
    lastRunRequest.isPending,
    runDraft.data,
    runDraft.error,
    runDraft.isError,
    runDraft.isPending,
  ]);

  // Save the complete form, except Notes, after a short idle period. Each request is chained
  // behind the previous one so a slow older PUT can never finish after a newer PUT and
  // overwrite the latest form state in SQLite.
  useEffect(() => {
    if (!hydrated) return;
    const payload = draftFromForm(form);
    const timer = window.setTimeout(() => {
      draftSaveChainRef.current = draftSaveChainRef.current
        .catch(() => undefined)
        .then(() => saveDraftAsync(payload))
        .then(() => setDraftSaveError(null))
        .catch((error: unknown) => {
          setDraftSaveError(
            userFacingErrorMessage(
              error,
              "Unable to save the Tune draft. Changes will remain in this page until the server is available.",
            ),
          );
        });
    }, 400);
    return () => window.clearTimeout(timer);
  }, [form, hydrated, saveDraftAsync]);

  // Default to the first available template once the list loads, so a first-time visitor
  // doesn't have to know a template name exists before they can start a run at all -- and
  // so "Reset to defaults" (which clears `form.template` back to "") gets a sensible default
  // back rather than an empty dropdown.
  //
  // The gating check reads `prev.template` from inside the *functional* `setForm` updater
  // rather than this effect's own `form.template` closure. That distinction is load-bearing:
  // when `templates.data` and a saved draft resolve in the same React batch, this
  // effect and the hydration effect above both run in the *same* commit, and both see that
  // render's stale, pre-update `form.template` ("") -- reading it directly here would then
  // unconditionally queue an update that clobbers the prefill's own just-queued update with
  // the alphabetically-first template. The functional updater instead evaluates `prev` at
  // *application* time, after the prefill effect's update has already been applied (it's
  // declared first above, so it runs -- and its `setForm` call is queued -- first in this
  // commit), so it correctly sees the just-prefilled template and leaves it alone. Returning
  // `prev` unchanged when there's nothing to do also means this never schedules a wasted
  // re-render on the (common) already-templated path.
  useEffect(() => {
    if (duplicateState || !hydrated || preserveBlankTemplateRef.current) {
      return;
    }
    setForm((prev) => {
      if (prev.template || !templates.data || templates.data.length === 0) {
        return prev;
      }
      return { ...prev, template: templates.data[0].name };
    });
  }, [duplicateState, hydrated, templates.data, form.template]);

  /** Resets every field back to the hardcoded defaults and persists that choice as the draft. */
  function resetToDefaults() {
    setPrefillSource(null);
    preserveBlankTemplateRef.current = false;
    setForm(initialForm);
  }

  function set<K extends keyof FormState>(key: K, value: FormState[K]) {
    setForm((prev) => ({ ...prev, [key]: value }));
  }

  const tagPreviewLabels: Record<TagOverrideKey, string> = {
    processVariable: "Process variable (PV)",
    manipulatedVariable: "Manipulated variable (MV)",
    setpointVariable: "Setpoint",
    controllerMode: "Controller mode",
    modeAttribute: "Mode attribute",
    proportionalConstant: "Proportional constant",
    integralConstant: "Integral constant",
    derivativeConstant: "Derivative constant",
  };

  function templateTagFor(key: TagOverrideKey, tagname = form.tagname): string {
    return (
      (activeTemplate &&
        derivedTagPreview(tagname, activeTemplate).find(
          (row) => row.label === tagPreviewLabels[key],
        )?.tag) ??
      ""
    );
  }

  const valuePreviewLabels: Record<ValueMappingKey, string> = {
    direction: "Controller direction",
    pvRangeHigh: "PV range high",
    pvRangeLow: "PV range low",
    mvRangeHigh: "MV range high",
    mvRangeLow: "MV range low",
  };

  function templateValueTagFor(
    key: ValueMappingKey,
    tagname: string,
    templateName: string,
  ): string {
    const template = templates.data?.find((item) => item.name === templateName);
    return (
      (template &&
        derivedTagPreview(tagname, template).find(
          (row) => row.label === valuePreviewLabels[key],
        )?.tag) ??
      ""
    );
  }

  function setTagSource(key: TagOverrideKey, source: TagMappingSource) {
    setForm((prev) => {
      const value =
        source === "custom" && !prev.tagOverrides[key].trim()
          ? templateTagFor(key, prev.tagname)
          : prev.tagOverrides[key];
      return {
        ...prev,
        tagSources: { ...prev.tagSources, [key]: source },
        tagOverrides: { ...prev.tagOverrides, [key]: value },
      };
    });
  }

  function setTagValue(key: TagOverrideKey, value: string) {
    setForm((prev) => ({
      ...prev,
      tagOverrides: { ...prev.tagOverrides, [key]: value },
    }));
  }

  function setValueSource(key: ValueMappingKey, source: ValueMappingSource) {
    setForm((prev) => {
      if (prev.driver === "simulator") return prev;
      const customTag =
        source === "custom" && !prev.valueTagOverrides[key].trim()
          ? templateValueTagFor(key, prev.tagname, prev.template)
          : prev.valueTagOverrides[key];
      return {
        ...prev,
        valueSources: { ...prev.valueSources, [key]: source },
        valueTagOverrides: {
          ...prev.valueTagOverrides,
          [key]: customTag,
        },
      };
    });
  }

  function setValueTag(key: ValueMappingKey, value: string) {
    setForm((prev) => ({
      ...prev,
      valueTagOverrides: { ...prev.valueTagOverrides, [key]: value },
    }));
  }

  function setMappingValue<
    K extends
      | "opcDirection"
      | "opcPvRangeHigh"
      | "opcPvRangeLow"
      | "opcMvRangeHigh"
      | "opcMvRangeLow"
      | "simDirection"
      | "simPvRangeHigh"
      | "simPvRangeLow"
      | "simMvRangeHigh"
      | "simMvRangeLow",
  >(key: K, value: FormState[K]) {
    setForm((prev) => ({ ...prev, [key]: value }));
  }

  function resetTag(key: TagOverrideKey) {
    setForm((prev) => ({
      ...prev,
      tagSources: { ...prev.tagSources, [key]: "template" },
      tagOverrides: { ...prev.tagOverrides, [key]: "" },
    }));
  }

  function resetValue(key: ValueMappingKey) {
    setForm((prev) => {
      if (prev.driver === "simulator") return prev;
      const valueKey =
        key === "direction"
          ? "opcDirection"
          : key === "pvRangeHigh"
            ? "opcPvRangeHigh"
            : key === "pvRangeLow"
              ? "opcPvRangeLow"
              : key === "mvRangeHigh"
                ? "opcMvRangeHigh"
                : "opcMvRangeLow";
      return {
        ...prev,
        valueSources: { ...prev.valueSources, [key]: "tag" },
        valueTagOverrides: { ...prev.valueTagOverrides, [key]: "" },
        [valueKey]: "",
      };
    });
  }

  function resetMapping() {
    setForm((prev) => ({
      ...prev,
      tagSources: { ...DEFAULT_TAG_MAPPING_SOURCES },
      valueSources: { ...DEFAULT_VALUE_MAPPING_SOURCES },
      tagOverrides: { ...EMPTY_TAG_OVERRIDES },
      valueTagOverrides: { ...EMPTY_VALUE_TAG_OVERRIDES },
      opcDirection: "",
      opcPvRangeHigh: "",
      opcPvRangeLow: "",
      opcMvRangeHigh: "",
      opcMvRangeLow: "",
    }));
  }

  function setDriver(value: TuneDriver) {
    setForm((prev) => {
      if (value !== "simulator") return { ...prev, driver: value };
      return {
        ...prev,
        driver: value,
        simPvRangeHigh: prev.simPvRangeHigh === "" ? 100 : prev.simPvRangeHigh,
        simPvRangeLow: prev.simPvRangeLow === "" ? 0 : prev.simPvRangeLow,
        simMvRangeHigh: prev.simMvRangeHigh === "" ? 100 : prev.simMvRangeHigh,
        simMvRangeLow: prev.simMvRangeLow === "" ? 0 : prev.simMvRangeLow,
        simDirection: prev.simDirection === "" ? "reverse" : prev.simDirection,
      };
    });
  }

  function setTemplate(value: string) {
    setForm((prev) => {
      const nextTemplate = templates.data?.find(
        (template) => template.name === value,
      );
      const tagname = nextTemplate
        ? replaceTagSuffix(prev.tagname, nextTemplate.process_variable_suffix)
        : prev.tagname;
      return { ...prev, template: value, tagname };
    });
  }

  function setProcessType(value: ProcessType) {
    setForm((prev) => ({
      ...prev,
      processType: value,
      // Leaving PID selected for a process type that no longer allows it would silently
      // submit an invalid combination; reset to PI instead, mirroring the legacy app's
      // dynamic controller-type dropdown.
      controllerType:
        prev.controllerType === "pid" && !TEMPERATURE_PROCESS_TYPES.has(value)
          ? "pi"
          : prev.controllerType,
    }));
  }

  const controllerTypeOptions = TEMPERATURE_PROCESS_TYPES.has(form.processType)
    ? CONTROLLER_TYPES
    : CONTROLLER_TYPES.filter((c) => c !== "pid");

  function submitTune() {
    setValidationError(null);
    const request = buildRequest(form);
    if (typeof request === "string") {
      setValidationError(request);
      return;
    }
    startRun.mutate(request, {
      onSuccess: (data) => navigate(`/runs/${data.id}`),
    });
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    submitTune();
  }

  return (
    <div>
      <PageHeading
        title="New tune"
        description="Configure and start a tune."
        actions={
          <>
            <Button
              variant="primary"
              disabled={startRun.isPending}
              onClick={submitTune}
            >
              {startRun.isPending ? "Starting…" : "Start tune"}
            </Button>
            <Link to="/runs">
              <Button>Cancel</Button>
            </Link>
            <Button onClick={resetToDefaults}>Reset to defaults</Button>
          </>
        }
      />

      {prefillSource !== null && (
        <div className="mb-4 rounded-lg border border-slate-700 bg-slate-900/60 px-4 py-3 text-sm text-slate-300">
          {prefillSource.kind === "duplicate"
            ? `Loaded settings from tune #${prefillSource.runId}.`
            : prefillSource.kind === "draft"
              ? "Loaded your saved Tune draft."
              : "Loaded settings from the most recent tune."}{" "}
          Change anything below, or "Reset to defaults" to return to the
          built-in defaults.
        </div>
      )}

      {(draftLoadError || draftSaveError) && (
        <div className="mb-4 space-y-2">
          {draftLoadError && <ErrorBanner message={draftLoadError} />}
          {draftSaveError && <ErrorBanner message={draftSaveError} />}
        </div>
      )}

      {validationError && (
        <div className="mb-4">
          <ErrorBanner message={validationError} />
        </div>
      )}
      {startRun.isError && (
        <div className="mb-4">
          <ErrorBanner
            message={userFacingErrorMessage(
              startRun.error,
              "Unable to start the tune.",
            )}
          />
        </div>
      )}
      {templates.isError && (
        <div className="mb-4">
          <ErrorBanner
            message={userFacingErrorMessage(
              templates.error,
              "Unable to load templates.",
            )}
          />
        </div>
      )}

      <form onSubmit={handleSubmit}>
        <FormSection title="Connection" collapsible defaultOpen>
          <SelectField
            label="Driver"
            value={form.driver}
            onChange={(v) => setDriver(v)}
            options={DRIVERS}
            displayLabel={(v) => DRIVER_LABELS[v]}
          />
          <div>
            <SelectField
              label="Template"
              value={form.template}
              onChange={setTemplate}
              options={(templates.data ?? []).map((t) => t.name)}
              placeholder={
                templates.isPending ? "Loading templates…" : "Choose a template"
              }
            />
            <span className="mt-1 block text-xs text-slate-500">
              {form.driver === "simulator"
                ? "The simulator ignores DCS tag mappings, but the template still formats calculated PID values (for example, gain versus proportional band)."
                : "Maps the connected DCS/PLC's item IDs and PID conventions."}
            </span>
          </div>
          <TextField
            label="Bridge host"
            disabled={form.driver === "simulator"}
            value={form.bridgeHost}
            onChange={(v) => set("bridgeHost", v)}
            placeholder="Defaults to this server's own configured bridge host"
            hint={
              form.driver === "simulator"
                ? "Disabled — the simulator never contacts a gateway."
                : "opcda-bridge gateway address (host:port)."
            }
          />
          <div>
            <TextField
              label="OPC DA server ProgID"
              required={form.driver === "opcda"}
              disabled={form.driver === "simulator"}
              value={form.server}
              onChange={(v) => set("server", v)}
              placeholder="e.g. Matrikon.OPC.Simulation"
              hint={
                form.driver === "simulator"
                  ? "Disabled — the simulator never contacts a gateway."
                  : undefined
              }
            />
            {form.driver === "opcda" && (
              <OpcServerDiscovery
                bridgeHost={form.bridgeHost}
                onSelect={(v) => set("server", v)}
              />
            )}
          </div>
          <div>
            <TextField
              label="Tag name"
              required={form.driver !== "simulator"}
              disabled={form.driver === "simulator"}
              value={form.tagname}
              onChange={(v) => set("tagname", v)}
              hint={
                form.driver === "simulator"
                  ? "Disabled — the simulator hardcodes its own PV/MV tags and ignores this."
                  : "PV tag prefix; the rest of the tag set is derived from it via the template's suffixes."
              }
            />
            {form.driver === "opcda" && (
              <div className="mt-1">
                <Button
                  onClick={() => setTagBrowserOpen(true)}
                  disabled={!form.server.trim()}
                  title={
                    form.server.trim()
                      ? undefined
                      : "Enter an OPC DA server ProgID first."
                  }
                >
                  Browse tags
                </Button>
              </div>
            )}
          </div>
          <TextAreaField
            label="Notes"
            value={form.notes}
            onChange={(v) => set("notes", v)}
            full
            placeholder="Optional context, observations, or follow-up actions"
            hint="Notes can be edited or cleared from the tune history."
          />
        </FormSection>

        <FormSection title="Test parameters" collapsible defaultOpen>
          <SelectField
            label="Process type"
            value={form.processType}
            onChange={setProcessType}
            options={PROCESS_TYPES}
            displayLabel={(v) => PROCESS_TYPE_LABELS[v]}
          />
          <SelectField
            label="Controller type"
            value={form.controllerType}
            onChange={(v) => set("controllerType", v)}
            options={controllerTypeOptions}
            displayLabel={(v) => CONTROLLER_TYPE_LABELS[v]}
          />
          <NumberField
            label="Relay amplitude (%)"
            required
            value={form.relayAmp}
            onChange={(v) => set("relayAmp", v)}
            min={0.1}
            max={50}
            step={0.1}
            hint="0.1–50% of the MV range."
          />
          <div />
          <NumberField
            label="Cycles to skip"
            value={form.cyclesSkip}
            onChange={(v) => set("cyclesSkip", v)}
            min={0}
            step={1}
            hint="Blank = looked up per process type."
          />
          <NumberField
            label="Cycles to count"
            value={form.cyclesCount}
            onChange={(v) => set("cyclesCount", v)}
            min={1}
            step={1}
            hint="Blank = looked up per process type."
          />
          <NumberField
            label="Noise protection (s)"
            value={form.noiseProtectionSecs}
            onChange={(v) => set("noiseProtectionSecs", v)}
            min={0}
            step={1}
            hint="Blank = looked up per process type."
          />
          <NumberField
            label="MRFT delay padding (s)"
            value={form.mrftDelay}
            onChange={(v) => set("mrftDelay", v)}
            min={0}
            step={1}
            hint="Pre/post-test recording-only ticks."
          />
          <NumberField
            label="Poll interval (ms)"
            value={form.pollIntervalMs}
            onChange={(v) => set("pollIntervalMs", v)}
            min={1}
            step={1}
          />
          <NumberField
            label="Run timeout (s)"
            value={form.timeoutSecs}
            onChange={(v) => set("timeoutSecs", v)}
            min={1}
            step={1}
            hint="Hard cap on this run's total duration."
          />
          <NumberField
            label="Communication timeout (s)"
            value={form.opTimeoutSecs}
            onChange={(v) => set("opTimeoutSecs", v)}
            min={1}
            step={1}
            disabled={form.driver === "simulator"}
            hint={
              form.driver === "simulator"
                ? "Disabled — the simulator has no out-of-process I/O to time out."
                : "Cap on any single driver read/write."
            }
          />
          <NumberField
            label="Restore timeout (s)"
            value={form.restoreTimeoutSecs}
            onChange={(v) => set("restoreTimeoutSecs", v)}
            min={1}
            step={1}
            disabled={form.driver === "simulator"}
            hint={
              form.driver === "simulator"
                ? "Disabled — the simulator has no out-of-process I/O to time out."
                : "Cap on restoring the loop afterward."
            }
          />
        </FormSection>

        <FormSection title="Loop mapping" collapsible defaultOpen>
          <LoopMappingEditor
            state={form}
            template={activeTemplate}
            onTagSourceChange={setTagSource}
            onTagChange={setTagValue}
            onValueSourceChange={setValueSource}
            onValueTagChange={setValueTag}
            onValueChange={setMappingValue}
            onResetTag={resetTag}
            onResetValue={resetValue}
            onResetAll={resetMapping}
          />
        </FormSection>

        {form.driver === "simulator" && (
          <FormSection title="Simulator parameters" collapsible defaultOpen>
            <NumberField
              label="Process gain"
              value={form.simGain}
              onChange={(v) => set("simGain", v)}
              step="any"
            />
            <NumberField
              label="Time constant τ (s)"
              value={form.simTau}
              onChange={(v) => set("simTau", v)}
              step="any"
            />
            <NumberField
              label="Dead time (s)"
              value={form.simDeadTime}
              onChange={(v) => set("simDeadTime", v)}
              step="any"
            />
            <NumberField
              label="Measurement noise"
              value={form.simNoise}
              onChange={(v) => set("simNoise", v)}
              step="any"
            />
            <NumberField
              label="RNG seed"
              value={form.simSeed}
              onChange={(v) => set("simSeed", v)}
              min={0}
              step={1}
              hint="Fixed seed = reproducible noise."
            />
            <div />
            <NumberField
              label="Initial PV"
              value={form.simInitialPv}
              onChange={(v) => set("simInitialPv", v)}
              step="any"
            />
            <NumberField
              label="Initial MV"
              value={form.simInitialMv}
              onChange={(v) => set("simInitialMv", v)}
              step="any"
            />
          </FormSection>
        )}

        <FormSection title="Automatic PID settings" collapsible defaultOpen>
          <SelectField
            label="Apply PID settings on completion"
            value={form.writePid}
            onChange={(v) => set("writePid", v)}
            options={RESPONSE_LEVELS}
            displayLabel={(v) => RESPONSE_LEVEL_LABELS[v]}
            placeholder="Do not apply automatically"
            disabled={form.driver === "simulator"}
            hint={
              form.driver === "simulator"
                ? "Disabled — the simulator has no PID constant tags to write to."
                : undefined
            }
          />
          <CheckboxField
            label="Allow automatic PID write"
            checked={form.yes}
            onChange={(v) => set("yes", v)}
            disabled={form.driver === "simulator"}
            hint={
              form.driver === "simulator"
                ? "Disabled — the simulator has no PID constant tags to write to."
                : "Required when automatic PID settings are selected — applying changes to a live loop without a prompt must be deliberate."
            }
          />
        </FormSection>
      </form>

      {tagBrowserOpen && (
        <OpcTagBrowserModal
          bridgeHost={form.bridgeHost}
          opcServer={form.server}
          template={activeTemplate}
          initialTag={form.tagname}
          onClose={() => setTagBrowserOpen(false)}
          onSelect={(tag) => set("tagname", tag)}
        />
      )}
    </div>
  );
}
