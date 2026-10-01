use super::{ApiError, DriverKind, StartRunRequest, TuneDriver, TuneOutcome, TuneRunRow};
use bhtune_runtime::tune::{TuneRequest, ValidatedTuneRequest};

impl StartRunRequest {
    /// Converts this HTTP request into the runtime's validated tune request.
    ///
    /// Operational timing values are not request fields; `prepare()` resolves and validates
    /// the global configuration before connecting to a driver or mutating the database/live
    /// loop.
    pub(crate) fn into_validated_tune_request(self) -> Result<ValidatedTuneRequest, ApiError> {
        let driver =
            DriverKind::try_from(self.driver).map_err(|e| ApiError::BadRequest(e.to_string()))?;

        ValidatedTuneRequest::try_from(TuneRequest {
            tagname: self.tagname,
            template: self.template,
            process_type: self.process_type,
            controller_type: self.controller_type,
            relay_amp: self.relay_amp,
            cycles_skip: self.cycles_skip,
            cycles_count: self.cycles_count,
            noise_protection_secs: self.noise_protection_secs,
            driver,
            bridge_host: self.bridge_host,
            server: self.server,
            sim_gain: self.sim_gain,
            sim_tau: self.sim_tau,
            sim_dead_time: self.sim_dead_time,
            sim_noise: self.sim_noise,
            sim_seed: self.sim_seed,
            sim_initial_pv: self.sim_initial_pv,
            sim_initial_mv: self.sim_initial_mv,
            pv_range_high: self.pv_range_high,
            pv_range_low: self.pv_range_low,
            mv_range_high: self.mv_range_high,
            mv_range_low: self.mv_range_low,
            direction: self.direction,
            tag_overrides: self.tag_overrides,
            notes: self.notes,
            yes: self.yes,
            write_pid: self.write_pid,
        })
        .map_err(|error| ApiError::BadRequest(error.to_string()))
    }
}

/// Checks that `run` is eligible for a post-hoc PID write or revert (`api-post-run-write`):
/// finished (not still running its own test), used the `opcda` driver, has PID constant
/// tags in its snapshotted [`bhtune_core::LoopTags`], and recorded the OPC server/bridge
/// host it actually connected through. Shared by [`write_run`] and [`revert_run`] -- both
/// need exactly the same eligibility, only the *target* values to write differ.
pub(super) fn require_writable_run(run: &TuneRunRow) -> Result<(), ApiError> {
    if run.outcome == TuneOutcome::Running {
        return Err(ApiError::BadRequest(format!(
            "run {} is still running; wait for it to finish before writing or reverting PID \
             constants",
            run.id
        )));
    }
    if run.driver != TuneDriver::Opcda {
        return Err(ApiError::BadRequest(format!(
            "run {} used the {:?} driver, which has no live loop to write PID constants to",
            run.id, run.driver
        )));
    }
    if run.tags.proportional_constant.is_none()
        || run.tags.integral_constant.is_none()
        || run.tags.derivative_constant.is_none()
    {
        return Err(ApiError::BadRequest(format!(
            "run {}'s snapshotted tags have no PID constant tags configured",
            run.id
        )));
    }
    if run.opc_server.is_none() || run.bridge_host.is_none() {
        return Err(ApiError::BadRequest(format!(
            "run {} has no recorded OPC server/bridge host; refusing to guess which \
             connection to use",
            run.id
        )));
    }
    Ok(())
}
