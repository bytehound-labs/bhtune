import type { SubmitEvent } from "react";
import type { components } from "../../api/schema";
import { OpcServerDiscovery } from "../../components/OpcServerDiscovery";
import {
  Button,
  CheckboxField,
  FormSection,
  NumberField,
  SelectField,
  TextAreaField,
  TextField,
} from "../../components/ui";
import {
  CONTROLLER_TYPE_LABELS,
  DRIVER_LABELS,
  PROCESS_TYPE_LABELS,
  RESPONSE_LEVEL_LABELS,
} from "../../lib/enumLabels";
import { LoopMappingEditor } from "./LoopMappingEditor";
import {
  CONTROLLER_TYPES,
  DRIVERS,
  PROCESS_TYPES,
  RESPONSE_LEVELS,
  TEMPERATURE_PROCESS_TYPES,
  type ControllerType,
  type FormState,
  type ProcessType,
  type TuneDriver,
} from "./newRunFormState";
import {
  type ControllerDirection,
  type NumOrBlank,
  type TagMappingSource,
  type TagOverrideKey,
  type ValueMappingKey,
  type ValueMappingSource,
} from "./mappingState";

type TemplateResponse = components["schemas"]["TemplateResponse"];

type FormChange = <K extends keyof FormState>(
  key: K,
  value: FormState[K],
) => void;

type NewRunFormProps = {
  readonly form: FormState;
  readonly template: TemplateResponse | undefined;
  readonly templates: readonly TemplateResponse[] | undefined;
  readonly templatesPending: boolean;
  readonly onSubmit: (event: SubmitEvent<HTMLFormElement>) => void;
  readonly onChange: FormChange;
  readonly onTagNameChange: (value: string) => void;
  readonly onDriverChange: (value: TuneDriver) => void;
  readonly onTemplateChange: (value: string) => void;
  readonly onProcessTypeChange: (value: ProcessType) => void;
  readonly onResetProcessDefaults: () => void;
  readonly onTagSourceChange: (
    key: TagOverrideKey,
    source: TagMappingSource,
  ) => void;
  readonly onTagChange: (key: TagOverrideKey, value: string) => void;
  readonly onValueSourceChange: (
    key: ValueMappingKey,
    source: ValueMappingSource,
  ) => void;
  readonly onValueTagChange: (key: ValueMappingKey, value: string) => void;
  readonly onValueChange: (
    key:
      | "opcDirection"
      | "opcPvRangeHigh"
      | "opcPvRangeLow"
      | "opcMvRangeHigh"
      | "opcMvRangeLow"
      | "simDirection"
      | "simPvRangeHigh"
      | "simPvRangeLow"
      | "simMvRangeHigh"
      | "simMvRangeLow",
    value: NumOrBlank | ControllerDirection,
  ) => void;
  readonly onResetTag: (key: TagOverrideKey) => void;
  readonly onResetValue: (key: ValueMappingKey) => void;
  readonly onResetAll: () => void;
  readonly onOpenTagBrowser: () => void;
};

function templateHint(driver: TuneDriver): string {
  if (driver === "simulator") {
    return "The simulator ignores DCS tag mappings, but the template still formats calculated PID values (for example, gain versus proportional band).";
  }
  return "Maps the connected DCS/PLC's item IDs and PID conventions.";
}

function disabledGatewayHint(driver: TuneDriver): string | undefined {
  if (driver === "simulator") {
    return "Disabled — the simulator never contacts a gateway.";
  }
  return "opcda-bridge gateway address (host:port).";
}

function tagNameHint(driver: TuneDriver): string {
  if (driver === "simulator") {
    return "Disabled — the simulator hardcodes its own PV/MV tags and ignores this.";
  }
  return "PV tag prefix; the rest of the tag set is derived from it via the template's suffixes.";
}

function connectionFields({
  form,
  templates,
  templatesPending,
  onChange,
  onTagNameChange,
  onDriverChange,
  onTemplateChange,
  onOpenTagBrowser,
}: Pick<
  NewRunFormProps,
  | "form"
  | "templates"
  | "templatesPending"
  | "onChange"
  | "onTagNameChange"
  | "onDriverChange"
  | "onTemplateChange"
  | "onOpenTagBrowser"
>) {
  const isSimulator = form.driver === "simulator";
  const hasServer = form.server.trim().length > 0;

  return (
    <FormSection title="Connection" collapsible defaultOpen>
      <SelectField
        label="Driver"
        value={form.driver}
        onChange={onDriverChange}
        options={DRIVERS}
        displayLabel={(value) => DRIVER_LABELS[value]}
      />
      <div>
        <SelectField
          label="Template"
          value={form.template}
          onChange={onTemplateChange}
          options={(templates ?? []).map((template) => template.name)}
          placeholder={
            templatesPending ? "Loading templates…" : "Choose a template"
          }
        />
        <span className="mt-1 block text-xs text-slate-500">
          {templateHint(form.driver)}
        </span>
      </div>
      <TextField
        label="Bridge host"
        disabled={isSimulator}
        value={form.bridgeHost}
        onChange={(value) => onChange("bridgeHost", value)}
        placeholder="Defaults to this server's own configured bridge host"
        hint={disabledGatewayHint(form.driver)}
      />
      <div>
        <TextField
          label="OPC DA server ProgID"
          required={!isSimulator}
          disabled={isSimulator}
          value={form.server}
          onChange={(value) => onChange("server", value)}
          placeholder="e.g. Matrikon.OPC.Simulation"
          hint={disabledGatewayHint(form.driver)}
        />
        {form.driver === "opcda" && (
          <OpcServerDiscovery
            bridgeHost={form.bridgeHost}
            onSelect={(value) => onChange("server", value)}
          />
        )}
      </div>
      <div>
        <TextField
          label="Tag name"
          required={!isSimulator}
          disabled={isSimulator}
          value={form.tagname}
          onChange={onTagNameChange}
          hint={tagNameHint(form.driver)}
        />
        {form.driver === "opcda" && (
          <div className="mt-1">
            <Button
              onClick={onOpenTagBrowser}
              disabled={!hasServer}
              title={
                hasServer ? undefined : "Enter an OPC DA server ProgID first."
              }
            >
              Browse tags
            </Button>
          </div>
        )}
      </div>
      <TextAreaField
        label="Notes"
        value={form.notes}
        onChange={(value) => onChange("notes", value)}
        full
        placeholder="Optional context, observations, or follow-up actions"
        hint="Notes can be edited or cleared from the tune history."
      />
    </FormSection>
  );
}

function controllerTypeOptions(
  processType: ProcessType,
): readonly ControllerType[] {
  if (TEMPERATURE_PROCESS_TYPES.has(processType)) return CONTROLLER_TYPES;
  return CONTROLLER_TYPES.filter((controllerType) => controllerType !== "pid");
}

function testParameterFields({
  form,
  onChange,
  onProcessTypeChange,
  onResetProcessDefaults,
}: Pick<
  NewRunFormProps,
  "form" | "onChange" | "onProcessTypeChange" | "onResetProcessDefaults"
>) {
  return (
    <FormSection title="Test parameters" collapsible defaultOpen>
      <SelectField
        label="Process type"
        value={form.processType}
        onChange={onProcessTypeChange}
        options={PROCESS_TYPES}
        displayLabel={(value) => PROCESS_TYPE_LABELS[value]}
      />
      <SelectField
        label="Controller type"
        value={form.controllerType}
        onChange={(value) => onChange("controllerType", value)}
        options={controllerTypeOptions(form.processType)}
        displayLabel={(value) => CONTROLLER_TYPE_LABELS[value]}
      />
      <NumberField
        label="Relay amplitude (%)"
        required
        value={form.relayAmp}
        onChange={(value) => onChange("relayAmp", value)}
        min={0.1}
        max={50}
        step={0.1}
        hint="0.1–50% of the MV range."
      />
      <div />
      <fieldset className="rounded-md border border-slate-800 p-4 sm:col-span-2">
        <legend className="px-2 text-sm font-semibold text-slate-300">
          Process defaults
        </legend>
        <p className="mb-4 text-sm text-slate-400">
          These values follow Process type. Changing Process type or resetting
          them replaces all three values.
        </p>
        <div className="grid gap-4 sm:grid-cols-2">
          <NumberField
            label="Cycles to skip"
            required
            value={form.cyclesSkip}
            onChange={(value) => onChange("cyclesSkip", value)}
            min={0}
            step={1}
          />
          <NumberField
            label="Cycles to count"
            required
            value={form.cyclesCount}
            onChange={(value) => onChange("cyclesCount", value)}
            min={1}
            step={1}
          />
          <NumberField
            label="Noise protection (s)"
            required
            value={form.noiseProtectionSecs}
            onChange={(value) => onChange("noiseProtectionSecs", value)}
            min={0}
            step={1}
          />
          <div className="flex items-end">
            <Button onClick={onResetProcessDefaults}>
              Reset process defaults
            </Button>
          </div>
        </div>
      </fieldset>
      <p className="text-sm text-slate-400 sm:col-span-2">
        MRFT timing and safety limits are managed globally in Configuration and
        apply to new tunes.
      </p>
    </FormSection>
  );
}

function simulatorParameterFields({
  form,
  onChange,
}: Pick<NewRunFormProps, "form" | "onChange">) {
  if (form.driver !== "simulator") return null;

  return (
    <FormSection title="Simulator parameters" collapsible defaultOpen>
      <NumberField
        label="Process gain"
        value={form.simGain}
        onChange={(value) => onChange("simGain", value)}
        step="any"
      />
      <NumberField
        label="Time constant τ (s)"
        value={form.simTau}
        onChange={(value) => onChange("simTau", value)}
        step="any"
      />
      <NumberField
        label="Dead time (s)"
        value={form.simDeadTime}
        onChange={(value) => onChange("simDeadTime", value)}
        step="any"
      />
      <NumberField
        label="Measurement noise"
        value={form.simNoise}
        onChange={(value) => onChange("simNoise", value)}
        step="any"
      />
      <NumberField
        label="RNG seed"
        value={form.simSeed}
        onChange={(value) => onChange("simSeed", value)}
        min={0}
        step={1}
        hint="Fixed seed = reproducible noise."
      />
      <div />
      <NumberField
        label="Initial PV"
        value={form.simInitialPv}
        onChange={(value) => onChange("simInitialPv", value)}
        step="any"
      />
      <NumberField
        label="Initial MV"
        value={form.simInitialMv}
        onChange={(value) => onChange("simInitialMv", value)}
        step="any"
      />
    </FormSection>
  );
}

function automaticPidFields({
  form,
  onChange,
}: Pick<NewRunFormProps, "form" | "onChange">) {
  const isSimulator = form.driver === "simulator";
  const disabledHint = isSimulator
    ? "Disabled — the simulator has no PID constant tags to write to."
    : undefined;

  return (
    <FormSection title="Automatic PID settings" collapsible defaultOpen>
      <SelectField
        label="Apply PID settings on completion"
        value={form.writePid}
        onChange={(value) => onChange("writePid", value)}
        options={RESPONSE_LEVELS}
        displayLabel={(value) => RESPONSE_LEVEL_LABELS[value]}
        placeholder="Do not apply automatically"
        disabled={isSimulator}
        hint={disabledHint}
      />
      <CheckboxField
        label="Allow automatic PID write"
        checked={form.yes}
        onChange={(value) => onChange("yes", value)}
        disabled={isSimulator}
        hint={
          disabledHint ??
          "Required when automatic PID settings are selected — applying changes to a live loop without a prompt must be deliberate."
        }
      />
    </FormSection>
  );
}

export function NewRunForm({
  form,
  template,
  templates,
  templatesPending,
  onSubmit,
  onChange,
  onTagNameChange,
  onDriverChange,
  onTemplateChange,
  onProcessTypeChange,
  onResetProcessDefaults,
  onTagSourceChange,
  onTagChange,
  onValueSourceChange,
  onValueTagChange,
  onValueChange,
  onResetTag,
  onResetValue,
  onResetAll,
  onOpenTagBrowser,
}: NewRunFormProps) {
  return (
    <form onSubmit={onSubmit}>
      {connectionFields({
        form,
        templates,
        templatesPending,
        onChange,
        onTagNameChange,
        onDriverChange,
        onTemplateChange,
        onOpenTagBrowser,
      })}
      {testParameterFields({
        form,
        onChange,
        onProcessTypeChange,
        onResetProcessDefaults,
      })}
      <FormSection title="Loop mapping" collapsible defaultOpen>
        <LoopMappingEditor
          state={form}
          template={template}
          onTagSourceChange={onTagSourceChange}
          onTagChange={onTagChange}
          onValueSourceChange={onValueSourceChange}
          onValueTagChange={onValueTagChange}
          onValueChange={onValueChange}
          onResetTag={onResetTag}
          onResetValue={onResetValue}
          onResetAll={onResetAll}
        />
      </FormSection>
      {simulatorParameterFields({ form, onChange })}
      {automaticPidFields({ form, onChange })}
    </form>
  );
}
