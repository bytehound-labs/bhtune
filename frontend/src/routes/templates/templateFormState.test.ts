import { describe, expect, it } from "vitest";
import type { DcsTemplate } from "../../api/templates";
import {
  blankTemplateForm,
  templateFormStateToTemplate,
  templateToFormState,
} from "./templateFormState";

const template: DcsTemplate = {
  name: "Example DCS",
  revert_mode: false,
  proportional_type: "band",
  integral_type: "reset_rate",
  integral_unit: "seconds",
  derivative_type: "derivative_gain",
  derivative_unit: "minutes",
  process_variable_suffix: "PV",
  manipulated_variable_suffix: "OUT",
  setpoint_variable_suffix: "SP",
  controller_direction_suffix: "DIR",
  controller_mode_suffix: "MODE",
  mode_attribute_suffix: "ATTR",
  upper_pv_range_suffix: "PVEUHI",
  lower_pv_range_suffix: "PVEULO",
  upper_mv_range_suffix: "OUTHI",
  lower_mv_range_suffix: "OUTLO",
  proportional_constant_suffix: "PB",
  integral_constant_suffix: "RR",
  derivative_constant_suffix: "KD",
  mode_manual_value: "MAN",
  mode_auto_value: "AUTO",
  mode_attribute_program_value: "PROG",
  controller_action_direct_value: "D",
  description: "A representative control system.",
  source: "Engineering manual",
  versions: ["R5", "R6"],
};

describe("template form conversions", () => {
  it("round-trips a template and joins its version list for the form", () => {
    const form = templateToFormState(template);

    expect(form.versionsText).toBe("R5, R6");
    expect(templateFormStateToTemplate(form)).toEqual(template);
  });

  it("uses an empty version field when the template has no versions", () => {
    expect(
      templateToFormState({ ...template, versions: undefined }).versionsText,
    ).toBe("");
  });

  it("trims version tokens and normalizes blank optional fields to null", () => {
    const result = templateFormStateToTemplate({
      ...blankTemplateForm,
      name: "Example DCS",
      versionsText: " R5, , R6 ,, ",
    });

    expect(result.versions).toEqual(["R5", "R6"]);
    expect(result.description).toBeNull();
    expect(result.source).toBeNull();
    expect(result.mode_attribute_program_value).toBeNull();
  });
});
