import type { RunDetailResponse, SampleResponse } from "../api/runs";

export interface TrendPoint {
  time: string;
  pv: number;
  mv: number;
}

export const DEFAULT_TREND_POLL_INTERVAL_MS = 800;
export const TREND_STARTUP_INTERVALS = 12;

/**
 * Keeps short trends left-anchored by reserving a horizon of expected samples. The values
 * are Unix seconds because this helper is used by uPlot's time scale.
 */
export function trendXRange(
  dataMin: number,
  dataMax: number,
  pollIntervalMs: number | null | undefined,
): [number, number] {
  const intervalMs =
    typeof pollIntervalMs === "number" &&
    Number.isFinite(pollIntervalMs) &&
    pollIntervalMs > 0
      ? pollIntervalMs
      : DEFAULT_TREND_POLL_INTERVAL_MS;
  const startupHorizonSeconds = (TREND_STARTUP_INTERVALS * intervalMs) / 1000;

  return [dataMin, Math.max(dataMax, dataMin + startupHorizonSeconds)];
}

/**
 * Builds the points shown by a run trend. Initial readings and the terminal restored-MV
 * point are presentation-only boundaries; persisted samples remain unchanged for history
 * and export.
 */
export function composeTrendPoints(
  samples: readonly SampleResponse[],
  initialReadings: RunDetailResponse["initial_readings"],
  startedAt: string,
  completedAt: string | null | undefined,
  includeRestoredMv: boolean,
): TrendPoint[] {
  const points = samples.map((sample): TrendPoint => ({
    time: sample.sample.time,
    pv: sample.sample.pv,
    mv: sample.state.mv_value_current,
  }));

  if (initialReadings) {
    const firstSampleTime = points[0] ? Date.parse(points[0].time) : Number.NaN;
    const startedTime = Date.parse(startedAt);
    const initialTime = Number.isFinite(startedTime)
      ? Number.isFinite(firstSampleTime) && startedTime >= firstSampleTime
        ? firstSampleTime - 1
        : startedTime
      : firstSampleTime - 1;

    if (Number.isFinite(initialTime)) {
      points.unshift({
        time: new Date(initialTime).toISOString(),
        pv: initialReadings.pv_ini,
        mv: initialReadings.mv_ini,
      });
    }
  }

  if (includeRestoredMv && initialReadings && samples.length > 0) {
    const lastSample = samples[samples.length - 1];
    const lastSampleTime = Date.parse(lastSample.sample.time);
    const completedTime = completedAt ? Date.parse(completedAt) : Number.NaN;

    if (Number.isFinite(lastSampleTime) && Number.isFinite(completedTime)) {
      points.push({
        time: new Date(
          Math.max(completedTime, lastSampleTime + 1),
        ).toISOString(),
        pv: lastSample.sample.pv,
        mv: initialReadings.mv_ini,
      });
    }
  }

  return points;
}
