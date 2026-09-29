import { CheckboxField, FormSection, SelectField } from "../../components/ui";
import { RESPONSE_LEVEL_LABELS } from "../../lib/enumLabels";
import { RESPONSE_LEVELS } from "./newRunFormState";
import type { NewRunSectionProps } from "./newRunSectionShared";

export function WriteBackFields({
  form,
  onChange,
}: Pick<NewRunSectionProps, "form" | "onChange">) {
  const isSimulator = form.driver === "simulator";
  const disabledHint = isSimulator
    ? "Disabled — the simulator has no PID constant tags to write to."
    : undefined;

  return (
    <FormSection
      title="Automatic PID settings"
      collapsible
      defaultOpen
      documentationId="new-tune.automatic-pid"
    >
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
