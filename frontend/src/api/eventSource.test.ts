import { describe, expect, it } from "vitest";
import type { components } from "./schema";
import { parseRunStreamEvent } from "./eventSource";

type InitialReadingsResponse = components["schemas"]["InitialReadingsResponse"];
type SampleResponse = components["schemas"]["SampleResponse"];

const initialReadings: InitialReadingsResponse = {
  controller_direction: "reverse",
  mode_attribute_raw: null,
  mode_raw: "MAN",
  mv_ini: 50,
  mv_range_high: 100,
  mv_range_low: 0,
  pv_ini: 40,
  pv_range_high: 100,
  pv_range_low: 0,
  setpoint_ini: 45,
};

const sample: SampleResponse = {
  pv_quality: "good",
  sample: {
    pv: 42,
    time: "2026-09-29T08:00:00Z",
  },
  state: {
    counter_all_switches: 1,
    cycles_completed: 0,
    cycles_remaining: 2,
    hysteresis: 1.25,
    mv_sign_next_step: 1,
    mv_value_current: 49,
  },
  tick_index: 0,
};

function serialize(payload: unknown): string {
  return JSON.stringify(payload);
}

describe("parseRunStreamEvent", () => {
  it("accepts a valid initial-readings event", () => {
    expect(parseRunStreamEvent("initial", serialize(initialReadings))).toEqual({
      type: "initial",
      data: initialReadings,
    });
  });

  it("accepts a valid sample event", () => {
    expect(parseRunStreamEvent("sample", serialize(sample))).toEqual({
      type: "sample",
      data: sample,
    });
  });

  it("accepts a valid done event", () => {
    const done = { outcome: "completed" } satisfies {
      outcome: components["schemas"]["TuneOutcome"];
    };

    expect(parseRunStreamEvent("done", serialize(done))).toEqual({
      type: "done",
      data: done,
    });
  });

  it.each([
    ["initial", { ...initialReadings, pv_ini: undefined }],
    ["sample", { ...sample, sample: { pv: 42 } }],
    ["done", {}],
  ])(
    "ignores an event with missing required fields (%s)",
    (eventType, payload) => {
      expect(parseRunStreamEvent(eventType, serialize(payload))).toBeNull();
    },
  );

  it.each([
    ["initial", { ...initialReadings, mv_ini: "50" }],
    [
      "sample",
      {
        ...sample,
        state: { ...sample.state, cycles_completed: "0" },
      },
    ],
    ["done", { outcome: "finished" }],
  ])(
    "ignores an event with fields of the wrong type (%s)",
    (eventType, payload) => {
      expect(parseRunStreamEvent(eventType, serialize(payload))).toBeNull();
    },
  );

  it("ignores invalid JSON and leaves the event type non-terminal", () => {
    expect(parseRunStreamEvent("sample", "{")).toBeNull();
  });

  it("ignores non-string payloads and unknown event types", () => {
    expect(parseRunStreamEvent("sample", { tick_index: 0 })).toBeNull();
    expect(parseRunStreamEvent("heartbeat", "{}")).toBeNull();
  });
});
