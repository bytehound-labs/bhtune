import { describe, expect, it } from "vitest";
import type { StartRunRequest } from "../../api/runs";
import { applyTagNameChange } from "./applyTagNameChange";
import { draftFromForm, formFromDraft } from "./draftAutosave";
import { buildRequest, formFromRequest } from "./formRequest";
import { initialForm, type FormState } from "./newRunFormState";
import {
  prefillMessage,
  resolveTemplateDefaulting,
  type NewRunPageState,
  type PrefillSource,
} from "./prefillPrecedence";

function createOpcForm(): FormState {
  return {
    ...initialForm,
    driver: "opcda",
    template: "Yokogawa CentumVP",
    tagname: "FCS0217!204FC03010.PV",
    server: "Yokogawa.CSHIS_OPC.1",
    bridgeHost: "gateway.example.test:7600",
    tagSources: {
      ...initialForm.tagSources,
      processVariable: "custom",
      manipulatedVariable: "custom",
    },
    valueSources: {
      ...initialForm.valueSources,
      direction: "fixed",
      pvRangeHigh: "custom",
      pvRangeLow: "fixed",
      mvRangeHigh: "tag",
      mvRangeLow: "fixed",
    },
    valueTagOverrides: {
      ...initialForm.valueTagOverrides,
      pvRangeHigh: "FCS0217!204FC03010.PVEUHI",
    },
    opcDirection: "reverse",
    opcPvRangeHigh: "",
    opcPvRangeLow: 0,
    opcMvRangeHigh: "",
    opcMvRangeLow: 100,
    tagOverrides: {
      ...initialForm.tagOverrides,
      processVariable: "FCS0217!204FC03010.PV",
      manipulatedVariable: "FCS0217!204FC03010.OUT",
    },
  };
}

function requestFromForm(form: FormState): StartRunRequest {
  const request = buildRequest(form);
  if (typeof request === "string") {
    throw new TypeError(`Expected a valid form, got: ${request}`);
  }
  return request;
}

describe("New Run form conversions", () => {
  it("round-trips simulator request values through form state", () => {
    const form: FormState = {
      ...initialForm,
      template: "Honeywell Experion",
      tagname: "Unit1.LIC101.PV",
      notes: "run-specific note",
      cyclesSkip: 2,
      cyclesCount: 3,
      noiseProtectionSecs: 4,
      simDirection: "direct",
      simPvRangeHigh: 120,
      simPvRangeLow: 10,
      simMvRangeHigh: 90,
      simMvRangeLow: 20,
      simGain: 2.4,
      simTau: 8,
      simDeadTime: 1,
      simNoise: 0.5,
      simSeed: 23,
      simInitialPv: 70,
      simInitialMv: 45,
    };
    const request = requestFromForm(form);
    const restoredForm = formFromRequest(request);

    expect(restoredForm.notes).toBe("");
    expect(requestFromForm(restoredForm)).toEqual({
      ...request,
      notes: undefined,
    });
  });

  it("round-trips OPC custom tags and fixed values through request state", () => {
    const form = {
      ...createOpcForm(),
      notes: "run-specific note",
    };
    const request = requestFromForm(form);
    const restoredForm = formFromRequest(request);

    expect(request.tag_overrides).toMatchObject({
      process_variable: "FCS0217!204FC03010.PV",
      manipulated_variable: "FCS0217!204FC03010.OUT",
      upper_pv_range: "FCS0217!204FC03010.PVEUHI",
    });
    expect(restoredForm.tagSources).toEqual(form.tagSources);
    expect(restoredForm.valueSources).toEqual(form.valueSources);
    expect(restoredForm.tagOverrides).toEqual(form.tagOverrides);
    expect(restoredForm.valueTagOverrides).toEqual(form.valueTagOverrides);
    expect(restoredForm.notes).toBe("");
    expect(requestFromForm(restoredForm)).toEqual({
      ...request,
      notes: undefined,
    });
  });

  it("round-trips saved drafts while leaving run-specific notes blank", () => {
    const form = {
      ...createOpcForm(),
      notes: "do not copy this note to another run",
    };

    expect(formFromDraft(draftFromForm(form))).toEqual({
      ...form,
      notes: "",
    });
  });
});

describe("New Run split behavior", () => {
  it("resets custom mappings when the base tag changes and keeps fixed values", () => {
    const form = createOpcForm();
    expect(applyTagNameChange(form, form.tagname)).toBe(form);

    const changed = applyTagNameChange(form, "FCS0217!204FC03011.PV");
    expect(changed.tagname).toBe("FCS0217!204FC03011.PV");
    expect(changed.tagSources.processVariable).toBe("template");
    expect(changed.tagSources.manipulatedVariable).toBe("template");
    expect(changed.tagOverrides).toEqual(initialForm.tagOverrides);
    expect(changed.valueTagOverrides).toEqual(initialForm.valueTagOverrides);
    expect(changed.valueSources.direction).toBe("fixed");
    expect(changed.valueSources.pvRangeHigh).toBe("tag");
    expect(changed.valueSources.pvRangeLow).toBe("fixed");
    expect(changed.opcDirection).toBe("reverse");
    expect(changed.opcPvRangeLow).toBe(0);
    expect(changed.opcMvRangeLow).toBe(100);
  });

  it("hydrates null direction and ranges as tag sources", () => {
    const request = requestFromForm(createOpcForm());
    const overrides = { ...(request.tag_overrides ?? {}) };
    delete overrides.controller_direction;
    delete overrides.upper_pv_range;
    delete overrides.lower_pv_range;
    delete overrides.upper_mv_range;
    delete overrides.lower_mv_range;
    const restored = formFromRequest({
      ...request,
      direction: null,
      pv_range_high: null,
      pv_range_low: null,
      mv_range_high: null,
      mv_range_low: null,
      tag_overrides: overrides,
    });
    expect(restored.valueSources).toEqual({
      direction: "tag",
      pvRangeHigh: "tag",
      pvRangeLow: "tag",
      mvRangeHigh: "tag",
      mvRangeLow: "tag",
    });
  });

  it("applies template defaulting against queued state", () => {
    const source: PrefillSource = { kind: "duplicate", runId: 7 };
    expect(prefillMessage(source)).toBe("Loaded settings from tune #7.");

    const blank: NewRunPageState = {
      form: { ...initialForm, template: "" },
      hydrated: true,
      prefillSource: null,
      draftLoadError: null,
      preserveBlankTemplate: false,
      templateDefaultingResolved: false,
    };
    expect(
      resolveTemplateDefaulting(blank, "Honeywell Experion").form.template,
    ).toBe("Honeywell Experion");

    const queued: NewRunPageState = {
      ...blank,
      form: { ...blank.form, template: "Yokogawa CentumVP" },
    };
    expect(
      resolveTemplateDefaulting(queued, "Honeywell Experion").form.template,
    ).toBe("Yokogawa CentumVP");
  });
});
