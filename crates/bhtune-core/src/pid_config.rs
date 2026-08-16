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

#[cfg(test)]
mod tests {
    use super::*;

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
