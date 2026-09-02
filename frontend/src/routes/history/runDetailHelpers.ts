import type { RunDetailResponse } from "../../api/runs";
import {
  DRIVER_LABELS,
  TUNING_RESULT_INVALID_REASON_LABELS,
} from "../../lib/enumLabels";

export type RunResult = RunDetailResponse["results"][number];
export type RunWrite = RunDetailResponse["writes"][number];
export type ValidRunResult = RunResult & {
  readonly status: "valid";
  readonly kp: number;
  readonly ti_minutes: number;
  readonly td_minutes: number;
  readonly proportional: number;
  readonly integral: number;
  readonly derivative: number;
};

export interface WriteEligibility {
  readonly eligible: boolean;
  readonly reason?: string;
}

export function formatNumber(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  return String(Number(value.toFixed(4)));
}

export function isValidRunResult(result: RunResult): result is ValidRunResult {
  return (
    result.status === "valid" &&
    [
      result.kp,
      result.ti_minutes,
      result.td_minutes,
      result.proportional,
      result.integral,
      result.derivative,
    ].every((value) => typeof value === "number" && Number.isFinite(value))
  );
}

export function invalidResultReason(result: RunResult): string {
  if (result.invalid_reason) {
    return TUNING_RESULT_INVALID_REASON_LABELS[result.invalid_reason];
  }
  return "The calculated values are not safe to use.";
}

/**
 * Mirrors `routes::runs::require_writable_run` so post-run PID controls can explain why
 * they are disabled before making a request.
 */
export function writeEligibility(run: RunDetailResponse): WriteEligibility {
  if (run.outcome === "running") {
    return {
      eligible: false,
      reason: "This tune is still in progress; wait for it to finish.",
    };
  }
  if (run.driver !== "opcda") {
    return {
      eligible: false,
      reason: `${DRIVER_LABELS[run.driver]} tunes have no live loop to change PID settings on.`,
    };
  }
  if (!run.pid_constant_tags) {
    return {
      eligible: false,
      reason: "This tune's template has no PID constant tags configured.",
    };
  }
  if (!run.opc_server || !run.bridge_host) {
    return {
      eligible: false,
      reason: "This tune has no recorded OPC server/bridge host connection.",
    };
  }
  return { eligible: true };
}

export function writeKey(write: RunWrite): string {
  return [
    write.kind,
    write.response_level,
    write.written_at,
    write.proportional_previous,
    write.integral_previous,
    write.derivative_previous,
    write.proportional_written,
    write.integral_written,
    write.derivative_written,
  ].join(":");
}

export function writeFailureMessage(write: RunWrite): string {
  if (write.kind === "revert") {
    return "The previous PID values could not be restored. Check the OPC connection and try again.";
  }
  return "The PID settings could not be applied. Check the OPC connection and try again.";
}
