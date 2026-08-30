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
