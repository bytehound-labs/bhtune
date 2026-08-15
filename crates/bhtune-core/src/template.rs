//! DCS/PLC template semantics: one instance per control-system convention (Yokogawa,
//! Honeywell, etc.), describing how that DCS expresses PID parameters and the OPC
//! item-name suffix convention used to derive a full tag set from a single PV tag (see
//! [`crate::tags::derive_tag`]).
//!
//! The built-in templates are not hardcoded Rust -- they are parsed from an embedded TOML
//! catalog (`templates/builtin.toml`), so adding support for a new DCS/PLC family is a data
//! file change, not a Rust change. See AGENTS.md's "Community DCS/PLC template catalog"
//! section for the full design and contribution rationale.

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

    /// DCS/PLC releases this mapping is known to apply to (e.g. `["R5", "R6"]`), in each
    /// vendor's own version-naming convention rather than a normalized scheme. A newer
    /// release that changes tag conventions gets its *own* template entry with its own
    /// `name` and `versions` list -- never an edit to this one in place, since sites still
    /// running the older release depend on the existing mapping (see
    /// `docs/dcs-templates.md`). May be empty for a contribution whose version coverage
    /// isn't yet known; `name` is what makes a template unique, not `versions`.
    #[serde(default)]
    pub versions: Vec<String>,
    /// Free-text description of the control system this template targets.
    #[serde(default)]
    pub description: Option<String>,
    /// Citation for where this mapping came from (a manual, a field deployment).
    /// Provenance, not a correctness guarantee -- there is deliberately no separate
    /// "verified" trust field; everything accepted into the catalog is treated as verified,
    /// and real mapping errors are fixed as bugs when they surface.
    #[serde(default)]
    pub source: Option<String>,
}

impl DcsTemplate {
    /// Validates cross-field invariants a data file can't express on its own: a name, a PV
    /// suffix, and an MV suffix are always required (without them tag derivation is
    /// impossible); a mode suffix requires both a manual and an auto value; a
    /// mode-attribute suffix requires its program value. Mirrors `LoopConfig::validate`'s
    /// rationale (see AGENTS.md's "Live-plant safety hardening") -- a half-configured
    /// template should fail loudly at parse/import time, not mid-tune against a live loop.
    /// Called on every template parsed from the embedded catalog
    /// ([`parse_catalog`]), an imported file, or the user catalog.
    pub fn validate(&self) -> Result<(), TemplateError> {
        if self.name.trim().is_empty() {
            return Err(TemplateError::EmptyName);
        }
        if self.process_variable_suffix.is_empty() {
            return Err(TemplateError::EmptyField {
                name: self.name.clone(),
                field: "process_variable_suffix",
            });
        }
        if self.manipulated_variable_suffix.is_empty() {
            return Err(TemplateError::EmptyField {
                name: self.name.clone(),
                field: "manipulated_variable_suffix",
            });
        }
        if !self.controller_mode_suffix.is_empty() {
            if self.mode_manual_value.is_empty() {
                return Err(TemplateError::MissingModeValue {
                    name: self.name.clone(),
                    field: "mode_manual_value",
                });
            }
            if self.mode_auto_value.is_empty() {
                return Err(TemplateError::MissingModeValue {
                    name: self.name.clone(),
                    field: "mode_auto_value",
                });
            }
        }
        if !self.mode_attribute_suffix.is_empty() && self.mode_attribute_program_value.is_none() {
            return Err(TemplateError::MissingModeAttributeProgramValue {
                name: self.name.clone(),
            });
        }
        Ok(())
    }
}

/// Why [`DcsTemplate::validate`] or [`parse_catalog`] rejected a template.
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateError {
    /// The catalog's TOML could not be parsed at all (malformed syntax, wrong shape).
    Toml(toml::de::Error),
    /// `name` was empty or all whitespace.
    EmptyName,
    /// A required suffix field was empty.
    EmptyField { name: String, field: &'static str },
    /// `controller_mode_suffix` was set but the manual or auto value for it was empty.
    MissingModeValue { name: String, field: &'static str },
    /// `mode_attribute_suffix` was set but `mode_attribute_program_value` was `None`.
    MissingModeAttributeProgramValue { name: String },
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::Toml(e) => write!(f, "invalid template catalog: {e}"),
            TemplateError::EmptyName => write!(f, "template name must not be empty"),
            TemplateError::EmptyField { name, field } => {
                write!(f, "template '{name}': {field} must not be empty")
            }
            TemplateError::MissingModeValue { name, field } => write!(
                f,
                "template '{name}': controller_mode_suffix is set but {field} is empty"
            ),
            TemplateError::MissingModeAttributeProgramValue { name } => write!(
                f,
                "template '{name}': mode_attribute_suffix is set but \
                 mode_attribute_program_value is missing"
            ),
        }
    }
}

impl std::error::Error for TemplateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TemplateError::Toml(e) => Some(e),
            _ => None,
        }
    }
}

impl From<toml::de::Error> for TemplateError {
    fn from(error: toml::de::Error) -> Self {
        TemplateError::Toml(error)
    }
}

/// The embedded/user catalog's top-level shape: a TOML `[[template]]` array of tables. Also
/// used in reverse by [`to_catalog_toml`] (bhtune-cli's `template export --format toml`), so
/// export and import always agree on the exact same wire shape with no separate format to
/// keep in sync by hand.
#[derive(Debug, Serialize, Deserialize)]
struct Catalog {
    #[serde(rename = "template")]
    templates: Vec<DcsTemplate>,
}

/// Parses a TOML catalog (the `[[template]]` array-of-tables format used by
/// `templates/builtin.toml` and the user catalog file bhtune-cli auto-loads) and validates
/// every template it contains. Pure -- takes an in-memory string and does no I/O itself;
/// all file reading is the caller's job (bhtune-cli's `template-user-catalog`/
/// `template-cli`), keeping this crate's "no I/O" rule intact.
pub fn parse_catalog(input: &str) -> Result<Vec<DcsTemplate>, TemplateError> {
    let catalog: Catalog = toml::from_str(input)?;
    for template in &catalog.templates {
        template.validate()?;
    }
    Ok(catalog.templates)
}

/// Serializes `templates` as a TOML catalog in the exact `[[template]]` array-of-tables
/// shape [`parse_catalog`] reads back -- the inverse operation. Used by bhtune-cli's
/// `template export --format toml` (a single template exports as a one-entry catalog) so
/// the contribution loop is export -> annotate -> PR with no hand-transcription step. Pure,
/// like [`parse_catalog`]: writing the result to a file is the caller's job.
pub fn to_catalog_toml(templates: Vec<DcsTemplate>) -> Result<String, toml::ser::Error> {
    toml::to_string_pretty(&Catalog { templates })
}

/// The embedded catalog TOML, compiled into the binary so it can never go missing from a
/// shipped install -- see `templates/builtin.toml` for the actual data and its contribution
/// rules.
const BUILTIN_CATALOG: &str = include_str!("../templates/builtin.toml");

/// The DCS/PLC templates shipped by default, parsed from the embedded catalog.
///
/// # Panics
///
/// Panics if the embedded catalog fails to parse or validate. This can only happen from a
/// bad edit to `templates/builtin.toml` itself; this module's
/// `embedded_catalog_parses_and_validates` test proves it never does in practice, so a
/// malformed contribution fails CI rather than shipping.
pub fn built_in_templates() -> Vec<DcsTemplate> {
    parse_catalog(BUILTIN_CATALOG).expect("embedded builtin.toml catalog must parse and validate")
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

    #[test]
    fn embedded_catalog_parses_and_validates() {
        // `built_in_templates()` already calls `parse_catalog(...).expect(...)`
        // internally, so simply calling it without panicking proves this -- but assert on
        // the result explicitly too, so a future refactor that swallows the panic still
        // gets caught here.
        let templates = parse_catalog(BUILTIN_CATALOG).unwrap();
        for template in &templates {
            assert!(template.validate().is_ok(), "{}", template.name);
        }
    }

    #[test]
    fn built_in_templates_carry_their_researched_versions() {
        let templates = built_in_templates();
        let versions = |name: &str| -> Vec<String> {
            templates
                .iter()
                .find(|t| t.name == name)
                .unwrap()
                .versions
                .clone()
        };
        assert_eq!(versions("Yokogawa CentumVP"), vec!["R5", "R6"]);
        assert_eq!(versions("Honeywell Experion"), vec!["R400", "R410", "R430"]);
        assert_eq!(
            versions("Schneider Modicon"),
            vec!["Unity Pro V8.0", "Unity Pro V8.1", "Unity Pro V11.0"]
        );
        assert_eq!(
            versions("Allen-Bradley PlantPAx"),
            vec!["3.0", "3.5", "4.0"]
        );
    }

    #[test]
    fn built_in_templates_have_a_description_and_source() {
        for template in built_in_templates() {
            assert!(template.description.is_some(), "{}", template.name);
            assert!(template.source.is_some(), "{}", template.name);
        }
    }

    /// A minimal single-template TOML document that passes `validate()` as-is, used as a
    /// base for the `parse_catalog` rejection tests below via targeted `str::replace`
    /// edits. `controller_mode_suffix`/`mode_manual_value`/`mode_auto_value` are left blank
    /// (no mode concept) and `mode_attribute_suffix` is blank too, so none of
    /// `validate()`'s conditional checks fire unless a test deliberately arms them.
    fn minimal_valid_toml() -> &'static str {
        r#"
[[template]]
name = "Test DCS"
revert_mode = false
proportional_type = "gain"
integral_type = "reset_time"
integral_unit = "seconds"
derivative_type = "derivative_time"
derivative_unit = "seconds"
process_variable_suffix = "PV"
manipulated_variable_suffix = "MV"
setpoint_variable_suffix = "SV"
controller_direction_suffix = "DR"
controller_mode_suffix = ""
mode_attribute_suffix = ""
upper_pv_range_suffix = "SH"
lower_pv_range_suffix = "SL"
upper_mv_range_suffix = "MSH"
lower_mv_range_suffix = "MSL"
proportional_constant_suffix = "P"
integral_constant_suffix = "I"
derivative_constant_suffix = "D"
mode_manual_value = ""
mode_auto_value = ""
controller_action_direct_value = "0"
"#
    }

    #[test]
    fn parse_catalog_accepts_a_minimal_valid_template() {
        let templates = parse_catalog(minimal_valid_toml()).unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "Test DCS");
        assert!(templates[0].versions.is_empty());
        assert_eq!(templates[0].description, None);
        assert_eq!(templates[0].source, None);
    }

    #[test]
    fn parse_catalog_rejects_malformed_toml() {
        let err = parse_catalog("this is not [[ valid toml").unwrap_err();
        assert!(matches!(err, TemplateError::Toml(_)));
    }

    #[test]
    fn parse_catalog_rejects_an_empty_pv_suffix() {
        let toml = minimal_valid_toml().replace(
            r#"process_variable_suffix = "PV""#,
            r#"process_variable_suffix = """#,
        );
        let err = parse_catalog(&toml).unwrap_err();
        assert!(matches!(
            err,
            TemplateError::EmptyField {
                field: "process_variable_suffix",
                ..
            }
        ));
    }

    #[test]
    fn parse_catalog_rejects_an_empty_mv_suffix() {
        let toml = minimal_valid_toml().replace(
            r#"manipulated_variable_suffix = "MV""#,
            r#"manipulated_variable_suffix = """#,
        );
        let err = parse_catalog(&toml).unwrap_err();
        assert!(matches!(
            err,
            TemplateError::EmptyField {
                field: "manipulated_variable_suffix",
                ..
            }
        ));
    }

    #[test]
    fn parse_catalog_rejects_a_mode_suffix_without_manual_value() {
        let toml = minimal_valid_toml().replace(
            r#"controller_mode_suffix = """#,
            r#"controller_mode_suffix = "MODE""#,
        );
        // mode_manual_value/mode_auto_value are still "" in this variant.
        let err = parse_catalog(&toml).unwrap_err();
        assert!(matches!(
            err,
            TemplateError::MissingModeValue {
                field: "mode_manual_value",
                ..
            }
        ));
    }

    #[test]
    fn parse_catalog_rejects_a_mode_suffix_without_auto_value() {
        let toml = minimal_valid_toml()
            .replace(
                r#"controller_mode_suffix = """#,
                r#"controller_mode_suffix = "MODE""#,
            )
            .replace(r#"mode_manual_value = """#, r#"mode_manual_value = "MAN""#);
        // mode_auto_value stays "" in this variant.
        let err = parse_catalog(&toml).unwrap_err();
        assert!(matches!(
            err,
            TemplateError::MissingModeValue {
                field: "mode_auto_value",
                ..
            }
        ));
    }

    #[test]
    fn parse_catalog_rejects_a_mode_attribute_suffix_without_program_value() {
        let toml = minimal_valid_toml().replace(
            r#"mode_attribute_suffix = """#,
            r#"mode_attribute_suffix = "MODEATTR""#,
        );
        let err = parse_catalog(&toml).unwrap_err();
        assert!(matches!(
            err,
            TemplateError::MissingModeAttributeProgramValue { .. }
        ));
    }

    #[test]
    fn to_catalog_toml_round_trips_the_built_in_templates() {
        let original = built_in_templates();
        let toml = to_catalog_toml(original.clone()).unwrap();
        let parsed = parse_catalog(&toml).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn to_catalog_toml_with_one_template_produces_a_single_template_block() {
        let template = parse_catalog(minimal_valid_toml()).unwrap().remove(0);
        let toml = to_catalog_toml(vec![template.clone()]).unwrap();
        assert_eq!(toml.matches("[[template]]").count(), 1);
        let parsed = parse_catalog(&toml).unwrap();
        assert_eq!(parsed, vec![template]);
    }

    #[test]
    fn to_catalog_toml_with_no_templates_produces_an_empty_catalog() {
        let toml = to_catalog_toml(vec![]).unwrap();
        assert_eq!(parse_catalog(&toml).unwrap(), Vec::new());
    }

    #[test]
    fn validate_rejects_an_empty_name() {
        let mut template = built_in_templates().remove(0);
        template.name = "   ".to_string();
        assert_eq!(template.validate(), Err(TemplateError::EmptyName));
    }

    #[test]
    fn validate_accepts_every_built_in_template() {
        for template in built_in_templates() {
            assert!(template.validate().is_ok(), "{}", template.name);
        }
    }

    #[test]
    fn template_error_is_a_std_error() {
        let err = TemplateError::EmptyName;
        let _: Box<dyn std::error::Error> = Box::new(err);
    }

    #[test]
    fn template_error_display_names_the_template_and_field() {
        let err = TemplateError::EmptyField {
            name: "My DCS".to_string(),
            field: "process_variable_suffix",
        };
        let msg = err.to_string();
        assert!(msg.contains("My DCS"));
        assert!(msg.contains("process_variable_suffix"));
    }

    #[test]
    fn template_error_toml_variant_has_a_source() {
        use std::error::Error as _;
        let err = parse_catalog("this is not [[ valid toml").unwrap_err();
        assert!(err.source().is_some());
    }

    #[test]
    fn template_error_display_covers_every_remaining_variant() {
        let toml_err = parse_catalog("this is not [[ valid toml").unwrap_err();
        assert!(toml_err.to_string().contains("invalid template catalog"));

        assert_eq!(
            TemplateError::EmptyName.to_string(),
            "template name must not be empty"
        );

        let mode_value_err = TemplateError::MissingModeValue {
            name: "My DCS".to_string(),
            field: "mode_manual_value",
        };
        let msg = mode_value_err.to_string();
        assert!(msg.contains("My DCS"));
        assert!(msg.contains("mode_manual_value"));

        let mode_attr_err = TemplateError::MissingModeAttributeProgramValue {
            name: "My DCS".to_string(),
        };
        assert!(mode_attr_err.to_string().contains("My DCS"));
    }

    #[test]
    fn template_error_non_toml_variants_have_no_source() {
        use std::error::Error as _;
        assert!(TemplateError::EmptyName.source().is_none());
    }
}
