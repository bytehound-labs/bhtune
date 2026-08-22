import { useEffect, useRef, useState } from "react";
import { Link, useLocation, useNavigate } from "react-router";
import { useLastRunRequest, useStartRun } from "../../api/runs";
import type { StartRunRequest } from "../../api/runs";
import { useTemplates } from "../../api/templates";
import { userFacingErrorMessage } from "../../api/errors";
import type { components } from "../../api/schema";
import {
  CONTROLLER_TYPE_LABELS,
  DIRECTION_LABELS,
  DRIVER_LABELS,
  PROCESS_TYPE_LABELS,
  RESPONSE_LEVEL_LABELS,
} from "../../lib/enumLabels";
import { OpcServerDiscovery } from "../../components/OpcServerDiscovery";
import { OpcTagBrowserModal } from "../../components/OpcTagBrowserModal";
import {
  Button,
  CheckboxField,
  ErrorBanner,
  FormSection,
  NumberField,
  PageHeading,
  SelectField,
  TextAreaField,
  TextField,
} from "../../components/ui";

type TuneDriver = components["schemas"]["TuneDriver"];
type ProcessType = components["schemas"]["ProcessType"];
type ControllerType = components["schemas"]["ControllerType"];
type ControllerDirection = components["schemas"]["ControllerDirection"];
type ResponseLevel = components["schemas"]["ResponseLevel"];

const DRIVERS: readonly TuneDriver[] = ["simulator", "opcda"];
const PROCESS_TYPES: readonly ProcessType[] = [
  "flow",
  "pressure_line",
  "pressure_vessel",
  "level",
  "temperature_mixing",
  "temperature_heat_exchange",
];
const CONTROLLER_TYPES: readonly ControllerType[] = ["p", "pi", "pid"];
const DIRECTIONS: readonly ControllerDirection[] = ["direct", "reverse"];
const RESPONSE_LEVELS: readonly ResponseLevel[] = [
  "aggressive",
  "moderate",
  "sluggish",
];

/** Mirrors `bhtune_core::ProcessType::allows_pid` — PID is only offered for the two
 * Temperature process types, exactly like the legacy app's dynamic controller-type list. */
const TEMPERATURE_PROCESS_TYPES = new Set<ProcessType>([
  "temperature_mixing",
  "temperature_heat_exchange",
]);

type NumOrBlank = number | "";

type FormState = {
  driver: TuneDriver;
  template: string;
  notes: string;
  tagname: string;
  server: string;
  bridgeHost: string;
  processType: ProcessType;
  controllerType: ControllerType;
  relayAmp: NumOrBlank;
  cyclesSkip: NumOrBlank;
  cyclesCount: NumOrBlank;
  noiseProtectionSecs: NumOrBlank;
  mrftDelay: NumOrBlank;
  pollIntervalMs: NumOrBlank;
  timeoutSecs: NumOrBlank;
  opTimeoutSecs: NumOrBlank;
  restoreTimeoutSecs: NumOrBlank;
  allowUncertainQuality: boolean;
  direction: "" | ControllerDirection;
  pvRangeHigh: NumOrBlank;
  pvRangeLow: NumOrBlank;
  mvRangeHigh: NumOrBlank;
  mvRangeLow: NumOrBlank;
  simGain: NumOrBlank;
  simTau: NumOrBlank;
  simDeadTime: NumOrBlank;
  simNoise: NumOrBlank;
  simSeed: NumOrBlank;
  simInitialPv: NumOrBlank;
  simInitialMv: NumOrBlank;
  writePid: "" | ResponseLevel;
  yes: boolean;
};

/**
 * Every default here matches `StartRunRequest`'s own `#[serde(default = ...)]` values (see
 * `bhtune-server`'s `routes::runs`) or `bhtune-cli`'s `SimulateArgs` defaults, field-for-field
 * — so a first-time visitor who changes nothing except picking a template gets a working
 * simulator-backed run, and every other pre-filled number is exactly what omitting the
 * matching CLI flag would produce.
 */
const initialForm: FormState = {
  driver: "simulator",
  template: "",
  notes: "",
  tagname: "Sim.Loop1.PV",
  server: "",
  bridgeHost: "",
  processType: "flow",
  controllerType: "pi",
  relayAmp: 10,
  cyclesSkip: "",
  cyclesCount: "",
  noiseProtectionSecs: "",
  mrftDelay: 0,
  pollIntervalMs: 800,
  timeoutSecs: 3600,
  opTimeoutSecs: 30,
  restoreTimeoutSecs: 30,
  allowUncertainQuality: false,
  // The simulator has no range/direction tags at all, so these four are hard-required
  // whenever `driver` is "simulator" (see `build_loop_tags` in `bhtune-cli`). Defaulted to
  // exactly the same 0-100% span and direction `bhtune simulate`'s CLI convenience path
  // uses, matching the default `driver: "simulator"` above so a first-time visitor can
  // submit immediately.
  direction: "reverse",
  pvRangeHigh: 100,
  pvRangeLow: 0,
  mvRangeHigh: 100,
  mvRangeLow: 0,
  simGain: 1,
  simTau: 2,
  simDeadTime: 5,
  simNoise: 0,
  simSeed: 0,
  simInitialPv: 50,
  simInitialMv: 50,
  writePid: "",
  yes: false,
};

function toOptional(value: NumOrBlank): number | undefined {
  return value === "" ? undefined : value;
}

function toNumOrBlank(value: number | null | undefined): NumOrBlank {
  return value ?? "";
}

/**
 * Converts a stored [`StartRunRequest`] (from `GET /api/runs/last-request` or a specific
 * run's own `original_request`) into `FormState`, so the form can prefill from a past run's
 * remembered settings (`ui-prefill-last-run`).
 *
 * `bhtune-cli`'s `RequestSnapshot` (what actually populates `request_json`) always resolves
 * the fields that carry a CLI/server default — `mrft_delay`, `poll_interval_ms`, every
 * timeout, every `sim_*` field, `yes`, `allow_uncertain_quality` — to a concrete value before
 * it's stored, so those are simply copied across; the `?? initialForm...` fallback only ever
 * matters for a foreign/pre-`db-run-request-snapshot` row that somehow lacks the field. Every
 * other optional field (cycles, ranges, direction, connection overrides) is
 * shown *blank* when absent rather than substituting today's hardcoded default — an absent
 * value there specifically means "the engineer relied on a default last time", which is
 * exactly what should be shown again, not silently overwritten. Notes are intentionally
 * excluded from remembered settings, so every new or duplicate tune starts with an empty
 * notes field for fresh operator context.
 */
function formFromRequest(request: StartRunRequest): FormState {
  return {
    driver: request.driver,
    template: request.template,
    notes: "",
    tagname: request.tagname,
    server: request.server ?? "",
    bridgeHost: request.bridge_host ?? "",
    processType: request.process_type,
    controllerType: request.controller_type,
    relayAmp: request.relay_amp,
    cyclesSkip: toNumOrBlank(request.cycles_skip),
    cyclesCount: toNumOrBlank(request.cycles_count),
    noiseProtectionSecs: toNumOrBlank(request.noise_protection_secs),
    mrftDelay: request.mrft_delay ?? initialForm.mrftDelay,
    pollIntervalMs: request.poll_interval_ms ?? initialForm.pollIntervalMs,
    timeoutSecs: request.timeout_secs ?? initialForm.timeoutSecs,
    opTimeoutSecs: request.op_timeout_secs ?? initialForm.opTimeoutSecs,
    restoreTimeoutSecs:
      request.restore_timeout_secs ?? initialForm.restoreTimeoutSecs,
    allowUncertainQuality: request.allow_uncertain_quality ?? false,
    direction: request.direction ?? "",
    pvRangeHigh: toNumOrBlank(request.pv_range_high),
    pvRangeLow: toNumOrBlank(request.pv_range_low),
    mvRangeHigh: toNumOrBlank(request.mv_range_high),
    mvRangeLow: toNumOrBlank(request.mv_range_low),
    simGain: request.sim_gain ?? initialForm.simGain,
    simTau: request.sim_tau ?? initialForm.simTau,
    simDeadTime: request.sim_dead_time ?? initialForm.simDeadTime,
    simNoise: request.sim_noise ?? initialForm.simNoise,
    simSeed: request.sim_seed ?? initialForm.simSeed,
    simInitialPv: request.sim_initial_pv ?? initialForm.simInitialPv,
    simInitialMv: request.sim_initial_mv ?? initialForm.simInitialMv,
    writePid: request.write_pid ?? "",
    yes: request.yes ?? false,
  };
}

/**
 * Router `state` shape for navigating to this page to duplicate a specific historical run
 * (`RunDetailPage`'s "Duplicate this run" button) — as opposed to a plain visit to
 * `/runs/new`, which prefills from `GET /api/runs/last-request` (the *newest* run)
 * instead. Exported so `RunDetailPage` constructs it with type safety rather than an
 * untyped object literal that could silently drift from what this page reads.
 */
export interface DuplicateRunState {
  duplicateRequest: StartRunRequest;
  duplicateFromRunId: number;
}

/** Builds the request body, or returns a client-side validation message instead. Mirrors
 * `StartRunRequest::into_tune_args`'s own checks so most mistakes are caught before the round
 * trip — the server re-validates everything regardless, this is purely for fast feedback. */
function buildRequest(form: FormState): StartRunRequest | string {
  if (!form.template) return "Choose a template.";
  // Tag name is disabled (and therefore excluded from validation) for the simulator driver,
  // which hardcodes its own PV/MV tags and ignores this field entirely.
  if (form.driver !== "simulator" && !form.tagname.trim()) {
    return "Tag name is required.";
  }
  if (form.driver === "opcda" && !form.server.trim()) {
    return "OPC DA server ProgID is required for the opcda driver.";
  }
  if (form.relayAmp === "") return "Relay amplitude is required.";
  if (form.driver === "simulator") {
    // The simulator has no range/direction tags to read at all, so `bhtune-cli`'s
    // `build_loop_tags` hard-requires all four of these plus `direction` — mirrored here
    // verbatim so a missing field is caught before the round trip, not as a 400 from the
    // server after the run has already been attempted.
    if (form.pvRangeHigh === "") {
      return "PV range high is required for the simulator driver (it has no range tags to read).";
    }
    if (form.pvRangeLow === "") {
      return "PV range low is required for the simulator driver (it has no range tags to read).";
    }
    if (form.mvRangeHigh === "") {
      return "MV range high is required for the simulator driver (it has no range tags to read).";
    }
    if (form.mvRangeLow === "") {
      return "MV range low is required for the simulator driver (it has no range tags to read).";
    }
    if (!form.direction) {
      return "Controller direction is required for the simulator driver (it has no direction tag to read).";
    }
  }
  if (form.writePid && !form.yes) {
    return "Enable Allow automatic PID write to apply PID settings without a prompt, or clear the automatic PID setting.";
  }

  return {
    tagname: form.tagname.trim(),
    template: form.template,
    process_type: form.processType,
    controller_type: form.controllerType,
    relay_amp: form.relayAmp,
    cycles_skip: toOptional(form.cyclesSkip),
    cycles_count: toOptional(form.cyclesCount),
    noise_protection_secs: toOptional(form.noiseProtectionSecs),
    mrft_delay: toOptional(form.mrftDelay),
    driver: form.driver,
    bridge_host: form.bridgeHost.trim() || undefined,
    server: form.driver === "opcda" ? form.server.trim() : undefined,
    sim_gain: toOptional(form.simGain),
    sim_tau: toOptional(form.simTau),
    sim_dead_time: toOptional(form.simDeadTime),
    sim_noise: toOptional(form.simNoise),
    sim_seed: toOptional(form.simSeed),
    sim_initial_pv: toOptional(form.simInitialPv),
    sim_initial_mv: toOptional(form.simInitialMv),
    pv_range_high: toOptional(form.pvRangeHigh),
    pv_range_low: toOptional(form.pvRangeLow),
    mv_range_high: toOptional(form.mvRangeHigh),
    mv_range_low: toOptional(form.mvRangeLow),
    direction: form.direction || undefined,
    poll_interval_ms: toOptional(form.pollIntervalMs),
    timeout_secs: toOptional(form.timeoutSecs),
    notes: form.notes.trim() || undefined,
    yes: form.yes,
    write_pid: form.writePid || undefined,
    allow_uncertain_quality: form.allowUncertainQuality,
    op_timeout_secs: toOptional(form.opTimeoutSecs),
    restore_timeout_secs: toOptional(form.restoreTimeoutSecs),
  };
}

export function NewRunPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const templates = useTemplates();
  const startRun = useStartRun();
  const lastRunRequest = useLastRunRequest();

  // Set only by `RunDetailPage`'s "Duplicate this run" button -- a plain visit to
  // `/runs/new` has no location state and falls through to the last-run prefill below.
  const duplicateState = location.state as DuplicateRunState | null | undefined;

  const [form, setForm] = useState<FormState>(() =>
    duplicateState
      ? formFromRequest(duplicateState.duplicateRequest)
      : initialForm,
  );
  const [validationError, setValidationError] = useState<string | null>(null);
  // Tracks *why* the form currently looks the way it does, purely to show an explanatory
  // note -- not read anywhere else. `null` means "still the hardcoded defaults".
  const [prefillSource, setPrefillSource] = useState<
    { kind: "duplicate"; runId: number } | { kind: "last-run" } | null
  >(() =>
    duplicateState
      ? { kind: "duplicate", runId: duplicateState.duplicateFromRunId }
      : null,
  );
  const seededFromLastRunRef = useRef(false);
  const [tagBrowserOpen, setTagBrowserOpen] = useState(false);
  const activeTemplate = templates.data?.find((t) => t.name === form.template);

  // Prefill from the newest run's own settings on a plain visit -- a "Duplicate this run"
  // navigation already seeded the lazy `useState` initializer above from a *specific* run's
  // request instead, so this must not run in that case. Guarded to run at most once: a
  // background refetch of `useLastRunRequest` later (e.g. on window refocus) must never
  // clobber whatever the engineer has since typed.
  useEffect(() => {
    if (duplicateState) return;
    if (seededFromLastRunRef.current) return;
    if (!lastRunRequest.data) return;
    seededFromLastRunRef.current = true;
    setForm(formFromRequest(lastRunRequest.data));
    setPrefillSource({ kind: "last-run" });
  }, [duplicateState, lastRunRequest.data]);

  // Default to the first available template once the list loads, so a first-time visitor
  // doesn't have to know a template name exists before they can start a run at all -- and
  // so "Reset to defaults" (which clears `form.template` back to "") gets a sensible default
  // back rather than an empty dropdown.
  //
  // The gating check reads `prev.template` from inside the *functional* `setForm` updater
  // rather than this effect's own `form.template` closure. That distinction is load-bearing:
  // when `templates.data` and `lastRunRequest.data` resolve in the same React batch, this
  // effect and the prefill effect above both run in the *same* commit, and both see that
  // render's stale, pre-update `form.template` ("") -- reading it directly here would then
  // unconditionally queue an update that clobbers the prefill's own just-queued update with
  // the alphabetically-first template. The functional updater instead evaluates `prev` at
  // *application* time, after the prefill effect's update has already been applied (it's
  // declared first above, so it runs -- and its `setForm` call is queued -- first in this
  // commit), so it correctly sees the just-prefilled template and leaves it alone. Returning
  // `prev` unchanged when there's nothing to do also means this never schedules a wasted
  // re-render on the (common) already-templated path.
  useEffect(() => {
    if (duplicateState) return;
    setForm((prev) => {
      if (prev.template || !templates.data || templates.data.length === 0) {
        return prev;
      }
      return { ...prev, template: templates.data[0].name };
    });
  }, [duplicateState, templates.data, form.template]);

  /** Resets every field back to the hardcoded defaults, discarding whatever this page was
   * prefilled with. Marks the last-run seed as already applied so a still-in-flight
   * `useLastRunRequest` fetch that resolves afterward can't silently undo this. */
  function resetToDefaults() {
    seededFromLastRunRef.current = true;
    setPrefillSource(null);
    setForm(initialForm);
  }

  function set<K extends keyof FormState>(key: K, value: FormState[K]) {
    setForm((prev) => ({ ...prev, [key]: value }));
  }

  function setDriver(value: TuneDriver) {
    setForm((prev) => {
      if (value !== "simulator") return { ...prev, driver: value };
      // Switching to the simulator driver makes the range/direction fields hard-required
      // (see `buildRequest`); fill in any still left blank with the same defaults
      // `bhtune simulate`'s CLI convenience path uses, without touching a value the user
      // already set for a reason.
      return {
        ...prev,
        driver: value,
        pvRangeHigh: prev.pvRangeHigh === "" ? 100 : prev.pvRangeHigh,
        pvRangeLow: prev.pvRangeLow === "" ? 0 : prev.pvRangeLow,
        mvRangeHigh: prev.mvRangeHigh === "" ? 100 : prev.mvRangeHigh,
        mvRangeLow: prev.mvRangeLow === "" ? 0 : prev.mvRangeLow,
        direction: prev.direction === "" ? "reverse" : prev.direction,
      };
    });
  }

  function setProcessType(value: ProcessType) {
    setForm((prev) => ({
      ...prev,
      processType: value,
      // Leaving PID selected for a process type that no longer allows it would silently
      // submit an invalid combination; reset to PI instead, mirroring the legacy app's
      // dynamic controller-type dropdown.
      controllerType:
        prev.controllerType === "pid" && !TEMPERATURE_PROCESS_TYPES.has(value)
          ? "pi"
          : prev.controllerType,
    }));
  }

  const controllerTypeOptions = TEMPERATURE_PROCESS_TYPES.has(form.processType)
    ? CONTROLLER_TYPES
    : CONTROLLER_TYPES.filter((c) => c !== "pid");

  function submitTune() {
    setValidationError(null);
    const request = buildRequest(form);
    if (typeof request === "string") {
      setValidationError(request);
      return;
    }
    startRun.mutate(request, {
      onSuccess: (data) => navigate(`/runs/${data.id}`),
    });
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    submitTune();
  }

  return (
    <div>
      <PageHeading
        title="New tune"
        description="Configure and start a tune."
        actions={
          <>
            <Button
              variant="primary"
              disabled={startRun.isPending}
              onClick={submitTune}
            >
              {startRun.isPending ? "Starting…" : "Start tune"}
            </Button>
            <Link to="/runs">
              <Button>Cancel</Button>
            </Link>
            <Button
              onClick={resetToDefaults}
              disabled={prefillSource === null}
              title={
                prefillSource === null
                  ? "Already using the default settings."
                  : undefined
              }
            >
              Reset to defaults
            </Button>
          </>
        }
      />

      {prefillSource !== null && (
        <div className="mb-4 rounded-lg border border-slate-700 bg-slate-900/60 px-4 py-3 text-sm text-slate-300">
          {prefillSource.kind === "duplicate"
            ? `Loaded settings from tune #${prefillSource.runId}.`
            : "Loaded settings from the most recent tune."}{" "}
          Change anything below, or "Reset to defaults" to return to the
          built-in defaults.
        </div>
      )}

      {validationError && (
        <div className="mb-4">
          <ErrorBanner message={validationError} />
        </div>
      )}
      {startRun.isError && (
        <div className="mb-4">
          <ErrorBanner
            message={userFacingErrorMessage(
              startRun.error,
              "Unable to start the tune.",
            )}
          />
        </div>
      )}
      {templates.isError && (
        <div className="mb-4">
          <ErrorBanner
            message={userFacingErrorMessage(
              templates.error,
              "Unable to load templates.",
            )}
          />
        </div>
      )}

      <form onSubmit={handleSubmit}>
        <FormSection title="Connection">
          <SelectField
            label="Driver"
            value={form.driver}
            onChange={(v) => setDriver(v)}
            options={DRIVERS}
            displayLabel={(v) => DRIVER_LABELS[v]}
          />
          <SelectField
            label="Template"
            value={form.template}
            onChange={(v) => set("template", v)}
            options={(templates.data ?? []).map((t) => t.name)}
            placeholder={
              templates.isPending ? "Loading templates…" : "Choose a template"
            }
          />
          <TextAreaField
            label="Notes"
            value={form.notes}
            onChange={(v) => set("notes", v)}
            full
            placeholder="Optional context, observations, or follow-up actions"
            hint="Notes can be edited or cleared from the tune history."
          />
          <TextField
            label="Bridge host"
            disabled={form.driver === "simulator"}
            value={form.bridgeHost}
            onChange={(v) => set("bridgeHost", v)}
            placeholder="Defaults to this server's own configured bridge host"
            hint={
              form.driver === "simulator"
                ? "Disabled — the simulator never contacts a gateway."
                : "opcda-bridge gateway address (host:port)."
            }
          />
          <div>
            <TextField
              label="Tag name"
              required={form.driver !== "simulator"}
              disabled={form.driver === "simulator"}
              value={form.tagname}
              onChange={(v) => set("tagname", v)}
              hint={
                form.driver === "simulator"
                  ? "Disabled — the simulator hardcodes its own PV/MV tags and ignores this."
                  : "PV tag prefix; the rest of the tag set is derived from it via the template's suffixes."
              }
            />
            {form.driver === "opcda" && (
              <div className="mt-1">
                <Button
                  onClick={() => setTagBrowserOpen(true)}
                  disabled={!form.server.trim()}
                  title={
                    form.server.trim()
                      ? undefined
                      : "Enter an OPC DA server ProgID first."
                  }
                >
                  Browse tags…
                </Button>
              </div>
            )}
          </div>
          <div>
            <TextField
              label="OPC DA server ProgID"
              required={form.driver === "opcda"}
              disabled={form.driver === "simulator"}
              value={form.server}
              onChange={(v) => set("server", v)}
              placeholder="e.g. Matrikon.OPC.Simulation"
              hint={
                form.driver === "simulator"
                  ? "Disabled — the simulator never contacts a gateway."
                  : undefined
              }
            />
            {form.driver === "opcda" && (
              <OpcServerDiscovery
                bridgeHost={form.bridgeHost}
                onSelect={(v) => set("server", v)}
              />
            )}
          </div>
        </FormSection>

        <FormSection title="Test parameters">
          <SelectField
            label="Process type"
            value={form.processType}
            onChange={setProcessType}
            options={PROCESS_TYPES}
            displayLabel={(v) => PROCESS_TYPE_LABELS[v]}
          />
          <SelectField
            label="Controller type"
            value={form.controllerType}
            onChange={(v) => set("controllerType", v)}
            options={controllerTypeOptions}
            displayLabel={(v) => CONTROLLER_TYPE_LABELS[v]}
          />
          <NumberField
            label="Relay amplitude (%)"
            required
            value={form.relayAmp}
            onChange={(v) => set("relayAmp", v)}
            min={0.1}
            max={50}
            step={0.1}
            hint="0.1–50% of the MV range."
          />
          <div />
          <NumberField
            label="Cycles to skip"
            value={form.cyclesSkip}
            onChange={(v) => set("cyclesSkip", v)}
            min={0}
            step={1}
            hint="Blank = looked up per process type."
          />
          <NumberField
            label="Cycles to count"
            value={form.cyclesCount}
            onChange={(v) => set("cyclesCount", v)}
            min={1}
            step={1}
            hint="Blank = looked up per process type."
          />
          <NumberField
            label="Noise protection (s)"
            value={form.noiseProtectionSecs}
            onChange={(v) => set("noiseProtectionSecs", v)}
            min={0}
            step={1}
            hint="Blank = looked up per process type."
          />
          <NumberField
            label="MRFT delay padding (s)"
            value={form.mrftDelay}
            onChange={(v) => set("mrftDelay", v)}
            min={0}
            step={1}
            hint="Pre/post-test recording-only ticks."
          />
          <NumberField
            label="Poll interval (ms)"
            value={form.pollIntervalMs}
            onChange={(v) => set("pollIntervalMs", v)}
            min={1}
            step={1}
          />
          <NumberField
            label="Run timeout (s)"
            value={form.timeoutSecs}
            onChange={(v) => set("timeoutSecs", v)}
            min={1}
            step={1}
            hint="Hard cap on this run's total duration."
          />
          <NumberField
            label="Communication timeout (s)"
            value={form.opTimeoutSecs}
            onChange={(v) => set("opTimeoutSecs", v)}
            min={1}
            step={1}
            disabled={form.driver === "simulator"}
            hint={
              form.driver === "simulator"
                ? "Disabled — the simulator has no out-of-process I/O to time out."
                : "Cap on any single driver read/write."
            }
          />
          <NumberField
            label="Restore timeout (s)"
            value={form.restoreTimeoutSecs}
            onChange={(v) => set("restoreTimeoutSecs", v)}
            min={1}
            step={1}
            disabled={form.driver === "simulator"}
            hint={
              form.driver === "simulator"
                ? "Disabled — the simulator has no out-of-process I/O to time out."
                : "Cap on restoring the loop afterward."
            }
          />
          <CheckboxField
            label="Allow uncertain quality"
            checked={form.allowUncertainQuality}
            onChange={(v) => set("allowUncertainQuality", v)}
            disabled={form.driver === "simulator"}
            hint={
              form.driver === "simulator"
                ? "Disabled — the simulator always reports Good quality."
                : "Accept Quality::Uncertain readings instead of aborting. Bad quality is never accepted."
            }
          />
        </FormSection>

        <FormSection title="Tag mapping overrides">
          <SelectField
            label="Controller direction"
            value={form.direction}
            onChange={(v) => set("direction", v)}
            options={DIRECTIONS}
            displayLabel={(v) => DIRECTION_LABELS[v]}
            placeholder="Auto-detect (read live tag)"
            required={form.driver === "simulator"}
            hint={
              form.driver === "simulator"
                ? "Required — the simulator has no direction tag to read."
                : undefined
            }
          />
          <div />
          <NumberField
            label="PV range high"
            required={form.driver === "simulator"}
            value={form.pvRangeHigh}
            onChange={(v) => set("pvRangeHigh", v)}
            step="any"
            hint={
              form.driver === "simulator"
                ? "Required — the simulator has no range tags to read."
                : "Overrides a live tag read."
            }
          />
          <NumberField
            label="PV range low"
            required={form.driver === "simulator"}
            value={form.pvRangeLow}
            onChange={(v) => set("pvRangeLow", v)}
            step="any"
            hint={
              form.driver === "simulator"
                ? "Required — the simulator has no range tags to read."
                : "Overrides a live tag read."
            }
          />
          <NumberField
            label="MV range high"
            required={form.driver === "simulator"}
            value={form.mvRangeHigh}
            onChange={(v) => set("mvRangeHigh", v)}
            step="any"
            hint={
              form.driver === "simulator"
                ? "Required — the simulator has no range tags to read."
                : "Overrides a live tag read."
            }
          />
          <NumberField
            label="MV range low"
            required={form.driver === "simulator"}
            value={form.mvRangeLow}
            onChange={(v) => set("mvRangeLow", v)}
            step="any"
            hint={
              form.driver === "simulator"
                ? "Required — the simulator has no range tags to read."
                : "Overrides a live tag read."
            }
          />
        </FormSection>

        {form.driver === "simulator" && (
          <FormSection title="Simulator parameters">
            <NumberField
              label="Process gain"
              value={form.simGain}
              onChange={(v) => set("simGain", v)}
              step="any"
            />
            <NumberField
              label="Time constant τ (s)"
              value={form.simTau}
              onChange={(v) => set("simTau", v)}
              step="any"
            />
            <NumberField
              label="Dead time (s)"
              value={form.simDeadTime}
              onChange={(v) => set("simDeadTime", v)}
              step="any"
            />
            <NumberField
              label="Measurement noise"
              value={form.simNoise}
              onChange={(v) => set("simNoise", v)}
              step="any"
            />
            <NumberField
              label="RNG seed"
              value={form.simSeed}
              onChange={(v) => set("simSeed", v)}
              min={0}
              step={1}
              hint="Fixed seed = reproducible noise."
            />
            <div />
            <NumberField
              label="Initial PV"
              value={form.simInitialPv}
              onChange={(v) => set("simInitialPv", v)}
              step="any"
            />
            <NumberField
              label="Initial MV"
              value={form.simInitialMv}
              onChange={(v) => set("simInitialMv", v)}
              step="any"
            />
          </FormSection>
        )}

        <FormSection title="Automatic PID settings">
          <SelectField
            label="Apply PID settings on completion"
            value={form.writePid}
            onChange={(v) => set("writePid", v)}
            options={RESPONSE_LEVELS}
            displayLabel={(v) => RESPONSE_LEVEL_LABELS[v]}
            placeholder="Do not apply automatically"
            disabled={form.driver === "simulator"}
            hint={
              form.driver === "simulator"
                ? "Disabled — the simulator has no PID constant tags to write to."
                : undefined
            }
          />
          <CheckboxField
            label="Allow automatic PID write"
            checked={form.yes}
            onChange={(v) => set("yes", v)}
            disabled={form.driver === "simulator"}
            hint={
              form.driver === "simulator"
                ? "Disabled — the simulator has no PID constant tags to write to."
                : "Required when automatic PID settings are selected — applying changes to a live loop without a prompt must be deliberate."
            }
          />
        </FormSection>
      </form>

      {tagBrowserOpen && (
        <OpcTagBrowserModal
          bridgeHost={form.bridgeHost}
          opcServer={form.server}
          template={activeTemplate}
          onClose={() => setTagBrowserOpen(false)}
          onSelect={(tag) => set("tagname", tag)}
        />
      )}
    </div>
  );
}
