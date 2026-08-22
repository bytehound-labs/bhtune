import type { components } from "../../api/schema";

export type TuneDriver = components["schemas"]["TuneDriver"];
export type ControllerDirection = components["schemas"]["ControllerDirection"];
export type TagOverrideKey =
  | "processVariable"
  | "manipulatedVariable"
  | "setpointVariable"
  | "controllerMode"
  | "modeAttribute"
  | "proportionalConstant"
  | "integralConstant"
  | "derivativeConstant";

export type NumOrBlank = number | "";
export type TagMappingSource = "template" | "custom";
export type ValueMappingSource = "tag" | "custom" | "fixed";

export type TagOverrideFormState = Record<TagOverrideKey, string>;

export type TagMappingSources = Record<TagOverrideKey, TagMappingSource>;

export type ValueMappingKey =
  "direction" | "pvRangeHigh" | "pvRangeLow" | "mvRangeHigh" | "mvRangeLow";

export type ValueMappingSources = Record<ValueMappingKey, ValueMappingSource>;

/** Custom OPC read tags for direction and range values. */
export type ValueTagOverrideFormState = Record<ValueMappingKey, string>;

export const EMPTY_TAG_OVERRIDES: TagOverrideFormState = {
  processVariable: "",
  manipulatedVariable: "",
  setpointVariable: "",
  controllerMode: "",
  modeAttribute: "",
  proportionalConstant: "",
  integralConstant: "",
  derivativeConstant: "",
};

export const DEFAULT_TAG_MAPPING_SOURCES: TagMappingSources = {
  processVariable: "template",
  manipulatedVariable: "template",
  setpointVariable: "template",
  controllerMode: "template",
  modeAttribute: "template",
  proportionalConstant: "template",
  integralConstant: "template",
  derivativeConstant: "template",
};

export const DEFAULT_VALUE_MAPPING_SOURCES: ValueMappingSources = {
  direction: "tag",
  pvRangeHigh: "tag",
  pvRangeLow: "tag",
  mvRangeHigh: "tag",
  mvRangeLow: "tag",
};

export const EMPTY_VALUE_TAG_OVERRIDES: ValueTagOverrideFormState = {
  direction: "",
  pvRangeHigh: "",
  pvRangeLow: "",
  mvRangeHigh: "",
  mvRangeLow: "",
};
