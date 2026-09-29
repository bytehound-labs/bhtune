import type { SimulatorCapabilities } from "../../api/capabilities";
import { FormSection, NumberField } from "../../components/ui";
import type { NumOrBlank } from "./mappingState";
import type { FormState } from "./newRunFormState";
import type { NewRunSectionProps } from "./newRunSectionShared";

type SimulatorParameterProps = Pick<
  NewRunSectionProps,
  "form" | "onChange" | "simulatorCapabilities"
> &
  Partial<Pick<NewRunSectionProps, "fieldError">>;

function simulatorPvSpan(form: FormState): number | undefined {
  if (
    typeof form.simPvRangeHigh !== "number" ||
    typeof form.simPvRangeLow !== "number"
  ) {
    return undefined;
  }
  return form.simPvRangeHigh - form.simPvRangeLow;
}

function numericBounds(low: NumOrBlank, high: NumOrBlank) {
  return {
    min: typeof low === "number" ? low : undefined,
    max: typeof high === "number" ? high : undefined,
  };
}

function SimulatorModelInfo({
  simulatorCapabilities,
}: Pick<SimulatorParameterProps, "simulatorCapabilities">) {
  const samplingDescription = simulatorCapabilities
    ? `The Demo uses a fixed ${simulatorCapabilities.defaults.poll_interval_ms} ms simulated step.`
    : "Full mode uses the configured [tuning].poll_interval_ms value.";

  return (
    <div
      className="rounded-md border border-slate-700 bg-slate-950/50 p-4 sm:col-span-2"
      data-testid="simulator-model-info"
    >
      <h3 className="text-sm font-semibold text-slate-200">
        Model used: first-order-plus-dead-time (FOPDT)
      </h3>
      <p className="mt-2 text-sm text-slate-400">
        All current process categories use this same generic physical model.
        Process type changes MRFT tuning correlations and parameter defaults; it
        does not select a different simulated plant.
      </p>

      <div className="mt-4 grid gap-4 text-sm sm:grid-cols-2">
        <div>
          <p className="font-medium text-slate-300">Transfer function</p>
          <code className="mt-1 block overflow-x-auto rounded bg-slate-900 px-3 py-2 text-xs text-emerald-300">
            G(s) = K · exp(−θs) / (τs + 1)
          </code>
        </div>
        <div>
          <p className="font-medium text-slate-300">Continuous-time form</p>
          <code className="mt-1 block overflow-x-auto rounded bg-slate-900 px-3 py-2 text-xs text-emerald-300">
            τ · dPV/dt = −(PV − PV₀) + K · (MV − MV₀)
          </code>
        </div>
      </div>

      <div className="mt-4">
        <p className="font-medium text-slate-300">
          Exact update for each simulated step
        </p>
        <code className="mt-1 block overflow-x-auto rounded bg-slate-900 px-3 py-2 text-xs text-emerald-300">
          PV_next = PV · e^(−Δt/τ) + (1 − e^(−Δt/τ)) · (PV₀ − K · MV₀ + K ·
          MV_delayed)
        </code>
      </div>

      <dl className="mt-4 grid gap-3 text-xs text-slate-400 sm:grid-cols-3">
        <div>
          <dt className="font-medium text-slate-200">K</dt>
          <dd>Process gain: PV change per unit MV change.</dd>
        </div>
        <div>
          <dt className="font-medium text-slate-200">τ</dt>
          <dd>Time constant: response speed after the delay.</dd>
        </div>
        <div>
          <dt className="font-medium text-slate-200">θ</dt>
          <dd>Dead time: MV samples delayed before affecting PV.</dd>
        </div>
      </dl>

      <p className="mt-4 text-xs text-slate-500">
        {samplingDescription} Dead time is represented by delaying MV through
        approximately ceil(θ / Δt) samples. Measurement noise is sampled
        uniformly from the configured range and added after the model update.
        Simulator time advances without waiting on wall-clock time, and the RNG
        seed makes noisy runs reproducible.
      </p>
      <p className="mt-2 text-xs text-slate-500">
        MRFT drives the simulated MV directly. The separate VirtualPid helper is
        for closed-loop validation and is not used during a tune.
      </p>
    </div>
  );
}

function SimulatorProcessFields({
  form,
  onChange,
  simulatorCapabilities,
  fieldError,
}: SimulatorParameterProps) {
  const limits = simulatorCapabilities?.limits;
  const pvSpan = simulatorPvSpan(form);
  const maxNoise =
    limits && pvSpan !== undefined
      ? Math.max(0, pvSpan * limits.max_noise_fraction_of_pv_span)
      : undefined;
  let gainHint: string | undefined;
  if (limits) {
    if (limits.sim_gain.absolute_min) {
      gainHint = `Allowed magnitude: ${limits.sim_gain.absolute_min}–${limits.sim_gain.max}.`;
    } else {
      gainHint = `Allowed range: ${limits.sim_gain.min}–${limits.sim_gain.max}.`;
    }
  }
  return (
    <>
      <NumberField
        label="Process gain"
        value={form.simGain}
        onChange={(value) => onChange("simGain", value)}
        min={limits?.sim_gain.min}
        max={limits?.sim_gain.max}
        step="any"
        hint={gainHint}
        error={fieldError?.("simGain")}
      />
      <NumberField
        label="Time constant τ (s)"
        value={form.simTau}
        onChange={(value) => onChange("simTau", value)}
        min={limits?.sim_tau.min}
        max={limits?.sim_tau.max}
        step="any"
        error={fieldError?.("simTau")}
      />
      <NumberField
        label="Dead time (s)"
        value={form.simDeadTime}
        onChange={(value) => onChange("simDeadTime", value)}
        min={limits?.sim_dead_time.min}
        max={limits?.sim_dead_time.max}
        step="any"
        error={fieldError?.("simDeadTime")}
      />
      <NumberField
        label="Measurement noise"
        value={form.simNoise}
        onChange={(value) => onChange("simNoise", value)}
        min={limits ? 0 : undefined}
        max={maxNoise}
        step="any"
        hint={simulatorNoiseHint(limits)}
        error={fieldError?.("simNoise")}
      />
      <NumberField
        label="RNG seed"
        value={form.simSeed}
        onChange={(value) => onChange("simSeed", value)}
        min={limits?.sim_seed.min}
        max={limits?.sim_seed.max}
        step={1}
        hint="Fixed seed = reproducible noise."
        error={fieldError?.("simSeed")}
      />
    </>
  );
}

function simulatorNoiseHint(
  limits: SimulatorCapabilities["limits"] | undefined,
): string | undefined {
  return limits
    ? `At most ${limits.max_noise_fraction_of_pv_span * 100}% of the PV span.`
    : undefined;
}

function SimulatorInitialFields({
  form,
  onChange,
  simulatorCapabilities,
  fieldError,
}: SimulatorParameterProps) {
  const pvBounds = simulatorCapabilities
    ? numericBounds(form.simPvRangeLow, form.simPvRangeHigh)
    : { min: undefined, max: undefined };
  const mvBounds = simulatorCapabilities
    ? numericBounds(form.simMvRangeLow, form.simMvRangeHigh)
    : { min: undefined, max: undefined };
  return (
    <>
      <div />
      <NumberField
        label="Initial PV"
        value={form.simInitialPv}
        onChange={(value) => onChange("simInitialPv", value)}
        min={pvBounds.min}
        max={pvBounds.max}
        step="any"
        error={fieldError?.("simInitialPv")}
      />
      <NumberField
        label="Initial MV"
        value={form.simInitialMv}
        onChange={(value) => onChange("simInitialMv", value)}
        min={mvBounds.min}
        max={mvBounds.max}
        step="any"
        error={fieldError?.("simInitialMv")}
      />
    </>
  );
}

function SimulatorRangeFields({
  form,
  onChange,
  simulatorCapabilities,
  fieldError,
}: SimulatorParameterProps) {
  if (!simulatorCapabilities) return null;
  const { range_endpoint: endpoint, range_span: span } =
    simulatorCapabilities.limits;
  return (
    <>
      <NumberField
        label="PV range low"
        value={form.simPvRangeLow}
        onChange={(value) => onChange("simPvRangeLow", value)}
        min={endpoint.min}
        max={endpoint.max}
        step="any"
        error={fieldError?.("simPvRangeLow")}
      />
      <NumberField
        label="PV range high"
        value={form.simPvRangeHigh}
        onChange={(value) => onChange("simPvRangeHigh", value)}
        min={endpoint.min}
        max={endpoint.max}
        step="any"
        hint={`PV span must be ${span.min}–${span.max}.`}
        error={fieldError?.("simPvRangeHigh")}
      />
      <NumberField
        label="MV range low"
        value={form.simMvRangeLow}
        onChange={(value) => onChange("simMvRangeLow", value)}
        min={endpoint.min}
        max={endpoint.max}
        step="any"
        error={fieldError?.("simMvRangeLow")}
      />
      <NumberField
        label="MV range high"
        value={form.simMvRangeHigh}
        onChange={(value) => onChange("simMvRangeHigh", value)}
        min={endpoint.min}
        max={endpoint.max}
        step="any"
        hint={`MV span must be ${span.min}–${span.max}.`}
        error={fieldError?.("simMvRangeHigh")}
      />
    </>
  );
}

export function SimulatorModelSection(props: SimulatorParameterProps) {
  if (props.form.driver !== "simulator") return null;
  return <SimulatorModelInfo {...props} />;
}

export function SimulatorParameterSection(props: SimulatorParameterProps) {
  if (props.form.driver !== "simulator") return null;
  return (
    <FormSection
      title="Simulator parameters"
      collapsible
      defaultOpen
      documentationId="new-tune.simulator-parameters"
    >
      <SimulatorProcessFields {...props} />
      <SimulatorInitialFields {...props} />
      <SimulatorRangeFields {...props} />
    </FormSection>
  );
}
