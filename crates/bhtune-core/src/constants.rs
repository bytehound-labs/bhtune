//! Tuning-constant lookup matrices: per-process-type, per-response-level, per-controller
//! type multipliers used to turn a measured relay-test oscillation into Kp/Ti/Td.
//!
//! These constants come from published relay-feedback tuning correlations, not from
//! anything derived at runtime — they are a fixed lookup table indexed by
//! [`crate::process_type::ProcessType`], [`ResponseLevel`], and
//! [`crate::controller_type::ControllerType`].

use serde::{Deserialize, Serialize};

use crate::{controller_type::ControllerType, process_type::ProcessType};

/// How aggressively a tuning result should push the loop: a faster response trades off
/// stability margin, a more sluggish response trades off speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ResponseLevel {
    Aggressive = 0,
    Moderate = 1,
    Sluggish = 2,
}

impl ResponseLevel {
    pub const ALL: [ResponseLevel; 3] = [
        ResponseLevel::Aggressive,
        ResponseLevel::Moderate,
        ResponseLevel::Sluggish,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// The resolved set of tuning constants for one (process type, response level, controller
/// type) combination.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TuningConstants {
    /// Proportional-gain multiplier. The only constant that varies by controller type for
    /// a P-only controller — a P-only controller has no integral or derivative term, so
    /// `beta`/`c2`/`c3` are always zero for [`ControllerType::P`], but `c1` is not.
    pub c1: f32,
    /// Integral-time multiplier. Zero for [`ControllerType::P`] (no integral term).
    pub c2: f32,
    /// Derivative-time multiplier. Zero for every controller type except
    /// [`ControllerType::Pid`], and only [`ProcessType::allows_pid`] process types have a
    /// nonzero value there.
    pub c3: f32,
    /// Hysteresis multiplier used during the relay test itself. Zero for
    /// [`ControllerType::P`] (no integral term to protect from switching noise).
    pub beta: f32,
}

/// Default number of relay cycles to skip before counting begins, indexed by
/// [`ProcessType::index`].
pub(crate) const DEFAULT_CYCLES_SKIP: [u32; 6] = [1, 1, 1, 1, 1, 1];

/// Default number of relay cycles to count/average, indexed by [`ProcessType::index`].
pub(crate) const DEFAULT_CYCLES_TEST: [u32; 6] = [2, 2, 1, 1, 1, 1];

/// Default noise-protection delay in seconds, indexed by [`ProcessType::index`].
pub(crate) const DEFAULT_NOISE_PROTECTION_SECS: [u32; 6] = [3, 3, 10, 10, 20, 20];

/// `C1[process_type][response_level][controller_type]` — the only matrix that varies by
/// response level, since it is the proportional-gain multiplier.
const C1: [[[f32; 3]; 3]; 6] = [
    // Flow
    [[0.5, 0.451, 0.0], [0.333, 0.302, 0.0], [0.25, 0.226, 0.0]],
    // PressureLine
    [[0.5, 0.442, 0.0], [0.333, 0.296, 0.0], [0.25, 0.221, 0.0]],
    // PressureVessel
    [
        [0.333, 0.331, 0.0],
        [0.222, 0.222, 0.0],
        [0.167, 0.166, 0.0],
    ],
    // Level (intentionally identical to PressureVessel)
    [
        [0.333, 0.331, 0.0],
        [0.222, 0.222, 0.0],
        [0.167, 0.166, 0.0],
    ],
    // TemperatureMixing
    [
        [0.5, 0.47, 0.498],
        [0.25, 0.235, 0.249],
        [0.167, 0.155, 0.164],
    ],
    // TemperatureHeatExchange
    [
        [0.333, 0.325, 0.332],
        [0.222, 0.218, 0.222],
        [0.167, 0.163, 0.166],
    ],
];

/// `C2[process_type][controller_type]` — the integral-time multiplier. Does not vary by
/// response level in the source data (all three response-level rows are identical), so
/// this crate models it as a 2D table.
const C2: [[f32; 3]; 6] = [
    [0.0, 0.331, 0.0],   // Flow
    [0.0, 0.302, 0.0],   // PressureLine
    [0.0, 1.216, 0.0],   // PressureVessel
    [0.0, 1.216, 0.0],   // Level (intentionally identical to PressureVessel)
    [0.0, 0.436, 0.162], // TemperatureMixing
    [0.0, 0.704, 0.275], // TemperatureHeatExchange
];

/// `C3[process_type][controller_type]` — the derivative-time multiplier. Nonzero only for
/// the PID column of the two temperature process types.
const C3: [[f32; 3]; 6] = [
    [0.0, 0.0, 0.0],  // Flow
    [0.0, 0.0, 0.0],  // PressureLine
    [0.0, 0.0, 0.0],  // PressureVessel
    [0.0, 0.0, 0.0],  // Level
    [0.0, 0.0, 0.14], // TemperatureMixing
    [0.0, 0.0, 0.09], // TemperatureHeatExchange
];

/// `BETA[process_type][controller_type]` — the relay hysteresis multiplier. Does not vary
/// by response level in the source data, so this crate models it as a 2D table.
const BETA: [[f32; 3]; 6] = [
    [0.0, 0.433, 0.0],   // Flow
    [0.0, 0.466, 0.0],   // PressureLine
    [0.0, 0.13, 0.0],    // PressureVessel
    [0.0, 0.13, 0.0],    // Level (intentionally identical to PressureVessel)
    [0.0, 0.343, 0.102], // TemperatureMixing
    [0.0, 0.221, 0.013], // TemperatureHeatExchange
];

/// Looks up the tuning constants for one (process type, controller type, response level)
/// combination. `response_level` only affects `c1`; `c2`/`c3`/`beta` are constant across
/// response levels.
pub fn lookup(
    process_type: ProcessType,
    controller_type: ControllerType,
    response_level: ResponseLevel,
) -> TuningConstants {
    let p = process_type.index();
    let c = controller_type.index();
    TuningConstants {
        c1: C1[p][response_level.index()][c],
        c2: C2[p][c],
        c3: C3[p][c],
        beta: BETA[p][c],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_pi_aggressive_matches_known_constants() {
        let tc = lookup(
            ProcessType::Flow,
            ControllerType::Pi,
            ResponseLevel::Aggressive,
        );
        assert_eq!(tc.c1, 0.451);
        assert_eq!(tc.c2, 0.331);
        assert_eq!(tc.c3, 0.0);
        assert_eq!(tc.beta, 0.433);
    }

    #[test]
    fn flow_pi_moderate_and_sluggish_differ_only_in_c1() {
        let moderate = lookup(
            ProcessType::Flow,
            ControllerType::Pi,
            ResponseLevel::Moderate,
        );
        let sluggish = lookup(
            ProcessType::Flow,
            ControllerType::Pi,
            ResponseLevel::Sluggish,
        );
        assert_eq!(moderate.c1, 0.302);
        assert_eq!(sluggish.c1, 0.226);
        assert_eq!(moderate.c2, sluggish.c2);
        assert_eq!(moderate.c3, sluggish.c3);
        assert_eq!(moderate.beta, sluggish.beta);
    }

    /// A P-only controller has no integral or derivative term, so `beta`/`c2`/`c3` are
    /// always zero for it — but `c1` (the proportional-gain multiplier) is not, since P is
    /// still a real controller structure with its own gain calibration.
    #[test]
    fn p_only_controller_has_zero_beta_c2_c3_but_nonzero_c1() {
        for pt in ProcessType::ALL {
            for response in ResponseLevel::ALL {
                let tc = lookup(pt, ControllerType::P, response);
                assert_eq!(tc.beta, 0.0, "{pt:?}/{response:?} beta");
                assert_eq!(tc.c2, 0.0, "{pt:?}/{response:?} c2");
                assert_eq!(tc.c3, 0.0, "{pt:?}/{response:?} c3");
                assert!(tc.c1 > 0.0, "{pt:?}/{response:?} c1 should be nonzero");
            }
        }
    }

    /// Only the two temperature process types allow PID, and this is structurally encoded
    /// in the constant tables themselves: the PID column is zero across the board for
    /// every other process type.
    #[test]
    fn non_temperature_process_types_have_zero_pid_column() {
        let non_temperature = [
            ProcessType::Flow,
            ProcessType::PressureLine,
            ProcessType::PressureVessel,
            ProcessType::Level,
        ];
        for pt in non_temperature {
            for response in ResponseLevel::ALL {
                let tc = lookup(pt, ControllerType::Pid, response);
                assert_eq!(tc.c1, 0.0, "{pt:?}/{response:?} c1");
                assert_eq!(tc.c2, 0.0, "{pt:?} c2");
                assert_eq!(tc.c3, 0.0, "{pt:?} c3");
                assert_eq!(tc.beta, 0.0, "{pt:?} beta");
            }
        }
    }

    #[test]
    fn temperature_mixing_pid_is_nonzero() {
        let tc = lookup(
            ProcessType::TemperatureMixing,
            ControllerType::Pid,
            ResponseLevel::Moderate,
        );
        assert_eq!(tc.c1, 0.249);
        assert_eq!(tc.c2, 0.162);
        assert_eq!(tc.c3, 0.14);
        assert_eq!(tc.beta, 0.102);
    }

    #[test]
    fn temperature_heat_exchange_pid_is_nonzero() {
        let tc = lookup(
            ProcessType::TemperatureHeatExchange,
            ControllerType::Pid,
            ResponseLevel::Moderate,
        );
        assert_eq!(tc.c1, 0.222);
        assert_eq!(tc.c2, 0.275);
        assert_eq!(tc.c3, 0.09);
        assert_eq!(tc.beta, 0.013);
    }

    /// The source calibration data for PressureVessel and Level is byte-for-byte
    /// identical across every matrix; this pins that down as an intentional fact rather
    /// than something a future edit might "fix" into a divergence.
    #[test]
    fn pressure_vessel_and_level_share_identical_constants() {
        for controller in ControllerType::ALL {
            for response in ResponseLevel::ALL {
                let vessel = lookup(ProcessType::PressureVessel, controller, response);
                let level = lookup(ProcessType::Level, controller, response);
                assert_eq!(vessel, level, "{controller:?}/{response:?}");
            }
        }
    }

    #[test]
    fn default_cycle_and_noise_tables_have_six_entries() {
        assert_eq!(DEFAULT_CYCLES_SKIP.len(), 6);
        assert_eq!(DEFAULT_CYCLES_TEST.len(), 6);
        assert_eq!(DEFAULT_NOISE_PROTECTION_SECS.len(), 6);
    }

    #[test]
    fn response_level_serde_round_trip() {
        for rl in ResponseLevel::ALL {
            let json = serde_json::to_string(&rl).unwrap();
            let back: ResponseLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(rl, back);
        }
    }

    #[test]
    fn tuning_constants_serde_round_trip() {
        let tc = lookup(
            ProcessType::Flow,
            ControllerType::Pi,
            ResponseLevel::Aggressive,
        );
        let json = serde_json::to_string(&tc).unwrap();
        let back: TuningConstants = serde_json::from_str(&json).unwrap();
        assert_eq!(tc, back);
    }
}
