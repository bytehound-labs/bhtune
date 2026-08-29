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
  const intervalMs = resolvePollInterval(pollIntervalMs);
  const startupHorizonSeconds = (TREND_STARTUP_INTERVALS * intervalMs) / 1000;

  return [dataMin, Math.max(dataMax, dataMin + startupHorizonSeconds)];
}

function resolvePollInterval(
  pollIntervalMs: number | null | undefined,
): number {
  if (
    typeof pollIntervalMs === "number" &&
    Number.isFinite(pollIntervalMs) &&
    pollIntervalMs > 0
  ) {
    return pollIntervalMs;
  }
  return DEFAULT_TREND_POLL_INTERVAL_MS;
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
  const points = samples.map(toTrendPoint);

  if (initialReadings) {
    prependInitialReading(points, initialReadings, startedAt);
  }

  if (includeRestoredMv) {
    appendRestoredMv(points, samples, initialReadings, completedAt);
  }

  return points;
}

function toTrendPoint(sample: SampleResponse): TrendPoint {
  return {
    time: sample.sample.time,
    pv: sample.sample.pv,
    mv: sample.state.mv_value_current,
  };
}

function prependInitialReading(
  points: TrendPoint[],
  initialReadings: NonNullable<RunDetailResponse["initial_readings"]>,
  startedAt: string,
) {
  const firstPoint = points.at(0);
  const firstSampleTime = firstPoint ? Date.parse(firstPoint.time) : Number.NaN;
  const initialTime = initialBoundaryTime(
    Date.parse(startedAt),
    firstSampleTime,
  );

  if (Number.isFinite(initialTime)) {
    points.unshift({
      time: new Date(initialTime).toISOString(),
      pv: initialReadings.pv_ini,
      mv: initialReadings.mv_ini,
    });
  }
}

function initialBoundaryTime(
  startedTime: number,
  firstSampleTime: number,
): number {
  const sampleStartsBeforeRun =
    Number.isFinite(firstSampleTime) && startedTime >= firstSampleTime;
  if (!Number.isFinite(startedTime) || sampleStartsBeforeRun) {
    return firstSampleTime - 1;
  }
  return startedTime;
}

function appendRestoredMv(
  points: TrendPoint[],
  samples: readonly SampleResponse[],
  initialReadings: RunDetailResponse["initial_readings"],
  completedAt: string | null | undefined,
) {
  if (!initialReadings || samples.length === 0) return;

  const lastSample = samples.at(-1);
  if (!lastSample) return;

  const lastSampleTime = Date.parse(lastSample.sample.time);
  const completedTime = completedAt ? Date.parse(completedAt) : Number.NaN;
  if (!Number.isFinite(lastSampleTime) || !Number.isFinite(completedTime)) {
    return;
  }

  points.push({
    time: new Date(Math.max(completedTime, lastSampleTime + 1)).toISOString(),
    pv: lastSample.sample.pv,
    mv: initialReadings.mv_ini,
  });
}
