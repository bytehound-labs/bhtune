import { expect, test } from "@playwright/test";
import {
  DEFAULT_TREND_POLL_INTERVAL_MS,
  TREND_STARTUP_INTERVALS,
  composeTrendPoints,
  trendXRange,
} from "../src/lib/trend";
import type { RunDetailResponse, SampleResponse } from "../src/api/runs";

type InitialReadings = NonNullable<RunDetailResponse["initial_readings"]>;

function sample(time: string, pv: number, mv: number): SampleResponse {
  return {
    pv_quality: "good",
    sample: { time, pv },
    state: {
      counter_all_switches: 0,
      cycles_completed: 0,
      cycles_remaining: 1,
      hysteresis: 0,
      mv_sign_next_step: 1,
      mv_value_current: mv,
    },
    tick_index: 0,
  };
}

const initialReadings: InitialReadings = {
  controller_direction: "reverse",
  mode_attribute_raw: null,
  mode_raw: "MAN",
  mv_ini: 10,
  mv_range_high: 100,
  mv_range_low: 0,
  pv_ini: 20,
  pv_range_high: 100,
  pv_range_low: 0,
  setpoint_ini: 25,
};

test.describe("trend x-axis range", () => {
  test("reserves twelve configured poll intervals for short trends", () => {
    const range = trendXRange(100, 104, 800);

    expect(range[0]).toBe(100);
    expect(range[1]).toBe(100 + (TREND_STARTUP_INTERVALS * 800) / 1000);
  });

  test("keeps the real maximum once the trend exceeds the startup horizon", () => {
    expect(trendXRange(100, 120, 800)).toEqual([100, 120]);
  });

  test("uses the default poll interval for missing or invalid snapshots", () => {
    const expectedMax =
      100 + (TREND_STARTUP_INTERVALS * DEFAULT_TREND_POLL_INTERVAL_MS) / 1000;

    for (const pollIntervalMs of [
      undefined,
      null,
      0,
      -1,
      Number.NaN,
      Number.POSITIVE_INFINITY,
    ]) {
      expect(trendXRange(100, 104, pollIntervalMs)[1]).toBe(expectedMax);
    }
  });

  test("adds initial and terminal restored-MV presentation points", () => {
    const points = composeTrendPoints(
      [sample("2026-01-01T00:00:00.800Z", 22, 30)],
      initialReadings,
      "2026-01-01T00:00:00.000Z",
      "2026-01-01T00:00:01.600Z",
      true,
    );

    expect(points).toEqual([
      {
        time: "2026-01-01T00:00:00.000Z",
        pv: 20,
        mv: 10,
      },
      {
        time: "2026-01-01T00:00:00.800Z",
        pv: 22,
        mv: 30,
      },
      {
        time: "2026-01-01T00:00:01.600Z",
        pv: 22,
        mv: 10,
      },
    ]);
  });
});
