use bhtune_core::{ControllerDirection, ControllerType, ProcessType, ResponseLevel, TagOverrides};
use bhtune_db::models::TuneDriver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    Opcda,
    Simulator,
}

impl From<DriverKind> for TuneDriver {
    fn from(value: DriverKind) -> Self {
        match value {
            DriverKind::Opcda => Self::Opcda,
            DriverKind::Simulator => Self::Simulator,
        }
    }
}

impl TryFrom<TuneDriver> for DriverKind {
    type Error = UnsupportedDriverKind;

    fn try_from(value: TuneDriver) -> Result<Self, Self::Error> {
        match value {
            TuneDriver::Opcda => Ok(Self::Opcda),
            TuneDriver::Simulator => Ok(Self::Simulator),
            TuneDriver::Replay => Err(UnsupportedDriverKind),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedDriverKind;

impl std::fmt::Display for UnsupportedDriverKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the replay driver cannot be used to start a new tune run (it exists only for \
             offline golden-trace validation, not live/simulated tuning)"
        )
    }
}

impl std::error::Error for UnsupportedDriverKind {}

/// Transport-neutral inputs for a tune run.
///
/// CLI and HTTP adapters construct this type from their own request models. Runtime timing
/// values are resolved from [`crate::config::BhtuneConfig`] during preparation.
#[derive(Debug, Clone)]
pub struct TuneRequest {
    pub tagname: String,
    pub template: String,
    pub process_type: ProcessType,
    pub controller_type: ControllerType,
    pub relay_amp: f32,
    pub cycles_skip: Option<u32>,
    pub cycles_count: Option<u32>,
    pub noise_protection_secs: Option<u32>,
    pub driver: DriverKind,
    pub bridge_host: Option<String>,
    pub server: Option<String>,
    pub sim_gain: f32,
    pub sim_tau: f32,
    pub sim_dead_time: f32,
    pub sim_noise: f32,
    pub sim_seed: u64,
    pub sim_initial_pv: f32,
    pub sim_initial_mv: f32,
    pub pv_range_high: Option<f32>,
    pub pv_range_low: Option<f32>,
    pub mv_range_high: Option<f32>,
    pub mv_range_low: Option<f32>,
    pub direction: Option<ControllerDirection>,
    pub tag_overrides: Option<TagOverrides>,
    pub notes: Option<String>,
    pub yes: bool,
    pub write_pid: Option<ResponseLevel>,
    #[cfg(test)]
    pub mrft_delay: u32,
    #[cfg(test)]
    pub poll_interval_ms: u64,
    #[cfg(test)]
    pub timeout_secs: u64,
    #[cfg(test)]
    pub op_timeout_secs: u64,
    #[cfg(test)]
    pub restore_timeout_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::{DriverKind, UnsupportedDriverKind};
    use bhtune_db::models::TuneDriver;

    #[test]
    fn driver_kind_maps_supported_drivers_and_rejects_replay() {
        assert_eq!(TuneDriver::from(DriverKind::Opcda), TuneDriver::Opcda);
        assert_eq!(
            DriverKind::try_from(TuneDriver::Opcda),
            Ok(DriverKind::Opcda)
        );
        assert_eq!(
            DriverKind::try_from(TuneDriver::Simulator),
            Ok(DriverKind::Simulator)
        );
        assert_eq!(
            DriverKind::try_from(TuneDriver::Replay),
            Err(UnsupportedDriverKind)
        );
    }

    #[test]
    fn unsupported_driver_error_explains_replay_scope() {
        assert_eq!(
            UnsupportedDriverKind.to_string(),
            "the replay driver cannot be used to start a new tune run (it exists only for offline golden-trace validation, not live/simulated tuning)"
        );
    }
}
