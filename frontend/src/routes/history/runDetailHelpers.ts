import type { RunDetailResponse } from "../../api/runs";
import { DRIVER_LABELS } from "../../lib/enumLabels";

export type RunResult = RunDetailResponse["results"][number];
export type RunWrite = RunDetailResponse["writes"][number];

export interface WriteEligibility {
  readonly eligible: boolean;
  readonly reason?: string;
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
