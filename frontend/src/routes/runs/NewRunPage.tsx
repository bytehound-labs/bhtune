import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router";
import { useStartRun } from "../../api/runs";
import type { StartRunRequest } from "../../api/runs";
import { useTemplates } from "../../api/templates";
import type { components } from "../../api/schema";
import {
  Button,
  CheckboxField,
  ErrorBanner,
  FormSection,
  NumberField,
  PageHeading,
  SelectField,
  TextField,
} from "../../components/ui";

type TuneBackend = components["schemas"]["TuneBackend"];
type ProcessType = components["schemas"]["ProcessType"];
type ControllerType = components["schemas"]["ControllerType"];
type ControllerDirection = components["schemas"]["ControllerDirection"];
type ResponseLevel = components["schemas"]["ResponseLevel"];

const BACKENDS: readonly TuneBackend[] = ["simulator", "opcda"];
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
  backend: TuneBackend;
  template: string;
  name: string;
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
  backend: "simulator",
  template: "",
  name: "",
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
  // whenever `backend` is "simulator" (see `build_loop_tags` in `bhtune-cli`). Defaulted to
  // exactly the same 0-100% span and direction `bhtune simulate`'s CLI convenience path
  // uses, matching the default `backend: "simulator"` above so a first-time visitor can
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

/** Builds the request body, or returns a client-side validation message instead. Mirrors
 * `StartRunRequest::into_tune_args`'s own checks so most mistakes are caught before the round
 * trip — the server re-validates everything regardless, this is purely for fast feedback. */
function buildRequest(form: FormState): StartRunRequest | string {
  if (!form.template) return "Choose a template.";
  if (!form.tagname.trim()) return "Tag name is required.";
  if (form.backend === "opcda" && !form.server.trim()) {
    return "OPC DA server ProgID is required for the opcda backend.";
  }
  if (form.relayAmp === "") return "Relay amplitude is required.";
  if (form.backend === "simulator") {
    // The simulator has no range/direction tags to read at all, so `bhtune-cli`'s
    // `build_loop_tags` hard-requires all four of these plus `direction` — mirrored here
    // verbatim so a missing field is caught before the round trip, not as a 400 from the
    // server after the run has already been attempted.
    if (form.pvRangeHigh === "") {
      return "PV range high is required for the simulator backend (it has no range tags to read).";
    }
    if (form.pvRangeLow === "") {
      return "PV range low is required for the simulator backend (it has no range tags to read).";
    }
    if (form.mvRangeHigh === "") {
      return "MV range high is required for the simulator backend (it has no range tags to read).";
    }
    if (form.mvRangeLow === "") {
      return "MV range low is required for the simulator backend (it has no range tags to read).";
    }
    if (!form.direction) {
      return "Controller direction is required for the simulator backend (it has no direction tag to read).";
    }
  }
  if (form.writePid && !form.yes) {
    return "Confirm the write-back checkbox to enable an automatic PID write, or clear the write-back level.";
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
    backend: form.backend,
    bridge_host: form.bridgeHost.trim() || undefined,
    server: form.backend === "opcda" ? form.server.trim() : undefined,
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
    name: form.name.trim() || undefined,
    yes: form.yes,
    write_pid: form.writePid || undefined,
    allow_uncertain_quality: form.allowUncertainQuality,
    op_timeout_secs: toOptional(form.opTimeoutSecs),
    restore_timeout_secs: toOptional(form.restoreTimeoutSecs),
  };
}

export function NewRunPage() {
  const navigate = useNavigate();
  const templates = useTemplates();
  const startRun = useStartRun();
  const [form, setForm] = useState<FormState>(initialForm);
  const [validationError, setValidationError] = useState<string | null>(null);

  // Default to the first available template once the list loads, so a first-time visitor
  // doesn't have to know a template name exists before they can start a run at all.
  useEffect(() => {
    if (!form.template && templates.data && templates.data.length > 0) {
      setForm((prev) => ({ ...prev, template: templates.data[0].name }));
    }
  }, [templates.data, form.template]);

  function set<K extends keyof FormState>(key: K, value: FormState[K]) {
    setForm((prev) => ({ ...prev, [key]: value }));
  }

  function setBackend(value: TuneBackend) {
    setForm((prev) => {
      if (value !== "simulator") return { ...prev, backend: value };
      // Switching to the simulator backend makes the range/direction fields hard-required
      // (see `buildRequest`); fill in any still left blank with the same defaults
      // `bhtune simulate`'s CLI convenience path uses, without touching a value the user
      // already set for a reason.
      return {
        ...prev,
        backend: value,
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

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
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

  return (
    <div>
      <PageHeading
        title="New run"
        description="Starts a tune over HTTP — the same MRFT engine and backends the CLI uses, driven from the browser."
        actions={
          <Link to="/runs">
            <Button>Cancel</Button>
          </Link>
        }
      />

      {validationError && (
        <div className="mb-4">
          <ErrorBanner message={validationError} />
        </div>
      )}
      {startRun.isError && (
        <div className="mb-4">
          <ErrorBanner message={startRun.error.message} />
        </div>
      )}
      {templates.isError && (
        <div className="mb-4">
          <ErrorBanner
            message={`Could not load templates: ${templates.error.message}`}
          />
        </div>
      )}

      <form onSubmit={handleSubmit}>
        <FormSection title="Connection">
          <SelectField
            label="Backend"
            value={form.backend}
            onChange={(v) => setBackend(v)}
            options={BACKENDS}
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
          <TextField
            label="Run name"
            value={form.name}
            onChange={(v) => set("name", v)}
            placeholder="Defaults to the tag name"
            hint="A friendly label recorded as this run's loop name."
          />
          <TextField
            label="Tag name"
            required
            value={form.tagname}
            onChange={(v) => set("tagname", v)}
            hint={
              form.backend === "simulator"
                ? "Ignored for the simulator backend, but still required."
                : "PV tag prefix; the rest of the tag set is derived from it via the template's suffixes."
            }
          />
          {form.backend === "opcda" && (
            <>
              <TextField
                label="OPC DA server ProgID"
                required
                value={form.server}
                onChange={(v) => set("server", v)}
                placeholder="e.g. Matrikon.OPC.Simulation"
              />
              <TextField
                label="Bridge host"
                value={form.bridgeHost}
                onChange={(v) => set("bridgeHost", v)}
                placeholder="Defaults to this server's own configured bridge host"
                hint="opcda-bridge gateway address (host:port)."
              />
            </>
          )}
        </FormSection>

        <FormSection title="Test parameters">
          <SelectField
            label="Process type"
            value={form.processType}
            onChange={setProcessType}
            options={PROCESS_TYPES}
          />
          <SelectField
            label="Controller type"
            value={form.controllerType}
            onChange={(v) => set("controllerType", v)}
            options={controllerTypeOptions}
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
            label="Op timeout (s)"
            value={form.opTimeoutSecs}
            onChange={(v) => set("opTimeoutSecs", v)}
            min={1}
            step={1}
            hint="Cap on any single backend read/write."
          />
          <NumberField
            label="Restore timeout (s)"
            value={form.restoreTimeoutSecs}
            onChange={(v) => set("restoreTimeoutSecs", v)}
            min={1}
            step={1}
            hint="Cap on restoring the loop afterward."
          />
          <CheckboxField
            label="Allow uncertain quality"
            checked={form.allowUncertainQuality}
            onChange={(v) => set("allowUncertainQuality", v)}
            hint="Accept Quality::Uncertain readings instead of aborting. Bad quality is never accepted."
          />
        </FormSection>

        <FormSection title="Tag mapping overrides">
          <SelectField
            label="Controller direction"
            value={form.direction}
            onChange={(v) => set("direction", v)}
            options={DIRECTIONS}
            placeholder="Auto-detect (read live tag)"
            required={form.backend === "simulator"}
            hint={
              form.backend === "simulator"
                ? "Required — the simulator has no direction tag to read."
                : undefined
            }
          />
          <div />
          <NumberField
            label="PV range high"
            required={form.backend === "simulator"}
            value={form.pvRangeHigh}
            onChange={(v) => set("pvRangeHigh", v)}
            step="any"
            hint={
              form.backend === "simulator"
                ? "Required — the simulator has no range tags to read."
                : "Overrides a live tag read."
            }
          />
          <NumberField
            label="PV range low"
            required={form.backend === "simulator"}
            value={form.pvRangeLow}
            onChange={(v) => set("pvRangeLow", v)}
            step="any"
            hint={
              form.backend === "simulator"
                ? "Required — the simulator has no range tags to read."
                : "Overrides a live tag read."
            }
          />
          <NumberField
            label="MV range high"
            required={form.backend === "simulator"}
            value={form.mvRangeHigh}
            onChange={(v) => set("mvRangeHigh", v)}
            step="any"
            hint={
              form.backend === "simulator"
                ? "Required — the simulator has no range tags to read."
                : "Overrides a live tag read."
            }
          />
          <NumberField
            label="MV range low"
            required={form.backend === "simulator"}
            value={form.mvRangeLow}
            onChange={(v) => set("mvRangeLow", v)}
            step="any"
            hint={
              form.backend === "simulator"
                ? "Required — the simulator has no range tags to read."
                : "Overrides a live tag read."
            }
          />
        </FormSection>

        {form.backend === "simulator" && (
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

        <FormSection title="Write-back">
          <SelectField
            label="Write PID on completion"
            value={form.writePid}
            onChange={(v) => set("writePid", v)}
            options={RESPONSE_LEVELS}
            placeholder="Don't write back automatically"
          />
          <CheckboxField
            label="Confirm unattended write-back"
            checked={form.yes}
            onChange={(v) => set("yes", v)}
            hint="Required whenever a write-back level is chosen — writing to a live loop with no prompt must be a deliberate choice."
          />
        </FormSection>

        <div className="flex gap-2">
          <Button type="submit" variant="primary" disabled={startRun.isPending}>
            {startRun.isPending ? "Starting…" : "Start tune"}
          </Button>
          <Link to="/runs">
            <Button>Cancel</Button>
          </Link>
        </div>
      </form>
    </div>
  );
}
