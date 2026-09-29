import { FormSection, NumberField, SelectField } from "../../components/ui";
import {
  CONTROLLER_TYPE_LABELS,
  PROCESS_TYPE_LABELS,
} from "../../lib/enumLabels";
import {
  SimulatorModelSection,
  SimulatorParameterSection,
} from "./SimulatorFields";
import {
  demoControllerTypesFor,
  demoDefaultControllerTypeFor,
} from "./newRunFormState";
import { templateHint, type NewRunSectionProps } from "./newRunSectionShared";

export function DemoRunFields({
  form,
  onChange,
  onProcessTypeChange,
  simulatorCapabilities,
}: Pick<
  NewRunSectionProps,
  "form" | "onChange" | "onProcessTypeChange" | "simulatorCapabilities"
>) {
  if (!simulatorCapabilities) return null;
  const processTypes = simulatorCapabilities.process_types;
  const processType = processTypes.includes(form.processType)
    ? form.processType
    : processTypes[0];
  if (!processType) return null;
  const controllerTypes = processType
    ? demoControllerTypesFor(simulatorCapabilities, processType)
    : [];
  const controllerType = controllerTypes.includes(form.controllerType)
    ? form.controllerType
    : demoDefaultControllerTypeFor(simulatorCapabilities, processType);
  return (
    <>
      <FormSection
        title="Demo tune settings"
        collapsible
        defaultOpen
        documentationId="new-tune.demo-settings"
      >
        <div>
          <SelectField
            label="Template"
            value={form.template}
            onChange={(value) => onChange("template", value)}
            options={simulatorCapabilities.templates}
          />
          <span className="mt-1 block text-xs text-slate-500">
            {templateHint("simulator")}
          </span>
        </div>
        <SelectField
          label="Process type"
          value={processType ?? ""}
          onChange={onProcessTypeChange}
          options={processTypes}
          displayLabel={(value) => PROCESS_TYPE_LABELS[value]}
        />
        <SelectField
          label="Controller type"
          value={controllerType ?? ""}
          onChange={(value) => onChange("controllerType", value)}
          options={controllerTypes}
          displayLabel={(value) => CONTROLLER_TYPE_LABELS[value]}
        />
        <NumberField
          label="Relay amplitude (%)"
          value={form.relayAmp}
          onChange={(value) => onChange("relayAmp", value)}
          min={simulatorCapabilities.limits.relay_amp.min}
          max={simulatorCapabilities.limits.relay_amp.max}
          step={0.1}
          required
          hint={`Allowed range: ${simulatorCapabilities.limits.relay_amp.min}–${simulatorCapabilities.limits.relay_amp.max}%.`}
        />
        <NumberField
          label="Cycles to skip"
          value={form.cyclesSkip}
          onChange={(value) => onChange("cyclesSkip", value)}
          min={simulatorCapabilities.limits.cycles_skip.min}
          max={simulatorCapabilities.limits.cycles_skip.max}
          step={1}
          required
        />
        <NumberField
          label="Cycles to count"
          value={form.cyclesCount}
          onChange={(value) => onChange("cyclesCount", value)}
          min={simulatorCapabilities.limits.cycles_count.min}
          max={simulatorCapabilities.limits.cycles_count.max}
          step={1}
          required
          hint={`Demo limit: ${simulatorCapabilities.limits.cycles_count.max} cycles per run.`}
        />
        <NumberField
          label="Noise protection (s)"
          value={form.noiseProtectionSecs}
          onChange={(value) => onChange("noiseProtectionSecs", value)}
          min={simulatorCapabilities.limits.noise_protection_secs.min}
          max={simulatorCapabilities.limits.noise_protection_secs.max}
          step={1}
          required
        />
        <p className="text-sm text-slate-400 sm:col-span-2">
          The server fixes the tag identity. It uses{" "}
          {simulatorCapabilities.defaults.poll_interval_ms} ms sampling and a{" "}
          {simulatorCapabilities.defaults.run_timeout_secs}s run timeout. Demo
          runs never connect to OPC DA or write PID values.
        </p>
      </FormSection>
      <SimulatorParameterSection
        form={form}
        onChange={onChange}
        simulatorCapabilities={simulatorCapabilities}
      />
      <SimulatorModelSection
        form={form}
        onChange={onChange}
        simulatorCapabilities={simulatorCapabilities}
      />
    </>
  );
}
