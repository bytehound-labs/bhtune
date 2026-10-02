export const HISTORY_PAGE_SIZE = 50;

export const HISTORY_PROCESS_TYPES = [
  "flow",
  "pressure_line",
  "pressure_vessel",
  "level",
  "temperature_mixing",
  "temperature_heat_exchange",
] as const;
export const HISTORY_OUTCOMES = [
  "running",
  "completed",
  "failed",
  "aborted",
] as const;
export const HISTORY_DRIVERS = ["opcda", "simulator", "replay"] as const;

export type HistoryUrlState = {
  readonly processType: (typeof HISTORY_PROCESS_TYPES)[number] | "";
  readonly outcome: (typeof HISTORY_OUTCOMES)[number] | "";
  readonly driver: (typeof HISTORY_DRIVERS)[number] | "";
  readonly offset: number;
};

type HistoryUrlParameter = "process_type" | "outcome" | "driver" | "offset";

export type ParsedHistoryUrl = {
  readonly state: HistoryUrlState;
  readonly invalidParameters: readonly HistoryUrlParameter[];
};

const HISTORY_FILTER_PARAMETERS = [
  "process_type",
  "outcome",
  "driver",
] as const;

const EMPTY_HISTORY_STATE: HistoryUrlState = {
  processType: "",
  outcome: "",
  driver: "",
  offset: 0,
};

function readFilter<Value extends string>(
  searchParams: URLSearchParams,
  parameter: Exclude<HistoryUrlParameter, "offset">,
  allowedValues: readonly Value[],
  unavailable: boolean,
  invalidParameters: Set<HistoryUrlParameter>,
): Value | "" {
  const values = searchParams.getAll(parameter);
  if (values.length === 0) return "";
  if (unavailable && values.some((value) => value !== "")) {
    invalidParameters.add(parameter);
    return "";
  }
  if (values.length > 1) {
    invalidParameters.add(parameter);
    return "";
  }

  const value = values[0];
  if (value === undefined || value === "") return "";
  if (allowedValues.includes(value as Value)) return value as Value;

  invalidParameters.add(parameter);
  return "";
}

function readOffset(
  searchParams: URLSearchParams,
  invalidParameters: Set<HistoryUrlParameter>,
): number {
  const values = searchParams.getAll("offset");
  if (values.length === 0) return 0;
  if (values.length > 1) {
    invalidParameters.add("offset");
    return 0;
  }

  const value = values[0];
  if (value === undefined || value === "") return 0;
  if (!/^(0|[1-9]\d*)$/.test(value)) {
    invalidParameters.add("offset");
    return 0;
  }

  const offset = Number(value);
  if (!Number.isSafeInteger(offset) || offset % HISTORY_PAGE_SIZE !== 0) {
    invalidParameters.add("offset");
    return 0;
  }
  return offset;
}

export function parseHistoryUrlState(
  searchParams: URLSearchParams,
  demo: boolean,
): ParsedHistoryUrl {
  const invalidParameters = new Set<HistoryUrlParameter>();
  const state: HistoryUrlState = {
    processType: readFilter(
      searchParams,
      "process_type",
      HISTORY_PROCESS_TYPES,
      demo,
      invalidParameters,
    ),
    outcome: readFilter(
      searchParams,
      "outcome",
      HISTORY_OUTCOMES,
      demo,
      invalidParameters,
    ),
    driver: readFilter(
      searchParams,
      "driver",
      HISTORY_DRIVERS,
      demo,
      invalidParameters,
    ),
    offset: readOffset(searchParams, invalidParameters),
  };
  return { state, invalidParameters: [...invalidParameters] };
}

export function historyUrlSearchParams(
  current: URLSearchParams,
  state: HistoryUrlState,
  demo: boolean,
): URLSearchParams {
  const searchParams = new URLSearchParams(current);
  for (const parameter of [...HISTORY_FILTER_PARAMETERS, "offset"]) {
    searchParams.delete(parameter);
  }

  if (!demo && state.processType) {
    searchParams.set("process_type", state.processType);
  }
  if (!demo && state.outcome) {
    searchParams.set("outcome", state.outcome);
  }
  if (!demo && state.driver) {
    searchParams.set("driver", state.driver);
  }
  if (state.offset > 0) {
    searchParams.set("offset", String(state.offset));
  }
  return searchParams;
}

export function emptyHistoryUrlState(): HistoryUrlState {
  return EMPTY_HISTORY_STATE;
}
