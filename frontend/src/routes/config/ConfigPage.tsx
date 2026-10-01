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

type ConfigValidationField = "retentionDays" | keyof ConfigForm["tuning"];

interface ConfigValidationError {
  readonly field: ConfigValidationField;
  readonly message: string;
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

function nullableConfigNumber(value: number | ""): number | null {
  if (value === "") return null;
  return value;
}

function retentionRequestValue(
  form: ConfigForm,
): ConfigForm["retentionDays"] | null {
  if (form.retentionMode === "forever") return null;
  return form.retentionDays;
}

function tuningRequestValue(form: ConfigForm) {
  if (form.resetTuning) {
    return {
      mrft_delay_secs: null,
      poll_interval_ms: null,
      timeout_secs: null,
      op_timeout_secs: null,
      restore_timeout_secs: null,
    };
  }

  return {
    mrft_delay_secs: nullableConfigNumber(form.tuning.mrftDelaySecs),
    poll_interval_ms: nullableConfigNumber(form.tuning.pollIntervalMs),
    timeout_secs: nullableConfigNumber(form.tuning.timeoutSecs),
    op_timeout_secs: nullableConfigNumber(form.tuning.opTimeoutSecs),
    restore_timeout_secs: nullableConfigNumber(form.tuning.restoreTimeoutSecs),
  };
}

function configRequestFromForm(form: ConfigForm) {
  return {
    allow_uncertain_quality: form.allowUncertainQuality,
    retention_days: retentionRequestValue(form),
    tuning: tuningRequestValue(form),
  };
}

function validateConfigForm(form: ConfigForm): ConfigValidationError | null {
  if (
    form.retentionMode === "days" &&
    (typeof form.retentionDays !== "number" ||
      !Number.isInteger(form.retentionDays) ||
      form.retentionDays < 1)
  ) {
    return {
      field: "retentionDays",
      message: "Enter a positive whole number of retention days.",
    };
  }

  const tuningValues = form.tuning;
  const invalidTuningField = (
    [
      ["mrftDelaySecs", tuningValues.mrftDelaySecs, 0, 3600],
      ["pollIntervalMs", tuningValues.pollIntervalMs, 1, null],
      ["timeoutSecs", tuningValues.timeoutSecs, 1, null],
      ["opTimeoutSecs", tuningValues.opTimeoutSecs, 1, null],
      ["restoreTimeoutSecs", tuningValues.restoreTimeoutSecs, 1, null],
    ] as const
  ).find(
    ([, value, minimum, maximum]) =>
      typeof value !== "number" ||
      !Number.isInteger(value) ||
      value < minimum ||
      (maximum !== null && value > maximum),
  );
  if (invalidTuningField) {
    return {
      field: invalidTuningField[0],
      message:
        "Enter valid whole-number values for all tune timing and safety settings.",
    };
  }
  return null;
}

function ConfigStatusMessages({
  isDirty,
  saveMessage,
  saveError,
  isConflict,
  onReload,
}: {
  readonly isDirty: boolean;
  readonly saveMessage: string | null;
  readonly saveError: unknown;
  readonly isConflict: boolean;
  readonly onReload: () => void;
}) {
  return (
    <>
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
            <Button onClick={onReload}>Reload configuration</Button>
          </div>
        </div>
      )}
    </>
  );
}

function ConfigRetentionSettings({
  form,
  validationError,
  onUpdate,
  onRetentionModeChange,
}: {
  readonly form: ConfigForm;
  readonly validationError: ConfigValidationError | null;
  readonly onUpdate: (key: "retentionDays", value: number | "") => void;
  readonly onRetentionModeChange: (mode: RetentionMode) => void;
}) {
  return (
    <FormSection
      title="History retention"
      documentationId="config.history-retention"
    >
      <fieldset className="min-w-0">
        <legend className="text-xs uppercase tracking-wide text-slate-500">
          Retain completed runs
        </legend>
        <div className="mt-2 space-y-2 text-sm text-slate-200">
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="retention"
              checked={form.retentionMode === "forever"}
              onChange={() => onRetentionModeChange("forever")}
            />{" "}
            Retain forever
          </label>
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="retention"
              checked={form.retentionMode === "days"}
              onChange={() => onRetentionModeChange("days")}
            />{" "}
            Delete older runs automatically
          </label>
        </div>
      </fieldset>
      <NumberField
        label="Retention days"
        value={form.retentionDays}
        onChange={(value) => onUpdate("retentionDays", value)}
        min={1}
        step={1}
        disabled={form.retentionMode === "forever"}
        required={form.retentionMode === "days"}
        hint={
          form.retentionMode === "forever"
            ? "No automatic deletion."
            : "Must be a positive whole number. The server applies retention during maintenance sweeps."
        }
        error={
          validationError?.field === "retentionDays"
            ? validationError.message
            : undefined
        }
      />
    </FormSection>
  );
}

function ConfigTuningSettings({
  form,
  config,
  validationError,
  onUpdate,
  onReset,
}: {
  readonly form: ConfigForm;
  readonly config: GlobalConfigResponse;
  readonly validationError: ConfigValidationError | null;
  readonly onUpdate: (
    key: keyof ConfigForm["tuning"],
    value: number | "",
  ) => void;
  readonly onReset: () => void;
}) {
  const tuning = form.tuning;
  return (
    <FormSection
      title="Tune timing and safety"
      documentationId="config.tune-timing-safety"
    >
      <p className="text-sm text-slate-400">
        These settings apply to future tunes. Changes do not alter runs that are
        already prepared or in progress.
      </p>
      <NumberField
        label="MRFT delay"
        value={tuning.mrftDelaySecs}
        onChange={(value) => onUpdate("mrftDelaySecs", value)}
        min={0}
        max={3600}
        step={1}
        required
        hint={`Effective: ${config.effective.tuning.mrft_delay_secs} s (${tuningSource(config, "mrft_delay_secs")}).`}
        error={
          validationError?.field === "mrftDelaySecs"
            ? validationError.message
            : undefined
        }
      />
      <NumberField
        label="Poll interval"
        value={tuning.pollIntervalMs}
        onChange={(value) => onUpdate("pollIntervalMs", value)}
        min={1}
        step={1}
        required
        hint={`Effective: ${config.effective.tuning.poll_interval_ms} ms (${tuningSource(config, "poll_interval_ms")}).`}
        error={
          validationError?.field === "pollIntervalMs"
            ? validationError.message
            : undefined
        }
      />
      <NumberField
        label="Whole-run timeout"
        value={tuning.timeoutSecs}
        onChange={(value) => onUpdate("timeoutSecs", value)}
        min={1}
        step={1}
        required
        hint={`Effective: ${config.effective.tuning.timeout_secs} s (${tuningSource(config, "timeout_secs")}).`}
        error={
          validationError?.field === "timeoutSecs"
            ? validationError.message
            : undefined
        }
      />
      <NumberField
        label="Driver-operation timeout"
        value={tuning.opTimeoutSecs}
        onChange={(value) => onUpdate("opTimeoutSecs", value)}
        min={1}
        step={1}
        required
        hint={`Effective: ${config.effective.tuning.op_timeout_secs} s (${tuningSource(config, "op_timeout_secs")}).`}
        error={
          validationError?.field === "opTimeoutSecs"
            ? validationError.message
            : undefined
        }
      />
      <NumberField
        label="Restore timeout"
        value={tuning.restoreTimeoutSecs}
        onChange={(value) => onUpdate("restoreTimeoutSecs", value)}
        min={1}
        step={1}
        required
        hint={`Effective: ${config.effective.tuning.restore_timeout_secs} s (${tuningSource(config, "restore_timeout_secs")}). OPC DA tunes require at least 4 s.`}
        error={
          validationError?.field === "restoreTimeoutSecs"
            ? validationError.message
            : undefined
        }
      />
      <div className="flex flex-wrap items-center gap-3">
        <Button onClick={onReset}>Reset tuning to built-in defaults</Button>
        {form.resetTuning && (
          <span className="text-sm text-slate-400">
            Saving will remove all five [tuning] overrides.
          </span>
        )}
      </div>
    </FormSection>
  );
}

function ConfigSaveActions({
  isDirty,
  savePending,
  onDiscard,
}: {
  readonly isDirty: boolean;
  readonly savePending: boolean;
  readonly onDiscard: () => void;
}) {
  return (
    <div className="flex items-center gap-3">
      <Button
        type="submit"
        variant="primary"
        disabled={!isDirty || savePending}
      >
        {savePending ? "Saving…" : "Save configuration"}
      </Button>
      {isDirty && <Button onClick={onDiscard}>Discard changes</Button>}
    </div>
  );
}

function ConfigGuidance({ config }: { readonly config: GlobalConfigResponse }) {
  return (
    <div className="mt-8 space-y-4">
      <section data-doc-section="config.guidance">
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
                {config.config_path}
              </dd>
            </div>
            <div>
              <dt className="text-xs uppercase tracking-wide text-slate-500">
                Effective source
              </dt>
              <dd className="mt-1 text-slate-300">
                Allow Uncertain:{" "}
                {sourceLabel(config.source.allow_uncertain_quality)}; retention:{" "}
                {sourceLabel(config.source.retention_days)}
              </dd>
            </div>
            {config.backup_path && (
              <div>
                <dt className="text-xs uppercase tracking-wide text-slate-500">
                  Previous configuration backup
                </dt>
                <dd className="mt-1 break-all font-mono text-slate-200">
                  {config.backup_path}
                </dd>
              </div>
            )}
          </dl>
          <div className="mt-4 space-y-2 text-sm text-slate-400">
            <p>
              Saving writes the global settings to this configuration file.
              Command-line and environment overrides may take precedence over
              file values.
            </p>
            <p>
              Effective policy: Uncertain quality is{" "}
              {config.effective.allow_uncertain_quality
                ? "accepted"
                : "rejected"}
              ; retention is{" "}
              {config.effective.retention_days === null
                ? "disabled"
                : `${config.effective.retention_days} days`}
              .
            </p>
          </div>
        </Card>
      </section>
    </div>
  );
}

export function ConfigPage() {
  const config = useConfig();
  const saveConfig = useSaveConfig();
  const [form, setForm] = useState<ConfigForm>(defaultForm);
  const [savedFormKey, setSavedFormKey] = useState<string | null>(null);
  const [saveMessage, setSaveMessage] = useState<string | null>(null);
  const [validationError, setValidationError] =
    useState<ConfigValidationError | null>(null);

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

  const updateForm = (nextForm: ConfigForm) => {
    setSaveMessage(null);
    setValidationError(null);
    setForm(nextForm);
    if (savedFormKey === null && config.data) {
      setSavedFormKey(formKey(loadedForm));
    }
  };

  const update = <K extends keyof ConfigForm>(key: K, value: ConfigForm[K]) => {
    updateForm({ ...displayedForm, [key]: value });
  };

  const updateTuning = <K extends keyof ConfigForm["tuning"]>(
    key: K,
    value: ConfigForm["tuning"][K],
  ) => {
    updateForm({
      ...displayedForm,
      tuning: { ...displayedForm.tuning, [key]: value },
      resetTuning: false,
    });
  };

  const updateRetentionMode = (mode: RetentionMode) => {
    updateForm({
      ...displayedForm,
      retentionMode: mode,
      retentionDays: mode === "forever" ? "" : displayedForm.retentionDays,
    });
  };

  const resetTuning = () =>
    updateForm({
      ...displayedForm,
      tuning: defaultTuning,
      resetTuning: true,
    });

  const request = useMemo(
    () => configRequestFromForm(displayedForm),
    [displayedForm],
  );

  const currentConfig = config.data;

  const save = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!currentConfig) {
      return;
    }
    const validation = validateConfigForm(displayedForm);
    if (validation) {
      setSaveMessage(null);
      setValidationError(validation);
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
          setValidationError(null);
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
      setValidationError(null);
      saveConfig.reset();
    }
  };

  const discardChanges = () => {
    if (!currentConfig) return;
    const nextForm = formFromResponse(currentConfig);
    setForm(nextForm);
    setSavedFormKey(formKey(nextForm));
    setSaveMessage(null);
    setValidationError(null);
    saveConfig.reset();
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
        documentationId="config.page"
        description="Global policies used by every tune and by history maintenance."
      />

      <form onSubmit={save} noValidate className="space-y-6">
        <ConfigStatusMessages
          isDirty={isDirty}
          saveMessage={saveMessage}
          saveError={saveError}
          isConflict={isConflict}
          onReload={() => void reload()}
        />

        <FormSection
          title="OPC quality policy"
          documentationId="config.opc-quality-policy"
        >
          <CheckboxField
            label="Allow Uncertain quality"
            checked={displayedForm.allowUncertainQuality}
            onChange={(value) => update("allowUncertainQuality", value)}
            hint="When enabled, Uncertain OPC readings are accepted for tuning. Bad readings are always rejected."
          />
        </FormSection>

        <ConfigRetentionSettings
          form={displayedForm}
          validationError={validationError}
          onUpdate={update}
          onRetentionModeChange={updateRetentionMode}
        />

        <ConfigTuningSettings
          form={displayedForm}
          config={currentConfig}
          validationError={validationError}
          onUpdate={updateTuning}
          onReset={resetTuning}
        />

        <ConfigSaveActions
          isDirty={isDirty}
          savePending={saveConfig.isPending}
          onDiscard={discardChanges}
        />
      </form>

      <ConfigGuidance config={currentConfig} />
    </div>
  );
}
