//! OPC tag-name derivation: expands a single PV tag into the full tag set a loop needs,
//! using the active DCS/PLC template's suffix conventions.

use serde::{Deserialize, Serialize};

use crate::{direction::ControllerDirection, template::DcsTemplate};

/// A per-loop input that is either read live from an OPC tag, or supplied as a fixed value
/// set once by the user (e.g. a range limit that has no corresponding DCS tag).
///
/// Uses adjacent tagging (`{"kind": "tag", "data": "..."}`) rather than internal tagging:
/// serde cannot internally tag a newtype variant holding a bare string or number, and
/// adjacent tagging is also a cleaner shape for a generated TypeScript discriminated
/// union than the default externally-tagged representation would be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum TagOrValue<T> {
    Tag(String),
    Value(T),
}

/// Replaces everything after the last `.` in `pv_tag` with `suffix`. If no `.` is present,
/// falls back to the last `!`. If neither separator is present, the whole tag is replaced.
/// Returns `None` if `suffix` is blank — the convention for "this tag is not applicable
/// for the active DCS template" (e.g. Mode Attribute on a DCS with no such concept).
///
/// A `.` anywhere in `pv_tag` always wins over `!`, even if the `!` appears later in the
/// string. This deliberately mirrors how site tag hierarchies are named in practice: `.`
/// separates hierarchy levels in most DCS/PLC naming schemes, while `!` shows up only as
/// an occasional alternate separator deeper in a path — so a `.` present anywhere is the
/// stronger signal for where the hierarchy prefix actually ends.
pub fn derive_tag(pv_tag: &str, suffix: &str) -> Option<String> {
    if suffix.trim().is_empty() {
        return None;
    }

    let cut = pv_tag.rfind('.').or_else(|| pv_tag.rfind('!'));
    Some(match cut {
        Some(idx) => format!("{}{}", &pv_tag[..=idx], suffix),
        None => suffix.to_string(),
    })
}

/// The full set of OPC tags (and static-value overrides) a configured loop needs. Optional
/// tags are `None` when the active template's suffix convention disables that feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopTags {
    pub process_variable: String,
    pub manipulated_variable: String,
    pub setpoint_variable: Option<String>,
    pub controller_mode: Option<String>,
    pub mode_attribute: Option<String>,

    pub upper_pv_range: TagOrValue<f32>,
    pub lower_pv_range: TagOrValue<f32>,
    pub upper_mv_range: TagOrValue<f32>,
    pub lower_mv_range: TagOrValue<f32>,
    pub controller_direction: TagOrValue<ControllerDirection>,

    pub proportional_constant: Option<String>,
    pub integral_constant: Option<String>,
    pub derivative_constant: Option<String>,
}

impl LoopTags {
    /// Derives a full tag set from a single PV tag and the active DCS template's suffix
    /// conventions. Every input that can be tag-backed starts out as `TagOrValue::Tag`;
    /// callers wanting a static-value override should replace the relevant field
    /// afterward.
    pub fn derive_from_pv_tag(pv_tag: &str, template: &DcsTemplate) -> LoopTags {
        LoopTags {
            process_variable: derive_tag(pv_tag, &template.process_variable_suffix)
                .unwrap_or_default(),
            manipulated_variable: derive_tag(pv_tag, &template.manipulated_variable_suffix)
                .unwrap_or_default(),
            setpoint_variable: derive_tag(pv_tag, &template.setpoint_variable_suffix),
            controller_mode: derive_tag(pv_tag, &template.controller_mode_suffix),
            mode_attribute: derive_tag(pv_tag, &template.mode_attribute_suffix),
            upper_pv_range: TagOrValue::Tag(
                derive_tag(pv_tag, &template.upper_pv_range_suffix).unwrap_or_default(),
            ),
            lower_pv_range: TagOrValue::Tag(
                derive_tag(pv_tag, &template.lower_pv_range_suffix).unwrap_or_default(),
            ),
            upper_mv_range: TagOrValue::Tag(
                derive_tag(pv_tag, &template.upper_mv_range_suffix).unwrap_or_default(),
            ),
            lower_mv_range: TagOrValue::Tag(
                derive_tag(pv_tag, &template.lower_mv_range_suffix).unwrap_or_default(),
            ),
            controller_direction: TagOrValue::Tag(
                derive_tag(pv_tag, &template.controller_direction_suffix).unwrap_or_default(),
            ),
            proportional_constant: derive_tag(pv_tag, &template.proportional_constant_suffix),
            integral_constant: derive_tag(pv_tag, &template.integral_constant_suffix),
            derivative_constant: derive_tag(pv_tag, &template.derivative_constant_suffix),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::built_in_templates;

    #[test]
    fn replaces_after_last_dot() {
        assert_eq!(
            derive_tag("Unit1.LIC101.PV", "OP"),
            Some("Unit1.LIC101.OP".to_string())
        );
    }

    #[test]
    fn falls_back_to_last_exclamation_when_no_dot_present() {
        assert_eq!(
            derive_tag("Unit1!LIC101!PV", "OP"),
            Some("Unit1!LIC101!OP".to_string())
        );
    }

    #[test]
    fn dot_takes_priority_over_exclamation_even_if_earlier_in_the_string() {
        assert_eq!(derive_tag("A.B!C", "X"), Some("A.X".to_string()));
    }

    #[test]
    fn replaces_whole_tag_when_neither_separator_present() {
        assert_eq!(derive_tag("LIC101PV", "OP"), Some("OP".to_string()));
    }

    #[test]
    fn replaces_whole_tag_for_empty_pv_tag() {
        assert_eq!(derive_tag("", "OP"), Some("OP".to_string()));
    }

    #[test]
    fn blank_suffix_means_not_applicable() {
        assert_eq!(derive_tag("Unit1.LIC101.PV", ""), None);
        assert_eq!(derive_tag("Unit1.LIC101.PV", "   "), None);
    }

    #[test]
    fn derive_from_pv_tag_uses_template_suffixes() {
        let templates = built_in_templates();
        let honeywell = templates
            .iter()
            .find(|t| t.name == "Honeywell Experion")
            .unwrap();
        let tags = LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", honeywell);

        assert_eq!(tags.process_variable, "Unit1.LIC101.PV");
        assert_eq!(tags.manipulated_variable, "Unit1.LIC101.OP");
        assert_eq!(tags.setpoint_variable, Some("Unit1.LIC101.SP".to_string()));
        assert_eq!(tags.controller_mode, Some("Unit1.LIC101.MODE".to_string()));
        assert_eq!(
            tags.mode_attribute,
            Some("Unit1.LIC101.MODEATTR".to_string())
        );
        assert_eq!(
            tags.upper_pv_range,
            TagOrValue::Tag("Unit1.LIC101.PVEUHI".to_string())
        );
        assert_eq!(
            tags.controller_direction,
            TagOrValue::Tag("Unit1.LIC101.CTLACTN".to_string())
        );
    }

    #[test]
    fn derive_from_pv_tag_yields_none_for_blank_suffix_fields() {
        let templates = built_in_templates();
        let yokogawa = templates
            .iter()
            .find(|t| t.name == "Yokogawa CentumVP")
            .unwrap();
        let tags = LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", yokogawa);
        assert_eq!(tags.mode_attribute, None);
    }

    #[test]
    fn tag_or_value_serde_round_trip() {
        let tag: TagOrValue<f32> = TagOrValue::Tag("Unit1.PV".to_string());
        let json = serde_json::to_string(&tag).unwrap();
        let back: TagOrValue<f32> = serde_json::from_str(&json).unwrap();
        assert_eq!(tag, back);

        let value: TagOrValue<f32> = TagOrValue::Value(42.5);
        let json = serde_json::to_string(&value).unwrap();
        let back: TagOrValue<f32> = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn tag_or_value_uses_adjacently_tagged_wire_shape() {
        let tag: TagOrValue<f32> = TagOrValue::Tag("Unit1.PV".to_string());
        assert_eq!(
            serde_json::to_string(&tag).unwrap(),
            r#"{"kind":"tag","data":"Unit1.PV"}"#
        );

        let value: TagOrValue<f32> = TagOrValue::Value(42.5);
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            r#"{"kind":"value","data":42.5}"#
        );
    }

    #[test]
    fn loop_tags_serde_round_trip() {
        let templates = built_in_templates();
        let schneider = templates
            .iter()
            .find(|t| t.name == "Schneider Modicon")
            .unwrap();
        let tags = LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", schneider);
        let json = serde_json::to_string(&tags).unwrap();
        let back: LoopTags = serde_json::from_str(&json).unwrap();
        assert_eq!(tags, back);
    }
}
