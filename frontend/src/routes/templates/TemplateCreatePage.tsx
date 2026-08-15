import { useState } from "react";
import { useNavigate, Link } from "react-router";
import { useCreateTemplate } from "../../api/templates";
import type { DcsTemplate } from "../../api/templates";
import {
  Button,
  CheckboxField,
  ErrorBanner,
  FormSection,
  PageHeading,
  SelectField,
  TextField,
} from "../../components/ui";

const PROPORTIONAL_TYPES = ["gain", "band"] as const;
const INTEGRAL_TYPES = ["reset_time", "reset_rate", "reset_gain"] as const;
const DERIVATIVE_TYPES = ["derivative_time", "derivative_gain"] as const;
const TIME_UNITS = ["seconds", "minutes"] as const;

/** Local form state: `DcsTemplate` with `versions` as one comma-separated string, since a
 * plain text input is simpler than a dynamic list editor for what's usually 0-3 short
 * tokens (`"R5, R6"`) — converted back to `string[]` on submit. */
type TemplateFormState = Omit<DcsTemplate, "versions"> & {
  versionsText: string;
};

const emptyForm: TemplateFormState = {
  name: "",
  revert_mode: true,
  proportional_type: "gain",
  integral_type: "reset_time",
  integral_unit: "minutes",
  derivative_type: "derivative_time",
  derivative_unit: "minutes",
  process_variable_suffix: "",
  manipulated_variable_suffix: "",
  setpoint_variable_suffix: "",
  controller_direction_suffix: "",
  controller_mode_suffix: "",
  mode_attribute_suffix: "",
  upper_pv_range_suffix: "",
  lower_pv_range_suffix: "",
  upper_mv_range_suffix: "",
  lower_mv_range_suffix: "",
  proportional_constant_suffix: "",
  integral_constant_suffix: "",
  derivative_constant_suffix: "",
  mode_manual_value: "",
  mode_auto_value: "",
  mode_attribute_program_value: "",
  controller_action_direct_value: "",
  description: "",
  source: "",
  versionsText: "",
};

export function TemplateCreatePage() {
  const navigate = useNavigate();
  const createTemplate = useCreateTemplate();
  const [form, setForm] = useState<TemplateFormState>(emptyForm);

  function set<K extends keyof TemplateFormState>(
    key: K,
    value: TemplateFormState[K],
  ) {
    setForm((prev) => ({ ...prev, [key]: value }));
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const { versionsText, ...rest } = form;
    const template: DcsTemplate = {
      ...rest,
      description: rest.description || null,
      source: rest.source || null,
      mode_attribute_program_value: rest.mode_attribute_program_value || null,
      versions: versionsText
        .split(",")
        .map((v) => v.trim())
        .filter((v) => v.length > 0),
    };
    createTemplate.mutate(template, {
      onSuccess: () =>
        navigate(`/templates/${encodeURIComponent(template.name)}`),
    });
  }

  return (
    <div>
      <PageHeading
        title="New template"
        description="Creates a user-owned template. There is no update endpoint — fix a mistake by deleting and recreating it."
        actions={
          <Link to="/templates">
            <Button>Cancel</Button>
          </Link>
        }
      />

      {createTemplate.isError && (
        <div className="mb-4">
          <ErrorBanner message={createTemplate.error.message} />
        </div>
      )}

      <form onSubmit={handleSubmit}>
        <FormSection title="Identity">
          <TextField
            label="Name"
            required
            value={form.name}
            onChange={(v) => set("name", v)}
            placeholder="e.g. Yokogawa CentumVP"
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

        <div className="flex gap-2">
          <Button
            type="submit"
            variant="primary"
            disabled={createTemplate.isPending}
          >
            {createTemplate.isPending ? "Creating…" : "Create template"}
          </Button>
          <Link to="/templates">
            <Button>Cancel</Button>
          </Link>
        </div>
      </form>
    </div>
  );
}
