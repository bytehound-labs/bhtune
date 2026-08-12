//! Per-run test configuration: process type, controller type, relay amplitude, and MRFT
//! cycle/timing parameters.

use serde::{Deserialize, Serialize};

use crate::{controller_type::ControllerType, process_type::ProcessType};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoopConfig {
    pub process_type: ProcessType,
    pub controller_type: ControllerType,
    /// Relay amplitude as a percentage of the MV range. This type does not enforce bounds
    /// itself yet — real range validation is tracked in `docs/v1-checklist.md`.
    pub relay_amp_percent: f32,
    pub num_cycles_skip: u32,
    pub num_cycles_count: u32,
    pub noise_protection_secs: u32,
    /// Pre/post-test recording padding, in seconds. PV is still read and recorded during
    /// this period; no switch evaluation happens.
    pub mrft_delay_secs: u32,
}

impl LoopConfig {
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
}

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
}
