import { describe, expect, it } from "vitest";
import type { RunDetailResponse, SampleResponse } from "../api/runs";
import { composeTrendPoints } from "./trend";

const startedAt = "2026-01-02T00:00:00.000Z";

const initialReadings: NonNullable<RunDetailResponse["initial_readings"]> = {
  controller_direction: "reverse",
  mv_ini: 25,
  mv_range_high: 100,
  mv_range_low: 0,
  pv_ini: 45,
  pv_range_high: 100,
  pv_range_low: 0,
};

function sample(
  tickIndex: number,
  time: string,
  pv: number,
  mv: number,
): SampleResponse {
  return {
    tick_index: tickIndex,
    pv_quality: "good",
    sample: { time, pv },
    state: {
      counter_all_switches: tickIndex,
      cycles_completed: 0,
      cycles_remaining: 2,
      hysteresis: 0,
      mv_sign_next_step: 0,
      mv_value_current: mv,
    },
  };
}

describe("composeTrendPoints", () => {
  it("adds initial and restored-MV boundaries around persisted samples", () => {
    const samples = [
      sample(0, "2026-01-02T00:00:00.800Z", 46, 60),
      sample(1, "2026-01-02T00:00:01.600Z", 49, 40),
    ];

    expect(
      composeTrendPoints(
        samples,
        initialReadings,
        startedAt,
        "2026-01-02T00:00:02.000Z",
        true,
      ),
    ).toEqual([
      { time: startedAt, pv: 45, mv: 25 },
      { time: "2026-01-02T00:00:00.800Z", pv: 46, mv: 60 },
      { time: "2026-01-02T00:00:01.600Z", pv: 49, mv: 40 },
      { time: "2026-01-02T00:00:02.000Z", pv: 49, mv: 25 },
    ]);
    expect(samples).toHaveLength(2);
  });

  it("places the initial boundary just before a sample that predates the run", () => {
    expect(
      composeTrendPoints(
        [sample(0, "2026-01-02T00:00:00.800Z", 46, 60)],
        initialReadings,
        "2026-01-02T00:00:00.900Z",
        null,
        false,
      ),
    ).toEqual([
      { time: "2026-01-02T00:00:00.799Z", pv: 45, mv: 25 },
      { time: "2026-01-02T00:00:00.800Z", pv: 46, mv: 60 },
    ]);
  });

  it("keeps a restored point after the final sample if completion predates it", () => {
    const points = composeTrendPoints(
      [
        sample(0, "2026-01-02T00:00:00.800Z", 46, 60),
        sample(1, "2026-01-02T00:00:01.600Z", 49, 40),
      ],
      initialReadings,
      startedAt,
      "2026-01-02T00:00:01.500Z",
      true,
    );

    expect(points.at(-1)).toEqual({
      time: "2026-01-02T00:00:01.601Z",
      pv: 49,
      mv: 25,
    });
  });

  it("does not add presentation boundaries without samples or readings", () => {
    expect(
      composeTrendPoints([], null, startedAt, "2026-01-02T00:00:02.000Z", true),
    ).toEqual([]);
  });
});
