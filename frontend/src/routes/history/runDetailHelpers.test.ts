import { describe, expect, it } from "vitest";
import {
  formatNumber,
  formatPidRecorded,
  isValidRunResult,
  isWritableRunResult,
  type RunResult,
} from "./runDetailHelpers";

const result: RunResult = {
  response_level: "moderate",
  status: "valid",
  invalid_reason: null,
  kp: 0.6,
  ti_minutes: 1.5,
  td_minutes: 0,
  proportional: 155.21378,
  integral: 2.482169,
  derivative: 0,
  controller_values: {
    response_level: "moderate",
    proportional: { value: 155.2, display: "155.2" },
    integral: { value: 2.5, display: "2.5" },
    derivative: { value: 0, display: "0.0" },
  },
  controller_target_error: null,
};

describe("PID precision readiness", () => {
  it("uses backend targets without altering raw calculation validity", () => {
    expect(isValidRunResult(result)).toBe(true);
    expect(isWritableRunResult(result)).toBe(true);
    const unwritable = {
      ...result,
      controller_values: null,
      controller_target_error:
        "Template rounding would erase an active term to zero.",
    };
    expect(isValidRunResult(unwritable)).toBe(true);
    expect(isWritableRunResult(unwritable)).toBe(false);
    expect(
      isWritableRunResult({ ...result, controller_values: undefined }),
    ).toBe(false);
    expect(
      isWritableRunResult({
        ...result,
        controller_target_error: "Unsafe target.",
      }),
    ).toBe(false);
    expect(isWritableRunResult({ ...result, status: "invalid" })).toBe(false);
  });

  it("preserves recorded restore/audit precision without changing general numeric formatting", () => {
    expect(formatPidRecorded(1.234567)).toBe("1.234567");
    expect(formatPidRecorded(0.0000004873)).toBe("4.873e-7");
    expect(formatPidRecorded(9999)).toBe("9999");
    expect(formatPidRecorded(null)).toBe("—");
    expect(formatPidRecorded(undefined)).toBe("—");
    expect(formatNumber(1.234567)).toBe("1.2346");
  });
});
