//! Enums for how a DCS/PLC expresses PID parameters, avoiding the fragile pattern of
//! comparing live values against magic display strings (`"Kp - Proportional Gain"`, etc.).

use serde::{Deserialize, Serialize};

/// How a DCS expresses the proportional term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProportionalType {
    /// Kp: dimensionless gain.
    Gain,
    /// PB: proportional band, as a percentage. `PB = 100 / Kp`.
    Band,
}

/// How a DCS expresses the integral term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum IntegralType {
    /// Ti: reset time.
    ResetTime,
    /// Ri: reset rate, `Ri = 1 / Ti`.
    ResetRate,
    /// Ki: reset gain, `Ki = Kp / Ti`.
    ResetGain,
}

/// How a DCS expresses the derivative term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DerivativeType {
    /// Td: derivative time.
    DerivativeTime,
    /// Kd: derivative gain, `Kd = Kp * Td`.
    DerivativeGain,
}

/// The time unit a DCS expects for integral/derivative parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TimeUnit {
    Seconds,
    Minutes,
}

/// Precision shared by the active P/I/D terms in a template's final controller units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PidRoundingKind {
    DecimalPlaces,
    SignificantDigits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PidRounding {
    pub kind: PidRoundingKind,
    #[cfg_attr(feature = "utoipa", schema(minimum = 0, maximum = 7))]
    #[cfg_attr(feature = "schemars", schemars(range(min = 0, max = 7)))]
    pub digits: u8,
}

impl Default for PidRounding {
    fn default() -> Self {
        Self {
            kind: PidRoundingKind::SignificantDigits,
            digits: 3,
        }
    }
}

/// A controller value and its canonical text; parsing `display` as `f32` yields `value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct RoundedPidValue {
    pub value: f32,
    pub display: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidRoundingError {
    InvalidDigits { kind: PidRoundingKind, digits: u8 },
    NonFinite,
    NonPositive,
    RoundedToZero,
}

impl std::fmt::Display for PidRoundingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDigits { kind, digits } => {
                let bounds = match kind {
                    PidRoundingKind::DecimalPlaces => "0-7 decimal places",
                    PidRoundingKind::SignificantDigits => "1-7 significant digits",
                };
                write!(f, "PID rounding digits {digits} must be {bounds}")
            }
            Self::NonFinite => f.write_str("PID value must remain finite after rounding"),
            Self::NonPositive => f.write_str("an active PID term must be positive"),
            Self::RoundedToZero => {
                f.write_str("template PID rounding would erase an active term to zero; increase the template precision")
            }
        }
    }
}

impl std::error::Error for PidRoundingError {}

impl PidRounding {
    pub fn validate(self) -> Result<(), PidRoundingError> {
        if self.digits > 7 || (self.kind == PidRoundingKind::SignificantDigits && self.digits == 0)
        {
            return Err(PidRoundingError::InvalidDigits {
                kind: self.kind,
                digits: self.digits,
            });
        }
        Ok(())
    }

    /// Rounds the stored `f32` to nearest, with exact halfway cases away from zero.
    /// Widening before scaling avoids overflow and false halfway cases from `f32`
    /// multiplication. Calculations themselves are not changed.
    pub fn round_value(self, value: f32) -> Result<RoundedPidValue, PidRoundingError> {
        self.validate()?;
        if !value.is_finite() {
            return Err(PidRoundingError::NonFinite);
        }
        let wide = f64::from(value);
        let places = match self.kind {
            PidRoundingKind::DecimalPlaces => i32::from(self.digits),
            PidRoundingKind::SignificantDigits if value != 0.0 => {
                i32::from(self.digits) - 1 - wide.abs().log10().floor() as i32
            }
            PidRoundingKind::SignificantDigits => 0,
        };
        let scale = 10_f64.powi(places);
        let rounded = (wide * scale).round() / scale;
        let rounded = if rounded == 0.0 { 0.0 } else { rounded };
        let target = rounded as f32;
        if !target.is_finite() {
            return Err(PidRoundingError::NonFinite);
        }
        let display = match self.kind {
            PidRoundingKind::DecimalPlaces => {
                format!("{rounded:.digits$}", digits = usize::from(self.digits))
            }
            PidRoundingKind::SignificantDigits if rounded != 0.0 => {
                let exponent = rounded.abs().log10().floor() as i32;
                let precision = usize::from(self.digits - 1);
                if exponent >= i32::from(self.digits) || exponent < -4 {
                    format!("{rounded:.precision$e}")
                } else {
                    let places = (i32::from(self.digits) - 1 - exponent) as usize;
                    format!("{rounded:.places$}")
                }
            }
            PidRoundingKind::SignificantDigits => "0".to_string(),
        };
        Ok(RoundedPidValue {
            value: target,
            display,
        })
    }

    pub(crate) fn disabled_value(self, value: f32) -> RoundedPidValue {
        let display = match self.kind {
            PidRoundingKind::DecimalPlaces => {
                format!("{value:.digits$}", digits = usize::from(self.digits))
            }
            PidRoundingKind::SignificantDigits => value.to_string(),
        };
        RoundedPidValue { value, display }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rounding(kind: PidRoundingKind, digits: u8) -> PidRounding {
        PidRounding { kind, digits }
    }

    #[test]
    fn rounding_defaults_to_three_significant_digits_and_round_trips() {
        let policy = PidRounding::default();
        assert_eq!(policy, rounding(PidRoundingKind::SignificantDigits, 3));
        for kind in [
            PidRoundingKind::DecimalPlaces,
            PidRoundingKind::SignificantDigits,
        ] {
            let policy = rounding(kind, 3);
            let json = serde_json::to_string(&policy).unwrap();
            assert_eq!(serde_json::from_str::<PidRounding>(&json).unwrap(), policy);
            assert!(json.contains(match kind {
                PidRoundingKind::DecimalPlaces => "decimal_places",
                PidRoundingKind::SignificantDigits => "significant_digits",
            }));
        }
    }

    #[test]
    fn rounding_validates_precision_bounds() {
        for digits in 0..=8 {
            for kind in [
                PidRoundingKind::DecimalPlaces,
                PidRoundingKind::SignificantDigits,
            ] {
                let policy = rounding(kind, digits);
                let valid = digits <= 7 && (kind == PidRoundingKind::DecimalPlaces || digits > 0);
                assert_eq!(policy.validate().is_ok(), valid);
                if !valid {
                    let error = policy.round_value(1.0).unwrap_err();
                    assert!(error.to_string().contains("digits"));
                    assert!(std::error::Error::source(&error).is_none());
                }
            }
        }
    }

    #[test]
    fn decimal_rounding_matches_yokogawa_results_and_keeps_one_decimal() {
        let policy = rounding(PidRoundingKind::DecimalPlaces, 1);
        for (raw, target, display) in [
            (155.21378, 155.2, "155.2"),
            (231.79277, 231.8, "231.8"),
            (309.74078, 309.7, "309.7"),
            (2.482169, 2.5, "2.5"),
            (0.0, 0.0, "0.0"),
            (-0.0, 0.0, "0.0"),
            (1.25, 1.3, "1.3"),
            (-1.25, -1.3, "-1.3"),
        ] {
            let rounded = policy.round_value(raw).unwrap();
            assert_eq!(rounded.value, target);
            assert_eq!(rounded.display, display);
            assert_eq!(rounded.display.parse::<f32>().unwrap(), rounded.value);
        }
    }

    #[test]
    fn rounding_respects_the_stored_f32_on_either_side_of_a_halfway_case() {
        let policy = rounding(PidRoundingKind::DecimalPlaces, 1);
        for (raw, expected) in [
            (f32::from_bits(1.25_f32.to_bits() - 1), 1.2),
            (1.25, 1.3),
            (f32::from_bits(1.25_f32.to_bits() + 1), 1.3),
            (1.05_f32, 1.0),
            (f32::from_bits(1.05_f32.to_bits() + 1), 1.1),
        ] {
            assert_eq!(policy.round_value(raw).unwrap().value, expected);
        }
    }

    #[test]
    fn significant_rounding_preserves_small_values_and_handles_carry() {
        let policy = PidRounding::default();
        for (raw, target, display) in [
            (0.004873, 0.00487, "0.00487"),
            (2.482169, 2.48, "2.48"),
            (155.21378, 155.0, "155"),
            (9.999, 10.0, "10.0"),
            (999.9, 1000.0, "1.00e3"),
            (0.0000004873, 0.000000487, "4.87e-7"),
            (0.0, 0.0, "0"),
            (-0.0, 0.0, "0"),
        ] {
            let rounded = policy.round_value(raw).unwrap();
            assert_eq!(rounded.value, target);
            assert_eq!(rounded.display, display);
            assert_eq!(rounded.display.parse::<f32>().unwrap(), rounded.value);
        }
    }

    #[test]
    fn rounding_handles_extreme_finite_values_without_scaling_overflow() {
        for policy in [
            rounding(PidRoundingKind::DecimalPlaces, 7),
            rounding(PidRoundingKind::SignificantDigits, 3),
        ] {
            for raw in [f32::MIN_POSITIVE, f32::from_bits(1), f32::MAX, -f32::MAX] {
                let rounded = policy.round_value(raw).unwrap();
                assert!(rounded.value.is_finite());
                assert_eq!(rounded.display.parse::<f32>().unwrap(), rounded.value);
                assert_eq!(policy.round_value(rounded.value).unwrap(), rounded);
            }
        }
        let error = rounding(PidRoundingKind::SignificantDigits, 4)
            .round_value(f32::MAX)
            .unwrap_err();
        assert!(error.to_string().contains("finite"));
    }

    #[test]
    fn rounding_rejects_non_finite_values() {
        for raw in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let error = PidRounding::default().round_value(raw).unwrap_err();
            assert!(error.to_string().contains("finite"));
        }
    }

    proptest! {
        #[test]
        fn rounding_is_idempotent_and_display_is_the_same_controller_value(
            raw in -1_000_000_f32..1_000_000_f32,
            digits in 1_u8..=7,
            decimal_places in any::<bool>(),
        ) {
            let kind = if decimal_places {
                PidRoundingKind::DecimalPlaces
            } else {
                PidRoundingKind::SignificantDigits
            };
            let policy = rounding(kind, digits);
            let rounded = policy.round_value(raw).unwrap();
            prop_assert_eq!(rounded.display.parse::<f32>().unwrap(), rounded.value);
            prop_assert_eq!(policy.round_value(rounded.value).unwrap(), rounded);
        }
    }

    #[test]
    fn proportional_type_serde_round_trip() {
        for pt in [ProportionalType::Gain, ProportionalType::Band] {
            let json = serde_json::to_string(&pt).unwrap();
            let back: ProportionalType = serde_json::from_str(&json).unwrap();
            assert_eq!(pt, back);
        }
    }

    #[test]
    fn integral_type_serde_round_trip() {
        for it in [
            IntegralType::ResetTime,
            IntegralType::ResetRate,
            IntegralType::ResetGain,
        ] {
            let json = serde_json::to_string(&it).unwrap();
            let back: IntegralType = serde_json::from_str(&json).unwrap();
            assert_eq!(it, back);
        }
    }

    #[test]
    fn derivative_type_serde_round_trip() {
        for dt in [
            DerivativeType::DerivativeTime,
            DerivativeType::DerivativeGain,
        ] {
            let json = serde_json::to_string(&dt).unwrap();
            let back: DerivativeType = serde_json::from_str(&json).unwrap();
            assert_eq!(dt, back);
        }
    }

    #[test]
    fn time_unit_serde_round_trip() {
        for tu in [TimeUnit::Seconds, TimeUnit::Minutes] {
            let json = serde_json::to_string(&tu).unwrap();
            let back: TimeUnit = serde_json::from_str(&json).unwrap();
            assert_eq!(tu, back);
        }
    }

    #[test]
    fn serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&ProportionalType::Band).unwrap(),
            "\"band\""
        );
        assert_eq!(
            serde_json::to_string(&IntegralType::ResetGain).unwrap(),
            "\"reset_gain\""
        );
        assert_eq!(
            serde_json::to_string(&DerivativeType::DerivativeGain).unwrap(),
            "\"derivative_gain\""
        );
        assert_eq!(
            serde_json::to_string(&TimeUnit::Minutes).unwrap(),
            "\"minutes\""
        );
    }
}
