import { describe, expect, it } from "vitest";
import {
  CONTROLLER_TYPE_LABELS,
  DRIVER_LABELS,
  DIRECTION_LABELS,
  OUTCOME_LABELS,
  PROCESS_TYPE_LABELS,
  RESPONSE_LEVEL_LABELS,
  SAMPLE_QUALITY_LABELS,
  SAMPLE_QUALITY_TONE,
  SAMPLING_ADEQUACY_LABELS,
  SAMPLING_ADEQUACY_TONE,
  TUNING_RESULT_INVALID_REASON_LABELS,
} from "./enumLabels";

describe("enum label maps", () => {
  it("labels every process type", () => {
    expect(PROCESS_TYPE_LABELS).toEqual({
      flow: "Flow",
      pressure_line: "Pressure (Line)",
      pressure_vessel: "Pressure (Vessel)",
      level: "Level",
      temperature_mixing: "Temperature (Mixing)",
      temperature_heat_exchange: "Temperature (Heat Exchange)",
    });
  });

  it("labels every controller type", () => {
    expect(CONTROLLER_TYPE_LABELS).toEqual({
      p: "P",
      pi: "PI",
      pid: "PID",
    });
  });

  it("labels every controller direction", () => {
    expect(DIRECTION_LABELS).toEqual({
      direct: "Direct",
      reverse: "Reverse",
    });
  });

  it("labels every response level", () => {
    expect(RESPONSE_LEVEL_LABELS).toEqual({
      aggressive: "Aggressive",
      moderate: "Moderate",
      sluggish: "Sluggish",
    });
  });

  it("labels every driver", () => {
    expect(DRIVER_LABELS).toEqual({
      opcda: "OPC DA",
      simulator: "Simulator",
      replay: "Replay",
    });
  });

  it("labels every run outcome", () => {
    expect(OUTCOME_LABELS).toEqual({
      running: "Running",
      completed: "Completed",
      failed: "Failed",
      aborted: "Aborted",
    });
  });

  it("labels every sampling adequacy state and its badge tone", () => {
    expect(SAMPLING_ADEQUACY_LABELS).toEqual({
      adequate: "Adequate",
      marginal: "Marginal",
      not_assessed: "Not assessed",
    });
    expect(SAMPLING_ADEQUACY_TONE).toEqual({
      adequate: "success",
      marginal: "warning",
      not_assessed: "neutral",
    });
  });

  it("labels every sample quality and its badge tone", () => {
    expect(SAMPLE_QUALITY_LABELS).toEqual({
      good: "Good",
      uncertain: "Uncertain",
      bad: "Bad",
    });
    expect(SAMPLE_QUALITY_TONE).toEqual({
      good: "success",
      uncertain: "warning",
      bad: "error",
    });
  });

  it("labels every invalid tuning-result reason", () => {
    expect(TUNING_RESULT_INVALID_REASON_LABELS).toEqual({
      non_finite_pv_amplitude:
        "The measured PV amplitude was not a finite number.",
      non_positive_pv_amplitude:
        "The measured PV amplitude was zero or negative.",
      non_finite_period:
        "The measured oscillation period was not a finite number.",
      non_positive_period:
        "The measured oscillation period was zero or negative.",
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
    });
  });
});
