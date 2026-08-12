//! DCS/PLC template semantics: one instance per control-system convention (Yokogawa,
//! Honeywell, etc.), describing how that DCS expresses PID parameters and the OPC
//! item-name suffix convention used to derive a full tag set from a single PV tag (see
//! [`crate::tags::derive_tag`]).

use serde::{Deserialize, Serialize};

use crate::pid_config::{DerivativeType, IntegralType, ProportionalType, TimeUnit};

/// One DCS/PLC vendor's conventions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DcsTemplate {
    pub name: String,

    /// If true, the controller mode is switched back to its original mode (e.g.
    /// Auto/Cascade) after a completed MRFT test. Has no effect if the loop was already in
    /// Manual at test start.
    pub revert_mode: bool,

    pub proportional_type: ProportionalType,
    pub integral_type: IntegralType,
    pub integral_unit: TimeUnit,
    pub derivative_type: DerivativeType,
    pub derivative_unit: TimeUnit,

    /// OPC item-name suffixes, combined with a PV tag's path prefix by
    /// [`crate::tags::derive_tag`] to fill in the rest of the tag set. An empty suffix
    /// means the corresponding tag is not applicable for this DCS (e.g. some DCS families
    /// have no mode-attribute concept).
    pub process_variable_suffix: String,
    pub manipulated_variable_suffix: String,
    pub setpoint_variable_suffix: String,
    pub controller_direction_suffix: String,
    pub controller_mode_suffix: String,
    pub mode_attribute_suffix: String,
    pub upper_pv_range_suffix: String,
    pub lower_pv_range_suffix: String,
    pub upper_mv_range_suffix: String,
    pub lower_mv_range_suffix: String,
    pub proportional_constant_suffix: String,
    pub integral_constant_suffix: String,
    pub derivative_constant_suffix: String,

    /// The DCS-specific raw values a Mode tag holds for Manual/Auto.
    pub mode_manual_value: String,
    pub mode_auto_value: String,
    /// The raw value a Mode Attribute tag holds when in "Program" mode (permits external
    /// OPC writes). `None` when the DCS has no mode-attribute concept.
    pub mode_attribute_program_value: Option<String>,
    /// The raw value a Controller Direction tag holds when the controller is Direct
    /// acting; see [`crate::direction::ControllerDirection::from_raw_tag_value`].
    pub controller_action_direct_value: String,
}

/// The DCS/PLC templates shipped by default.
pub fn built_in_templates() -> Vec<DcsTemplate> {
    vec![
        yokogawa_centum_vp(),
        honeywell_experion(),
        schneider_modicon(),
        allen_bradley_plantpax(),
    ]
}

fn yokogawa_centum_vp() -> DcsTemplate {
    DcsTemplate {
        name: "Yokogawa CentumVP".to_string(),
        revert_mode: true,
        proportional_type: ProportionalType::Band,
        integral_type: IntegralType::ResetTime,
        integral_unit: TimeUnit::Seconds,
        derivative_type: DerivativeType::DerivativeTime,
        derivative_unit: TimeUnit::Seconds,
        process_variable_suffix: "PV".to_string(),
        manipulated_variable_suffix: "MV".to_string(),
        setpoint_variable_suffix: "SV".to_string(),
        controller_direction_suffix: "DR".to_string(),
        controller_mode_suffix: "MODE".to_string(),
        mode_attribute_suffix: String::new(),
        upper_pv_range_suffix: "SH".to_string(),
        lower_pv_range_suffix: "SL".to_string(),
        upper_mv_range_suffix: "MSH".to_string(),
        lower_mv_range_suffix: "MSL".to_string(),
        proportional_constant_suffix: "P".to_string(),
        integral_constant_suffix: "I".to_string(),
        derivative_constant_suffix: "D".to_string(),
        mode_manual_value: "MAN".to_string(),
        mode_auto_value: "AUT".to_string(),
        mode_attribute_program_value: None,
        controller_action_direct_value: "0".to_string(),
    }
}

fn honeywell_experion() -> DcsTemplate {
    DcsTemplate {
        name: "Honeywell Experion".to_string(),
        revert_mode: true,
        proportional_type: ProportionalType::Gain,
        integral_type: IntegralType::ResetTime,
        integral_unit: TimeUnit::Minutes,
        derivative_type: DerivativeType::DerivativeTime,
        derivative_unit: TimeUnit::Minutes,
        process_variable_suffix: "PV".to_string(),
        manipulated_variable_suffix: "OP".to_string(),
        setpoint_variable_suffix: "SP".to_string(),
        controller_direction_suffix: "CTLACTN".to_string(),
        controller_mode_suffix: "MODE".to_string(),
        mode_attribute_suffix: "MODEATTR".to_string(),
        upper_pv_range_suffix: "PVEUHI".to_string(),
        lower_pv_range_suffix: "PVEULO".to_string(),
        upper_mv_range_suffix: "CVEUHI".to_string(),
        lower_mv_range_suffix: "CVEULO".to_string(),
        proportional_constant_suffix: "K".to_string(),
        integral_constant_suffix: "T1".to_string(),
        derivative_constant_suffix: "T2".to_string(),
        mode_manual_value: "0".to_string(),
        mode_auto_value: "1".to_string(),
        mode_attribute_program_value: Some("2".to_string()),
        controller_action_direct_value: "0".to_string(),
    }
}

fn schneider_modicon() -> DcsTemplate {
    DcsTemplate {
        name: "Schneider Modicon".to_string(),
        revert_mode: true,
        proportional_type: ProportionalType::Gain,
        integral_type: IntegralType::ResetTime,
        integral_unit: TimeUnit::Seconds,
        derivative_type: DerivativeType::DerivativeTime,
        derivative_unit: TimeUnit::Seconds,
        process_variable_suffix: "PV".to_string(),
        manipulated_variable_suffix: "OUT".to_string(),
        setpoint_variable_suffix: "SP".to_string(),
        controller_direction_suffix: "DR".to_string(),
        controller_mode_suffix: "MAN_AUT".to_string(),
        mode_attribute_suffix: String::new(),
        upper_pv_range_suffix: "PV_SUP".to_string(),
        lower_pv_range_suffix: "PV_INF".to_string(),
        upper_mv_range_suffix: "OUT_SUP".to_string(),
        lower_mv_range_suffix: "OUT_INF".to_string(),
        proportional_constant_suffix: "KP".to_string(),
        integral_constant_suffix: "TI".to_string(),
        derivative_constant_suffix: "TD".to_string(),
        mode_manual_value: "false".to_string(),
        mode_auto_value: "true".to_string(),
        mode_attribute_program_value: None,
        controller_action_direct_value: "0".to_string(),
    }
}

fn allen_bradley_plantpax() -> DcsTemplate {
    DcsTemplate {
        name: "Allen-Bradley PlantPAx".to_string(),
        revert_mode: true,
        proportional_type: ProportionalType::Gain,
        integral_type: IntegralType::ResetTime,
        integral_unit: TimeUnit::Minutes,
        derivative_type: DerivativeType::DerivativeTime,
        derivative_unit: TimeUnit::Minutes,
        process_variable_suffix: "Inp_PV".to_string(),
        manipulated_variable_suffix: "PSet_SP".to_string(),
        setpoint_variable_suffix: "PSet_CV".to_string(),
        controller_direction_suffix: "Cfg_CtrlAction".to_string(),
        controller_mode_suffix: "Inp_OvrdCmd".to_string(),
        mode_attribute_suffix: String::new(),
        upper_pv_range_suffix: "Val_PVEUMax".to_string(),
        lower_pv_range_suffix: "Val_PVEUMin".to_string(),
        upper_mv_range_suffix: "Val_CVEUMax".to_string(),
        lower_mv_range_suffix: "Val_CVEUMin".to_string(),
        proportional_constant_suffix: "Cfg_PGain".to_string(),
        integral_constant_suffix: "Cfg_IGain".to_string(),
        derivative_constant_suffix: "Cfg_DGain".to_string(),
        mode_manual_value: "1".to_string(),
        mode_auto_value: "2".to_string(),
        mode_attribute_program_value: None,
        controller_action_direct_value: "0".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ships_exactly_four_templates() {
        let templates = built_in_templates();
        assert_eq!(templates.len(), 4);
        let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Yokogawa CentumVP",
                "Honeywell Experion",
                "Schneider Modicon",
                "Allen-Bradley PlantPAx"
            ]
        );
    }

    #[test]
    fn yokogawa_has_no_mode_attribute_concept() {
        let templates = built_in_templates();
        let yokogawa = templates
            .iter()
            .find(|t| t.name == "Yokogawa CentumVP")
            .unwrap();
        assert!(yokogawa.mode_attribute_suffix.is_empty());
        assert_eq!(yokogawa.mode_attribute_program_value, None);
    }

    #[test]
    fn honeywell_has_a_mode_attribute_concept() {
        let templates = built_in_templates();
        let honeywell = templates
            .iter()
            .find(|t| t.name == "Honeywell Experion")
            .unwrap();
        assert_eq!(honeywell.mode_attribute_suffix, "MODEATTR");
        assert_eq!(
            honeywell.mode_attribute_program_value,
            Some("2".to_string())
        );
    }

    #[test]
    fn yokogawa_uses_proportional_band_others_use_gain() {
        let templates = built_in_templates();
        for template in &templates {
            let expected = if template.name == "Yokogawa CentumVP" {
                ProportionalType::Band
            } else {
                ProportionalType::Gain
            };
            assert_eq!(template.proportional_type, expected, "{}", template.name);
        }
    }

    #[test]
    fn all_templates_use_reset_time_and_derivative_time() {
        for template in built_in_templates() {
            assert_eq!(template.integral_type, IntegralType::ResetTime);
            assert_eq!(template.derivative_type, DerivativeType::DerivativeTime);
        }
    }

    #[test]
    fn serde_round_trip() {
        for template in built_in_templates() {
            let json = serde_json::to_string(&template).unwrap();
            let back: DcsTemplate = serde_json::from_str(&json).unwrap();
            assert_eq!(template, back);
        }
    }
}
