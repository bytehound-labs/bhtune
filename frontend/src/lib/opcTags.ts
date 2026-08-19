import type { components } from "../api/schema";

type TemplateResponse = components["schemas"]["TemplateResponse"];

/**
 * A client-side mirror of `bhtune_core::tags::derive_tag` (Rust), used only to *preview* the
 * derived tag set in the OPC tag-tree browser (`ui-opc-browser`) before the user commits to
 * a selection -- `bhtune-core` itself is always the source of truth once a run actually
 * starts, this never replaces a server round trip.
 *
 * Replaces everything after the last `.` in `tag` with `suffix`; falls back to the last `!`
 * if no `.` is present; replaces the whole string if neither separator exists. Returns
 * `null` for a blank `suffix` -- the convention for "not applicable to this template" (e.g.
 * Mode Attribute on a DCS with no such concept).
 */
export function deriveTag(tag: string, suffix: string): string | null {
  if (suffix.trim() === "") return null;
  const dotIndex = tag.lastIndexOf(".");
  const bangIndex = tag.lastIndexOf("!");
  const cut = dotIndex >= 0 ? dotIndex : bangIndex;
  if (cut < 0) return suffix;
  return `${tag.slice(0, cut + 1)}${suffix}`;
}

export interface DerivedTagPreviewRow {
  label: string;
  tag: string | null;
}

/**
 * Builds the "what would this template derive from this tag?" preview shown when a node is
 * selected in the OPC tag-tree browser -- the clearest available explanation of how a
 * template's suffixes work, since it shows the *actual* tag names that would result from the
 * exact tag just picked, not an abstract description.
 *
 * `tag` need not already end in the template's PV suffix: `deriveTag` only looks at the
 * shared prefix up to the last `.`/`!`, so picking any tag under a loop's hierarchy (its PV,
 * its mode, even a branch node) yields the identical derived set -- matching
 * `bhtune_core::tags::LoopTags::derive_from_pv_tag`'s own behavior, which is why bhtune's
 * "Tag name" field stores a full suffixed tag (e.g. `"Unit1.LIC101.PV"`), not a bare loop
 * name. Mirrors that function's field order and suffix choices field-for-field, skipping
 * only the mode/direction *value* fields (`mode_manual_value`/`mode_auto_value`/
 * `controller_action_direct_value`) -- those are raw values compared against a tag's
 * contents, not name suffixes, so they never participate in tag derivation.
 */
export function derivedTagPreview(
  tag: string,
  template: TemplateResponse,
): DerivedTagPreviewRow[] {
  return [
    {
      label: "Process variable (PV)",
      tag: deriveTag(tag, template.process_variable_suffix),
    },
    {
      label: "Manipulated variable (MV)",
      tag: deriveTag(tag, template.manipulated_variable_suffix),
    },
    {
      label: "Setpoint",
      tag: deriveTag(tag, template.setpoint_variable_suffix),
    },
    {
      label: "Controller mode",
      tag: deriveTag(tag, template.controller_mode_suffix),
    },
    {
      label: "Mode attribute",
      tag: deriveTag(tag, template.mode_attribute_suffix),
    },
    {
      label: "Controller direction",
      tag: deriveTag(tag, template.controller_direction_suffix),
    },
    {
      label: "PV range high",
      tag: deriveTag(tag, template.upper_pv_range_suffix),
    },
    {
      label: "PV range low",
      tag: deriveTag(tag, template.lower_pv_range_suffix),
    },
    {
      label: "MV range high",
      tag: deriveTag(tag, template.upper_mv_range_suffix),
    },
    {
      label: "MV range low",
      tag: deriveTag(tag, template.lower_mv_range_suffix),
    },
    {
      label: "Proportional constant",
      tag: deriveTag(tag, template.proportional_constant_suffix),
    },
    {
      label: "Integral constant",
      tag: deriveTag(tag, template.integral_constant_suffix),
    },
    {
      label: "Derivative constant",
      tag: deriveTag(tag, template.derivative_constant_suffix),
    },
  ];
}
