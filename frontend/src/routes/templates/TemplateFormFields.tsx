import {
  CheckboxField,
  FormSection,
  SelectField,
  TextField,
} from "../../components/ui";
import {
  DERIVATIVE_TYPES,
  INTEGRAL_TYPES,
  PROPORTIONAL_TYPES,
  TIME_UNITS,
  type TemplateFormState,
} from "./templateFormState";

/**
 * Every `DcsTemplate` field, laid out in the same `FormSection`s for both the Create and
 * Edit pages. `nameEditable` is `false` on the Edit page: `PUT /api/templates/{name}`
 * rejects a body whose `name` doesn't match the path (see `bhtune-server`'s
 * `update_template` doc comment), so renaming a template means deleting and recreating it,
 * not editing its name in place.
 */
export function TemplateFormFields({
  form,
  set,
  nameEditable = true,
}: {
  form: TemplateFormState;
  set: <K extends keyof TemplateFormState>(
    key: K,
    value: TemplateFormState[K],
  ) => void;
  nameEditable?: boolean;
}) {
  return (
    <>
      <FormSection title="Identity">
        <TextField
          label="Name"
          required
          disabled={!nameEditable}
          value={form.name}
          onChange={(v) => set("name", v)}
          placeholder="e.g. Yokogawa CentumVP"
          hint={
            nameEditable
              ? undefined
              : "Renaming isn't supported here — delete and recreate the template instead."
          }
        />
        <TextField
          label="Versions"
          value={form.versionsText}
          onChange={(v) => set("versionsText", v)}
          placeholder="R5, R6"
          hint="Comma-separated releases this mapping is known to apply to."
        />
        <TextField
          label="Description"
          value={form.description ?? ""}
          onChange={(v) => set("description", v)}
          full
        />
        <TextField
          label="Source"
          value={form.source ?? ""}
          onChange={(v) => set("source", v)}
          placeholder="Citation: a manual, a field deployment"
          full
        />
      </FormSection>

      <FormSection title="Behavior">
        <CheckboxField
          label="Revert mode after test"
          hint="Switch the controller back to its original mode after a completed MRFT test."
          checked={form.revert_mode}
          onChange={(v) => set("revert_mode", v)}
        />
        <div />
        <SelectField
          label="Proportional type"
          value={form.proportional_type}
          onChange={(v) => set("proportional_type", v)}
          options={PROPORTIONAL_TYPES}
        />
        <div />
        <SelectField
          label="Integral type"
          value={form.integral_type}
          onChange={(v) => set("integral_type", v)}
          options={INTEGRAL_TYPES}
        />
        <SelectField
          label="Integral unit"
          value={form.integral_unit}
          onChange={(v) => set("integral_unit", v)}
          options={TIME_UNITS}
        />
        <SelectField
          label="Derivative type"
          value={form.derivative_type}
          onChange={(v) => set("derivative_type", v)}
          options={DERIVATIVE_TYPES}
        />
        <SelectField
          label="Derivative unit"
          value={form.derivative_unit}
          onChange={(v) => set("derivative_unit", v)}
          options={TIME_UNITS}
        />
      </FormSection>

      <FormSection title="Tag suffixes">
        <TextField
          label="Process variable"
          required
          value={form.process_variable_suffix}
          onChange={(v) => set("process_variable_suffix", v)}
        />
        <TextField
          label="Manipulated variable"
          required
          value={form.manipulated_variable_suffix}
          onChange={(v) => set("manipulated_variable_suffix", v)}
        />
        <TextField
          label="Setpoint"
          value={form.setpoint_variable_suffix}
          onChange={(v) => set("setpoint_variable_suffix", v)}
        />
        <TextField
          label="Controller direction"
          value={form.controller_direction_suffix}
          onChange={(v) => set("controller_direction_suffix", v)}
        />
        <TextField
          label="Controller mode"
          value={form.controller_mode_suffix}
          onChange={(v) => set("controller_mode_suffix", v)}
        />
        <TextField
          label="Mode attribute"
          value={form.mode_attribute_suffix}
          onChange={(v) => set("mode_attribute_suffix", v)}
          hint="Leave blank if this DCS has no mode-attribute concept."
        />
        <TextField
          label="Upper PV range"
          value={form.upper_pv_range_suffix}
          onChange={(v) => set("upper_pv_range_suffix", v)}
        />
        <TextField
          label="Lower PV range"
          value={form.lower_pv_range_suffix}
          onChange={(v) => set("lower_pv_range_suffix", v)}
        />
        <TextField
          label="Upper MV range"
          value={form.upper_mv_range_suffix}
          onChange={(v) => set("upper_mv_range_suffix", v)}
        />
        <TextField
          label="Lower MV range"
          value={form.lower_mv_range_suffix}
          onChange={(v) => set("lower_mv_range_suffix", v)}
        />
        <TextField
          label="Proportional constant"
          value={form.proportional_constant_suffix}
          onChange={(v) => set("proportional_constant_suffix", v)}
        />
        <TextField
          label="Integral constant"
          value={form.integral_constant_suffix}
          onChange={(v) => set("integral_constant_suffix", v)}
        />
        <TextField
          label="Derivative constant"
          value={form.derivative_constant_suffix}
          onChange={(v) => set("derivative_constant_suffix", v)}
        />
      </FormSection>

      <FormSection title="Mode values">
        <TextField
          label="Manual value"
          value={form.mode_manual_value}
          onChange={(v) => set("mode_manual_value", v)}
          hint="The raw value the Mode tag holds when the controller is in Manual."
        />
        <TextField
          label="Auto value"
          value={form.mode_auto_value}
          onChange={(v) => set("mode_auto_value", v)}
          hint="The raw value the Mode tag holds when the controller is in Auto/Cascade."
        />
        <TextField
          label="Mode-attribute Program value"
          value={form.mode_attribute_program_value ?? ""}
          onChange={(v) => set("mode_attribute_program_value", v)}
          hint="Required only if a Mode attribute suffix is set above."
        />
        <TextField
          label="Controller-direction Direct value"
          value={form.controller_action_direct_value}
          onChange={(v) => set("controller_action_direct_value", v)}
          hint="The raw value the Controller Direction tag holds when Direct acting."
        />
      </FormSection>
    </>
  );
}
