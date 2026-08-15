//! Per-run test configuration: process type, controller type, relay amplitude, and MRFT
//! cycle/timing parameters.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{controller_type::ControllerType, process_type::ProcessType};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct LoopConfig {
    pub process_type: ProcessType,
    pub controller_type: ControllerType,
    /// Relay amplitude as a percentage of the MV range. Not enforced by the type itself --
    /// call [`LoopConfig::validate`] on a `LoopConfig` built from external input (CLI flags,
    /// an imported template) before using it, to catch an out-of-range value before it
    /// reaches a live loop.
    pub relay_amp_percent: f32,
    pub num_cycles_skip: u32,
    pub num_cycles_count: u32,
    pub noise_protection_secs: u32,
    /// Pre/post-test recording padding, in seconds. PV is still read and recorded during
    /// this period; no switch evaluation happens.
    pub mrft_delay_secs: u32,
}

impl LoopConfig {
    /// Minimum allowed `relay_amp_percent`. A relay step smaller than this is not a
    /// meaningfully-sized perturbation to reliably induce a detectable oscillation, and is
    /// far more likely to be a data-entry slip (a misplaced decimal point) than a genuine
    /// choice.
    pub const RELAY_AMP_PERCENT_MIN: f32 = 0.1;
    /// Maximum allowed `relay_amp_percent`. Half of the entire MV span in a single relay step
    /// is already an unusually aggressive, disruptive perturbation to a live process --
    /// legitimate tunes use single-digit to low-double-digit percentages. A value above this
    /// is far more likely to be a mistake than a deliberate choice; this is exactly the
    /// failure mode this bound closes off, where an unvalidated field let a stray four-digit
    /// debug shortcut reach a live control loop as a "relay amplitude".
    pub const RELAY_AMP_PERCENT_MAX: f32 = 50.0;

    /// Maximum allowed `mrft_delay_secs`. Chosen to match the default `--timeout-secs` (one
    /// hour): genuine pre/post-test recording padding is realistically a few minutes at
    /// most, so anything larger is far more likely a units mistake (e.g. milliseconds typed
    /// as seconds) than a deliberate choice.
    pub const MRFT_DELAY_SECS_MAX: u32 = 3600;

    /// Applies `process_type`'s default skip/test/noise-protection values, keeping the
    /// existing `controller_type`, `relay_amp_percent`, and `mrft_delay_secs`. Downgrades
    /// `controller_type` from PID to PI if the new process type doesn't allow PID.
    pub fn with_process_type(mut self, process_type: ProcessType) -> LoopConfig {
        self.process_type = process_type;
        self.num_cycles_skip = process_type.default_cycles_skip();
        self.num_cycles_count = process_type.default_cycles_test();
        self.noise_protection_secs = process_type.default_noise_protection_secs();
        if !self.controller_type.is_allowed_for(process_type) {
            self.controller_type = ControllerType::Pi;
        }
        self
    }

    /// Validates fields whose legality can't be expressed in the type system alone: relay
    /// amplitude against `[RELAY_AMP_PERCENT_MIN, RELAY_AMP_PERCENT_MAX]`, `num_cycles_count`
    /// must be at least 1 (zero previously reached `tuning_math::measure_oscillation`'s
    /// internal `assert!` and panicked mid-run, after the loop had already been switched to
    /// manual and stroked -- see `docs/v1-checklist.md` §2), and `mrft_delay_secs` against
    /// `MRFT_DELAY_SECS_MAX`. This is real range validation at the model/construction level,
    /// not just a client-side keystroke filter or a single "not blank" check, so it applies
    /// no matter how the `LoopConfig` was built (CLI flags, an imported template, or a
    /// future web GUI request).
    pub fn validate(&self) -> Result<(), LoopConfigError> {
        let amp = self.relay_amp_percent;
        if !amp.is_finite()
            || !(Self::RELAY_AMP_PERCENT_MIN..=Self::RELAY_AMP_PERCENT_MAX).contains(&amp)
        {
            return Err(LoopConfigError::RelayAmpOutOfRange { value: amp });
        }
        if self.num_cycles_count < 1 {
            return Err(LoopConfigError::CyclesCountMustBeAtLeastOne);
        }
        if self.mrft_delay_secs > Self::MRFT_DELAY_SECS_MAX {
            return Err(LoopConfigError::MrftDelayOutOfRange {
                value: self.mrft_delay_secs,
            });
        }
        Ok(())
    }
}

/// Why a [`LoopConfig`] failed [`LoopConfig::validate`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoopConfigError {
    /// `relay_amp_percent` was non-finite (NaN/infinite) or outside
    /// `[LoopConfig::RELAY_AMP_PERCENT_MIN, LoopConfig::RELAY_AMP_PERCENT_MAX]`.
    RelayAmpOutOfRange { value: f32 },
    /// `num_cycles_count` was `0` -- at least one full relay cycle is required to measure an
    /// oscillation at all.
    CyclesCountMustBeAtLeastOne,
    /// `mrft_delay_secs` exceeded [`LoopConfig::MRFT_DELAY_SECS_MAX`].
    MrftDelayOutOfRange { value: u32 },
}

impl fmt::Display for LoopConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoopConfigError::RelayAmpOutOfRange { value } => write!(
                f,
                "relay amplitude {value}% is out of range: must be a finite value from {}% to \
                 {}% of the MV range",
                LoopConfig::RELAY_AMP_PERCENT_MIN,
                LoopConfig::RELAY_AMP_PERCENT_MAX,
            ),
            LoopConfigError::CyclesCountMustBeAtLeastOne => {
                write!(f, "cycles count must be at least 1")
            }
            LoopConfigError::MrftDelayOutOfRange { value } => write!(
                f,
                "mrft delay {value}s is out of range: must be at most {}s",
                LoopConfig::MRFT_DELAY_SECS_MAX,
            ),
        }
    }
}

impl std::error::Error for LoopConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LoopConfig {
        LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 3,
            mrft_delay_secs: 0,
        }
    }

    #[test]
    fn with_process_type_applies_defaults() {
        let cfg = sample().with_process_type(ProcessType::TemperatureHeatExchange);
        assert_eq!(cfg.process_type, ProcessType::TemperatureHeatExchange);
        assert_eq!(cfg.num_cycles_skip, 1);
        assert_eq!(cfg.num_cycles_count, 1);
        assert_eq!(cfg.noise_protection_secs, 20);
        // unrelated fields untouched
        assert_eq!(cfg.relay_amp_percent, 5.0);
        assert_eq!(cfg.mrft_delay_secs, 0);
    }

    #[test]
    fn with_process_type_keeps_controller_type_when_still_allowed() {
        let cfg = sample().with_process_type(ProcessType::Level);
        assert_eq!(cfg.controller_type, ControllerType::Pi);
    }

    #[test]
    fn with_process_type_downgrades_pid_when_no_longer_allowed() {
        let mut cfg = sample();
        cfg.controller_type = ControllerType::Pid;
        let cfg = cfg.with_process_type(ProcessType::Flow);
        assert_eq!(cfg.controller_type, ControllerType::Pi);
    }

    #[test]
    fn with_process_type_keeps_pid_for_temperature_types() {
        let mut cfg = sample();
        cfg.controller_type = ControllerType::Pid;
        let cfg = cfg.with_process_type(ProcessType::TemperatureMixing);
        assert_eq!(cfg.controller_type, ControllerType::Pid);
    }

    #[test]
    fn serde_round_trip() {
        let cfg = sample();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: LoopConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn validate_accepts_a_typical_relay_amplitude() {
        let mut cfg = sample();
        cfg.relay_amp_percent = 10.0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_accepts_the_minimum_boundary() {
        let mut cfg = sample();
        cfg.relay_amp_percent = LoopConfig::RELAY_AMP_PERCENT_MIN;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_rejects_just_below_the_minimum() {
        let mut cfg = sample();
        cfg.relay_amp_percent = LoopConfig::RELAY_AMP_PERCENT_MIN - 0.01;
        assert!(matches!(
            cfg.validate(),
            Err(LoopConfigError::RelayAmpOutOfRange { .. })
        ));
    }

    #[test]
    fn validate_accepts_the_maximum_boundary() {
        let mut cfg = sample();
        cfg.relay_amp_percent = LoopConfig::RELAY_AMP_PERCENT_MAX;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_rejects_just_above_the_maximum() {
        let mut cfg = sample();
        cfg.relay_amp_percent = LoopConfig::RELAY_AMP_PERCENT_MAX + 0.01;
        assert!(matches!(
            cfg.validate(),
            Err(LoopConfigError::RelayAmpOutOfRange { .. })
        ));
    }

    #[test]
    fn validate_rejects_zero() {
        let mut cfg = sample();
        cfg.relay_amp_percent = 0.0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_negative_values() {
        let mut cfg = sample();
        cfg.relay_amp_percent = -5.0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_nan() {
        let mut cfg = sample();
        cfg.relay_amp_percent = f32::NAN;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_infinite() {
        let mut cfg = sample();
        cfg.relay_amp_percent = f32::INFINITY;
        assert!(cfg.validate().is_err());
    }

    /// The motivating case: BHTune's predecessor let a leftover debug shortcut leave a
    /// four-digit value in this exact field with only a "not blank" check to catch it (see
    /// `docs/v1-checklist.md` §2). `validate` must reject it.
    #[test]
    fn validate_rejects_a_legacy_style_four_digit_value() {
        let mut cfg = sample();
        cfg.relay_amp_percent = 2014.0;
        assert!(matches!(
            cfg.validate(),
            Err(LoopConfigError::RelayAmpOutOfRange { value }) if value == 2014.0
        ));
    }

    #[test]
    fn relay_amp_out_of_range_display_names_the_value_and_the_bounds() {
        let err = LoopConfigError::RelayAmpOutOfRange { value: 2014.0 };
        let message = err.to_string();
        assert!(message.contains("2014"));
        assert!(message.contains(&LoopConfig::RELAY_AMP_PERCENT_MIN.to_string()));
        assert!(message.contains(&LoopConfig::RELAY_AMP_PERCENT_MAX.to_string()));
    }

    #[test]
    fn validate_accepts_a_typical_cycles_count() {
        let mut cfg = sample();
        cfg.num_cycles_count = 3;
        assert!(cfg.validate().is_ok());
    }

    /// The reproduced panic: `--cycles-count 0` used to reach
    /// `tuning_math::measure_oscillation`'s internal `assert!` and panic mid-run, after the
    /// loop had already been switched to manual and stroked. `validate` must reject it before
    /// any of that happens.
    #[test]
    fn validate_rejects_zero_cycles_count() {
        let mut cfg = sample();
        cfg.num_cycles_count = 0;
        assert_eq!(
            cfg.validate(),
            Err(LoopConfigError::CyclesCountMustBeAtLeastOne)
        );
    }

    #[test]
    fn validate_accepts_one_cycles_count() {
        let mut cfg = sample();
        cfg.num_cycles_count = 1;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_accepts_zero_mrft_delay() {
        let mut cfg = sample();
        cfg.mrft_delay_secs = 0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_accepts_the_mrft_delay_maximum_boundary() {
        let mut cfg = sample();
        cfg.mrft_delay_secs = LoopConfig::MRFT_DELAY_SECS_MAX;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_rejects_just_above_the_mrft_delay_maximum() {
        let mut cfg = sample();
        cfg.mrft_delay_secs = LoopConfig::MRFT_DELAY_SECS_MAX + 1;
        assert_eq!(
            cfg.validate(),
            Err(LoopConfigError::MrftDelayOutOfRange {
                value: LoopConfig::MRFT_DELAY_SECS_MAX + 1
            })
        );
    }

    #[test]
    fn cycles_count_error_display_names_the_requirement() {
        let message = LoopConfigError::CyclesCountMustBeAtLeastOne.to_string();
        assert!(message.contains("at least 1"));
    }

    #[test]
    fn mrft_delay_out_of_range_display_names_the_value_and_the_bound() {
        let err = LoopConfigError::MrftDelayOutOfRange { value: 9999 };
        let message = err.to_string();
        assert!(message.contains("9999"));
        assert!(message.contains(&LoopConfig::MRFT_DELAY_SECS_MAX.to_string()));
    }

    /// `LoopConfigError` must be usable as a trait object / via `?` in a `Result<_,
    /// anyhow::Error>` call site (`bhtune-cli`'s `build_loop_config`, in particular), which
    /// requires a real `std::error::Error` impl, not just `Display`.
    #[test]
    fn loop_config_error_is_a_std_error() {
        let err = LoopConfigError::RelayAmpOutOfRange { value: 2014.0 };
        let _: Box<dyn std::error::Error> = Box::new(err);
    }
}
