import type { DcsTemplate } from "../../api/templates";

export const PROPORTIONAL_TYPES = ["gain", "band"] as const;
export const INTEGRAL_TYPES = [
  "reset_time",
  "reset_rate",
  "reset_gain",
] as const;
export const DERIVATIVE_TYPES = ["derivative_time", "derivative_gain"] as const;
export const TIME_UNITS = ["seconds", "minutes"] as const;

/** Local form state: `DcsTemplate` with `versions` as one comma-separated string, since a
 * plain text input is simpler than a dynamic list editor for what's usually 0-3 short
 * tokens (`"R5, R6"`) — converted back to `string[]` on submit. Shared by the Create and
 * Edit pages so the two forms can never drift apart. */
export type TemplateFormState = Omit<DcsTemplate, "versions"> & {
  versionsText: string;
};

export const blankTemplateForm: TemplateFormState = {
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

/** Converts a stored [`DcsTemplate`] (as returned by `GET /api/templates/{name}`) into this
 * form's local state, for the Edit page to pre-populate its fields from. The inverse of
 * {@link templateFormStateToTemplate}. */
export function templateToFormState(template: DcsTemplate): TemplateFormState {
  const { versions, ...rest } = template;
  return {
    ...rest,
    versionsText: versions ? versions.join(", ") : "",
  };
}

/** Converts this form's local state back into a [`DcsTemplate`] ready to `POST`/`PUT`. */
export function templateFormStateToTemplate(
  form: TemplateFormState,
): DcsTemplate {
  const { versionsText, ...rest } = form;
  return {
    ...rest,
    description: rest.description || null,
    source: rest.source || null,
    mode_attribute_program_value: rest.mode_attribute_program_value || null,
    versions: versionsText
      .split(",")
      .map((v) => v.trim())
      .filter((v) => v.length > 0),
  };
}
