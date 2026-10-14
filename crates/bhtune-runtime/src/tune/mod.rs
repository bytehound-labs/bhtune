//! `bhtune tune`/`bhtune simulate`: runs a full MRFT test end-to-end — resolving a template,
//! deriving tags, transitioning the loop to Manual, polling the driver and driving a real
//! [`bhtune_core::MrftEngine`], persisting every tick and the final calculated results, then
//! restoring the loop and optionally writing back the chosen PID constants.
//!
//! The separate `bhtune check` preflight validates the same request inputs and reads
//! controller state without preparing or persisting a run. It uses a read-only database
//! connection and `ReadOnlyDriver` so it cannot mutate the database or controller.
//!
//! Mirrors the legacy `MRFTstart`/`ReadInitialOPCvalues`/`ChangeControllerModeToMan`/
//! `ResetOPC` sequence from `OPCClass.cs`. The mode-transition and write-back steps
//! automatically no-op for the simulator driver, since its [`bhtune_core::LoopTags`] has no
//! setpoint/mode/mode-attribute/PID-constant tags at all (see `build_loop_tags` below) — no
//! separate "is this the simulator?" branching is needed in that logic.

#![allow(rustdoc::broken_intra_doc_links)]

mod actuation;
mod config;
mod outcome;
mod poll;
mod preflight;
mod prepare;
mod quality;
mod recovery;
mod request;
mod restore;
mod timing;
mod writeback;

#[cfg(test)]
use crate::cancel::CtrlC;

pub use actuation::MV_ACTUATION_CONFIRMATION_SECS;
pub use config::validate_restore_timeout_secs;
pub use outcome::{
    AbortReason, RunOutcome, TuneOutcome, TuneRunReport, WriteBackOutcome, tune_outcome_for_run,
};
pub use preflight::{
    PreflightCheck, PreflightCheckStatus, PreflightReport, PreflightTagRead, preflight,
};
pub use prepare::{PreparedTune, drive, drive_report, prepare, prepare_owned};
pub use quality::sample_quality_from_driver;
pub use recovery::{
    RecoveryStepReport, RecoveryStepStatus, RestoreLoopReport, RestoreLoopRequest,
    StartupOrphanSweepReport, recover_startup_orphans, restore_loop,
};
pub use request::{
    DEFAULT_SIM_DEAD_TIME, DEFAULT_SIM_GAIN, DEFAULT_SIM_INITIAL_VALUE, DEFAULT_SIM_NOISE,
    DEFAULT_SIM_SEED, DEFAULT_SIM_TAU, DriverKind, TuneRequest, TuneRequestValidationError,
    UnsupportedDriverKind, ValidatedTuneRequest, validate_finite_f32, validate_positive_u32,
};
pub use writeback::{
    PidWriteOutcome, PidWritePreview, WriteBackHandler, WriteBackSelection, WriteBackSkipReason,
    controller_pid_for_result, pid_parameters_for_result, pid_write_preview, write_pid_values,
    write_pid_values_with_owner,
};
#[allow(unused_imports)]
pub(crate) use writeback::{read_previous_pid_values, write_and_verify_pid_value};

/// Turns a missing value that the caller already checked into a recoverable error.
///
/// A panic after the loop may already be in manual would skip restore. The message names
/// the invariant that failed so the error is actionable in logs.
trait RequireInvariant<T> {
    fn require_invariant(self, message: &'static str) -> anyhow::Result<T>;
}
impl<T> RequireInvariant<T> for Option<T> {
    fn require_invariant(self, message: &'static str) -> anyhow::Result<T> {
        let Some(value) = self else {
            return Err(anyhow::anyhow!(message));
        };
        Ok(value)
    }
}
#[cfg(test)]
async fn run(
    pool: &bhtune_db::SqlitePool,
    request: TuneRequest,
    config: &crate::config::BhtuneConfig,
) -> anyhow::Result<TuneOutcome> {
    run_with_ctrl_c(pool, request, config, &mut CtrlC::never()).await
}

#[cfg(test)]
async fn run_with_ctrl_c(
    pool: &bhtune_db::SqlitePool,
    request: TuneRequest,
    config: &crate::config::BhtuneConfig,
    ctrl_c: &mut CtrlC,
) -> anyhow::Result<TuneOutcome> {
    let prepared = prepare(pool, request, config).await?;
    let report = drive_report(pool, prepared, ctrl_c, None).await?;
    Ok(tune_outcome_for_run(&report.outcome))
}

#[cfg(test)]
#[test]
fn mv_actuation_abort_format_falls_back_for_other_abort_reasons() {
    assert_eq!(
        outcome::format_mv_actuation_abort_reason(&AbortReason::UserInterrupt),
        "UserInterrupt"
    );
}

#[test]
fn require_invariant_returns_the_value_or_an_error() {
    assert_eq!(Some(3).require_invariant("missing").unwrap(), 3);
    let err = None::<i32>
        .require_invariant("missing invariant")
        .unwrap_err();
    assert!(err.to_string().contains("missing invariant"));
}

#[cfg(test)]
mod tests;
