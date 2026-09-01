import type { components } from "../api/schema";

/**
 * Human-readable display text for the raw snake_case enum values the API sends over the
 * wire (e.g. `pressure_line`). Every `<select>` option and every plain-text rendering of one
 * of these enums (a history table cell, a `<Field>` value) should go through the matching
 * map here rather than showing the wire value directly.
 *
 * `PROCESS_TYPE_LABELS`, `CONTROLLER_TYPE_LABELS`, `DIRECTION_LABELS`, and
 * `RESPONSE_LEVEL_LABELS` reuse the legacy app's own dropdown text verbatim
 * (Flow / Pressure (Line) / Pressure (Vessel) / Level / Temperature (Mixing) / Temperature
 * (Heat Exchange); P / PI / PID; Direct / Reverse; Aggressive / Moderate / Sluggish) --
 * control engineers already know this vocabulary, so there was no reason to invent new
 * wording. `DRIVER_LABELS`/`OUTCOME_LABELS` have no legacy precedent (bhtune is the first
 * place either concept is user-facing) and use plain title-cased text.
 */
export const PROCESS_TYPE_LABELS: Record<
  components["schemas"]["ProcessType"],
  string
> = {
  flow: "Flow",
  pressure_line: "Pressure (Line)",
  pressure_vessel: "Pressure (Vessel)",
  level: "Level",
  temperature_mixing: "Temperature (Mixing)",
  temperature_heat_exchange: "Temperature (Heat Exchange)",
};

export const CONTROLLER_TYPE_LABELS: Record<
  components["schemas"]["ControllerType"],
  string
> = {
  p: "P",
  pi: "PI",
  pid: "PID",
};

export const DIRECTION_LABELS: Record<
  components["schemas"]["ControllerDirection"],
  string
> = {
  direct: "Direct",
  reverse: "Reverse",
};

export const RESPONSE_LEVEL_LABELS: Record<
  components["schemas"]["ResponseLevel"],
  string
> = {
  aggressive: "Aggressive",
  moderate: "Moderate",
  sluggish: "Sluggish",
};

export const DRIVER_LABELS: Record<
  components["schemas"]["TuneDriver"],
  string
> = {
  opcda: "OPC DA",
  simulator: "Simulator",
  replay: "Replay",
};

export const OUTCOME_LABELS: Record<
  components["schemas"]["TuneOutcome"],
  string
> = {
  running: "Running",
  completed: "Completed",
  failed: "Failed",
  aborted: "Aborted",
};

export const SAMPLING_ADEQUACY_LABELS: Record<
  components["schemas"]["SamplingAdequacy"],
  string
> = {
  adequate: "Adequate",
  marginal: "Marginal",
  not_assessed: "Not assessed",
};

export const SAMPLING_ADEQUACY_TONE: Record<
  components["schemas"]["SamplingAdequacy"],
  "success" | "warning" | "neutral"
> = {
  adequate: "success",
  marginal: "warning",
  not_assessed: "neutral",
};

export const SAMPLE_QUALITY_LABELS: Record<
  components["schemas"]["SampleQuality"],
  string
> = {
  good: "Good",
  uncertain: "Uncertain",
  bad: "Bad",
};

/** `<Badge>` tone for each `SampleQuality` -- first needed by the OPC tag-tree browser's
 * "Test connection" read (`ui-opc-browser`), the first place this project renders OPC/sample
 * quality in the UI at all. */
export const SAMPLE_QUALITY_TONE: Record<
  components["schemas"]["SampleQuality"],
  "success" | "warning" | "error"
> = {
  good: "success",
  uncertain: "warning",
  bad: "error",
};

export const TUNING_RESULT_INVALID_REASON_LABELS: Record<
  components["schemas"]["TuningResultInvalidReason"],
  string
> = {
  non_finite_pv_amplitude: "The measured PV amplitude was not a finite number.",
  non_positive_pv_amplitude: "The measured PV amplitude was zero or negative.",
  non_finite_period: "The measured oscillation period was not a finite number.",
  non_positive_period: "The measured oscillation period was zero or negative.",
  non_finite_frequency:
    "The calculated oscillation frequency was not a finite number.",
  non_positive_frequency:
    "The calculated oscillation frequency was zero or negative.",
  non_finite_kp: "The calculated Kp was not a finite number.",
  non_finite_ti_minutes: "The calculated Ti was not a finite number.",
  non_finite_td_minutes: "The calculated Td was not a finite number.",
  non_finite_proportional:
    "The calculated proportional setting was not a finite number.",
  non_finite_integral:
    "The calculated integral setting was not a finite number.",
  non_finite_derivative:
    "The calculated derivative setting was not a finite number.",
};

export const MV_ACTUATION_KIND_LABELS: Record<
  components["schemas"]["MvActuationKind"],
  string
> = {
  relay: "Relay",
  restore: "Restore",
};

export const MV_ACTUATION_STATUS_LABELS: Record<
  components["schemas"]["MvActuationStatus"],
  string
> = {
  pending: "Pending",
  confirmed: "Confirmed",
  failed: "Failed",
  unverified: "Unverified",
  superseded: "Superseded",
};
