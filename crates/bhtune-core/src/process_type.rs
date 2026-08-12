//! The process types the tuning-constant matrices are calibrated for.

use serde::{Deserialize, Serialize};

/// A process/loop category. Each has its own row in the tuning-constant matrices in
/// [`crate::constants`] and its own default cycle/noise-protection settings.
///
/// Discriminants double as the row index into those matrices — see
/// [`ProcessType::index`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessType {
    Flow = 0,
    PressureLine = 1,
    PressureVessel = 2,
    Level = 3,
    TemperatureMixing = 4,
    TemperatureHeatExchange = 5,
}

impl ProcessType {
    /// All variants, in matrix-row order.
    pub const ALL: [ProcessType; 6] = [
        ProcessType::Flow,
        ProcessType::PressureLine,
        ProcessType::PressureVessel,
        ProcessType::Level,
        ProcessType::TemperatureMixing,
        ProcessType::TemperatureHeatExchange,
    ];

    /// Only the two temperature process types allow a full PID controller; every other
    /// process type is restricted to P or PI. See [`crate::controller_type::ControllerType::is_allowed_for`].
    pub fn allows_pid(self) -> bool {
        matches!(
            self,
            ProcessType::TemperatureMixing | ProcessType::TemperatureHeatExchange
        )
    }

    /// Row index into the tuning-constant matrices in [`crate::constants`].
    pub(crate) fn index(self) -> usize {
        self as usize
    }

    /// Default number of relay cycles to skip before counting begins, applied whenever the
    /// process type changes.
    pub fn default_cycles_skip(self) -> u32 {
        crate::constants::DEFAULT_CYCLES_SKIP[self.index()]
    }

    /// Default number of relay cycles to count/average once the skip period ends.
    pub fn default_cycles_test(self) -> u32 {
        crate::constants::DEFAULT_CYCLES_TEST[self.index()]
    }

    /// Default noise-protection delay, in seconds, before a switch is accepted.
    pub fn default_noise_protection_secs(self) -> u32 {
        crate::constants::DEFAULT_NOISE_PROTECTION_SECS[self.index()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_matches_discriminant() {
        assert_eq!(ProcessType::Flow.index(), 0);
        assert_eq!(ProcessType::PressureLine.index(), 1);
        assert_eq!(ProcessType::PressureVessel.index(), 2);
        assert_eq!(ProcessType::Level.index(), 3);
        assert_eq!(ProcessType::TemperatureMixing.index(), 4);
        assert_eq!(ProcessType::TemperatureHeatExchange.index(), 5);
    }

    #[test]
    fn only_temperature_types_allow_pid() {
        for pt in ProcessType::ALL {
            let expected = matches!(
                pt,
                ProcessType::TemperatureMixing | ProcessType::TemperatureHeatExchange
            );
            assert_eq!(pt.allows_pid(), expected, "{pt:?}");
        }
    }

    #[test]
    fn all_contains_every_variant_in_index_order() {
        for (i, pt) in ProcessType::ALL.iter().enumerate() {
            assert_eq!(pt.index(), i);
        }
    }

    #[test]
    fn defaults_are_looked_up_per_process_type() {
        assert_eq!(ProcessType::Flow.default_cycles_skip(), 1);
        assert_eq!(ProcessType::Flow.default_cycles_test(), 2);
        assert_eq!(ProcessType::Flow.default_noise_protection_secs(), 3);

        assert_eq!(
            ProcessType::TemperatureHeatExchange.default_cycles_skip(),
            1
        );
        assert_eq!(
            ProcessType::TemperatureHeatExchange.default_cycles_test(),
            1
        );
        assert_eq!(
            ProcessType::TemperatureHeatExchange.default_noise_protection_secs(),
            20
        );
    }

    #[test]
    fn serde_round_trip() {
        for pt in ProcessType::ALL {
            let json = serde_json::to_string(&pt).unwrap();
            let back: ProcessType = serde_json::from_str(&json).unwrap();
            assert_eq!(pt, back);
        }
    }

    #[test]
    fn serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&ProcessType::TemperatureHeatExchange).unwrap(),
            "\"temperature_heat_exchange\""
        );
    }
}
