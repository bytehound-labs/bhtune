import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, apiErrorMessage } from "./errors";

/** The server owns the effective values; the frontend edits the two persisted global policies. */
export interface GlobalConfigResponse {
  revision: string;
  config_path: string;
  toml: {
    allow_uncertain_quality: boolean | null;
    retention_days: number | null;
  };
  effective: {
    allow_uncertain_quality: boolean;
    retention_days: number | null;
  };
  source: {
    allow_uncertain_quality: string;
    retention_days: string;
  };
  backup_path: string | null;
}

export interface UpdateGlobalConfigRequest {
  revision: string;
  allow_uncertain_quality: boolean;
  retention_days: number | null;
}

const configKey = ["config"] as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
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
    !isRecord(value.effective) ||
    typeof value.effective.allow_uncertain_quality !== "boolean" ||
    (value.effective.retention_days !== null &&
      (typeof value.effective.retention_days !== "number" ||
        !Number.isInteger(value.effective.retention_days) ||
        value.effective.retention_days < 1)) ||
    !isRecord(value.source) ||
    typeof value.source.allow_uncertain_quality !== "string" ||
    typeof value.source.retention_days !== "string" ||
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
    },
    effective: {
      allow_uncertain_quality: value.effective.allow_uncertain_quality,
      retention_days: value.effective.retention_days,
    },
    source: {
      allow_uncertain_quality: value.source.allow_uncertain_quality,
      retention_days: value.source.retention_days,
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
