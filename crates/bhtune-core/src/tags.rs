//! OPC tag-name derivation: expands a single PV tag into the full tag set a loop needs,
//! using the active DCS/PLC template's suffix conventions.

use serde::{Deserialize, Serialize};

use crate::{direction::ControllerDirection, template::DcsTemplate};

/// Per-tune replacements for template-derived OPC tag names.
///
/// A missing or blank field keeps the active template's derived tag. The first eight fields
/// replace template-derived tag names; the final five replace the tags used to read direction
/// and range values. Fixed direction and range values remain separate `LoopTags` inputs and are
/// applied after these read-tag overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct TagOverrides {
    #[serde(default)]
    pub process_variable: Option<String>,
    #[serde(default)]
    pub manipulated_variable: Option<String>,
    #[serde(default)]
    pub setpoint_variable: Option<String>,
    #[serde(default)]
    pub controller_mode: Option<String>,
    #[serde(default)]
    pub mode_attribute: Option<String>,
    #[serde(default)]
    pub proportional_constant: Option<String>,
    #[serde(default)]
    pub integral_constant: Option<String>,
    #[serde(default)]
    pub derivative_constant: Option<String>,
    #[serde(default)]
    pub controller_direction: Option<String>,
    #[serde(default)]
    pub upper_pv_range: Option<String>,
    #[serde(default)]
    pub lower_pv_range: Option<String>,
    #[serde(default)]
    pub upper_mv_range: Option<String>,
    #[serde(default)]
    pub lower_mv_range: Option<String>,
}

/// An invalid per-tune tag override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagOverridesError {
    /// A tag contains a control character that cannot be a valid OPC item identifier.
    ControlCharacter { field: &'static str },
}

impl std::fmt::Display for TagOverridesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ControlCharacter { field } => {
                write!(
                    formatter,
                    "tag override '{field}' contains a control character"
                )
            }
        }
    }
}

impl std::error::Error for TagOverridesError {}

impl TagOverrides {
    /// Returns whether every override is missing or blank.
    pub fn is_empty(&self) -> bool {
        [
            self.process_variable.as_deref(),
            self.manipulated_variable.as_deref(),
            self.setpoint_variable.as_deref(),
            self.controller_mode.as_deref(),
            self.mode_attribute.as_deref(),
            self.proportional_constant.as_deref(),
            self.integral_constant.as_deref(),
            self.derivative_constant.as_deref(),
            self.controller_direction.as_deref(),
            self.upper_pv_range.as_deref(),
            self.lower_pv_range.as_deref(),
            self.upper_mv_range.as_deref(),
            self.lower_mv_range.as_deref(),
        ]
        .into_iter()
        .all(|value| value.is_none_or(|value| value.trim().is_empty()))
    }

    /// Validates override strings before any driver connection or live loop mutation.
    pub fn validate(&self) -> Result<(), TagOverridesError> {
        let fields = [
            ("process_variable", self.process_variable.as_deref()),
            ("manipulated_variable", self.manipulated_variable.as_deref()),
            ("setpoint_variable", self.setpoint_variable.as_deref()),
            ("controller_mode", self.controller_mode.as_deref()),
            ("mode_attribute", self.mode_attribute.as_deref()),
            (
                "proportional_constant",
                self.proportional_constant.as_deref(),
            ),
            ("integral_constant", self.integral_constant.as_deref()),
            ("derivative_constant", self.derivative_constant.as_deref()),
            ("controller_direction", self.controller_direction.as_deref()),
            ("upper_pv_range", self.upper_pv_range.as_deref()),
            ("lower_pv_range", self.lower_pv_range.as_deref()),
            ("upper_mv_range", self.upper_mv_range.as_deref()),
            ("lower_mv_range", self.lower_mv_range.as_deref()),
        ];
        for (field, value) in fields {
            if value.is_some_and(|value| value.chars().any(char::is_control)) {
                return Err(TagOverridesError::ControlCharacter { field });
            }
        }
        Ok(())
    }

    /// Applies every non-blank override to an already-derived tag set.
    pub fn apply_to(&self, tags: &mut LoopTags) {
        apply_string_override(&mut tags.process_variable, self.process_variable.as_deref());
        apply_string_override(
            &mut tags.manipulated_variable,
            self.manipulated_variable.as_deref(),
        );
        apply_optional_string_override(
            &mut tags.setpoint_variable,
            self.setpoint_variable.as_deref(),
        );
        apply_optional_string_override(&mut tags.controller_mode, self.controller_mode.as_deref());
        apply_optional_string_override(&mut tags.mode_attribute, self.mode_attribute.as_deref());
        apply_optional_string_override(
            &mut tags.proportional_constant,
            self.proportional_constant.as_deref(),
        );
        apply_optional_string_override(
            &mut tags.integral_constant,
            self.integral_constant.as_deref(),
        );
        apply_optional_string_override(
            &mut tags.derivative_constant,
            self.derivative_constant.as_deref(),
        );
        apply_tag_override(
            &mut tags.controller_direction,
            self.controller_direction.as_deref(),
        );
        apply_tag_override(&mut tags.upper_pv_range, self.upper_pv_range.as_deref());
        apply_tag_override(&mut tags.lower_pv_range, self.lower_pv_range.as_deref());
        apply_tag_override(&mut tags.upper_mv_range, self.upper_mv_range.as_deref());
        apply_tag_override(&mut tags.lower_mv_range, self.lower_mv_range.as_deref());
    }
}

fn apply_string_override(target: &mut String, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        *target = value.trim().to_string();
    }
}

fn apply_optional_string_override(target: &mut Option<String>, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        *target = Some(value.trim().to_string());
    }
}

fn apply_tag_override<T>(target: &mut TagOrValue<T>, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        *target = TagOrValue::Tag(value.trim().to_string());
    }
}

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
/// falls back to the last `!`, then the last `/`. If no separator is present, the whole tag is
/// replaced. Returns `None` if `suffix` is blank — the convention for "this tag is not
/// applicable for the active DCS template" (e.g. Mode Attribute on a DCS with no such concept).
///
/// A `.` anywhere in `pv_tag` always wins over `!`, even if the `!` appears later in the
/// string. The same precedence is retained when `/` is present so existing dotted and
/// exclamation-separated tag mappings remain unchanged, while slash-only namespaces such as
/// `FCS0201/Control/PV` derive correctly.
pub fn derive_tag(pv_tag: &str, suffix: &str) -> Option<String> {
    if suffix.trim().is_empty() {
        return None;
    }

    let cut = pv_tag
        .rfind('.')
        .or_else(|| pv_tag.rfind('!'))
        .or_else(|| pv_tag.rfind('/'));
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
    fn falls_back_to_last_slash_when_no_dot_or_exclamation_present() {
        assert_eq!(
            derive_tag("FCS0201/Control/PV", "MV"),
            Some("FCS0201/Control/MV".to_string())
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

    #[test]
    fn tag_overrides_apply_non_blank_values_and_leave_blank_values_alone() {
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Honeywell Experion")
            .unwrap();
        let mut tags = LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &template);
        let overrides = TagOverrides {
            process_variable: Some("  Unit1.LIC101.PY  ".to_string()),
            manipulated_variable: Some("Unit1.LIC101.MY".to_string()),
            setpoint_variable: Some("".to_string()),
            controller_mode: None,
            mode_attribute: Some("Unit1.LIC101.MA".to_string()),
            proportional_constant: None,
            integral_constant: Some("Unit1.LIC101.IY".to_string()),
            derivative_constant: None,
            controller_direction: None,
            upper_pv_range: None,
            lower_pv_range: None,
            upper_mv_range: None,
            lower_mv_range: None,
        };

        overrides.validate().unwrap();
        overrides.apply_to(&mut tags);

        assert_eq!(tags.process_variable, "Unit1.LIC101.PY");
        assert_eq!(tags.manipulated_variable, "Unit1.LIC101.MY");
        assert_eq!(tags.setpoint_variable, Some("Unit1.LIC101.SP".to_string()));
        assert_eq!(tags.mode_attribute, Some("Unit1.LIC101.MA".to_string()));
        assert_eq!(tags.integral_constant, Some("Unit1.LIC101.IY".to_string()));
    }

    #[test]
    fn tag_overrides_apply_custom_value_read_tags() {
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Honeywell Experion")
            .unwrap();
        let mut tags = LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &template);
        let overrides = TagOverrides {
            controller_direction: Some("Unit1.LIC101.ACTION".to_string()),
            upper_pv_range: Some("Unit1.LIC101.PV_HIGH".to_string()),
            lower_pv_range: Some("Unit1.LIC101.PV_LOW".to_string()),
            upper_mv_range: Some("Unit1.LIC101.MV_HIGH".to_string()),
            lower_mv_range: Some("Unit1.LIC101.MV_LOW".to_string()),
            ..TagOverrides::default()
        };

        overrides.validate().unwrap();
        overrides.apply_to(&mut tags);

        assert_eq!(
            tags.controller_direction,
            TagOrValue::Tag("Unit1.LIC101.ACTION".to_string())
        );
        assert_eq!(
            tags.upper_pv_range,
            TagOrValue::Tag("Unit1.LIC101.PV_HIGH".to_string())
        );
        assert_eq!(
            tags.lower_pv_range,
            TagOrValue::Tag("Unit1.LIC101.PV_LOW".to_string())
        );
        assert_eq!(
            tags.upper_mv_range,
            TagOrValue::Tag("Unit1.LIC101.MV_HIGH".to_string())
        );
        assert_eq!(
            tags.lower_mv_range,
            TagOrValue::Tag("Unit1.LIC101.MV_LOW".to_string())
        );
    }

    #[test]
    fn tag_overrides_validate_control_characters() {
        let overrides = TagOverrides {
            process_variable: Some("Unit1.LIC101\nPY".to_string()),
            ..TagOverrides::default()
        };
        assert_eq!(
            overrides.validate(),
            Err(TagOverridesError::ControlCharacter {
                field: "process_variable"
            })
        );
        assert_eq!(
            overrides.validate().unwrap_err().to_string(),
            "tag override 'process_variable' contains a control character"
        );
    }

    #[test]
    fn empty_tag_overrides_round_trip_and_report_empty() {
        let overrides = TagOverrides::default();
        assert!(overrides.is_empty());
        let json = serde_json::to_string(&overrides).unwrap();
        let back: TagOverrides = serde_json::from_str(&json).unwrap();
        assert_eq!(overrides, back);
    }
}
