import type { SimulatorCapabilities } from "../../api/capabilities";
import type { StartRunRequest } from "../../api/runs";
import {
  DEFAULT_VALUE_MAPPING_SOURCES,
  type ControllerDirection,
  type NumOrBlank,
  type TagOverrideKey,
  type ValueMappingKey,
  type ValueMappingSource,
  type ValueMappingSources,
} from "./mappingState";
import {
  demoControllerTypesFor,
  formTagOverrides,
  formValueTagOverrides,
  inferTagSources,
  initialForm,
  processDefaultFields,
  tagOverridesFromForm,
  toNumOrBlank,
  toOptional,
  valueSource,
  type DemoStartRunRequest,
  type FormState,
  type ProcessDefaults,
  type TagOverrides,
  type TuneDriver,
} from "./newRunFormState";

export type ValidationFieldKey =
  | keyof FormState
  | `tagOverrides.${TagOverrideKey}`
  | `valueTagOverrides.${ValueMappingKey}`;

const OPC_VALUE_FORM_FIELDS: Record<ValueMappingKey, ValidationFieldKey> = {
  direction: "opcDirection",
  pvRangeHigh: "opcPvRangeHigh",
  pvRangeLow: "opcPvRangeLow",
  mvRangeHigh: "opcMvRangeHigh",
  mvRangeLow: "opcMvRangeLow",
};

const SIMULATOR_VALUE_FORM_FIELDS: Record<ValueMappingKey, ValidationFieldKey> =
  {
    direction: "simDirection",
    pvRangeHigh: "simPvRangeHigh",
    pvRangeLow: "simPvRangeLow",
    mvRangeHigh: "simMvRangeHigh",
    mvRangeLow: "simMvRangeLow",
  };

function valueValidationField(
  form: FormState,
  key: ValueMappingKey,
): ValidationFieldKey {
  if (form.driver === "simulator") return SIMULATOR_VALUE_FORM_FIELDS[key];
  if (form.valueSources[key] === "custom") {
    return `valueTagOverrides.${key}`;
  }
  return OPC_VALUE_FORM_FIELDS[key];
}

const STATIC_VALIDATION_FIELD_PREFIXES: readonly (readonly [
  string,
  ValidationFieldKey,
])[] = [
  ["Tag name is required.", "tagname"],
  ["OPC DA server ProgID", "server"],
  ["Relay amplitude", "relayAmp"],
  ["Cycles to skip", "cyclesSkip"],
  ["Cycles to count", "cyclesCount"],
  ["Noise protection", "noiseProtectionSecs"],
  ["Process gain", "simGain"],
  ["Time constant", "simTau"],
  ["Dead time", "simDeadTime"],
  ["RNG seed", "simSeed"],
  ["Choose a process type", "processType"],
  ["Choose a controller type", "controllerType"],
  ["Enable Allow automatic PID write", "yes"],
  ["Initial PV", "simInitialPv"],
  ["Initial MV", "simInitialMv"],
  ["Measurement noise", "simNoise"],
  ["PV range span", "simPvRangeHigh"],
  ["MV range span", "simMvRangeHigh"],
];

const VALUE_VALIDATION_PREFIXES: readonly (readonly [
  string,
  ValueMappingKey,
])[] = [
  ["Controller direction", "direction"],
  ["PV range high", "pvRangeHigh"],
  ["PV range low", "pvRangeLow"],
  ["MV range high", "mvRangeHigh"],
  ["MV range low", "mvRangeLow"],
];

/** Maps client validation feedback to the form control that can resolve it. */
export function validationFieldForError(
  form: FormState,
  message: string,
): ValidationFieldKey | undefined {
  if (
    message === "Choose a template." ||
    message.startsWith("Choose a template supported")
  ) {
    return "template";
  }

  const staticField = STATIC_VALIDATION_FIELD_PREFIXES.find(([prefix]) =>
    message.startsWith(prefix),
  );
  if (staticField) return staticField[1];

  const valueMapping = VALUE_VALIDATION_PREFIXES.find(([prefix]) =>
    message.startsWith(prefix),
  );
  return valueMapping ? valueValidationField(form, valueMapping[1]) : undefined;
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
  if (form.driver === "opcda" && form.writePid && !form.yes) {
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
  },
  label: string,
): number | string {
  if (!Number.isFinite(value) || value < range.min || value > range.max) {
    return `${label} must be between ${range.min} and ${range.max}.`;
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
    direction: form.simDirection || capabilities.defaults.direction,
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
