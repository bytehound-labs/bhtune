import { useMemo, useState, type SubmitEvent } from "react";
import { ApiError, apiErrorMessage } from "../../api/errors";
import {
  type GlobalConfigResponse,
  useConfig,
  useSaveConfig,
} from "../../api/config";
import {
  Button,
  Card,
  CheckboxField,
  ErrorBanner,
  FormSection,
  LoadingState,
  NumberField,
  PageHeading,
} from "../../components/ui";

type RetentionMode = "forever" | "days";

interface ConfigForm {
  allowUncertainQuality: boolean;
  retentionMode: RetentionMode;
  retentionDays: number | "";
  tuning: {
    mrftDelaySecs: number | "";
    pollIntervalMs: number | "";
    timeoutSecs: number | "";
    opTimeoutSecs: number | "";
    restoreTimeoutSecs: number | "";
  };
  resetTuning: boolean;
}

const defaultTuning = {
  mrftDelaySecs: 0,
  pollIntervalMs: 800,
  timeoutSecs: 3600,
  opTimeoutSecs: 30,
  restoreTimeoutSecs: 30,
} as const;

const defaultForm: ConfigForm = {
  allowUncertainQuality: true,
  retentionMode: "forever",
  retentionDays: "",
  tuning: defaultTuning,
  resetTuning: false,
};

function formFromResponse(config: GlobalConfigResponse): ConfigForm {
  return {
    allowUncertainQuality: config.toml.allow_uncertain_quality ?? true,
    retentionMode: config.toml.retention_days === null ? "forever" : "days",
    retentionDays: config.toml.retention_days ?? "",
    tuning: {
      mrftDelaySecs:
        config.toml.tuning.mrft_delay_secs ??
        config.effective.tuning.mrft_delay_secs,
      pollIntervalMs:
        config.toml.tuning.poll_interval_ms ??
        config.effective.tuning.poll_interval_ms,
      timeoutSecs:
        config.toml.tuning.timeout_secs ?? config.effective.tuning.timeout_secs,
      opTimeoutSecs:
        config.toml.tuning.op_timeout_secs ??
        config.effective.tuning.op_timeout_secs,
      restoreTimeoutSecs:
        config.toml.tuning.restore_timeout_secs ??
        config.effective.tuning.restore_timeout_secs,
    },
    resetTuning: false,
  };
}

function formKey(form: ConfigForm): string {
  return JSON.stringify(form);
}

function sourceLabel(source: string): string {
  const labels: Record<string, string> = {
    default: "built-in default",
    config_file: "configuration file",
    environment: "environment variable",
    cli: "command-line option",
  };
  return labels[source] ?? source;
}

function tuningSource(
  config: GlobalConfigResponse,
  key: keyof GlobalConfigResponse["source"]["tuning"],
): string {
  return sourceLabel(config.source.tuning[key]);
}

export function ConfigPage() {
  const config = useConfig();
  const saveConfig = useSaveConfig();
  const [form, setForm] = useState<ConfigForm>(defaultForm);
  const [savedFormKey, setSavedFormKey] = useState<string | null>(null);
  const [saveMessage, setSaveMessage] = useState<string | null>(null);

  const loadedForm = config.data ? formFromResponse(config.data) : defaultForm;
  const displayedForm = savedFormKey === null ? loadedForm : form;
  const currentSavedFormKey =
    savedFormKey ?? (config.data ? formKey(loadedForm) : null);

  const isDirty =
    currentSavedFormKey !== null &&
    formKey(displayedForm) !== currentSavedFormKey;
  const saveError = saveConfig.error;
  const isConflict = saveError instanceof ApiError && saveError.status === 409;
  const requestError = config.error;

  const update = <K extends keyof ConfigForm>(key: K, value: ConfigForm[K]) => {
    setSaveMessage(null);
    setForm(() => ({ ...displayedForm, [key]: value }));
    if (savedFormKey === null && config.data) {
      setSavedFormKey(formKey(loadedForm));
    }
  };

  const updateTuning = <K extends keyof ConfigForm["tuning"]>(
    key: K,
    value: ConfigForm["tuning"][K],
  ) => {
    setSaveMessage(null);
    setForm(() => ({
      ...displayedForm,
      tuning: { ...displayedForm.tuning, [key]: value },
      resetTuning: false,
    }));
    if (savedFormKey === null && config.data) {
      setSavedFormKey(formKey(loadedForm));
    }
  };

  const request = useMemo(
    () => ({
      allow_uncertain_quality: displayedForm.allowUncertainQuality,
      retention_days:
        displayedForm.retentionMode === "forever"
          ? null
          : displayedForm.retentionDays,
      tuning: displayedForm.resetTuning
        ? {
            mrft_delay_secs: null,
            poll_interval_ms: null,
            timeout_secs: null,
            op_timeout_secs: null,
            restore_timeout_secs: null,
          }
        : {
            mrft_delay_secs:
              displayedForm.tuning.mrftDelaySecs === ""
                ? null
                : displayedForm.tuning.mrftDelaySecs,
            poll_interval_ms:
              displayedForm.tuning.pollIntervalMs === ""
                ? null
                : displayedForm.tuning.pollIntervalMs,
            timeout_secs:
              displayedForm.tuning.timeoutSecs === ""
                ? null
                : displayedForm.tuning.timeoutSecs,
            op_timeout_secs:
              displayedForm.tuning.opTimeoutSecs === ""
                ? null
                : displayedForm.tuning.opTimeoutSecs,
            restore_timeout_secs:
              displayedForm.tuning.restoreTimeoutSecs === ""
                ? null
                : displayedForm.tuning.restoreTimeoutSecs,
          },
    }),
    [displayedForm],
  );

  const currentConfig = config.data;

  const save = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!currentConfig) {
      return;
    }
    if (
      displayedForm.retentionMode === "days" &&
      (typeof displayedForm.retentionDays !== "number" ||
        !Number.isInteger(displayedForm.retentionDays) ||
        displayedForm.retentionDays < 1)
    ) {
      setSaveMessage("Enter a positive whole number of retention days.");
      return;
    }
    const tuningValues = displayedForm.tuning;
    if (
      typeof tuningValues.mrftDelaySecs !== "number" ||
      !Number.isInteger(tuningValues.mrftDelaySecs) ||
      tuningValues.mrftDelaySecs < 0 ||
      tuningValues.mrftDelaySecs > 3600 ||
      typeof tuningValues.pollIntervalMs !== "number" ||
      !Number.isInteger(tuningValues.pollIntervalMs) ||
      tuningValues.pollIntervalMs < 1 ||
      typeof tuningValues.timeoutSecs !== "number" ||
      !Number.isInteger(tuningValues.timeoutSecs) ||
      tuningValues.timeoutSecs < 1 ||
      typeof tuningValues.opTimeoutSecs !== "number" ||
      !Number.isInteger(tuningValues.opTimeoutSecs) ||
      tuningValues.opTimeoutSecs < 1 ||
      typeof tuningValues.restoreTimeoutSecs !== "number" ||
      !Number.isInteger(tuningValues.restoreTimeoutSecs) ||
      tuningValues.restoreTimeoutSecs < 1
    ) {
      setSaveMessage(
        "Enter valid whole-number values for all tune timing and safety settings.",
      );
      return;
    }

    saveConfig.mutate(
      {
        revision: currentConfig.revision,
        allow_uncertain_quality: request.allow_uncertain_quality,
        retention_days:
          request.retention_days === "" ? null : request.retention_days,
        tuning: request.tuning,
      },
      {
        onSuccess: (data) => {
          const nextForm = formFromResponse(data);
          setForm(nextForm);
          setSavedFormKey(formKey(nextForm));
          setSaveMessage("Configuration saved successfully.");
        },
      },
    );
  };

  const reload = async () => {
    const result = await config.refetch();
    if (result.data) {
      const nextForm = formFromResponse(result.data);
      setForm(nextForm);
      setSavedFormKey(formKey(nextForm));
      setSaveMessage(null);
      saveConfig.reset();
    }
  };

  if (config.isPending && !config.data) {
    return <LoadingState message="Loading global configuration…" />;
  }

  if (requestError && !config.data) {
    return (
      <div className="space-y-4">
        <PageHeading
          title="Configuration"
          description="Global policies used by BHTune."
        />
        <ErrorBanner message={apiErrorMessage(requestError)} />
        <Button onClick={() => void config.refetch()}>Retry</Button>
      </div>
    );
  }

  if (!currentConfig) {
    return <ErrorBanner message="Global configuration is unavailable." />;
  }

  return (
    <div>
      <PageHeading
        title="Configuration"
        description="Global policies used by every tune and by history maintenance."
      />

      <form onSubmit={save} className="space-y-6">
        {isDirty && (
          <div className="rounded-md border border-amber-800 bg-amber-950/50 px-4 py-3 text-sm text-amber-300">
            You have unsaved changes.
          </div>
        )}
        {saveMessage && (
          <div className="rounded-md border border-emerald-800 bg-emerald-950/50 px-4 py-3 text-sm text-emerald-300">
            {saveMessage}
          </div>
        )}
        {saveError && !isConflict && (
          <ErrorBanner message={apiErrorMessage(saveError)} />
        )}
        {isConflict && (
          <div className="rounded-md border border-amber-800 bg-amber-950/50 px-4 py-3 text-sm text-amber-300">
            The configuration changed elsewhere. Reload the latest values before
            saving again.
            <div className="mt-3">
              <Button onClick={() => void reload()}>
                Reload configuration
              </Button>
            </div>
          </div>
        )}

        <FormSection title="OPC quality policy">
          <CheckboxField
            label="Allow Uncertain quality"
            checked={displayedForm.allowUncertainQuality}
            onChange={(value) => update("allowUncertainQuality", value)}
            hint="When enabled, Uncertain OPC readings are accepted for tuning. Bad readings are always rejected."
          />
        </FormSection>

        <FormSection title="History retention">
          <div>
            <span className="text-xs uppercase tracking-wide text-slate-500">
              Retain completed runs
            </span>
            <div className="mt-2 space-y-2 text-sm text-slate-200">
              <label className="flex items-center gap-2">
                <input
                  type="radio"
                  name="retention"
                  checked={displayedForm.retentionMode === "forever"}
                  onChange={() => {
                    setSaveMessage(null);
                    setForm({
                      ...displayedForm,
                      retentionMode: "forever",
                      retentionDays: "",
                    });
                    if (savedFormKey === null && config.data) {
                      setSavedFormKey(formKey(loadedForm));
                    }
                  }}
                />{" "}
                Retain forever
              </label>
              <label className="flex items-center gap-2">
                <input
                  type="radio"
                  name="retention"
                  checked={displayedForm.retentionMode === "days"}
                  onChange={() => update("retentionMode", "days")}
                />{" "}
                Delete older runs automatically
              </label>
            </div>
          </div>
          <NumberField
            label="Retention days"
            value={displayedForm.retentionDays}
            onChange={(value) => update("retentionDays", value)}
            min={1}
            step={1}
            disabled={displayedForm.retentionMode === "forever"}
            required={displayedForm.retentionMode === "days"}
            hint={
              displayedForm.retentionMode === "forever"
                ? "No automatic deletion."
                : "Must be a positive whole number. The server applies retention during maintenance sweeps."
            }
          />
        </FormSection>

        <FormSection title="Tune timing and safety">
          <p className="text-sm text-slate-400">
            These settings apply to future tunes. Changes do not alter runs that
            are already prepared or in progress.
          </p>
          <NumberField
            label="MRFT delay"
            value={displayedForm.tuning.mrftDelaySecs}
            onChange={(value) => updateTuning("mrftDelaySecs", value)}
            min={0}
            max={3600}
            step={1}
            required
            hint={`Effective: ${currentConfig.effective.tuning.mrft_delay_secs} s (${tuningSource(currentConfig, "mrft_delay_secs")}).`}
          />
          <NumberField
            label="Poll interval"
            value={displayedForm.tuning.pollIntervalMs}
            onChange={(value) => updateTuning("pollIntervalMs", value)}
            min={1}
            step={1}
            required
            hint={`Effective: ${currentConfig.effective.tuning.poll_interval_ms} ms (${tuningSource(currentConfig, "poll_interval_ms")}).`}
          />
          <NumberField
            label="Whole-run timeout"
            value={displayedForm.tuning.timeoutSecs}
            onChange={(value) => updateTuning("timeoutSecs", value)}
            min={1}
            step={1}
            required
            hint={`Effective: ${currentConfig.effective.tuning.timeout_secs} s (${tuningSource(currentConfig, "timeout_secs")}).`}
          />
          <NumberField
            label="Driver-operation timeout"
            value={displayedForm.tuning.opTimeoutSecs}
            onChange={(value) => updateTuning("opTimeoutSecs", value)}
            min={1}
            step={1}
            required
            hint={`Effective: ${currentConfig.effective.tuning.op_timeout_secs} s (${tuningSource(currentConfig, "op_timeout_secs")}).`}
          />
          <NumberField
            label="Restore timeout"
            value={displayedForm.tuning.restoreTimeoutSecs}
            onChange={(value) => updateTuning("restoreTimeoutSecs", value)}
            min={1}
            step={1}
            required
            hint={`Effective: ${currentConfig.effective.tuning.restore_timeout_secs} s (${tuningSource(currentConfig, "restore_timeout_secs")}). OPC DA tunes require at least 4 s.`}
          />
          <div className="flex flex-wrap items-center gap-3">
            <Button
              onClick={() => {
                setSaveMessage(null);
                setForm({
                  ...displayedForm,
                  tuning: defaultTuning,
                  resetTuning: true,
                });
                if (savedFormKey === null && config.data) {
                  setSavedFormKey(formKey(loadedForm));
                }
              }}
            >
              Reset tuning to built-in defaults
            </Button>
            {displayedForm.resetTuning && (
              <span className="text-sm text-slate-400">
                Saving will remove all five [tuning] overrides.
              </span>
            )}
          </div>
        </FormSection>

        <div className="flex items-center gap-3">
          <Button
            type="submit"
            variant="primary"
            disabled={!isDirty || saveConfig.isPending}
          >
            {saveConfig.isPending ? "Saving…" : "Save configuration"}
          </Button>
          {isDirty && (
            <Button
              onClick={() => {
                setForm(formFromResponse(currentConfig));
                setSavedFormKey(formKey(formFromResponse(currentConfig)));
                setSaveMessage(null);
                saveConfig.reset();
              }}
            >
              Discard changes
            </Button>
          )}
        </div>
      </form>

      <div className="mt-8 space-y-4">
        <section>
          <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
            Configuration guidance
          </h2>
          <Card>
            <dl className="space-y-3 text-sm">
              <div>
                <dt className="text-xs uppercase tracking-wide text-slate-500">
                  Configuration file
                </dt>
                <dd className="mt-1 break-all font-mono text-slate-200">
                  {currentConfig.config_path}
                </dd>
              </div>
              <div>
                <dt className="text-xs uppercase tracking-wide text-slate-500">
                  Effective source
                </dt>
                <dd className="mt-1 text-slate-300">
                  Allow Uncertain:{" "}
                  {sourceLabel(currentConfig.source.allow_uncertain_quality)};
                  retention: {sourceLabel(currentConfig.source.retention_days)}
                </dd>
              </div>
              {currentConfig.backup_path && (
                <div>
                  <dt className="text-xs uppercase tracking-wide text-slate-500">
                    Previous configuration backup
                  </dt>
                  <dd className="mt-1 break-all font-mono text-slate-200">
                    {currentConfig.backup_path}
                  </dd>
                </div>
              )}
              <div className="text-slate-400">
                Saving writes the global settings to this configuration file.
                Command-line and environment overrides may take precedence over
                file values.
              </div>
              <div className="text-slate-400">
                Effective policy: Uncertain quality is{" "}
                {currentConfig.effective.allow_uncertain_quality
                  ? "accepted"
                  : "rejected"}
                ; retention is{" "}
                {currentConfig.effective.retention_days === null
                  ? "disabled"
                  : `${currentConfig.effective.retention_days} days`}
                .
              </div>
            </dl>
          </Card>
        </section>
      </div>
    </div>
  );
}
