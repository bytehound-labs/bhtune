import {
  EMPTY_TAG_OVERRIDES,
  EMPTY_VALUE_TAG_OVERRIDES,
  type TagOverrideKey,
  type ValueMappingKey,
} from "./mappingState";
import type { FormState } from "./newRunFormState";

const TAG_OVERRIDE_KEYS: readonly TagOverrideKey[] = [
  "processVariable",
  "manipulatedVariable",
  "setpointVariable",
  "controllerMode",
  "modeAttribute",
  "proportionalConstant",
  "integralConstant",
  "derivativeConstant",
];

const VALUE_MAPPING_KEYS: readonly ValueMappingKey[] = [
  "direction",
  "pvRangeHigh",
  "pvRangeLow",
  "mvRangeHigh",
  "mvRangeLow",
];

/**
 * Applies a new base tag and invalidates every custom mapping that could still point at the
 * previous loop. Fixed direction/range values are independent of the base tag and remain intact.
 */
export function applyTagNameChange(
  form: FormState,
  tagname: string,
): FormState {
  if (form.tagname === tagname) return form;

  const tagSources = { ...form.tagSources };
  for (const key of TAG_OVERRIDE_KEYS) {
    if (tagSources[key] === "custom") tagSources[key] = "template";
  }

  const valueSources = { ...form.valueSources };
  for (const key of VALUE_MAPPING_KEYS) {
    if (valueSources[key] === "custom") valueSources[key] = "tag";
  }

  return {
    ...form,
    tagname,
    tagSources,
    valueSources,
    tagOverrides: { ...EMPTY_TAG_OVERRIDES },
    valueTagOverrides: { ...EMPTY_VALUE_TAG_OVERRIDES },
  };
}
