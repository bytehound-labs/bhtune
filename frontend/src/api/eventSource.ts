import type { components } from "./schema";

type InitialReadingsResponse = components["schemas"]["InitialReadingsResponse"];
type SampleResponse = components["schemas"]["SampleResponse"];
type SampleQuality = components["schemas"]["SampleQuality"];
type ControllerDirection = components["schemas"]["ControllerDirection"];
type TuneOutcome = components["schemas"]["TuneOutcome"];

type RunStreamDone = { outcome: TuneOutcome };

export type RunStreamEvent =
  | { type: "initial"; data: InitialReadingsResponse }
  | { type: "sample"; data: SampleResponse }
  | { type: "done"; data: RunStreamDone };

type JsonObject = Record<string, unknown>;

const initialNumberFields = [
  "mv_ini",
  "mv_range_high",
  "mv_range_low",
  "pv_ini",
  "pv_range_high",
  "pv_range_low",
] as const;

const mrftStateNumberFields = [
  "counter_all_switches",
  "cycles_completed",
  "cycles_remaining",
  "hysteresis",
  "mv_sign_next_step",
  "mv_value_current",
] as const;

function isJsonObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function isString(value: unknown): value is string {
  return typeof value === "string";
}

function isNullableString(value: unknown): value is string | null {
  return value === null || isString(value);
}

function isOptionalField(
  object: JsonObject,
  field: string,
  isValid: (value: unknown) => boolean,
): boolean {
  return !Object.hasOwn(object, field) || isValid(object[field]);
}

function isControllerDirection(value: unknown): value is ControllerDirection {
  return value === "direct" || value === "reverse";
}

function isSampleQuality(value: unknown): value is SampleQuality {
  return value === "good" || value === "uncertain" || value === "bad";
}

function isTuneOutcome(value: unknown): value is TuneOutcome {
  return (
    value === "running" ||
    value === "completed" ||
    value === "failed" ||
    value === "aborted"
  );
}

function isInitialReadingsResponse(
  value: unknown,
): value is InitialReadingsResponse {
  return (
    isJsonObject(value) &&
    isControllerDirection(value.controller_direction) &&
    initialNumberFields.every((field) => isFiniteNumber(value[field])) &&
    isOptionalField(value, "mode_attribute_raw", isNullableString) &&
    isOptionalField(value, "mode_raw", isNullableString) &&
    isOptionalField(
      value,
      "setpoint_ini",
      (field) => field === null || isFiniteNumber(field),
    )
  );
}

function isSampleResponse(value: unknown): value is SampleResponse {
  if (!isJsonObject(value)) return false;

  const sample = value.sample;
  const state = value.state;
  return (
    isFiniteNumber(value.tick_index) &&
    isSampleQuality(value.pv_quality) &&
    isJsonObject(sample) &&
    isFiniteNumber(sample.pv) &&
    isString(sample.time) &&
    isJsonObject(state) &&
    mrftStateNumberFields.every((field) => isFiniteNumber(state[field]))
  );
}

function isRunStreamDone(value: unknown): value is RunStreamDone {
  return isJsonObject(value) && isTuneOutcome(value.outcome);
}

/** Parses one named SSE payload, returning null for malformed or mismatched events. */
export function parseRunStreamEvent(
  eventType: string,
  eventData: unknown,
): RunStreamEvent | null {
  if (typeof eventData !== "string") return null;

  let payload: unknown;
  try {
    payload = JSON.parse(eventData) as unknown;
  } catch {
    return null;
  }

  switch (eventType) {
    case "initial":
      return isInitialReadingsResponse(payload)
        ? { type: "initial", data: payload }
        : null;
    case "sample":
      return isSampleResponse(payload)
        ? { type: "sample", data: payload }
        : null;
    case "done":
      return isRunStreamDone(payload) ? { type: "done", data: payload } : null;
    default:
      return null;
  }
}
