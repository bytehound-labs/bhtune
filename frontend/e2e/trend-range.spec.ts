import { expect, test } from "@playwright/test";
import {
  DEFAULT_TREND_POLL_INTERVAL_MS,
  TREND_STARTUP_INTERVALS,
  trendXRange,
} from "../src/lib/trend";

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
});
