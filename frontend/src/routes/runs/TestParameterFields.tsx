import {
  Button,
  FormSection,
  NumberField,
  SelectField,
} from "../../components/ui";
import {
  CONTROLLER_TYPE_LABELS,
  PROCESS_TYPE_LABELS,
} from "../../lib/enumLabels";
import {
  CONTROLLER_TYPES,
  PROCESS_TYPES,
  TEMPERATURE_PROCESS_TYPES,
  type ControllerType,
  type ProcessType,
} from "./newRunFormState";
import type { NewRunSectionProps } from "./newRunSectionShared";

function controllerTypeOptions(
  processType: ProcessType,
): readonly ControllerType[] {
  if (TEMPERATURE_PROCESS_TYPES.has(processType)) return CONTROLLER_TYPES;
  return CONTROLLER_TYPES.filter((controllerType) => controllerType !== "pid");
}

export function TestParameterFields({
  form,
  onChange,
  onProcessTypeChange,
  onResetProcessDefaults,
}: Pick<
  NewRunSectionProps,
  "form" | "onChange" | "onProcessTypeChange" | "onResetProcessDefaults"
>) {
  return (
    <FormSection
      title="Test parameters"
      collapsible
      defaultOpen
      documentationId="new-tune.test-parameters"
    >
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
