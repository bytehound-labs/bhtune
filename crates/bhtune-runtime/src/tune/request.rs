use bhtune_core::{ControllerDirection, ControllerType, ProcessType, ResponseLevel, TagOverrides};
use bhtune_db::models::TuneDriver;

/// Default simulator process gain used by every tune-request adapter.
pub const DEFAULT_SIM_GAIN: f32 = 1.0;
/// Default simulator process time constant in seconds.
pub const DEFAULT_SIM_TAU: f32 = 2.0;
/// Default simulator dead time in seconds.
pub const DEFAULT_SIM_DEAD_TIME: f32 = 5.0;
/// Default simulator measurement-noise amplitude.
pub const DEFAULT_SIM_NOISE: f32 = 0.0;
/// Default simulator random-number-generator seed.
pub const DEFAULT_SIM_SEED: u64 = 0;
/// Default simulator initial PV and MV.
pub const DEFAULT_SIM_INITIAL_VALUE: f32 = 50.0;

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

/// A common tune-request field failed runtime validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuneRequestValidationError(String);

impl TuneRequestValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for TuneRequestValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TuneRequestValidationError {}

/// Rejects a non-finite `f32` request value and identifies its field.
pub fn validate_finite_f32(field: &str, value: f32) -> Result<(), TuneRequestValidationError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(TuneRequestValidationError::new(format!(
            "'{field}' must be a finite number (not NaN or infinite), got {value}"
        )))
    }
}

/// Rejects a zero `u32` value where the request requires a positive count.
pub fn validate_positive_u32(field: &str, value: u32) -> Result<(), TuneRequestValidationError> {
    if value > 0 {
        Ok(())
    } else {
        Err(TuneRequestValidationError::new(format!(
            "{field} must be at least 1"
        )))
    }
}

/// Transport-neutral inputs for a tune run.
///
/// CLI and HTTP adapters construct this type from their own request models. Runtime timing
/// values are resolved from [`crate::config::BhtuneConfig`] during preparation.
#[derive(Debug, Clone, PartialEq)]
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

/// A transport-neutral tune request that has passed common input validation.
///
/// Adapters use this type to share finite-number, positive-cycle-count, and tag-override
/// checks before calling [`super::prepare`]. The runtime also accepts an unvalidated
/// [`TuneRequest`] at that boundary and validates it before any database or driver work.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedTuneRequest(TuneRequest);

impl ValidatedTuneRequest {
    /// Borrows the validated transport-neutral inputs.
    pub fn as_request(&self) -> &TuneRequest {
        &self.0
    }

    /// Returns the transport-neutral inputs after validation.
    pub fn into_request(self) -> TuneRequest {
        self.0
    }
}

impl TryFrom<TuneRequest> for ValidatedTuneRequest {
    type Error = TuneRequestValidationError;

    fn try_from(request: TuneRequest) -> Result<Self, Self::Error> {
        for (field, value) in [
            ("relay_amp", request.relay_amp),
            ("sim_gain", request.sim_gain),
            ("sim_tau", request.sim_tau),
            ("sim_dead_time", request.sim_dead_time),
            ("sim_noise", request.sim_noise),
            ("sim_initial_pv", request.sim_initial_pv),
            ("sim_initial_mv", request.sim_initial_mv),
        ] {
            validate_finite_f32(field, value)?;
        }

        for (field, value) in [
            ("pv_range_high", request.pv_range_high),
            ("pv_range_low", request.pv_range_low),
            ("mv_range_high", request.mv_range_high),
            ("mv_range_low", request.mv_range_low),
        ] {
            if let Some(value) = value {
                validate_finite_f32(field, value)?;
            }
        }

        if let Some(cycles_count) = request.cycles_count {
            validate_positive_u32("cycles count", cycles_count)?;
        }

        if let Some(tag_overrides) = &request.tag_overrides {
            tag_overrides
                .validate()
                .map_err(|error| TuneRequestValidationError::new(error.to_string()))?;
        }

        Ok(Self(request))
    }
}

#[cfg(test)]
mod tests {
    use bhtune_core::{ControllerDirection, ControllerType, ProcessType, TagOverrides};
    use bhtune_db::models::TuneDriver;

    use super::{
        DEFAULT_SIM_DEAD_TIME, DEFAULT_SIM_GAIN, DEFAULT_SIM_INITIAL_VALUE, DEFAULT_SIM_NOISE,
        DEFAULT_SIM_SEED, DEFAULT_SIM_TAU, DriverKind, TuneRequest, TuneRequestValidationError,
        UnsupportedDriverKind, ValidatedTuneRequest, validate_finite_f32, validate_positive_u32,
    };

    fn valid_request() -> TuneRequest {
        TuneRequest {
            tagname: "Unit1.LIC101.PV".to_owned(),
            template: "Honeywell".to_owned(),
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp: 10.0,
            cycles_skip: None,
            cycles_count: None,
            noise_protection_secs: None,
            driver: DriverKind::Simulator,
            bridge_host: None,
            server: None,
            sim_gain: DEFAULT_SIM_GAIN,
            sim_tau: DEFAULT_SIM_TAU,
            sim_dead_time: DEFAULT_SIM_DEAD_TIME,
            sim_noise: DEFAULT_SIM_NOISE,
            sim_seed: DEFAULT_SIM_SEED,
            sim_initial_pv: DEFAULT_SIM_INITIAL_VALUE,
            sim_initial_mv: DEFAULT_SIM_INITIAL_VALUE,
            pv_range_high: None,
            pv_range_low: None,
            mv_range_high: None,
            mv_range_low: None,
            direction: Some(ControllerDirection::Reverse),
            tag_overrides: None,
            notes: None,
            yes: false,
            write_pid: None,
            #[cfg(test)]
            mrft_delay: 0,
            #[cfg(test)]
            poll_interval_ms: 800,
            #[cfg(test)]
            timeout_secs: 3600,
            #[cfg(test)]
            op_timeout_secs: 30,
            #[cfg(test)]
            restore_timeout_secs: 30,
        }
    }

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

    #[test]
    fn validation_helpers_accept_finite_and_positive_values() {
        assert!(validate_finite_f32("sim_gain", 1.0).is_ok());
        assert!(validate_positive_u32("cycles count", 1).is_ok());
    }

    #[test]
    fn validation_helpers_reject_non_finite_and_zero_values() {
        let non_finite = validate_finite_f32("sim_gain", f32::INFINITY).unwrap_err();
        assert_eq!(
            non_finite.to_string(),
            "'sim_gain' must be a finite number (not NaN or infinite), got inf"
        );
        let zero = validate_positive_u32("cycles count", 0).unwrap_err();
        assert_eq!(zero.to_string(), "cycles count must be at least 1");
    }

    #[test]
    fn validated_tune_request_preserves_valid_inputs_and_optional_defaults() {
        let request = valid_request();
        let validated = ValidatedTuneRequest::try_from(request.clone()).unwrap();
        assert_eq!(validated.as_request(), &request);
        assert_eq!(validated.into_request(), request);
    }

    #[test]
    fn validated_tune_request_rejects_non_finite_scalar_and_optional_range_values() {
        macro_rules! assert_invalid {
            ($field:literal, $set:expr) => {{
                let mut request = valid_request();
                $set(&mut request);
                let error = ValidatedTuneRequest::try_from(request).unwrap_err();
                assert!(error.to_string().contains($field), "{error}");
            }};
        }

        assert_invalid!("relay_amp", |request: &mut TuneRequest| {
            request.relay_amp = f32::NAN;
        });
        assert_invalid!("sim_gain", |request: &mut TuneRequest| {
            request.sim_gain = f32::INFINITY;
        });
        assert_invalid!("sim_tau", |request: &mut TuneRequest| {
            request.sim_tau = f32::NEG_INFINITY;
        });
        assert_invalid!("sim_dead_time", |request: &mut TuneRequest| {
            request.sim_dead_time = f32::NAN;
        });
        assert_invalid!("sim_noise", |request: &mut TuneRequest| {
            request.sim_noise = f32::INFINITY;
        });
        assert_invalid!("sim_initial_pv", |request: &mut TuneRequest| {
            request.sim_initial_pv = f32::NEG_INFINITY;
        });
        assert_invalid!("sim_initial_mv", |request: &mut TuneRequest| {
            request.sim_initial_mv = f32::NAN;
        });
        assert_invalid!("pv_range_high", |request: &mut TuneRequest| {
            request.pv_range_high = Some(f32::INFINITY);
        });
        assert_invalid!("pv_range_low", |request: &mut TuneRequest| {
            request.pv_range_low = Some(f32::NAN);
        });
        assert_invalid!("mv_range_high", |request: &mut TuneRequest| {
            request.mv_range_high = Some(f32::NEG_INFINITY);
        });
        assert_invalid!("mv_range_low", |request: &mut TuneRequest| {
            request.mv_range_low = Some(f32::INFINITY);
        });
    }

    #[test]
    fn validated_tune_request_rejects_zero_cycles_and_invalid_tag_overrides() {
        let mut request = valid_request();
        request.cycles_count = Some(0);
        assert_eq!(
            ValidatedTuneRequest::try_from(request).unwrap_err(),
            TuneRequestValidationError::new("cycles count must be at least 1")
        );

        let mut request = valid_request();
        request.cycles_count = Some(1);
        request.tag_overrides = Some(TagOverrides {
            process_variable: Some("Unit1.LIC101\nPV".to_owned()),
            ..TagOverrides::default()
        });
        let error = ValidatedTuneRequest::try_from(request).unwrap_err();
        assert!(error.to_string().contains("process_variable"));
    }
}
