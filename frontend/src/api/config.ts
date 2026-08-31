import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, apiErrorMessage } from "./errors";

interface GlobalTuningConfig {
  mrft_delay_secs: number;
  poll_interval_ms: number;
  timeout_secs: number;
  op_timeout_secs: number;
  restore_timeout_secs: number;
}

interface GlobalTuningTomlConfig {
  mrft_delay_secs: number | null;
  poll_interval_ms: number | null;
  timeout_secs: number | null;
  op_timeout_secs: number | null;
  restore_timeout_secs: number | null;
}

interface GlobalTuningSources {
  mrft_delay_secs: string;
  poll_interval_ms: string;
  timeout_secs: string;
  op_timeout_secs: string;
  restore_timeout_secs: string;
}

/** The server owns effective values; the frontend edits global policies and tuning settings. */
export interface GlobalConfigResponse {
  revision: string;
  config_path: string;
  toml: {
    allow_uncertain_quality: boolean | null;
    retention_days: number | null;
    tuning: GlobalTuningTomlConfig;
  };
  effective: {
    allow_uncertain_quality: boolean;
    retention_days: number | null;
    tuning: GlobalTuningConfig;
  };
  source: {
    allow_uncertain_quality: string;
    retention_days: string;
    tuning: GlobalTuningSources;
  };
  backup_path: string | null;
}

interface UpdateGlobalTuningRequest {
  mrft_delay_secs: number | null;
  poll_interval_ms: number | null;
  timeout_secs: number | null;
  op_timeout_secs: number | null;
  restore_timeout_secs: number | null;
}

export interface UpdateGlobalConfigRequest {
  revision: string;
  allow_uncertain_quality: boolean;
  retention_days: number | null;
  tuning?: UpdateGlobalTuningRequest;
}

const configKey = ["config"] as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isNonNegativeInteger(value: unknown): value is number {
  return (
    typeof value === "number" &&
    Number.isInteger(value) &&
    Number.isFinite(value) &&
    value >= 0
  );
}

function isPositiveInteger(value: unknown): value is number {
  return (
    typeof value === "number" &&
    Number.isInteger(value) &&
    Number.isFinite(value) &&
    value >= 1
  );
}

function isNullable(
  value: unknown,
  predicate: (candidate: unknown) => candidate is number,
): value is number | null {
  return value === null || predicate(value);
}

function parseConfig(value: unknown): GlobalConfigResponse {
  if (
    !isRecord(value) ||
    typeof value.revision !== "string" ||
    typeof value.config_path !== "string" ||
    !isRecord(value.toml) ||
    (value.toml.allow_uncertain_quality !== null &&
      typeof value.toml.allow_uncertain_quality !== "boolean") ||
    (value.toml.retention_days !== null &&
      (typeof value.toml.retention_days !== "number" ||
        !Number.isInteger(value.toml.retention_days) ||
        value.toml.retention_days < 1)) ||
    !isRecord(value.toml.tuning) ||
    !isNullable(value.toml.tuning.mrft_delay_secs, isNonNegativeInteger) ||
    !isNullable(value.toml.tuning.poll_interval_ms, isPositiveInteger) ||
    !isNullable(value.toml.tuning.timeout_secs, isPositiveInteger) ||
    !isNullable(value.toml.tuning.op_timeout_secs, isPositiveInteger) ||
    !isNullable(value.toml.tuning.restore_timeout_secs, isPositiveInteger) ||
    !isRecord(value.effective) ||
    typeof value.effective.allow_uncertain_quality !== "boolean" ||
    (value.effective.retention_days !== null &&
      (typeof value.effective.retention_days !== "number" ||
        !Number.isInteger(value.effective.retention_days) ||
        value.effective.retention_days < 1)) ||
    !isRecord(value.effective.tuning) ||
    !isNonNegativeInteger(value.effective.tuning.mrft_delay_secs) ||
    !isPositiveInteger(value.effective.tuning.poll_interval_ms) ||
    !isPositiveInteger(value.effective.tuning.timeout_secs) ||
    !isPositiveInteger(value.effective.tuning.op_timeout_secs) ||
    !isPositiveInteger(value.effective.tuning.restore_timeout_secs) ||
    !isRecord(value.source) ||
    typeof value.source.allow_uncertain_quality !== "string" ||
    typeof value.source.retention_days !== "string" ||
    !isRecord(value.source.tuning) ||
    typeof value.source.tuning.mrft_delay_secs !== "string" ||
    typeof value.source.tuning.poll_interval_ms !== "string" ||
    typeof value.source.tuning.timeout_secs !== "string" ||
    typeof value.source.tuning.op_timeout_secs !== "string" ||
    typeof value.source.tuning.restore_timeout_secs !== "string" ||
    (value.backup_path !== null && typeof value.backup_path !== "string")
  ) {
    throw new Error("The server returned an invalid global configuration.");
  }

  return {
    revision: value.revision,
    config_path: value.config_path,
    toml: {
      allow_uncertain_quality: value.toml.allow_uncertain_quality,
      retention_days: value.toml.retention_days,
      tuning: {
        mrft_delay_secs: value.toml.tuning.mrft_delay_secs,
        poll_interval_ms: value.toml.tuning.poll_interval_ms,
        timeout_secs: value.toml.tuning.timeout_secs,
        op_timeout_secs: value.toml.tuning.op_timeout_secs,
        restore_timeout_secs: value.toml.tuning.restore_timeout_secs,
      },
    },
    effective: {
      allow_uncertain_quality: value.effective.allow_uncertain_quality,
      retention_days: value.effective.retention_days,
      tuning: {
        mrft_delay_secs: value.effective.tuning.mrft_delay_secs,
        poll_interval_ms: value.effective.tuning.poll_interval_ms,
        timeout_secs: value.effective.tuning.timeout_secs,
        op_timeout_secs: value.effective.tuning.op_timeout_secs,
        restore_timeout_secs: value.effective.tuning.restore_timeout_secs,
      },
    },
    source: {
      allow_uncertain_quality: value.source.allow_uncertain_quality,
      retention_days: value.source.retention_days,
      tuning: {
        mrft_delay_secs: value.source.tuning.mrft_delay_secs,
        poll_interval_ms: value.source.tuning.poll_interval_ms,
        timeout_secs: value.source.tuning.timeout_secs,
        op_timeout_secs: value.source.tuning.op_timeout_secs,
        restore_timeout_secs: value.source.tuning.restore_timeout_secs,
      },
    },
    backup_path: value.backup_path,
  };
}

async function readJson(response: Response): Promise<unknown> {
  try {
    return await response.json();
  } catch {
    return undefined;
  }
}

async function requestConfig(
  method: "GET" | "PUT",
  body?: UpdateGlobalConfigRequest,
): Promise<GlobalConfigResponse> {
  const response = await fetch("/api/config", {
    method,
    headers: body ? { "Content-Type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  const payload = await readJson(response);
  if (!response.ok) {
    throw new ApiError(apiErrorMessage(payload), response.status);
  }
  return parseConfig(payload);
}

/** `GET /api/config` — loads the effective global policies and their provenance. */
export function useConfig() {
  return useQuery({
    queryKey: configKey,
    queryFn: () => requestConfig("GET"),
  });
}

/** `PUT /api/config` — persists the global policies and returns the new effective state. */
export function useSaveConfig() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (config: UpdateGlobalConfigRequest) =>
      requestConfig("PUT", config),
    onSuccess: (data) => {
      queryClient.setQueryData(configKey, data);
    },
  });
}
