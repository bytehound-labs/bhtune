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
pub use request::{
    DEFAULT_SIM_DEAD_TIME, DEFAULT_SIM_GAIN, DEFAULT_SIM_INITIAL_VALUE, DEFAULT_SIM_NOISE,
    DEFAULT_SIM_SEED, DEFAULT_SIM_TAU, DriverKind, TuneRequest, TuneRequestValidationError,
    UnsupportedDriverKind, ValidatedTuneRequest, validate_finite_f32, validate_positive_u32,
};
pub use writeback::{
    PidWriteOutcome, WriteBackHandler, WriteBackSelection, WriteBackSkipReason,
    pid_parameters_for_result, write_pid_values,
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
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use bhtune_core::{
        Action, ControllerDirection, DcsTemplate, InitialReadings, LoopConfig, LoopTags,
        MrftCompat, MrftEngine, MrftState, ProcessType, PvRange, ResponseLevel, TagOrValue,
        TagOverrides, Tick, TuningResultStatus, lookup,
    };
    use bhtune_db::SqlitePool;
    use bhtune_db::models::{
        EffectiveTuning, MvActuationKind, MvActuationStatus, NewTuneMvActuation, RollbackState,
        SampleQuality, TimingBasis, TimingMetrics, TuneDriver, TuneMvActuationRow, TuneResultRow,
        TuneRunRow, TuneSampleRow, TuneWriteRow,
    };
    use bhtune_driver::{Driver, TagValue, TagWrite};
    use chrono::{DateTime, Utc};
    use tokio::time::Instant;

    use super::actuation::*;
    use super::config::*;
    use super::poll::*;
    use super::prepare::*;
    use super::quality::*;
    use super::request::{DriverKind, TuneRequest};
    use super::restore::*;
    use super::timing::*;
    use super::writeback::*;
    use super::{
        AbortReason, RunOutcome, TuneOutcome, WriteBackOutcome, run, run_with_ctrl_c,
        tune_outcome_for_run,
    };
    use crate::cancel::CtrlC;
    use crate::driver::{SIMULATOR_MV_TAG, SIMULATOR_PV_TAG};
    use crate::timing::{PollTimingAccumulator, RunTimeAnchor, TickTimeSource};
    use bhtune_core::ControllerType;
    use bhtune_db::models::{SamplingAdequacy, TemplateOrigin};

    async fn seeded_pool() -> SqlitePool {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        bhtune_db::seed_builtin_templates(&pool, Utc::now())
            .await
            .unwrap();
        pool
    }

    async fn start_opc_test_run(
        pool: &SqlitePool,
        name: &str,
    ) -> (i64, LoopConfig, DcsTemplate, LoopTags) {
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            pool,
            None,
            name,
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        (run.id, config, template, tags)
    }

    async fn start_yokogawa_test_run(
        pool: &SqlitePool,
        name: &str,
    ) -> (i64, LoopConfig, DcsTemplate, LoopTags) {
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let config = build_loop_config(&args).unwrap();
        let template = yokogawa_template();
        let tags = yokogawa_tags();
        let run = TuneRunRow::start(
            pool,
            None,
            name,
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        (run.id, config, template, tags)
    }

    /// Keep the shared execution tests fast now that timing values come from global
    /// configuration rather than per-run arguments.
    fn test_config() -> crate::config::BhtuneConfig {
        crate::config::BhtuneConfig {
            tuning: crate::config::TuningConfig {
                mrft_delay_secs: Some(0),
                poll_interval_ms: Some(5),
                timeout_secs: Some(5),
                op_timeout_secs: Some(30),
                restore_timeout_secs: Some(30),
            },
            ..crate::config::BhtuneConfig::default()
        }
    }

    fn time_anchor_at(utc: DateTime<Utc>) -> RunTimeAnchor {
        RunTimeAnchor::from_parts(utc, tokio::time::Instant::now())
    }

    fn timing_for_args(args: &TuneRequest) -> PollTimingAccumulator {
        let basis = match args.driver {
            DriverKind::Opcda => TimingBasis::LiveMonotonic,
            DriverKind::Simulator => TimingBasis::SimulatedFixedStep,
        };
        PollTimingAccumulator::new(basis, args.poll_interval_ms)
    }

    #[test]
    fn simulator_driver_requires_every_fixed_range_and_direction_value() {
        let template = bhtune_core::built_in_templates().remove(0);
        for (field, clear) in [
            (
                "pv_range_high",
                (|args: &mut TuneRequest| args.pv_range_high = None) as fn(&mut TuneRequest),
            ),
            ("pv_range_low", |args: &mut TuneRequest| {
                args.pv_range_low = None;
            }),
            ("mv_range_high", |args: &mut TuneRequest| {
                args.mv_range_high = None;
            }),
            ("mv_range_low", |args: &mut TuneRequest| {
                args.mv_range_low = None;
            }),
            ("direction", |args: &mut TuneRequest| args.direction = None),
        ] {
            let mut args = fast_simulator_args();
            clear(&mut args);
            let error = build_loop_tags(&args, &template).unwrap_err();
            assert!(error.to_string().contains(&field.replace('_', "-")));
        }
    }

    #[test]
    fn restore_mv_errors_become_failed_restore_steps() {
        let outcome = restore_mv_outcome_or_failed(Err(anyhow::anyhow!("restore failed")));

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(detail))
                if detail == "restore failed"
        ));
    }

    fn valid_pid_result_row() -> TuneResultRow {
        TuneResultRow {
            id: 0,
            run_id: 1,
            response_level: ResponseLevel::Moderate,
            kp: Some(2.0),
            ti_minutes: Some(4.0),
            td_minutes: Some(0.5),
            proportional: Some(2.0),
            integral: Some(4.0),
            derivative: Some(0.5),
            status: TuningResultStatus::Valid,
            invalid_reason: None,
        }
    }

    #[test]
    fn pid_result_validation_rejects_every_malformed_shape() {
        let mut result = valid_pid_result_row();
        result.status = TuningResultStatus::Invalid;
        result.invalid_reason =
            Some(bhtune_core::TuningResultInvalidReason::NonPositivePvAmplitude);
        let error = pid_parameters_for_result(&result).unwrap_err();
        assert!(error.to_string().contains("PV amplitude is not positive"));

        let mut result = valid_pid_result_row();
        result.invalid_reason = Some(bhtune_core::TuningResultInvalidReason::NonFiniteKp);
        let error = pid_parameters_for_result(&result).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("has an invalid reason despite being marked valid")
        );

        let mut proportional_missing = valid_pid_result_row();
        proportional_missing.proportional = None;
        let mut integral_missing = valid_pid_result_row();
        integral_missing.integral = None;
        let mut derivative_missing = valid_pid_result_row();
        derivative_missing.derivative = None;
        for (result, expected) in [
            (proportional_missing, "missing its proportional value"),
            (integral_missing, "missing its integral value"),
            (derivative_missing, "missing its derivative value"),
        ] {
            assert!(
                pid_parameters_for_result(&result)
                    .unwrap_err()
                    .to_string()
                    .contains(expected)
            );
        }

        let mut result = valid_pid_result_row();
        result.integral = Some(f32::NAN);
        let error = pid_parameters_for_result(&result).unwrap_err();
        assert!(error.to_string().contains("non-finite PID value"));

        let valid = pid_parameters_for_result(&valid_pid_result_row()).unwrap();
        assert_eq!(valid.response_level, ResponseLevel::Moderate);
        assert_eq!(valid.proportional, 2.0);
    }
    #[test]
    fn utc_after_elapsed_rejects_a_duration_outside_chrono_range() {
        let error = utc_after_elapsed(Utc::now(), Duration::MAX).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("MV command time exceeded chrono's range")
        );
    }

    #[tokio::test]
    async fn relay_actuation_timestamp_conversion_error_is_propagated() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let now = Utc::now();
        let instant = Instant::now();

        let error = record_relay_actuation(
            &mut tracker,
            &pool,
            0,
            55.0,
            now,
            instant,
            instant,
            Duration::MAX,
            instant,
            1.0,
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("MV command time exceeded chrono's range")
        );
        assert!(tracker.pending.is_none());
    }

    fn delayed_live_timing_metrics() -> TimingMetrics {
        TimingMetrics {
            basis: TimingBasis::LiveMonotonic,
            requested_interval_ms: 800,
            sample_gap_count: 2,
            mean_sample_gap_ms: Some(1_200.0),
            max_sample_gap_ms: Some(1_600.0),
            missed_poll_opportunity_count: 1,
            measured_oscillation_period_ms: None,
            approximate_samples_per_period: None,
            sampling_adequacy: SamplingAdequacy::NotAssessed,
            poll_latency: None,
        }
    }

    /// A fast-converging simulator tune: proportionally scaled down from
    /// `bhtune-driver`'s own proven `FopdtConfig::new(1.0, 2.0, 5.0, 1.0)` E2E fixture (2
    /// ticks of lag, 5 ticks of dead time) so the whole test — which polls on a real
    /// `tokio::time::interval`, unlike that lower-level test's manually driven ticks —
    /// finishes in well under a second of real wall-clock time.
    fn fast_simulator_args() -> TuneRequest {
        TuneRequest {
            tagname: "ignored-for-simulator".to_string(),
            template: "Yokogawa CentumVP".to_string(),
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp: 10.0,
            cycles_skip: Some(1),
            cycles_count: Some(2),
            noise_protection_secs: Some(0),
            mrft_delay: 0,
            driver: DriverKind::Simulator,
            bridge_host: None,
            server: None,
            sim_gain: 1.0,
            sim_tau: 0.01,
            sim_dead_time: 0.025,
            sim_noise: 0.0,
            sim_seed: 0,
            sim_initial_pv: 50.0,
            sim_initial_mv: 50.0,
            pv_range_high: Some(100.0),
            pv_range_low: Some(0.0),
            mv_range_high: Some(100.0),
            mv_range_low: Some(0.0),
            direction: Some(ControllerDirection::Reverse),
            tag_overrides: None,
            poll_interval_ms: 5,
            // Keep ordinary tune tests bounded even when a mutation prevents the
            // simulator from completing. The dedicated timeout test overrides this.
            timeout_secs: 5,
            op_timeout_secs: 30,
            restore_timeout_secs: 30,
            notes: Some("test note".to_string()),
            yes: false,
            write_pid: None,
        }
    }

    fn completed_poll_for_settling(start: DateTime<Utc>, next_tick_index: i64) -> CompletedPoll {
        CompletedPoll {
            action: Action::Complete {
                peaks: vec![52.0, 48.0, 52.0],
                troughs: vec![46.0, 50.0],
                switch_times: vec![
                    start,
                    start + chrono::Duration::seconds(30),
                    start + chrono::Duration::seconds(60),
                    start + chrono::Duration::seconds(90),
                    start + chrono::Duration::seconds(120),
                ],
                mv_sign_init: 1,
            },
            state: MrftState {
                hysteresis: 0.0,
                mv_value_current: 45.0,
                mv_sign_next_step: 1,
                counter_all_switches: 5,
                cycles_completed: 2,
                cycles_remaining: 0,
            },
            next_tick_index,
            tick_time: TickTimeSource::FixedStep {
                current: start,
                step: chrono::Duration::milliseconds(5),
            },
        }
    }

    #[test]
    fn settling_duration_is_one_third_of_a_finite_positive_period() {
        assert_eq!(
            settling_duration(Some(4_500.0)),
            Some(Duration::from_millis(1_500))
        );
    }

    #[test]
    fn settling_duration_rejects_missing_non_finite_and_non_positive_periods() {
        for period in [
            None,
            Some(0.0),
            Some(-1.0),
            Some(f64::NAN),
            Some(f64::INFINITY),
            Some(f64::from_bits(1)),
        ] {
            assert_eq!(settling_duration(period), None);
        }
    }

    #[test]
    fn effective_timing_converts_to_and_from_the_persisted_snapshot() {
        let configured = crate::config::EffectiveTuningConfig {
            mrft_delay_secs: 7,
            poll_interval_ms: 125,
            timeout_secs: 901,
            op_timeout_secs: 17,
            restore_timeout_secs: 31,
        };

        let timing: EffectiveTiming = configured.into();
        assert_eq!(timing.mrft_delay_secs, 7);
        assert_eq!(timing.poll_interval_ms, 125);
        assert_eq!(timing.timeout_secs, 901);
        assert_eq!(timing.op_timeout_secs, 17);
        assert_eq!(timing.restore_timeout_secs, 31);

        let persisted: EffectiveTuning = timing.into();
        assert_eq!(
            persisted,
            EffectiveTuning {
                mrft_delay_secs: 7,
                poll_interval_ms: 125,
                timeout_secs: 901,
                op_timeout_secs: 17,
                restore_timeout_secs: 31,
            }
        );
    }

    #[test]
    fn auto_release_settling_is_eligible_only_for_live_auto_started_mode_changes() {
        let args = {
            let mut args = fast_simulator_args();
            args.driver = DriverKind::Opcda;
            args
        };
        let template = yokogawa_template();
        let tags = yokogawa_tags();
        let mut guard = MutationGuard {
            mode_written: true,
            ..MutationGuard::default()
        };

        let mut initial = InitialState {
            pv_ini: 50.0,
            mv_ini: 45.0,
            pv_range_high: 100.0,
            pv_range_low: 0.0,
            mv_range_high: 100.0,
            mv_range_low: 0.0,
            direction: ControllerDirection::Direct,
            mode_raw: Some("AUT".to_string()),
            mode_attribute_raw: None,
            setpoint_ini: Some(55.0),
        };
        assert!(should_settle_before_auto_release(
            &args, &tags, &template, &initial, &guard
        ));

        initial.mode_raw = Some("MAN".to_string());
        assert!(!should_settle_before_auto_release(
            &args, &tags, &template, &initial, &guard
        ));

        initial.mode_raw = Some("AUT".to_string());
        guard.mode_written = false;
        assert!(!should_settle_before_auto_release(
            &args, &tags, &template, &initial, &guard
        ));

        guard.mode_written = true;
        let mut simulator_args = args;
        simulator_args.driver = DriverKind::Simulator;
        assert!(!should_settle_before_auto_release(
            &simulator_args,
            &tags,
            &template,
            &initial,
            &guard
        ));
    }

    #[tokio::test]
    async fn live_auto_release_settling_persists_samples_freezes_state_and_releases_auto() {
        let pool = seeded_pool().await;
        let (run_id, _config, template, tags) =
            start_yokogawa_test_run(&pool, "yokogawa-auto-settling").await;
        let driver = yokogawa_driver_auto();
        let initial = read_initial_values(&driver, &tags, &template, true)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        assert!(guard.mode_written);

        let start = Utc::now();
        let mut completion = completed_poll_for_settling(start, 12);
        let frozen_state = completion.state;
        let args = {
            let mut args = fast_simulator_args();
            args.driver = DriverKind::Opcda;
            args
        };
        let effective_timing = test_effective_timing(&args);
        let mut timing = timing_for_args(&args);
        let mut ctrl_c = CtrlC::never();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_writer(std::io::sink)
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let restore = tracing::dispatcher::with_default(&dispatch, || async {
            attempt_restore_with_actuation_with_timing(
                &pool,
                run_id,
                &args,
                effective_timing,
                &driver,
                &tags,
                &template,
                &initial,
                &guard,
                true,
                &mut ctrl_c,
                &mut None,
                Some(&mut completion),
                // Leave enough wall-clock room for coarse Windows timer scheduling; the
                // persisted sample timestamps still use the fixed five-millisecond step.
                Some(1_500.0),
                Some(&mut timing),
            )
            .await
        })
        .await;

        assert!(matches!(restore, RestoreAttempt::Confirmed));
        assert_eq!(completion.state, frozen_state);

        let samples = TuneSampleRow::list_for_run(&pool, run_id).await.unwrap();
        assert!(samples.len() >= 2);
        assert_eq!(
            completion.next_tick_index,
            12 + i64::try_from(samples.len()).unwrap()
        );
        for (offset, sample) in samples.iter().enumerate() {
            assert_eq!(sample.tick_index, 12 + i64::try_from(offset).unwrap());
            assert_eq!(
                sample.sample.time,
                start + chrono::Duration::milliseconds(5 * (offset as i64 + 1))
            );
            assert_eq!(sample.state, frozen_state);
        }
        assert_eq!(
            completion.tick_time.next_timestamp().unwrap(),
            start + chrono::Duration::milliseconds(5 * (samples.len() as i64 + 1))
        );
        assert_eq!(
            driver.write_log(),
            vec![
                (tags.controller_mode.clone().unwrap(), "MAN".to_string()),
                (tags.manipulated_variable.clone(), "45".to_string()),
                (tags.controller_mode.clone().unwrap(), "AUT".to_string()),
                (tags.setpoint_variable.clone().unwrap(), "55".to_string(),),
            ]
        );
    }

    #[tokio::test]
    async fn settling_skips_when_completion_or_timing_state_is_unavailable() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let template = yokogawa_template();
        let tags = yokogawa_tags();
        let driver = yokogawa_driver_auto();
        let initial = read_initial_values(&driver, &tags, &template, true)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let effective_timing = test_effective_timing(&args);

        let completion_missing = attempt_restore_with_actuation_with_timing(
            &pool,
            0,
            &args,
            effective_timing,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            true,
            &mut CtrlC::never(),
            &mut None,
            None,
            Some(4_500.0),
            Some(&mut timing_for_args(&args)),
        )
        .await;
        assert!(matches!(completion_missing, RestoreAttempt::Confirmed));

        let mut completion = completed_poll_for_settling(Utc::now(), 10);
        let timing_missing = attempt_restore_with_actuation_with_timing(
            &pool,
            0,
            &args,
            effective_timing,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            true,
            &mut CtrlC::never(),
            &mut None,
            Some(&mut completion),
            Some(4_500.0),
            None,
        )
        .await;
        assert!(matches!(timing_missing, RestoreAttempt::Confirmed));
        assert_eq!(
            driver.value_of(tags.controller_mode.as_deref().unwrap()),
            Some("AUT".to_string())
        );
    }

    #[tokio::test]
    async fn settling_returns_without_polling_when_deadline_is_already_expired() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let tags = yokogawa_tags();
        let driver = yokogawa_driver_auto();
        let args = {
            let mut args = fast_simulator_args();
            args.driver = DriverKind::Opcda;
            args
        };
        let start = Utc::now();
        let mut completion = completed_poll_for_settling(start, 20);
        let original_tick = completion.next_tick_index;
        let mut timing = timing_for_args(&args);

        run_auto_release_settling(
            &pool,
            0,
            test_effective_timing(&args),
            &tags,
            &driver,
            &mut CtrlC::never(),
            true,
            &mut completion,
            &mut timing,
            Duration::ZERO,
            Instant::now() + Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(completion.next_tick_index, original_tick);
        assert!(driver.read_batches().is_empty());
    }

    #[tokio::test]
    async fn settling_fails_when_the_restore_deadline_expires() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let tags = yokogawa_tags();
        let driver = yokogawa_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let mut completion = completed_poll_for_settling(Utc::now(), 20);
        let mut timing = timing_for_args(&args);
        let restore_deadline = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("the test clock must be later than one second after its origin");

        let error = run_auto_release_settling(
            &pool,
            0,
            test_effective_timing(&args),
            &tags,
            &driver,
            &mut CtrlC::never(),
            true,
            &mut completion,
            &mut timing,
            Duration::from_secs(1),
            restore_deadline,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("restore_timeout_secs"));
        assert!(driver.read_batches().is_empty());
    }

    #[tokio::test]
    async fn settling_breaks_after_a_poll_wakes_past_its_deadline() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_yokogawa_test_run(&pool, "yokogawa-settling-deadline").await;
        let driver = yokogawa_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.poll_interval_ms = 1_000;
        let mut completion = completed_poll_for_settling(Utc::now(), 20);
        let mut timing = timing_for_args(&args);

        run_auto_release_settling(
            &pool,
            run_id,
            test_effective_timing(&args),
            &tags,
            &driver,
            &mut CtrlC::never(),
            true,
            &mut completion,
            &mut timing,
            Duration::from_millis(500),
            Instant::now() + Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(completion.next_tick_index, 21);
        assert_eq!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn settling_cancels_before_the_first_poll_when_ctrl_c_is_already_signalled() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let tags = yokogawa_tags();
        let driver = yokogawa_driver_auto();
        let args = {
            let mut args = fast_simulator_args();
            args.driver = DriverKind::Opcda;
            args
        };
        let mut completion = completed_poll_for_settling(Utc::now(), 20);
        let mut timing = timing_for_args(&args);
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();

        let error = run_auto_release_settling(
            &pool,
            0,
            test_effective_timing(&args),
            &tags,
            &driver,
            &mut ctrl_c,
            true,
            &mut completion,
            &mut timing,
            Duration::from_secs(1),
            Instant::now() + Duration::from_secs(30),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("during post-MRFT"));
        assert!(driver.read_batches().is_empty());
    }

    #[tokio::test]
    async fn settling_cancels_a_delayed_pv_read_on_ctrl_c() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let tags = yokogawa_tags();
        let driver =
            yokogawa_driver_auto().delaying_read(&tags.process_variable, Duration::from_secs(1));
        let args = {
            let mut args = fast_simulator_args();
            args.driver = DriverKind::Opcda;
            args
        };
        let mut completion = completed_poll_for_settling(Utc::now(), 20);
        let mut timing = timing_for_args(&args);
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = tx.send(1);
        });

        let error = run_auto_release_settling(
            &pool,
            0,
            test_effective_timing(&args),
            &tags,
            &driver,
            &mut ctrl_c,
            true,
            &mut completion,
            &mut timing,
            Duration::from_secs(2),
            Instant::now() + Duration::from_secs(30),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("while reading"));
        assert!(driver.delayed_read_was_cancelled(&tags.process_variable));
    }

    #[tokio::test]
    async fn settling_times_out_a_delayed_pv_read() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let tags = yokogawa_tags();
        let driver =
            yokogawa_driver_auto().delaying_read(&tags.process_variable, Duration::from_secs(2));
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.op_timeout_secs = 1;
        let mut completion = completed_poll_for_settling(Utc::now(), 20);
        let mut timing = timing_for_args(&args);

        let error = run_auto_release_settling(
            &pool,
            0,
            test_effective_timing(&args),
            &tags,
            &driver,
            &mut CtrlC::never(),
            true,
            &mut completion,
            &mut timing,
            Duration::from_secs(2),
            Instant::now() + Duration::from_secs(30),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("op_timeout_secs"));
        assert!(driver.delayed_read_was_cancelled(&tags.process_variable));
    }

    #[tokio::test]
    async fn settling_poor_quality_persists_the_triggering_sample_and_stays_manual() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_yokogawa_test_run(&pool, "yokogawa-settling-poor-quality").await;
        let driver = yokogawa_driver_auto().degrade_quality_after(
            &tags.process_variable,
            0,
            bhtune_driver::Quality::Bad,
        );
        let args = {
            let mut args = fast_simulator_args();
            args.driver = DriverKind::Opcda;
            args
        };
        let mut completion = completed_poll_for_settling(Utc::now(), 20);
        let original_state = completion.state;
        let mut timing = timing_for_args(&args);
        let error = run_auto_release_settling(
            &pool,
            run_id,
            test_effective_timing(&args),
            &tags,
            &driver,
            &mut CtrlC::never(),
            true,
            &mut completion,
            &mut timing,
            // Keep the injected quality failure immediate, but give slower Windows
            // runners time to reach the first scheduled settling poll.
            Duration::from_secs(1),
            Instant::now() + Duration::from_secs(30),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("Bad"));
        assert_eq!(completion.state, original_state);
        assert_eq!(completion.next_tick_index, 20);
        assert!(driver.write_log().is_empty());
        let samples = TuneSampleRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].tick_index, 20);
        assert_eq!(samples[0].pv_quality, SampleQuality::Bad);
    }

    #[tokio::test]
    async fn settling_failure_suppresses_auto_release_and_leaves_the_loop_manual() {
        let pool = seeded_pool().await;
        let (run_id, _config, template, tags) =
            start_yokogawa_test_run(&pool, "yokogawa-settling-read-error").await;
        let driver = yokogawa_driver_auto()
            .erroring_read_after(&tags.process_variable, 1)
            .erroring_write(tags.setpoint_variable.as_deref().unwrap());
        let initial = read_initial_values(&driver, &tags, &template, true)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let args = {
            let mut args = fast_simulator_args();
            args.driver = DriverKind::Opcda;
            args
        };
        let mut completion = completed_poll_for_settling(Utc::now(), 30);
        let mut timing = timing_for_args(&args);
        let mut ctrl_c = CtrlC::never();
        let result = attempt_restore_with_actuation_with_timing(
            &pool,
            run_id,
            &args,
            test_effective_timing(&args),
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            true,
            &mut ctrl_c,
            &mut None,
            Some(&mut completion),
            Some(4_500.0),
            Some(&mut timing),
        )
        .await;

        assert!(matches!(result, RestoreAttempt::Incomplete { .. }));
        assert_eq!(
            driver.value_of(tags.controller_mode.as_deref().unwrap()),
            Some("MAN".to_string())
        );
        assert!(
            !driver.write_log().iter().any(|(tag, value)| tag
                == tags.controller_mode.as_deref().unwrap()
                && value == "AUT")
        );
    }

    #[tokio::test]
    async fn settling_failure_with_confirmed_follow_up_restore_stays_manual() {
        let pool = seeded_pool().await;
        let (run_id, _config, template, tags) =
            start_yokogawa_test_run(&pool, "yokogawa-settling-confirmed-restore").await;
        let driver = yokogawa_driver_auto().degrade_quality_after(
            &tags.process_variable,
            1,
            bhtune_driver::Quality::Bad,
        );
        let initial = read_initial_values(&driver, &tags, &template, true)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let mut completion = completed_poll_for_settling(Utc::now(), 30);
        let mut timing = timing_for_args(&args);
        let result = attempt_restore_with_actuation_with_timing(
            &pool,
            run_id,
            &args,
            test_effective_timing(&args),
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            true,
            &mut CtrlC::never(),
            &mut None,
            Some(&mut completion),
            Some(4_500.0),
            Some(&mut timing),
        )
        .await;

        assert!(matches!(result, RestoreAttempt::Incomplete { .. }));
        assert_eq!(
            driver.value_of(tags.controller_mode.as_deref().unwrap()),
            Some("MAN".to_string())
        );
        assert!(
            !driver.write_log().iter().any(|(tag, value)| tag
                == tags.controller_mode.as_deref().unwrap()
                && value == "AUT")
        );
    }

    #[tokio::test]
    async fn a_full_simulator_tune_completes_and_persists_results() {
        let pool = seeded_pool().await;
        run(&pool, fast_simulator_args(), &test_config())
            .await
            .unwrap();

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Completed);
        assert_eq!(runs[0].loop_name, "ignored-for-simulator");
        assert_eq!(runs[0].notes.as_deref(), Some("test note"));
        assert!(runs[0].initial_readings.is_some());
        let timing = runs[0]
            .timing_metrics
            .expect("completed simulator run should record timing diagnostics");
        assert_eq!(timing.basis, TimingBasis::SimulatedFixedStep);
        assert_eq!(timing.requested_interval_ms, 5);
        assert!(timing.sample_gap_count > 0);
        assert_eq!(timing.mean_sample_gap_ms, Some(5.0));
        assert_eq!(timing.max_sample_gap_ms, Some(5.0));
        assert_eq!(timing.missed_poll_opportunity_count, 0);
        assert!(
            timing
                .measured_oscillation_period_ms
                .is_some_and(|period| period > 0.0)
        );
        assert!(
            timing
                .approximate_samples_per_period
                .is_some_and(|samples| samples > 1.0)
        );

        // A simulator run has no OPC DA connection at all -- `db-run-request-snapshot`
        // requires both to be `None` here regardless of whatever `--bridge-host` default
        // `prepare` resolved internally, since the driver never actually contacted a
        // gateway.
        assert_eq!(runs[0].opc_server, None);
        assert_eq!(runs[0].bridge_host, None);

        // The submitted request is snapshotted verbatim (pre-resolution), so a field the
        // test left unset (`server`) stays absent/null rather than showing a resolved
        // default.
        let request: serde_json::Value = serde_json::from_str(&runs[0].request_json).unwrap();
        assert_eq!(request["tagname"], "ignored-for-simulator");
        assert_eq!(request["driver"], "simulator");
        assert_eq!(request["server"], serde_json::Value::Null);
        assert_eq!(request["notes"], "test note");
        for timing_field in [
            "mrft_delay",
            "mrft_delay_secs",
            "poll_interval_ms",
            "timeout_secs",
            "op_timeout_secs",
            "restore_timeout_secs",
        ] {
            assert!(
                request.get(timing_field).is_none(),
                "{timing_field} must not be part of the per-run request snapshot"
            );
        }
        assert_eq!(
            runs[0].effective_tuning,
            Some(EffectiveTuning {
                mrft_delay_secs: 0,
                poll_interval_ms: 5,
                timeout_secs: 5,
                op_timeout_secs: 30,
                restore_timeout_secs: 30,
            })
        );
        assert!(
            TuneMvActuationRow::list_for_run(&pool, runs[0].id)
                .await
                .unwrap()
                .is_empty(),
            "simulator runs must not create OPC DA MV actuation audit rows"
        );

        let results = TuneResultRow::list_for_run(&pool, runs[0].id)
            .await
            .unwrap();
        assert_eq!(results.len(), 3);

        let samples = TuneSampleRow::list_for_run(&pool, runs[0].id)
            .await
            .unwrap();
        assert!(!samples.is_empty());

        // The simulator driver has no PID constant tags, so write-back must have been
        // skipped entirely rather than hanging on stdin.
        let writes = TuneWriteRow::list_for_run(&pool, runs[0].id).await.unwrap();
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn prepared_tune_run_id_matches_the_persisted_run_id() {
        let pool = seeded_pool().await;
        let first = prepare(&pool, fast_simulator_args(), &test_config())
            .await
            .unwrap();
        let second = prepare(&pool, fast_simulator_args(), &test_config())
            .await
            .unwrap();

        assert!(first.run_id() > 0);
        assert_eq!(second.run_id(), first.run_id() + 1);
    }

    #[tokio::test]
    async fn prepare_rejects_invalid_tag_overrides_before_creating_a_run() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.tag_overrides = Some(TagOverrides {
            process_variable: Some("bad\0tag".to_string()),
            ..TagOverrides::default()
        });

        let result = prepare(&pool, args, &test_config()).await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("process_variable"));
        assert!(
            TuneRunRow::list(
                &pool,
                &bhtune_db::models::TuneRunFilter::default(),
                bhtune_db::models::Pagination::first(10),
            )
            .await
            .unwrap()
            .is_empty()
        );
    }

    #[tokio::test]
    async fn prepare_refuses_an_incompatible_gateway_before_creating_a_run() {
        use crate::test_support::{MockBridgeService, start_mock_server};
        use opcda_bridge_proto::bridge::{
            GetGatewayInfoResponse, ProtocolFeature, ProtocolFeatureKind,
        };

        let (host, server) = start_mock_server(MockBridgeService {
            gateway_info_response: GetGatewayInfoResponse {
                application_version: "0.5.9".to_string(),
                compatibility_schema_version: 1,
                features: vec![ProtocolFeature {
                    kind: ProtocolFeatureKind::Core as i32,
                    min_version: 9,
                    max_version: 9,
                }],
            },
            ..Default::default()
        })
        .await;

        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.tagname = "Unit1.LIC101.PV".to_string();
        args.bridge_host = Some(host);
        args.server = Some("Sim.Server".to_string());

        let result = prepare(&pool, args, &test_config()).await;
        assert!(result.is_err(), "an incompatible core must refuse prepare");
        let error = result.err().unwrap();
        assert!(error.to_string().contains("incompatible"));
        assert!(
            TuneRunRow::list(
                &pool,
                &bhtune_db::models::TuneRunFilter::default(),
                bhtune_db::models::Pagination::first(10),
            )
            .await
            .unwrap()
            .is_empty()
        );
        server.shutdown().await;
    }

    #[tokio::test]
    async fn prepare_records_a_partial_gateway_snapshot() {
        use crate::test_support::{MockBridgeService, start_mock_server};
        use opcda_bridge_proto::bridge::{
            GetGatewayInfoResponse, ProtocolFeature, ProtocolFeatureKind,
        };

        let (host, server) = start_mock_server(MockBridgeService {
            gateway_info_response: GetGatewayInfoResponse {
                application_version: "0.5.9".to_string(),
                compatibility_schema_version: 1,
                features: vec![ProtocolFeature {
                    kind: ProtocolFeatureKind::Core as i32,
                    min_version: 1,
                    max_version: 1,
                }],
            },
            ..Default::default()
        })
        .await;

        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.tagname = "Unit1.LIC101.PV".to_string();
        args.bridge_host = Some(host);
        args.server = Some("Sim.Server".to_string());

        let prepared = prepare(&pool, args, &test_config()).await.unwrap();
        let run = TuneRunRow::get(&pool, prepared.run_id())
            .await
            .unwrap()
            .unwrap();
        let snapshot = run
            .gateway_compatibility_json
            .expect("a partial gateway check must be stored before the run starts");
        assert!(snapshot.contains("\"status\":\"partial\""));
        assert!(snapshot.contains("0.5.9"));
        server.shutdown().await;
    }

    #[tokio::test]
    async fn prepare_owned_rejects_a_non_simulator_before_creating_a_run() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;

        let error = prepare_owned(&pool, args, &test_config(), 1)
            .await
            .err()
            .expect("demo preparation must reject a non-simulator driver");

        assert!(
            error
                .to_string()
                .contains("demo sessions may only start simulator runs")
        );
        assert!(
            TuneRunRow::list(
                &pool,
                &bhtune_db::models::TuneRunFilter::default(),
                bhtune_db::models::Pagination::first(10),
            )
            .await
            .unwrap()
            .is_empty(),
            "the demo driver guard must run before the tune_runs insert"
        );
    }

    #[tokio::test]
    async fn prepare_owned_accepts_a_simulator_and_creates_a_demo_owned_run() {
        let pool = seeded_pool().await;
        let now = Utc::now();
        let session = bhtune_db::models::DemoSessionRow::create(
            &pool,
            &"0".repeat(64),
            now,
            now + chrono::Duration::hours(1),
        )
        .await
        .unwrap();

        let prepared = prepare_owned(&pool, fast_simulator_args(), &test_config(), session.id)
            .await
            .unwrap();

        let run = TuneRunRow::get(&pool, prepared.run_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.demo_session_id, Some(session.id));
        assert_eq!(run.outcome, bhtune_db::models::TuneOutcome::Running);
    }

    #[tokio::test]
    async fn drive_completes_a_prepared_simulator_run() {
        let pool = seeded_pool().await;
        let prepared = prepare(&pool, fast_simulator_args(), &test_config())
            .await
            .unwrap();
        let run_id = prepared.run_id();

        let outcome = drive(&pool, prepared, &mut CtrlC::never()).await.unwrap();

        assert_eq!(outcome, TuneOutcome::Completed);
        assert_eq!(
            TuneRunRow::get(&pool, run_id)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            bhtune_db::models::TuneOutcome::Completed
        );
    }

    #[tokio::test]
    async fn drive_marks_a_prepared_run_failed_when_execution_errors() {
        let pool = seeded_pool().await;
        let mut prepared = prepare(&pool, fast_simulator_args(), &test_config())
            .await
            .unwrap();
        let run_id = prepared.run_id();
        prepared.driver = Box::new(MockDriver::default().empty_read(SIMULATOR_PV_TAG));

        let err = drive(&pool, prepared, &mut CtrlC::never())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("no value"));
        let run = TuneRunRow::get(&pool, run_id).await.unwrap().unwrap();
        assert_eq!(run.outcome, bhtune_db::models::TuneOutcome::Failed);
        assert!(run.failure_reason.is_some());
    }

    /// Every range/direction override is CLI-supplied below, so `read_initial_values` never
    /// reads them from the driver; the mock only ever needs to answer for `pv_ini`/`mv_ini`
    /// and (for the Yokogawa template) has no mode/mode-attribute tags to read either. Fails
    /// starting at the 2nd `read` RPC call — after the one batched setup read — so the
    /// failure always lands on the first polling tick's PV read, deep inside
    /// `run_polling_loop`, not during setup. Also covers `db-run-request-snapshot`'s opcda
    /// path: `prepare` records the resolved connection and the request snapshot before the
    /// polling loop ever runs, so both must already be persisted on the row even though this
    /// run goes on to fail.
    #[tokio::test]
    async fn run_with_opcda_driver_fails_mid_poll_and_marks_the_run_failed() {
        use crate::test_support::{MockBridgeService, start_mock_server};
        use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue, WriteResponse};

        let (host, server) = start_mock_server(
            MockBridgeService {
                read_response: ReadResponse {
                    values: vec![ProtoTagValue {
                        tag_id: "ignored".to_string(),
                        value: "50".to_string(),
                        quality: "Good".to_string(),
                        timestamp: "2024-01-15 10:23:45".to_string(),
                    }],
                },
                write_response: WriteResponse {
                    tag_id: "ignored".to_string(),
                    success: true,
                    error: None,
                },
                ..Default::default()
            }
            .failing_read_from_call(2),
        )
        .await;

        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.tagname = "Unit1.LIC101.PV".to_string();
        args.bridge_host = Some(host.clone());
        args.server = Some("MockServer".to_string());

        let err = run(&pool, args, &test_config()).await.unwrap_err();
        assert!(err.to_string().contains("driver operation failed"));

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Failed);
        assert_eq!(runs[0].driver, bhtune_db::models::TuneDriver::Opcda);
        assert!(
            runs[0]
                .failure_reason
                .as_deref()
                .unwrap()
                .contains("driver operation failed")
        );

        // `record_connection` runs inside `prepare`, before the polling loop that goes on
        // to fail -- so the resolved connection and the submitted request must already be
        // persisted even though the run itself never completes (`db-run-request-snapshot`).
        assert_eq!(runs[0].opc_server.as_deref(), Some("MockServer"));
        assert_eq!(runs[0].bridge_host.as_deref(), Some(host.as_str()));
        let request: serde_json::Value = serde_json::from_str(&runs[0].request_json).unwrap();
        assert_eq!(request["tagname"], "Unit1.LIC101.PV");
        assert_eq!(request["driver"], "opcda");
        assert_eq!(request["server"], "MockServer");

        server.shutdown().await;
    }

    /// Proves `run()` actually resolves `bridge_host`/`server` from `app_config` (not just
    /// from `TuneRequest`) by leaving both CLI-facing fields unset and supplying them only via
    /// the config -- if resolution didn't happen, `driver::build` would either fail fast
    /// with "no OPC server specified" (server never resolved) or try to dial
    /// `DEFAULT_BRIDGE_HOST` instead of the mock (bridge_host never resolved), producing a
    /// different failure than the one asserted below. The mock is configured to fail
    /// starting on its very first `read` call so this stays a fast, deterministic setup
    /// failure -- there is no wall-clock timeout in `run_polling_loop` yet (that lands in
    /// `cli-safety`), so a config-resolution bug that instead let the run reach a real
    /// polling loop against a frozen PV value would hang this test forever rather than
    /// fail cleanly.
    #[tokio::test]
    async fn run_resolves_bridge_host_and_server_from_config_when_cli_flags_are_unset() {
        use crate::test_support::{MockBridgeService, start_mock_server};
        use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue};

        let (host, server) = start_mock_server(
            MockBridgeService {
                read_response: ReadResponse {
                    values: vec![ProtoTagValue {
                        tag_id: "ignored".to_string(),
                        value: "50".to_string(),
                        quality: "Good".to_string(),
                        timestamp: "2024-01-15 10:23:45".to_string(),
                    }],
                },
                ..Default::default()
            }
            .failing_read_from_call(1),
        )
        .await;

        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.tagname = "Unit1.LIC101.PV".to_string();
        args.bridge_host = None;
        args.server = None;

        let app_config = crate::config::BhtuneConfig {
            bridge_host: Some(host),
            server: Some("MockServer".to_string()),
            ..Default::default()
        };

        // A "driver operation failed" error (rather than "no OPC server specified" or a
        // connection error against the unresolved default host) proves setup got as far as
        // issuing a real RPC against the config-resolved mock server.
        let err = run(&pool, args, &app_config).await.unwrap_err();
        assert!(err.to_string().contains("driver operation failed"));

        server.shutdown().await;
    }

    #[tokio::test]
    async fn run_errors_when_opcda_server_is_unset_in_both_cli_and_config() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.server = None;

        let err = run(&pool, args, &test_config()).await.unwrap_err();
        assert!(err.to_string().contains("no OPC server specified"));
    }

    /// `[tuning].mrft_delay_secs` is whole seconds (the smallest non-zero value costs ~1s of logical
    /// simulator time both before switching and after completion). Fixed-step timestamps make
    /// the result deterministic, but the polling interval still paces those ticks in real time;
    /// the SQLite-backed test cannot use Tokio's paused clock without also expiring sqlx's own
    /// pool timers.
    #[tokio::test]
    async fn mrft_delay_pads_the_run_with_extra_recorded_samples() {
        let pool = seeded_pool().await;
        let args = fast_simulator_args();
        // This test intentionally consumes about two seconds before ordinary MRFT work. Keep
        // its safety budget independent of a loaded CI host while retaining the real timeout.
        let mut config = test_config();
        config.tuning.mrft_delay_secs = Some(1);
        config.tuning.timeout_secs = Some(30);
        run(&pool, args, &config).await.unwrap();

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Completed);

        // ~1s of pre-test padding plus ~1s of post-test padding at a 5ms poll interval is on
        // the order of 400 padding ticks alone, dwarfing the handful of ticks the actual
        // (near-instant) MRFT switching test itself takes -- so a generous lower bound
        // safely distinguishes "padding samples were recorded" from "they weren't".
        let samples = TuneSampleRow::list_for_run(&pool, runs[0].id)
            .await
            .unwrap();
        assert!(samples.len() > 100);
    }

    #[tokio::test]
    async fn mrft_delay_keeps_the_engine_idle_during_pre_test_padding() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let mut args = fast_simulator_args();
        args.mrft_delay = 1;
        args.cycles_count = Some(1_000);
        let config = build_loop_config(&args).unwrap();
        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "pre-delay",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();
        let driver = honeywell_driver_auto();
        let initial = sample_initial_state();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(1);
        });

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut ctrl_c,
            &mut MutationGuard::default(),
            true,
            &mut timing,
            &mut None,
            build_loop_config(&args).unwrap(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::UserInterrupt)
        ));
        assert!(
            !TuneSampleRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            driver.write_log().is_empty(),
            "MRFT writes must not occur during pre-test padding"
        );
    }

    #[tokio::test]
    async fn unknown_template_is_a_clean_error() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.template = "Does Not Exist".to_string();
        let err = run(&pool, args, &test_config()).await.unwrap_err();
        assert!(err.to_string().contains("Does Not Exist"));
    }

    #[test]
    fn build_loop_config_rejects_pid_for_a_non_temperature_process_type() {
        let mut args = fast_simulator_args();
        args.controller_type = ControllerType::Pid;
        args.process_type = ProcessType::Flow;
        let err = build_loop_config(&args).unwrap_err();
        assert!(err.to_string().contains("Pid"));
    }

    #[test]
    fn build_loop_config_uses_process_type_defaults_when_unset() {
        let mut args = fast_simulator_args();
        args.cycles_skip = None;
        args.cycles_count = None;
        args.noise_protection_secs = None;
        let config = build_loop_config(&args).unwrap();
        assert_eq!(
            config.num_cycles_skip,
            ProcessType::Flow.default_cycles_skip()
        );
        assert_eq!(
            config.num_cycles_count,
            ProcessType::Flow.default_cycles_test()
        );
        assert_eq!(
            config.noise_protection_secs,
            ProcessType::Flow.default_noise_protection_secs()
        );
    }

    #[test]
    fn build_loop_config_rejects_an_out_of_range_relay_amp_before_any_driver_or_db_io() {
        // Mirrors the `--write-pid`-requires-`--yes` fail-fast precedent: a bad
        // `--relay-amp` (including a leftover legacy debug-code magic number like 2014) must
        // be caught by `LoopConfig::validate` here, at construction time, not discovered
        // later against a live driver.
        let mut args = fast_simulator_args();
        args.relay_amp = 2014.0;
        let err = build_loop_config(&args).unwrap_err();
        assert!(err.to_string().contains("relay amplitude 2014"));
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn build_loop_config_rejects_a_relay_amp_below_the_minimum() {
        let mut args = fast_simulator_args();
        args.relay_amp = 0.0;
        let err = build_loop_config(&args).unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    /// The reproduced panic: `--cycles-count 0` used to reach
    /// `bhtune_core::measure_oscillation`'s internal `assert!` and panic mid-run, after the
    /// loop had already been switched to manual and stroked. `LoopConfig::validate` (called
    /// from `build_loop_config`, before any driver or DB I/O) must reject it cleanly
    /// instead. The clap-level `positive_u32` parser (see `args.rs`) also rejects `0` for
    /// this flag before it ever reaches here, but this test exercises the model-level
    /// guarantee directly, independent of how the value arrived.
    #[test]
    fn build_loop_config_rejects_zero_cycles_count_before_any_driver_or_db_io() {
        let mut args = fast_simulator_args();
        args.cycles_count = Some(0);
        let err = build_loop_config(&args).unwrap_err();
        assert!(err.to_string().contains("at least 1"));
    }

    #[test]
    fn build_loop_tags_simulator_requires_all_overrides() {
        let template = bhtune_core::built_in_templates().remove(0);
        let mut args = fast_simulator_args();
        args.pv_range_high = None;
        let err = build_loop_tags(&args, &template).unwrap_err();
        assert!(err.to_string().contains("--pv-range-high"));
    }

    #[test]
    fn build_loop_tags_simulator_requires_every_override_individually() {
        // Each of the 4 remaining mandatory simulator overrides has its own `ok_or_else`
        // error message; clearing exactly one at a time (rather than just the first, as
        // above) exercises each closure and confirms the flag name in every message.
        type ClearFn = fn(&mut TuneRequest);
        let template = bhtune_core::built_in_templates().remove(0);
        let cases: &[(&str, ClearFn)] = &[
            ("--pv-range-low", |a| a.pv_range_low = None),
            ("--mv-range-high", |a| a.mv_range_high = None),
            ("--mv-range-low", |a| a.mv_range_low = None),
            ("--direction", |a| a.direction = None),
        ];
        for (flag, clear) in cases {
            let mut args = fast_simulator_args();
            clear(&mut args);
            let err = build_loop_tags(&args, &template).unwrap_err();
            assert!(
                err.to_string().contains(flag),
                "expected error for missing {flag}, got: {err}"
            );
        }
    }

    #[test]
    fn build_loop_tags_simulator_uses_fixed_tag_names() {
        let template = bhtune_core::built_in_templates().remove(0);
        let args = fast_simulator_args();
        let tags = build_loop_tags(&args, &template).unwrap();
        assert_eq!(tags.process_variable, SIMULATOR_PV_TAG);
        assert_eq!(tags.manipulated_variable, SIMULATOR_MV_TAG);
        assert!(tags.controller_mode.is_none());
        assert!(tags.proportional_constant.is_none());
    }

    #[test]
    fn build_loop_tags_opcda_derives_and_applies_overrides() {
        let template = bhtune_core::built_in_templates().remove(0);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.tagname = "Unit1.LIC101.PV".to_string();
        args.direction = Some(ControllerDirection::Direct);
        args.tag_overrides = Some(TagOverrides {
            manipulated_variable: Some("Unit1.LIC101.PY".to_string()),
            proportional_constant: Some("Unit1.LIC101.PB".to_string()),
            ..TagOverrides::default()
        });
        let tags = build_loop_tags(&args, &template).unwrap();
        assert!(tags.process_variable.starts_with("Unit1.LIC101"));
        assert_eq!(tags.manipulated_variable, "Unit1.LIC101.PY");
        assert_eq!(
            tags.proportional_constant,
            Some("Unit1.LIC101.PB".to_string())
        );
        assert_eq!(
            tags.controller_direction,
            TagOrValue::Value(ControllerDirection::Direct)
        );
        assert_eq!(tags.upper_pv_range, TagOrValue::Value(100.0));
    }

    #[tokio::test]
    async fn a_ctrl_c_style_abort_restores_and_records_aborted() {
        // Exercises the DB/restore shape of an abort directly -- calling `restore` +
        // `TuneRunRow::abort` exactly as the real Ctrl+C path does -- rather than going
        // through a full `run_polling_loop`/`run_with_ctrl_c` cycle. `CtrlC::test_pair()` can
        // and does fake a real signal end to end elsewhere (see
        // `a_stalled_mv_write_during_a_tick_is_cancelled_and_still_records_the_sample` and
        // `run_with_ctrl_c_aborts_the_run_when_signalled_during_the_poll`, both further down
        // in this module); this test is kept as a narrower, cheaper check of just the
        // resulting database row shape.
        let pool = seeded_pool().await;
        let template = bhtune_core::built_in_templates().remove(0);
        let args = fast_simulator_args();
        let config = build_loop_config(&args).unwrap();
        let tags = build_loop_tags(&args, &template).unwrap();
        let driver = crate::driver::build(&args).await.unwrap();

        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "abort-test",
            TuneDriver::Simulator,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();

        let initial = read_initial_values(driver.as_ref(), &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(driver.as_ref(), &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let report = restore(driver.as_ref(), &tags, &template, &initial, &guard).await;
        assert!(report.all_succeeded());
        TuneRunRow::abort(&pool, run.id, Utc::now()).await.unwrap();

        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stored.outcome, bhtune_db::models::TuneOutcome::Aborted);
    }

    /// Unlike Ctrl+C/timeout (which need a real signal or elapsed wall-clock time and so are
    /// only exercised indirectly, see the test above), finding 5's `PoorQuality` abort is
    /// purely data-driven -- the driver just has to report a non-`Good` reading -- so this
    /// test drives `run_polling_loop` for real and checks its returned `PollOutcome`
    /// directly, then confirms `restore` leaves the loop in the same consistent state
    /// `execute`'s `Aborted` branch would.
    #[tokio::test]
    async fn poor_quality_pv_during_polling_aborts_records_the_sample_and_restores() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Bad);
        let args = fast_simulator_args();
        let config = build_loop_config(&args).unwrap();

        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "poor-quality-poll",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();

        // Built directly (bypassing `read_initial_values`) using `honeywell_driver_auto()`'s
        // own fixture values (see `sample_initial_state`, defined below), because
        // `read_initial_values` itself enforces finding 5 on this very same PV tag and would
        // hard-fail before ever reaching the polling loop this test targets.
        let initial = sample_initial_state();
        let beta = lookup(
            config.process_type,
            config.controller_type,
            ResponseLevel::Aggressive,
        )
        .beta;
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut None,
            build_loop_config(&args).unwrap(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::PoorQuality { ref tag, quality })
                if tag == &tags.process_variable && quality == bhtune_driver::Quality::Bad
        ));

        // The triggering sample was recorded (with its real, poor quality) before the abort
        // -- finding 5 explicitly requires the operator can see exactly what was seen when
        // the run gave up, not just that it gave up.
        let samples = TuneSampleRow::list_for_run(&pool, run.id).await.unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].pv_quality, SampleQuality::Bad);

        // Same DB-shape check the Ctrl+C-style test above uses: a `PoorQuality` abort must
        // be indistinguishable in its cleanup guarantees from any other abort reason. Uses a
        // bare default `MutationGuard` -- `transition_to_manual` was never called on this
        // path -- which is exactly what proves `restore`'s MV-revert step is unconditional
        // and never gated by the guard: it still succeeds and writes the MV back even though
        // `guard.mv_written` is `false`, while the guard-gated mode/setpoint/mode-attribute
        // steps correctly report `NotNeeded` since nothing was ever attempted for them.
        let guard = MutationGuard::default();
        let report = restore(&driver, &tags, &template, &initial, &guard).await;
        assert_eq!(report.mv, RestoreStepOutcome::Succeeded);
        assert_eq!(report.mode, RestoreStepOutcome::NotNeeded);
        assert_eq!(report.setpoint, RestoreStepOutcome::NotNeeded);
        assert_eq!(report.mode_attribute, RestoreStepOutcome::NotNeeded);
        TuneRunRow::abort(&pool, run.id, Utc::now()).await.unwrap();

        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stored.outcome, bhtune_db::models::TuneOutcome::Aborted);

        // The MV was actually written back to its initial value during restore.
        assert!(
            driver
                .write_log()
                .iter()
                .any(|(tag, value)| tag == &tags.manipulated_variable && value == "45")
        );
    }

    #[tokio::test]
    async fn live_sample_timestamp_includes_driver_read_delay() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .delaying_read(&tags.process_variable, Duration::from_millis(50))
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Bad);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let config = build_loop_config(&args).unwrap();
        let started_at = DateTime::UNIX_EPOCH;
        let time_anchor = time_anchor_at(started_at);
        let run = TuneRunRow::start(
            &pool,
            None,
            "delayed-live-read",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();
        let initial = sample_initial_state();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor,
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut None,
            build_loop_config(&args).unwrap(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::PoorQuality { .. })
        ));
        let samples = TuneSampleRow::list_for_run(&pool, run.id).await.unwrap();
        assert_eq!(samples.len(), 1);
        assert!(
            samples[0].sample.time - started_at >= chrono::Duration::milliseconds(45),
            "live sample timestamp should include the driver's 50 ms read delay"
        );
    }

    // --- safety-cancellation: `[tuning].op_timeout_secs` / mid-tick Ctrl+C via `bounded_driver_call`

    /// Proves the wiring, not just the mechanism (see the dedicated `bounded_driver_call`
    /// unit tests below for that): a PV read that never resolves at all -- the gateway is
    /// down, DCOM is wedged, the network is black-holed -- must abort the run via
    /// `[tuning].op_timeout_secs` rather than hang the poll loop forever, exactly the scenario
    /// finding 2 of the live-plant safety review names as the most severe of the three
    /// consequences of the pre-`safety-cancellation` design. Real (unpaused) time, paying a
    /// real ~1s wall-clock cost: `start_paused` interacts badly with the real sqlx
    /// `SqlitePool` this test also creates (it fast-forwards the pool's own internal
    /// connection-acquire timeout too), matching the documented precedent in
    /// `run_times_out_and_aborts_when_timeout_secs_elapses_before_completion` below.
    #[tokio::test]
    async fn a_stalled_pv_read_aborts_the_poll_loop_via_op_timeout_secs() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_read(&tags.process_variable);
        let mut args = fast_simulator_args();
        args.op_timeout_secs = 1;
        let config = build_loop_config(&args).unwrap();

        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "stalled-pv-read",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();

        let initial = sample_initial_state();
        let beta = lookup(
            config.process_type,
            config.controller_type,
            ResponseLevel::Aggressive,
        )
        .beta;
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut None,
            build_loop_config(&args).unwrap(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::OperationTimedOut {
                ref tag,
                op_timeout_secs,
            }) if tag == &tags.process_variable && op_timeout_secs == 1
        ));

        // Unlike the poor-quality/mid-write-cancellation cases, the PV read itself is what
        // stalled -- there is no valid tick/sample to record for this iteration at all.
        let samples = TuneSampleRow::list_for_run(&pool, run.id).await.unwrap();
        assert!(samples.is_empty());
    }

    /// The write-side counterpart of the read-stall test above, and also the one integration
    /// test exercising a mid-tick Ctrl+C (as opposed to the pre-existing idle-between-ticks
    /// coverage in `a_ctrl_c_style_abort_restores_and_records_aborted`): the PV read for tick
    /// 1 succeeds normally (so a real `tick`/`sample_quality` is in hand), the engine's very
    /// first `step` call always emits `Action::WriteMv` (see `MrftEngine::switch_is_needed`:
    /// `hysteresis` is still zero and `counter_all_switches == 0` on tick 1), and that write
    /// then hangs forever via `hanging_write`. A background task -- standing in for a human
    /// pressing Ctrl+C mid-write, which a unit test can't do with a real signal without
    /// hitting every other concurrently running test (see `tests/ctrlc_abort.rs`'s doc
    /// comment) -- sends the cancellation a short real delay later. Deliberately *not*
    /// `start_paused`: the delay has to be observed as "genuinely still in flight" by a
    /// concurrently running task, which paused virtual time (where nothing advances until
    /// every task is parked) can't model as naturally as a small real sleep.
    #[tokio::test]
    async fn a_stalled_mv_write_during_a_tick_is_cancelled_and_still_records_the_sample() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_write(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        // Large enough that the op-timeout branch never wins the race against the
        // background task's much shorter real delay below.
        args.op_timeout_secs = 30;
        let config = build_loop_config(&args).unwrap();

        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "stalled-mv-write",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();

        let initial = sample_initial_state();
        let beta = lookup(
            config.process_type,
            config.controller_type,
            ResponseLevel::Aggressive,
        )
        .beta;
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );

        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(1);
        });

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut ctrl_c,
            &mut MutationGuard::default(),
            true,
            &mut timing,
            &mut None,
            build_loop_config(&args).unwrap(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::UserInterrupt)
        ));

        // Unlike the read-stall case, a valid sample for this tick was already in hand
        // before the write was attempted, so it must still be recorded before aborting.
        let samples = TuneSampleRow::list_for_run(&pool, run.id).await.unwrap();
        assert_eq!(samples.len(), 1);

        // The hung write never actually completed -- the loop was left at its relay-test MV,
        // matching `warn_restore_incomplete`'s premise for why the operator warning names
        // the MV specifically.
        assert!(driver.write_log().is_empty());
    }

    // --- opcda-style mode-transition/restore/write-back coverage ---------------------------
    //
    // The tests above all use the simulator driver, whose `LoopTags` has no
    // setpoint/mode/mode-attribute/PID-constant tags at all (see `build_loop_tags`), so they
    // never exercise `transition_to_manual`/`restore`/`maybe_write_back`'s real opcda-style
    // logic. `MockDriver` below is a minimal in-memory `Driver` double with a configurable
    // tag/value map, used together with the real "Honeywell Experion" built-in template
    // (which has every optional tag suffix configured) to drive that logic directly.

    /// A minimal, fully in-memory [`Driver`] test double with a fixed tag-value map, plus
    /// the ability to inject specific-tag read/write failures. `std::sync::Mutex`, not
    /// `tokio::sync::Mutex` — matching `SimulatorDriver`'s own precedent — since no
    /// `.await` point is ever held across the lock.
    #[derive(Default)]
    struct MockDriver {
        values: std::sync::Mutex<std::collections::HashMap<String, String>>,
        read_sequences:
            std::sync::Mutex<std::collections::HashMap<String, std::collections::VecDeque<String>>>,
        read_batches: std::sync::Mutex<Vec<Vec<String>>>,
        reverse_read_results: bool,
        writes: std::sync::Mutex<Vec<(String, String)>>,
        reject_writes: std::collections::HashSet<String>,
        error_reads: std::collections::HashSet<String>,
        error_writes: std::collections::HashSet<String>,
        empty_reads: std::collections::HashSet<String>,
        /// Tags whose `read`/`write` never resolves (`.await`s `std::future::pending`
        /// forever), simulating a stalled OPC DA call (gateway down, DCOM wedged, network
        /// black-holed) so `[tuning].op_timeout_secs`/Ctrl+C-during-a-tick can actually be exercised.
        /// Finite delays are configured separately below; both delay forms await before any
        /// mutex guard is acquired.
        hang_reads: std::collections::HashSet<String>,
        hang_writes: std::collections::HashSet<String>,
        /// Per-tag finite read latency, used to prove that live monotonic sample timestamps
        /// include the time spent awaiting the driver rather than being captured before it.
        read_delays: std::collections::HashMap<String, Duration>,
        /// Tags whose configured finite read delay was cancelled before it completed.
        cancelled_delayed_reads: std::sync::Mutex<std::collections::HashSet<String>>,
        /// Per-tag finite write latency, used to exercise restore-budget behavior when the
        /// authoritative MV write is accepted near the initial restore deadline.
        write_delays: std::collections::HashMap<String, Duration>,
        /// Tags whose configured finite write delay was cancelled before it completed.
        cancelled_delayed_writes: std::sync::Mutex<std::collections::HashSet<String>>,
        /// Per-tag OPC quality override, defaulting to `Quality::Good` for any tag not
        /// listed -- matching a healthy real driver and letting most tests ignore quality
        /// entirely while a handful exercise finding 5's enforcement via `with_quality`.
        qualities: std::sync::Mutex<std::collections::HashMap<String, bhtune_driver::Quality>>,
        /// Per-tag: reports the tag's ordinarily-configured quality (from `qualities`,
        /// defaulting to `Good`) for the tag's first `usize` reads, then switches to the
        /// paired [`bhtune_driver::Quality`] for every read after that. Lets a test put a
        /// tag's *initial* read (before any mutation is attempted, subject to finding 5 the
        /// same as every other read) in good standing while still forcing quality to
        /// degrade partway through polling -- deterministically, with no reliance on real
        /// elapsed time or a Ctrl+C race, unlike `[tuning].timeout_secs`/
        /// `[tuning].op_timeout_secs`-driven
        /// aborts.
        degrade_quality_after: std::collections::HashMap<String, (usize, bhtune_driver::Quality)>,
        /// Tracks how many times each tag has been read so far, for
        /// `degrade_quality_after`/`erroring_read_after`.
        read_counts: std::sync::Mutex<std::collections::HashMap<String, usize>>,
        /// Per-tag: the tag's first `usize` reads resolve normally, then every read after
        /// that returns a transport-level error -- the same "succeeds at first, degrades
        /// partway through" shape as `degrade_quality_after`, but a hard read error rather
        /// than a quality downgrade, so `safety-writeback-rollback`'s pre-read-succeeds/
        /// verify-readback-errors path can be exercised distinctly from the
        /// verify-readback-reports-poor-quality path.
        error_reads_after: std::collections::HashMap<String, usize>,
        /// Tags whose float writes are silently perturbed by a fixed offset before being
        /// stored, simulating a DCS that clamps/rounds a written value rather than accepting
        /// it exactly -- lets a test exercise `pid_value_within_tolerance`'s rejection path
        /// deterministically.
        write_offsets: std::collections::HashMap<String, f32>,
        /// Per-tag prefix of float writes to perturb before later writes resume normal
        /// behavior, allowing an unconfirmed relay command followed by a healthy restore.
        prefix_write_offsets: std::collections::HashMap<String, (usize, f32)>,
        /// Per-tag: the tag's first `usize` writes are accepted normally, then every write
        /// after that is rejected (mirrors `reject_writes`'s `WriteOutcome::failure`, not a
        /// transport error). Lets a test make a constant's *forward* write succeed while its
        /// later *rollback* write (the same tag, written a second time with the previous
        /// value) is rejected -- exercising `rollback_state = Failed`.
        reject_writes_after: std::collections::HashMap<String, usize>,
        /// Tracks how many times each tag has been written so far, for
        /// `reject_writes_after`.
        write_counts: std::sync::Mutex<std::collections::HashMap<String, usize>>,
    }

    impl MockDriver {
        fn new(values: &[(&str, &str)]) -> MockDriver {
            MockDriver {
                values: std::sync::Mutex::new(
                    values
                        .iter()
                        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                        .collect(),
                ),
                ..Default::default()
            }
        }

        fn rejecting_write(mut self, tag: &str) -> MockDriver {
            self.reject_writes.insert(tag.to_string());
            self
        }

        fn erroring_read(mut self, tag: &str) -> MockDriver {
            self.error_reads.insert(tag.to_string());
            self
        }

        fn erroring_write(mut self, tag: &str) -> MockDriver {
            self.error_writes.insert(tag.to_string());
            self
        }

        fn empty_read(mut self, tag: &str) -> MockDriver {
            self.empty_reads.insert(tag.to_string());
            self
        }

        /// Makes reading `tag` hang forever -- see the `hang_reads` field doc comment.
        fn hanging_read(mut self, tag: &str) -> MockDriver {
            self.hang_reads.insert(tag.to_string());
            self
        }

        /// Makes writing `tag` hang forever -- see the `hang_writes` field doc comment.
        fn hanging_write(mut self, tag: &str) -> MockDriver {
            self.hang_writes.insert(tag.to_string());
            self
        }

        fn delaying_read(mut self, tag: &str, delay: Duration) -> MockDriver {
            self.read_delays.insert(tag.to_string(), delay);
            self
        }

        fn delaying_write(mut self, tag: &str, delay: Duration) -> MockDriver {
            self.write_delays.insert(tag.to_string(), delay);
            self
        }

        /// Returns multi-tag reads in reverse order, proving callers map responses by tag
        /// rather than relying on the driver's request-order convention.
        fn reversing_read_results(mut self) -> MockDriver {
            self.reverse_read_results = true;
            self
        }

        async fn apply_read_delay(&self, tags: &[String]) {
            if let Some(delay) = tags
                .iter()
                .filter_map(|tag| self.read_delays.get(tag))
                .max()
            {
                let delayed_tags = tags
                    .iter()
                    .filter(|tag| self.read_delays.contains_key(*tag))
                    .cloned()
                    .collect();
                let mut observer = DelayedReadObserver {
                    driver: self,
                    tags: delayed_tags,
                    completed: false,
                };
                tokio::time::sleep(*delay).await;
                observer.completed = true;
            }
        }

        async fn apply_write_delay(&self, tag: &str) {
            let Some(delay) = self.write_delays.get(tag).copied() else {
                return;
            };
            let mut observer = DelayedWriteObserver {
                driver: self,
                tag: tag.to_string(),
                completed: false,
            };
            tokio::time::sleep(delay).await;
            observer.completed = true;
        }

        /// Overrides a single tag's fixture value -- e.g. to make an otherwise-valid
        /// baseline driver (like `honeywell_driver_auto()`) report one bad reading.
        fn with_value(self, tag: &str, value: &str) -> MockDriver {
            self.values
                .lock()
                .unwrap()
                .insert(tag.to_string(), value.to_string());
            self
        }

        fn with_read_sequence(self, tag: &str, values: &[&str]) -> MockDriver {
            self.read_sequences.lock().unwrap().insert(
                tag.to_string(),
                values.iter().map(|value| (*value).to_string()).collect(),
            );
            self
        }

        /// Overrides a single tag's reported [`bhtune_driver::Quality`] -- every other tag
        /// keeps reporting `Quality::Good`, matching a healthy real driver.
        fn with_quality(self, tag: &str, quality: bhtune_driver::Quality) -> MockDriver {
            self.qualities
                .lock()
                .unwrap()
                .insert(tag.to_string(), quality);
            self
        }

        /// See the `degrade_quality_after` field doc comment: `tag`'s first `good_reads`
        /// reads keep reporting `Good` (or whatever `with_quality` set), then every read
        /// after that reports `degraded` instead.
        fn degrade_quality_after(
            mut self,
            tag: &str,
            good_reads: usize,
            degraded: bhtune_driver::Quality,
        ) -> MockDriver {
            self.degrade_quality_after
                .insert(tag.to_string(), (good_reads, degraded));
            self
        }

        /// See the `error_reads_after` field doc comment: `tag`'s first `good_reads` reads
        /// succeed normally, then every read after that returns a transport-level error.
        fn erroring_read_after(mut self, tag: &str, good_reads: usize) -> MockDriver {
            self.error_reads_after.insert(tag.to_string(), good_reads);
            self
        }

        /// See the `write_offsets` field doc comment: writing a float to `tag` silently
        /// stores `value + offset` instead of `value`, so a subsequent readback observes a
        /// value that differs from what was requested.
        fn distorting_write(mut self, tag: &str, offset: f32) -> MockDriver {
            self.write_offsets.insert(tag.to_string(), offset);
            self
        }

        fn distorting_first_writes(
            mut self,
            tag: &str,
            write_count: usize,
            offset: f32,
        ) -> MockDriver {
            self.prefix_write_offsets
                .insert(tag.to_string(), (write_count, offset));
            self
        }

        /// See the `reject_writes_after` field doc comment: `tag`'s first `good_writes`
        /// writes are accepted normally, then every write after that is rejected.
        fn rejecting_write_after(mut self, tag: &str, good_writes: usize) -> MockDriver {
            self.reject_writes_after
                .insert(tag.to_string(), good_writes);
            self
        }

        fn value_of(&self, tag: &str) -> Option<String> {
            self.values.lock().unwrap().get(tag).cloned()
        }

        fn write_log(&self) -> Vec<(String, String)> {
            self.writes.lock().unwrap().clone()
        }

        fn read_batches(&self) -> Vec<Vec<String>> {
            self.read_batches.lock().unwrap().clone()
        }

        fn next_read_count(&self, tag: &str) -> usize {
            let mut counts = self.read_counts.lock().unwrap();
            let count = counts.entry(tag.to_string()).or_insert(0);
            *count += 1;
            *count
        }

        fn quality_for_read(&self, tag: &str, count: usize) -> bhtune_driver::Quality {
            let baseline_quality = self
                .qualities
                .lock()
                .unwrap()
                .get(tag)
                .copied()
                .unwrap_or(bhtune_driver::Quality::Good);
            self.degrade_quality_after.get(tag).map_or(
                baseline_quality,
                |(good_reads, degraded)| {
                    if count > *good_reads {
                        *degraded
                    } else {
                        baseline_quality
                    }
                },
            )
        }

        fn read_tag(
            &self,
            tag: &str,
            store: &std::collections::HashMap<String, String>,
            sequences: &mut std::collections::HashMap<String, std::collections::VecDeque<String>>,
        ) -> bhtune_driver::DriverResult<Option<bhtune_driver::TagValue>> {
            if self.error_reads.contains(tag) {
                return Err(bhtune_driver::DriverError::Operation(Box::new(
                    std::io::Error::other("mock read error"),
                )));
            }
            if self.empty_reads.contains(tag) {
                return Ok(None);
            }

            let count = self.next_read_count(tag);
            if self
                .error_reads_after
                .get(tag)
                .is_some_and(|good_reads| count > *good_reads)
            {
                return Err(bhtune_driver::DriverError::Operation(Box::new(
                    std::io::Error::other("mock read error after good reads"),
                )));
            }

            let quality = self.quality_for_read(tag, count);
            let value = sequences
                .get_mut(tag)
                .and_then(std::collections::VecDeque::pop_front)
                .or_else(|| store.get(tag).cloned())
                .unwrap_or_default();
            Ok(Some(bhtune_driver::TagValue {
                tag: tag.to_string(),
                value,
                quality,
                timestamp: None,
            }))
        }

        fn delayed_read_was_cancelled(&self, tag: &str) -> bool {
            self.cancelled_delayed_reads.lock().unwrap().contains(tag)
        }

        fn delayed_write_was_cancelled(&self, tag: &str) -> bool {
            self.cancelled_delayed_writes.lock().unwrap().contains(tag)
        }
    }

    struct DelayedReadObserver<'a> {
        driver: &'a MockDriver,
        tags: Vec<String>,
        completed: bool,
    }

    impl Drop for DelayedReadObserver<'_> {
        fn drop(&mut self) {
            if !self.completed {
                self.driver
                    .cancelled_delayed_reads
                    .lock()
                    .unwrap()
                    .extend(self.tags.iter().cloned());
            }
        }
    }

    struct DelayedWriteObserver<'a> {
        driver: &'a MockDriver,
        tag: String,
        completed: bool,
    }

    impl Drop for DelayedWriteObserver<'_> {
        fn drop(&mut self) {
            if !self.completed {
                self.driver
                    .cancelled_delayed_writes
                    .lock()
                    .unwrap()
                    .insert(self.tag.clone());
            }
        }
    }

    #[test]
    fn delayed_write_observer_records_cancellation_only_when_incomplete() {
        let driver = MockDriver::default();
        {
            let _observer = DelayedWriteObserver {
                driver: &driver,
                tag: "MV".into(),
                completed: false,
            };
        }
        assert!(driver.delayed_write_was_cancelled("MV"));

        {
            let _observer = DelayedWriteObserver {
                driver: &driver,
                tag: "completed".into(),
                completed: true,
            };
        }
        assert!(!driver.delayed_write_was_cancelled("completed"));
    }

    #[async_trait::async_trait]
    impl Driver for MockDriver {
        async fn read(
            &self,
            tags: &[String],
        ) -> bhtune_driver::DriverResult<Vec<bhtune_driver::TagValue>> {
            self.read_batches.lock().unwrap().push(tags.to_vec());
            if tags.iter().any(|tag| self.hang_reads.contains(tag)) {
                std::future::pending::<()>().await;
            }
            self.apply_read_delay(tags).await;
            let store = self.values.lock().unwrap();
            let mut sequences = self.read_sequences.lock().unwrap();
            let mut out = Vec::new();
            for tag in tags {
                if let Some(value) = self.read_tag(tag, &store, &mut sequences)? {
                    out.push(value);
                }
            }
            if self.reverse_read_results {
                out.reverse();
            }
            Ok(out)
        }

        async fn write(
            &self,
            tag: &String,
            value: TagWrite,
        ) -> bhtune_driver::DriverResult<bhtune_driver::WriteOutcome> {
            if self.hang_writes.contains(tag) {
                std::future::pending::<()>().await;
            }
            self.apply_write_delay(tag).await;
            if self.error_writes.contains(tag) {
                return Err(bhtune_driver::DriverError::Operation(Box::new(
                    std::io::Error::other("mock write error"),
                )));
            }
            let text = match &value {
                TagWrite::Float(f) => f.to_string(),
                TagWrite::Raw(s) => s.clone(),
            };
            self.writes
                .lock()
                .unwrap()
                .push((tag.clone(), text.clone()));
            let write_count = {
                let mut counts = self.write_counts.lock().unwrap();
                let count = counts.entry(tag.clone()).or_insert(0);
                *count += 1;
                *count
            };
            if self.reject_writes.contains(tag) {
                return Ok(bhtune_driver::WriteOutcome::failure("mock rejected write"));
            }
            if let Some(good_writes) = self.reject_writes_after.get(tag)
                && write_count > *good_writes
            {
                return Ok(bhtune_driver::WriteOutcome::failure(
                    "mock rejected write after good writes",
                ));
            }
            // Store the (possibly silently distorted) value that a subsequent read would
            // observe, while `writes` above kept a log of what was actually requested -- see
            // the `write_offsets` field doc comment.
            let stored = if let TagWrite::Float(f) = value {
                let prefix_offset = self
                    .prefix_write_offsets
                    .get(tag)
                    .filter(|(prefix, _)| write_count <= *prefix)
                    .map_or(0.0, |(_, offset)| *offset);
                (f + self
                    .write_offsets
                    .get(tag)
                    .copied()
                    .unwrap_or(prefix_offset))
                .to_string()
            } else {
                text
            };
            self.values.lock().unwrap().insert(tag.clone(), stored);
            Ok(bhtune_driver::WriteOutcome::success())
        }

        async fn browse(
            &self,
            _request: bhtune_driver::BrowsePageRequest,
        ) -> bhtune_driver::DriverResult<bhtune_driver::BrowsePage> {
            Err(bhtune_driver::DriverError::Unsupported {
                operation: "browse",
            })
        }
    }

    #[tokio::test]
    async fn mock_driver_browse_is_unsupported() {
        // `tune`'s own logic never calls `Driver::browse` -- this only exists so
        // `MockDriver` satisfies the trait -- but it should still honor the same
        // "unsupported, not a panic" convention real drivers document for it.
        let err = MockDriver::new(&[])
            .browse(bhtune_driver::BrowsePageRequest::root(20))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            bhtune_driver::DriverError::Unsupported {
                operation: "browse"
            }
        ));
    }

    #[test]
    fn relay_actuation_tolerance_uses_span_allowance_and_step_cap() {
        let uncapped = mv_actuation_tolerance(MvActuationKind::Relay, 60.0, 50.0, 100.0).unwrap();
        assert!(uncapped > 0.1);
        assert!(uncapped < 0.101);

        let capped = mv_actuation_tolerance(MvActuationKind::Relay, 50.2, 50.0, 100.0).unwrap();
        assert!((capped - 0.05).abs() < 1e-6);

        let restore = mv_actuation_tolerance(MvActuationKind::Restore, 50.0, 50.2, 100.0).unwrap();
        assert!(restore > capped);
    }

    #[test]
    fn relay_actuation_tolerance_rejects_a_step_below_the_f32_floor() {
        let error =
            mv_actuation_tolerance(MvActuationKind::Relay, 50.0 + f32::EPSILON, 50.0, 100.0)
                .unwrap_err();
        assert!(error.to_string().contains("too small to verify safely"));
        assert!(mv_actuation_tolerance(MvActuationKind::Relay, 50.005, 50.0, 100.0).is_err());
    }

    #[test]
    fn relay_actuation_tolerance_rejects_large_magnitude_step_below_precision_floor() {
        let previous = 100_000_000.0_f32;
        let target = previous + 8.0;
        assert_eq!(target - previous, 8.0);
        let error =
            mv_actuation_tolerance(MvActuationKind::Relay, target, previous, 100.0).unwrap_err();
        assert!(error.to_string().contains("too small to verify safely"));
    }

    fn pending_actuation(
        id: Option<i64>,
        kind: MvActuationKind,
        target: f32,
        first_check_at: Instant,
        deadline: Instant,
        last_readback: Option<f32>,
    ) -> PendingMvActuation {
        let now = Instant::now();
        PendingMvActuation {
            id,
            kind,
            target,
            tolerance: 0.1,
            switch_tick: Utc::now(),
            switch_instant: now,
            accepted_instant: now,
            first_check_at,
            deadline,
            last_readback,
        }
    }

    fn tracker_with_pending(pending: PendingMvActuation) -> MvActuationTracker {
        MvActuationTracker {
            next_sequence: 1,
            previous_commanded_mv: 55.0,
            confirmed_mv: None,
            pending: Some(pending),
            mv_span: 100.0,
        }
    }

    fn batched_mv_value(tag: &str, value: &str, quality: bhtune_driver::Quality) -> TagValue {
        TagValue {
            tag: tag.to_string(),
            value: value.to_string(),
            quality,
            timestamp: None,
        }
    }

    fn pending_poll_test_state() -> (
        SqlitePool,
        EffectiveTiming,
        MvActuationTracker,
        PollTimingAccumulator,
    ) {
        let args = {
            let mut args = fast_simulator_args();
            args.driver = DriverKind::Opcda;
            args
        };
        let now = Instant::now();
        let pending = pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now,
            now + Duration::from_secs(10),
            None,
        );
        (
            SqlitePool::connect_lazy("sqlite::memory:").unwrap(),
            test_effective_timing(&args),
            tracker_with_pending(pending),
            timing_for_args(&args),
        )
    }

    #[tokio::test]
    async fn resolve_pending_mv_poll_reports_missing_mv_data() {
        let (pool, effective_timing, mut tracker, mut timing) = pending_poll_test_state();
        let error = resolve_pending_mv_poll(
            &pool,
            effective_timing,
            TickOperation::Completed(HashMap::new()),
            "Unit1.LIC101.OP",
            Utc::now(),
            Instant::now(),
            Duration::from_millis(1),
            false,
            &mut tracker,
            &mut timing,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("no value for tag"));
        assert!(tracker.pending.is_none());
    }

    #[tokio::test]
    async fn resolve_pending_mv_poll_reports_malformed_mv_data() {
        let (pool, effective_timing, mut tracker, mut timing) = pending_poll_test_state();
        let values = HashMap::from([(
            "Unit1.LIC101.OP".to_string(),
            batched_mv_value(
                "Unit1.LIC101.OP",
                "not-a-number",
                bhtune_driver::Quality::Good,
            ),
        )]);
        let error = resolve_pending_mv_poll(
            &pool,
            effective_timing,
            TickOperation::Completed(values),
            "Unit1.LIC101.OP",
            Utc::now(),
            Instant::now(),
            Duration::from_millis(1),
            false,
            &mut tracker,
            &mut timing,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("not a number"));
        assert!(tracker.pending.is_none());
    }

    #[tokio::test]
    async fn resolve_pending_mv_poll_preserves_a_cancelled_operation() {
        let (pool, effective_timing, mut tracker, mut timing) = pending_poll_test_state();
        let (reason, provided_evidence) = resolve_pending_mv_poll(
            &pool,
            effective_timing,
            TickOperation::Cancelled,
            "Unit1.LIC101.OP",
            Utc::now(),
            Instant::now(),
            Duration::from_millis(1),
            false,
            &mut tracker,
            &mut timing,
        )
        .await
        .unwrap();

        assert_eq!(reason, Some(AbortReason::UserInterrupt));
        assert!(!provided_evidence);
        assert!(tracker.pending.is_none());
    }

    #[tokio::test]
    async fn resolve_pending_mv_poll_preserves_a_timed_out_operation() {
        let (pool, effective_timing, mut tracker, mut timing) = pending_poll_test_state();
        let (reason, provided_evidence) = resolve_pending_mv_poll(
            &pool,
            effective_timing,
            TickOperation::TimedOut,
            "Unit1.LIC101.OP",
            Utc::now(),
            Instant::now(),
            Duration::from_millis(1),
            false,
            &mut tracker,
            &mut timing,
        )
        .await
        .unwrap();

        assert!(matches!(
            reason,
            Some(AbortReason::OperationTimedOut {
                tag,
                op_timeout_secs: 30,
            }) if tag == "Unit1.LIC101.OP"
        ));
        assert!(!provided_evidence);
        assert!(tracker.pending.is_none());
    }

    #[tokio::test]
    async fn audit_helpers_skip_rows_without_an_audit_id_or_pending_actuation() {
        let pool = seeded_pool().await;
        let now = Instant::now();
        let pending = pending_actuation(
            None,
            MvActuationKind::Restore,
            45.0,
            now,
            now + Duration::from_secs(1),
            None,
        );

        assert_eq!(
            record_actuation_observation(
                &pool,
                &pending,
                Utc::now(),
                Some(45.0),
                Some(SampleQuality::Good),
                ActuationAuditPolicy::Required,
            )
            .await
            .unwrap(),
            None
        );
        assert_eq!(
            record_final_actuation_observation(
                &pool,
                &pending,
                Utc::now(),
                Some(45.0),
                Some(SampleQuality::Good),
                MvActuationStatus::Confirmed,
                "",
            )
            .await,
            None
        );
        finalize_actuation_best_effort(
            &pool,
            &pending,
            MvActuationStatus::Superseded,
            "no audit row",
        )
        .await;

        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let mut tracker = MvActuationTracker::for_run(&args, &sample_initial_state()).unwrap();
        supersede_pending_actuation_best_effort(&pool, &mut tracker, "nothing pending").await;
        assert!(tracker.pending.is_none());
    }

    #[tokio::test]
    async fn audit_helpers_apply_required_and_best_effort_failure_policies() {
        let pool = seeded_pool().await;
        let now = Instant::now();
        let pending = pending_actuation(
            Some(i64::MAX),
            MvActuationKind::Restore,
            45.0,
            now,
            now + Duration::from_secs(1),
            None,
        );
        pool.close().await;

        assert_eq!(
            record_actuation_observation(
                &pool,
                &pending,
                Utc::now(),
                Some(45.0),
                Some(SampleQuality::Good),
                ActuationAuditPolicy::BestEffort,
            )
            .await
            .unwrap(),
            None
        );
        assert!(
            record_actuation_observation(
                &pool,
                &pending,
                Utc::now(),
                Some(45.0),
                Some(SampleQuality::Good),
                ActuationAuditPolicy::Required,
            )
            .await
            .is_err()
        );
        assert_eq!(
            record_final_actuation_observation(
                &pool,
                &pending,
                Utc::now(),
                Some(45.0),
                Some(SampleQuality::Good),
                MvActuationStatus::Confirmed,
                "closed pool",
            )
            .await,
            None
        );
        finalize_actuation_best_effort(
            &pool,
            &pending,
            MvActuationStatus::Superseded,
            "closed pool",
        )
        .await;
    }

    #[tokio::test]
    async fn replacement_before_any_readback_is_finalized_as_unverified() {
        let pool = seeded_pool().await;
        let now = Instant::now();
        let pending = pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now,
            now + Duration::from_secs(1),
            None,
        );
        let mut tracker = tracker_with_pending(pending);

        let reason =
            reject_replacement_for_pending_actuation(&pool, "Unit1.LIC101.OP", &mut tracker)
                .await
                .unwrap();

        assert!(matches!(
            reason,
            AbortReason::MvActuationUnconfirmed { readback: None, .. }
        ));
        assert!(tracker.pending.is_none());
    }

    #[tokio::test]
    async fn verification_without_pending_work_or_before_first_check_is_a_noop() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let mut tracker = MvActuationTracker::for_run(&args, &sample_initial_state()).unwrap();

        assert_eq!(
            verify_pending_mv_actuation_with(
                &pool,
                &args,
                "Unit1.LIC101.OP",
                &driver,
                &mut CtrlC::never(),
                false,
                &mut tracker,
                MvVerificationTrigger::Scheduled,
                MvVerificationCallLimit::None,
                ActuationAuditPolicy::Required,
            )
            .await
            .unwrap(),
            None
        );

        let now = Instant::now();
        tracker.pending = Some(pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now + Duration::from_secs(1),
            now + Duration::from_secs(2),
            None,
        ));
        assert_eq!(
            verify_pending_mv_actuation_with(
                &pool,
                &args,
                "Unit1.LIC101.OP",
                &driver,
                &mut CtrlC::never(),
                false,
                &mut tracker,
                MvVerificationTrigger::Scheduled,
                MvVerificationCallLimit::None,
                ActuationAuditPolicy::Required,
            )
            .await
            .unwrap(),
            None
        );
        assert!(driver.read_batches().is_empty());
    }

    #[tokio::test]
    async fn explicit_deadline_trigger_rejects_a_mismatch_before_the_clock_deadline() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-explicit-deadline").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let now = Instant::now();
        let mut tracker = MvActuationTracker::for_run(&args, &sample_initial_state()).unwrap();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                now,
                Utc::now(),
                now,
                0.1,
            )
            .await
            .unwrap();

        let outcome = verify_pending_mv_actuation_with(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            MvVerificationTrigger::Deadline,
            MvVerificationCallLimit::None,
            ActuationAuditPolicy::Required,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            Some(AbortReason::MvActuationUnconfirmed {
                readback: Some(45.0),
                ..
            })
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
    }

    #[tokio::test]
    async fn first_verification_uses_the_switch_tick_even_when_write_acceptance_is_late() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, _tags) =
            start_opc_test_run(&pool, "actuation-switch-causality").await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let switch_instant = Instant::now();
        let switch_tick = DateTime::UNIX_EPOCH + chrono::Duration::seconds(12);
        let accepted_instant = switch_instant + Duration::from_secs(3);
        let accepted_at = switch_tick + chrono::Duration::seconds(3);
        let first_check_at = switch_instant + Duration::from_secs(2);

        tracker
            .record_accepted_at_switch(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                switch_tick,
                switch_instant,
                first_check_at,
                accepted_at,
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();

        let pending = tracker.pending.as_ref().unwrap();
        assert_eq!(pending.switch_tick, switch_tick);
        assert_eq!(pending.switch_instant, switch_instant);
        assert_eq!(pending.accepted_instant, accepted_instant);
        assert_eq!(pending.first_check_at, first_check_at);
        assert_eq!(
            pending.deadline,
            accepted_instant + Duration::from_secs(MV_ACTUATION_CONFIRMATION_SECS)
        );
    }

    #[tokio::test]
    async fn accepted_mv_command_is_confirmed_without_waiting_when_readback_matches() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-confirmed").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let target = 55.0;
        write_value(&driver, &tags.manipulated_variable, target)
            .await
            .unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        let tolerance =
            mv_actuation_tolerance(MvActuationKind::Relay, target, initial.mv_ini, 100.0).unwrap();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                target,
                commanded_instant,
                commanded_at,
                commanded_instant,
                tolerance,
            )
            .await
            .unwrap();

        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, None);
        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, MvActuationStatus::Confirmed);
        assert_eq!(rows[0].attempt_count, 1);
        assert_eq!(rows[0].readback_mv, Some(target));
    }

    #[tokio::test]
    async fn later_retry_can_confirm_after_an_earlier_mismatch() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-late-confirmation").await;
        let driver =
            honeywell_driver_auto().with_read_sequence(&tags.manipulated_variable, &["50", "55"]);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();

        for _ in 0..2 {
            assert_eq!(
                verify_pending_mv_actuation(
                    &pool,
                    &args,
                    &tags.manipulated_variable,
                    &driver,
                    &mut CtrlC::never(),
                    false,
                    &mut tracker,
                    None,
                )
                .await
                .unwrap(),
                None
            );
        }

        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Confirmed);
        assert_eq!(rows[0].attempt_count, 2);
        assert_eq!(rows[0].readback_mv, Some(55.0));
    }

    #[tokio::test]
    async fn early_mismatch_stays_pending_but_blocks_a_replacement_relay() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-mismatch").await;
        let driver = honeywell_driver_auto().distorting_write(&tags.manipulated_variable, -5.0);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let target = 55.0;
        write_value(&driver, &tags.manipulated_variable, target)
            .await
            .unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        let tolerance =
            mv_actuation_tolerance(MvActuationKind::Relay, target, initial.mv_ini, 100.0).unwrap();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                target,
                commanded_instant,
                commanded_at,
                commanded_instant,
                tolerance,
            )
            .await
            .unwrap();

        let first = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();
        assert_eq!(first, None);
        assert!(tracker.pending.is_some());

        let forced = reject_replacement_for_pending_actuation(
            &pool,
            &tags.manipulated_variable,
            &mut tracker,
        )
        .await
        .unwrap();
        assert!(matches!(
            forced,
            AbortReason::MvActuationUnconfirmed {
                target: 55.0,
                readback: Some(50.0),
                ..
            }
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn later_check_reads_fresh_instead_of_failing_from_an_earlier_mismatch() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-deadline").await;
        let driver =
            honeywell_driver_auto().with_read_sequence(&tags.manipulated_variable, &["50", "55"]);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        write_value(&driver, &tags.manipulated_variable, 55.0)
            .await
            .unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            verify_pending_mv_actuation(
                &pool,
                &args,
                &tags.manipulated_variable,
                &driver,
                &mut CtrlC::never(),
                false,
                &mut tracker,
                None,
            )
            .await
            .unwrap(),
            None
        );
        // Trigger another verification without crossing the real confirmation deadline. The
        // second read must be evaluated on its own rather than inheriting the first mismatch.
        tracker.pending.as_mut().unwrap().deadline = Instant::now() + Duration::from_secs(1);

        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, None);
        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Confirmed);
        assert_eq!(rows[0].attempt_count, 2);
        assert_eq!(rows[0].readback_mv, Some(55.0));
    }

    #[tokio::test]
    async fn predeadline_read_is_dropped_and_late_matching_readback_still_fails() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-predeadline-bound").await;
        let driver = honeywell_driver_auto()
            .with_value(&tags.manipulated_variable, "55")
            .delaying_read(&tags.manipulated_variable, Duration::from_millis(50));
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.op_timeout_secs = 30;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        tracker.pending.as_mut().unwrap().deadline = Instant::now() + Duration::from_millis(25);

        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            Some(AbortReason::MvActuationUnconfirmed {
                readback: Some(55.0),
                ..
            })
        ));
        assert!(tracker.pending.is_none());
        assert_eq!(
            driver.read_batches(),
            vec![
                vec![tags.manipulated_variable.clone()],
                vec![tags.manipulated_variable.clone()]
            ],
            "the read cancelled at the deadline must not be reused as deadline evidence"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn fresh_deadline_read_is_tightly_bounded_below_the_operation_timeout() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-deadline-read-bound").await;
        let driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.op_timeout_secs = 30;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        tracker.pending.as_mut().unwrap().deadline = Instant::now();

        let started = Instant::now();
        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(
            outcome,
            Some(AbortReason::MvActuationUnconfirmed { readback: None, .. })
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn stalled_shared_pv_mv_poll_is_cancelled_without_recording_a_sample() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-shared-poll-cancel").await;
        let driver = honeywell_driver_auto()
            .delaying_read(&tags.manipulated_variable, Duration::from_secs(2))
            .degrade_quality_after(&tags.process_variable, 1, bhtune_driver::Quality::Bad);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.mrft_delay = 10;
        args.timeout_secs = 3;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant + Duration::from_secs(1),
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let mut tracker = Some(tracker);
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            Utc::now(),
            MrftCompat::default(),
        );
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(1);
        });
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            RunTimeAnchor::now(),
            &mut ctrl_c,
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::UserInterrupt)
        ));
        assert!(
            driver.delayed_read_was_cancelled(&tags.manipulated_variable),
            "the shared PV/MV read must be dropped when Ctrl+C cancels the operation"
        );
        assert_eq!(
            driver.read_batches(),
            vec![vec![
                tags.process_variable.clone(),
                tags.manipulated_variable.clone()
            ]]
        );
        assert!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .is_empty(),
            "a cancelled shared read has no valid PV sample to persist"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 0);
    }

    #[tokio::test]
    async fn stalled_shared_pv_mv_poll_times_out_without_recording_a_sample() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-shared-poll-timeout").await;
        let driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.mrft_delay = 10;
        args.op_timeout_secs = 0;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::OperationTimedOut {
                ref tag,
                op_timeout_secs: 0,
            }) if tag == &tags.manipulated_variable
        ));
        assert_eq!(
            driver.read_batches(),
            vec![vec![
                tags.process_variable.clone(),
                tags.manipulated_variable.clone()
            ]]
        );
        assert!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .is_empty(),
            "a timed-out shared read has no valid PV sample to persist"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 1);
        assert!(tracker.is_none() || tracker.as_ref().unwrap().pending.is_none());
    }

    #[tokio::test]
    async fn scheduled_mv_verification_keeps_an_early_mismatch_pending() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-early-mismatch").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.poll_interval_ms = 10_000;
        args.mrft_delay = 10;
        args.timeout_secs = 1;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let pending = tracker.pending.as_mut().unwrap();
        pending.first_check_at = accepted_instant;
        pending.deadline = accepted_instant + Duration::from_millis(200);
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::MvActuationUnconfirmed { .. })
        ));
        assert_eq!(
            driver.read_batches(),
            vec![
                vec![tags.manipulated_variable.clone()],
                vec![
                    tags.process_variable.clone(),
                    tags.manipulated_variable.clone()
                ],
                vec![tags.manipulated_variable.clone()]
            ]
        );
        assert_eq!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .len(),
            1
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
        assert_eq!(rows[0].attempt_count, 3);
    }

    #[tokio::test]
    async fn verification_deadline_wakes_without_waiting_for_a_long_poll_interval() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-deadline-wakeup").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.poll_interval_ms = 10_000;
        args.mrft_delay = 10;
        args.timeout_secs = 2;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(25);
        let pending = tracker.pending.as_mut().unwrap();
        pending.first_check_at = deadline;
        pending.deadline = deadline;
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let mut timing = timing_for_args(&args);

        let started = Instant::now();
        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::MvActuationUnconfirmed { .. })
        ));
        let reads = driver.read_batches();
        assert_eq!(
            reads[0],
            vec![
                tags.process_variable.clone(),
                tags.manipulated_variable.clone()
            ]
        );
        assert_eq!(reads[1], vec![tags.manipulated_variable.clone()]);
        assert_eq!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn due_mv_verification_precedes_a_due_pv_poll() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-before-due-poll").await;
        let driver = honeywell_driver_auto()
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Bad);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.poll_interval_ms = 10_000;
        args.mrft_delay = 10;
        args.timeout_secs = 2;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        tracker.pending.as_mut().unwrap().deadline = Instant::now();
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::MvActuationUnconfirmed { .. })
        ));
        assert_eq!(
            driver.read_batches(),
            vec![vec![tags.manipulated_variable.clone()]],
            "a due verification deadline must be handled before the due PV poll"
        );
        assert!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn replacement_preview_uses_deadline_verification_and_records_the_abort_sample() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "deadline-preview-verification").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.timeout_secs = 1;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let now = Instant::now();
        tracker.pending = Some(pending_actuation(
            None,
            MvActuationKind::Relay,
            35.0,
            now + Duration::from_secs(1),
            now,
            None,
        ));
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let state_before = engine.state();
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
                readback: Some(45.0),
                ..
            })
        ));
        assert_eq!(engine.state(), state_before);
        assert_eq!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn replacement_preview_commits_after_confirming_the_prior_command() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "confirmed-preview-replacement").await;
        let driver = honeywell_driver_auto().degrade_quality_after(
            &tags.process_variable,
            1,
            bhtune_driver::Quality::Bad,
        );
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.timeout_secs = 1;
        let initial = sample_initial_state();
        let now = Instant::now();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        tracker.pending = Some(pending_actuation(
            None,
            MvActuationKind::Relay,
            initial.mv_ini,
            now + Duration::from_secs(1),
            now + Duration::from_secs(2),
            None,
        ));
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let state_before = engine.state();
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::PoorQuality { .. })
        ));
        assert_ne!(engine.state(), state_before);
        assert_eq!(driver.write_log().len(), 1);
    }

    #[tokio::test]
    async fn pending_actuation_preview_does_not_commit_or_write_a_replacement_relay() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-preview").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.timeout_secs = 1;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                35.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 35.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            verify_pending_mv_actuation(
                &pool,
                &args,
                &tags.manipulated_variable,
                &driver,
                &mut CtrlC::never(),
                false,
                &mut tracker,
                None,
            )
            .await
            .unwrap(),
            None
        );
        assert_eq!(
            tracker.pending.as_ref().unwrap().last_readback,
            Some(initial.mv_ini)
        );
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let state_before = engine.state();
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
                readback: Some(45.0),
                ..
            })
        ));
        assert_eq!(engine.state(), state_before);
        assert!(driver.write_log().is_empty());
        assert_eq!(
            driver.read_batches(),
            vec![
                vec![tags.manipulated_variable.clone()],
                vec![
                    tags.process_variable.clone(),
                    tags.manipulated_variable.clone()
                ],
            ],
            "the replacement preview must use the fresh batched PV/MV read from the preview tick"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
    }

    #[tokio::test]
    async fn verification_operation_timeout_preserves_the_timed_out_outcome() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) = start_opc_test_run(&pool, "actuation-hang").await;
        let driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.op_timeout_secs = 1;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let started = Instant::now();
        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(
            outcome,
            Some(AbortReason::OperationTimedOut {
                op_timeout_secs: 1,
                ..
            })
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn ctrl_c_during_verification_preserves_pending_row_for_restore_handoff() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-ctrl-c").await;
        let hanging_driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let healthy_driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();

        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &hanging_driver,
            &mut ctrl_c,
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, Some(AbortReason::UserInterrupt));
        assert!(tracker.pending.is_none());

        let mut tracker = Some(tracker);
        let restored = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &healthy_driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();
        assert!(matches!(
            restored,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded)
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[1].status, MvActuationStatus::Confirmed);
    }

    #[tokio::test]
    async fn poor_quality_verification_preserves_the_poor_quality_outcome() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-quality-retry").await;
        let driver = honeywell_driver_auto()
            .with_quality(&tags.manipulated_variable, bhtune_driver::Quality::Bad);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();

        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            Some(AbortReason::PoorQuality {
                quality: bhtune_driver::Quality::Bad,
                ..
            })
        ));
        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].readback_quality, Some(SampleQuality::Bad));
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn verification_transport_error_is_an_ordinary_failed_run_error() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-timeout-retry").await;
        let driver = honeywell_driver_auto().erroring_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();

        let error = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("driver operation failed"));
        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 1);
        assert_eq!(rows[0].readback_mv, None);
    }

    #[tokio::test]
    async fn restore_verification_timeout_finalizes_the_pending_row_as_unverified() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-restore-verification-timeout").await;
        let driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Restore,
                initial.mv_ini,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Restore, initial.mv_ini, 55.0, 100.0)
                    .unwrap(),
            )
            .await
            .unwrap();

        let outcome = verify_pending_mv_actuation_with(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            MvVerificationTrigger::Deadline,
            MvVerificationCallLimit::Restore(Instant::now()),
            ActuationAuditPolicy::BestEffort,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            Some(AbortReason::MvActuationUnconfirmed { readback: None, .. })
        ));
        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn required_confirmation_audit_failure_keeps_the_pending_state_for_retry() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-required-audit-failure").await;
        let driver = honeywell_driver_auto().with_value(&tags.manipulated_variable, "55");
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        pool.close().await;

        let error = verify_pending_mv_actuation_with(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            MvVerificationTrigger::Scheduled,
            MvVerificationCallLimit::None,
            ActuationAuditPolicy::Required,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("pool"));
        assert!(tracker.pending.is_some());
        assert_eq!(tracker.confirmed_mv, None);
    }

    #[tokio::test]
    async fn best_effort_confirmation_audit_failure_does_not_block_restore_state() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-best-effort-audit-failure").await;
        let driver = honeywell_driver_auto().with_value(&tags.manipulated_variable, "55");
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Restore,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Restore, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        pool.close().await;

        let outcome = verify_pending_mv_actuation_with(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            MvVerificationTrigger::Scheduled,
            MvVerificationCallLimit::None,
            ActuationAuditPolicy::BestEffort,
        )
        .await
        .unwrap();

        assert_eq!(outcome, None);
        assert!(tracker.pending.is_none());
        assert_eq!(tracker.confirmed_mv, Some(55.0));
    }

    #[tokio::test]
    async fn restore_supersedes_final_snapback_still_pending_during_padding() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-restore").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        tracker.previous_commanded_mv = 55.0;
        write_value(&driver, &tags.manipulated_variable, initial.mv_ini)
            .await
            .unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                initial.mv_ini,
                commanded_instant + Duration::from_secs(10),
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, initial.mv_ini, 55.0, 100.0)
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut tracker = Some(tracker);

        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded)
        ));
        assert_eq!(
            driver.write_log().len(),
            1,
            "restore must adopt a confirmed final snapback instead of writing MV again"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, MvActuationKind::Relay);
        assert_eq!(rows[0].status, MvActuationStatus::Superseded);
        assert_eq!(rows[0].attempt_count, 1);
        assert!(
            rows[0]
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("no duplicate MV write"))
        );
    }

    #[tokio::test]
    async fn restore_rewrites_an_unconfirmed_final_snapback_without_waiting_twice() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-restore-rewrite").await;
        let driver = honeywell_driver_auto().with_value(&tags.manipulated_variable, "50");
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        tracker.previous_commanded_mv = 55.0;
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                initial.mv_ini,
                accepted_instant + Duration::from_secs(4),
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, initial.mv_ini, 55.0, 100.0)
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut tracker = Some(tracker);

        let started = Instant::now();
        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded)
        ));
        assert_eq!(
            driver.write_log(),
            vec![(
                tags.manipulated_variable.clone(),
                initial.mv_ini.to_string()
            )]
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, MvActuationStatus::Superseded);
        assert_eq!(rows[0].readback_mv, Some(50.0));
        assert_eq!(rows[1].kind, MvActuationKind::Restore);
        assert_eq!(rows[1].status, MvActuationStatus::Confirmed);
    }

    #[tokio::test]
    async fn slow_snapback_handoff_reserves_time_for_authoritative_restore() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-restore-budget").await;
        let driver = honeywell_driver_auto()
            .delaying_read(&tags.manipulated_variable, Duration::from_millis(1_100));
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.restore_timeout_secs = MV_ACTUATION_CONFIRMATION_SECS + 1;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        tracker.previous_commanded_mv = 55.0;
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                initial.mv_ini,
                accepted_instant + Duration::from_secs(4),
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, initial.mv_ini, 55.0, 100.0)
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut tracker = Some(tracker);

        let started = Instant::now();
        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded)
        ));
        assert_eq!(
            driver.write_log(),
            vec![(
                tags.manipulated_variable.clone(),
                initial.mv_ini.to_string()
            )],
            "the handoff read must fall back before consuming the restore write budget"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, MvActuationStatus::Superseded);
        assert_eq!(rows[1].status, MvActuationStatus::Confirmed);
    }

    #[tokio::test]
    async fn snapback_handoff_skips_reads_that_would_consume_the_restore_budget() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let now = Instant::now();

        for restore_deadline in [
            now + Duration::from_secs(1),
            Instant::now() + Duration::from_secs(MV_ACTUATION_CONFIRMATION_SECS),
        ] {
            let pending = pending_actuation(
                None,
                MvActuationKind::Relay,
                45.0,
                now,
                now + Duration::from_secs(10),
                None,
            );
            let mut tracker = tracker_with_pending(pending);
            let outcome = try_confirm_final_snapback_handoff(
                &pool,
                &args,
                &driver,
                "Unit1.LIC101.OP",
                45.0,
                false,
                &mut CtrlC::never(),
                &mut tracker,
                restore_deadline,
            )
            .await
            .unwrap();

            assert!(matches!(outcome, Some(RestoreHandoffOutcome::Rewrite)));
            assert!(tracker.pending.is_none());
        }
        assert!(driver.read_batches().is_empty());
    }

    #[tokio::test]
    async fn snapback_handoff_failures_fall_back_to_an_authoritative_rewrite() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let restore_deadline = Instant::now() + Duration::from_secs(10);

        let cases = [
            honeywell_driver_auto().erroring_read("Unit1.LIC101.OP"),
            honeywell_driver_auto()
                .hanging_read("Unit1.LIC101.OP")
                .with_quality("Unit1.LIC101.OP", bhtune_driver::Quality::Good),
            honeywell_driver_auto().with_quality("Unit1.LIC101.OP", bhtune_driver::Quality::Bad),
        ];
        for (index, driver) in cases.into_iter().enumerate() {
            let now = Instant::now();
            let pending = pending_actuation(
                None,
                MvActuationKind::Relay,
                45.0,
                now,
                now + Duration::from_secs(10),
                None,
            );
            let mut tracker = tracker_with_pending(pending);
            if index == 1 {
                args.op_timeout_secs = 0;
            } else {
                args.op_timeout_secs = 30;
            }
            let outcome = try_confirm_final_snapback_handoff(
                &pool,
                &args,
                &driver,
                "Unit1.LIC101.OP",
                45.0,
                false,
                &mut CtrlC::never(),
                &mut tracker,
                restore_deadline,
            )
            .await
            .unwrap();

            assert!(matches!(outcome, Some(RestoreHandoffOutcome::Rewrite)));
            assert!(tracker.pending.is_none());
        }
    }

    #[tokio::test]
    async fn snapback_handoff_preserves_pending_state_when_ctrl_c_interrupts_the_read() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto().hanging_read("Unit1.LIC101.OP");
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let now = Instant::now();
        let pending = pending_actuation(
            None,
            MvActuationKind::Relay,
            45.0,
            now,
            now + Duration::from_secs(10),
            None,
        );
        let mut tracker = tracker_with_pending(pending);
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();

        let outcome = try_confirm_final_snapback_handoff(
            &pool,
            &args,
            &driver,
            "Unit1.LIC101.OP",
            45.0,
            false,
            &mut ctrl_c,
            &mut tracker,
            Instant::now() + Duration::from_secs(10),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            Some(RestoreHandoffOutcome::Interrupted(_))
        ));
        assert!(tracker.pending.is_some());
    }

    #[tokio::test]
    async fn restore_mv_propagates_an_interrupted_final_snapback_handoff() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto().hanging_read("Unit1.LIC101.OP");
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let now = Instant::now();
        let mut tracker = Some(tracker_with_pending(pending_actuation(
            None,
            MvActuationKind::Relay,
            45.0,
            now,
            now + Duration::from_secs(10),
            None,
        )));
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();

        let outcome = restore_mv_with_verification(
            &pool,
            0,
            &args,
            &driver,
            "Unit1.LIC101.OP",
            45.0,
            false,
            &mut ctrl_c,
            &mut tracker,
            Instant::now() + Duration::from_secs(10),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Interrupted(ref detail)
                if detail.contains("final MRFT snapback")
        ));
        assert!(tracker.as_ref().unwrap().pending.is_some());
    }

    #[tokio::test]
    async fn restore_mv_without_a_tracker_reports_an_operation_timeout() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto().hanging_write("Unit1.LIC101.OP");
        let mut args = fast_simulator_args();
        args.op_timeout_secs = 0;
        let mut tracker = None;

        let outcome = restore_mv_with_verification(
            &pool,
            0,
            &args,
            &driver,
            "Unit1.LIC101.OP",
            45.0,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(ref detail))
                if detail.contains("restore write did not complete")
        ));
    }

    #[tokio::test]
    async fn tracked_restore_write_handles_deadline_cancel_and_operation_timeout() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto().hanging_write("Unit1.LIC101.OP");
        let initial = sample_initial_state();

        let now = Instant::now();
        let mut deadline_tracker = Some(tracker_with_pending(pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now,
            now + Duration::from_secs(1),
            None,
        )));
        let deadline_outcome = restore_mv_with_verification(
            &pool,
            0,
            &fast_simulator_args(),
            &driver,
            "Unit1.LIC101.OP",
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut deadline_tracker,
            Instant::now(),
        )
        .await
        .unwrap();
        assert!(matches!(
            deadline_outcome,
            RestoreMvOutcome::Interrupted(ref detail)
                if detail.contains("[tuning].restore_timeout_secs")
        ));

        let now = Instant::now();
        let mut cancel_tracker = Some(tracker_with_pending(pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now,
            now + Duration::from_secs(1),
            None,
        )));
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();
        let cancel_outcome = restore_mv_with_verification(
            &pool,
            0,
            &fast_simulator_args(),
            &driver,
            "Unit1.LIC101.OP",
            initial.mv_ini,
            false,
            &mut ctrl_c,
            &mut cancel_tracker,
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(matches!(
            cancel_outcome,
            RestoreMvOutcome::Interrupted(ref detail) if detail.contains("second Ctrl+C")
        ));

        let now = Instant::now();
        let mut timeout_tracker = Some(tracker_with_pending(pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now,
            now + Duration::from_secs(1),
            None,
        )));
        let mut timeout_args = fast_simulator_args();
        timeout_args.op_timeout_secs = 0;
        let timeout_outcome = restore_mv_with_verification(
            &pool,
            0,
            &timeout_args,
            &driver,
            "Unit1.LIC101.OP",
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut timeout_tracker,
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(matches!(
            timeout_outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(ref detail))
                if detail.contains("restore write did not complete")
        ));
    }

    #[tokio::test]
    async fn restore_verification_retries_a_mismatch_then_confirms() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "restore-retry-confirm").await;
        let driver =
            honeywell_driver_auto().with_read_sequence(&tags.manipulated_variable, &["50", "45"]);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = Some(MvActuationTracker::for_run(&args, &initial).unwrap());

        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded)
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].attempt_count, 2);
        assert_eq!(rows[0].status, MvActuationStatus::Confirmed);
    }

    #[tokio::test]
    async fn ctrl_c_during_restore_verification_interrupts_after_the_write() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "restore-verification-cancel").await;
        let driver = honeywell_driver_auto()
            .delaying_read(&tags.manipulated_variable, Duration::from_millis(500));
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = Some(MvActuationTracker::for_run(&args, &initial).unwrap());
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let _ = tx.send(1);
        });

        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut ctrl_c,
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Interrupted(ref detail)
                if detail.contains("confirming the restored MV")
        ));
    }

    #[tokio::test]
    async fn restore_verification_reports_the_expired_restore_deadline() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "restore-verification-deadline").await;
        let driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.op_timeout_secs = 30;
        let initial = sample_initial_state();
        let mut tracker = Some(MvActuationTracker::for_run(&args, &initial).unwrap());

        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_millis(30),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Interrupted(ref detail)
                if detail.contains("[tuning].restore_timeout_secs")
        ));
    }

    #[tokio::test]
    async fn failed_mv_restore_keeps_an_auto_starting_loop_in_manual() {
        let pool = seeded_pool().await;
        let (run_id, _config, template, tags) =
            start_opc_test_run(&pool, "actuation-restore-quality").await;
        let driver = honeywell_driver_auto()
            .with_quality(&tags.manipulated_variable, bhtune_driver::Quality::Bad);
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial);
        let guard = MutationGuard {
            mode_attribute_written: true,
            mode_written: true,
            mv_written: true,
        };

        let outcome = attempt_restore_with_actuation(
            &pool,
            run_id,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut CtrlC::never(),
            &mut tracker,
        )
        .await;

        assert!(matches!(outcome, RestoreAttempt::Incomplete { .. }));
        let writes = driver.write_log();
        assert!(
            !writes
                .iter()
                .any(|(tag, _)| Some(tag) == tags.controller_mode.as_ref()),
            "Auto release must be suppressed when the final MV restore is not confirmed"
        );
        assert!(
            writes
                .iter()
                .any(|(tag, _)| Some(tag) == tags.setpoint_variable.as_ref())
        );
        assert!(
            writes
                .iter()
                .any(|(tag, _)| Some(tag) == tags.mode_attribute.as_ref())
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, MvActuationKind::Restore);
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].readback_quality, Some(SampleQuality::Bad));
    }

    #[tokio::test]
    async fn restore_audit_failure_does_not_prevent_any_physical_restore_step() {
        let pool = seeded_pool().await;
        let (run_id, _config, template, tags) =
            start_opc_test_run(&pool, "actuation-restore-audit-failure").await;
        pool.close().await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial);
        let guard = MutationGuard {
            mode_attribute_written: true,
            mode_written: true,
            mv_written: true,
        };

        let outcome = attempt_restore_with_actuation(
            &pool,
            run_id,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut CtrlC::never(),
            &mut tracker,
        )
        .await;

        assert!(matches!(outcome, RestoreAttempt::Confirmed));
        let writes = driver.write_log();
        assert!(
            writes
                .iter()
                .any(|(tag, _)| tag == &tags.manipulated_variable)
        );
        assert!(
            writes
                .iter()
                .any(|(tag, _)| Some(tag) == tags.controller_mode.as_ref())
        );
        assert!(
            writes
                .iter()
                .any(|(tag, _)| Some(tag) == tags.setpoint_variable.as_ref())
        );
        assert!(
            writes
                .iter()
                .any(|(tag, _)| Some(tag) == tags.mode_attribute.as_ref())
        );
    }

    #[tokio::test]
    async fn completed_opc_run_audits_final_snapback_through_post_test_padding() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.cycles_skip = Some(0);
        args.cycles_count = Some(1);
        args.mrft_delay = 1;
        args.poll_interval_ms = 20;
        args.timeout_secs = 5;
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            &pool,
            None,
            "actuation-padding-snapback",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        let driver = honeywell_driver_auto()
            .with_read_sequence(&tags.process_variable, &["50", "55", "45", "55"]);

        let outcome = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            RunTimeAnchor::now(),
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, RunOutcome::Completed { .. }));
        let rows = TuneMvActuationRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].kind, MvActuationKind::Relay);
        assert_eq!(rows[2].target_mv, sample_initial_state().mv_ini);
        assert_eq!(rows[2].status, MvActuationStatus::Confirmed);
        assert!(
            TuneSampleRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .len()
                > 3,
            "post-test padding must continue recording PV samples"
        );
    }

    #[tokio::test]
    async fn execute_rejects_a_zero_effective_relay_step_before_any_mutation() {
        let pool = seeded_pool().await;
        let (run_id, config, template, tags) =
            start_opc_test_run(&pool, "actuation-zero-step").await;
        let driver = honeywell_driver_auto().with_value(&tags.manipulated_variable, "100");
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;

        let error = execute(
            &pool,
            run_id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            RunTimeAnchor::now(),
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("too small to verify safely"));
        assert!(driver.write_log().is_empty());
    }

    #[test]
    fn shared_restore_timeout_policy_keeps_simulator_positive_only() {
        assert!(validate_restore_timeout_secs(DriverKind::Simulator, 0).is_err());
        assert!(validate_restore_timeout_secs(DriverKind::Simulator, 1).is_ok());
        assert!(
            validate_restore_timeout_secs(DriverKind::Opcda, MV_ACTUATION_CONFIRMATION_SECS - 1)
                .is_err()
        );
        assert!(
            validate_restore_timeout_secs(DriverKind::Opcda, MV_ACTUATION_CONFIRMATION_SECS)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn prepare_rejects_short_opcda_restore_timeout_before_driver_or_database_mutation() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.server = Some("Mock.Server".to_string());
        args.bridge_host = Some("127.0.0.1:1".to_string());
        let mut config = test_config();
        config.tuning.restore_timeout_secs = Some(MV_ACTUATION_CONFIRMATION_SECS - 1);

        let error = prepare(&pool, args, &config)
            .await
            .err()
            .expect("short OPC DA restore timeout must be rejected");

        assert!(error.to_string().contains("tuning.restore_timeout_secs"));
        assert!(
            TuneRunRow::list(
                &pool,
                &bhtune_db::models::TuneRunFilter::default(),
                bhtune_db::models::Pagination::first(10),
            )
            .await
            .unwrap()
            .is_empty(),
            "validation must run before the tune_runs insert"
        );
    }

    async fn assert_prepare_metadata_failure_is_terminal(column: &str) {
        let pool = seeded_pool().await;
        let trigger_name = format!("fail_{column}");
        let trigger = format!(
            "CREATE TRIGGER {trigger_name} \
             BEFORE UPDATE OF {column} ON tune_runs
             BEGIN SELECT RAISE(ABORT, 'injected metadata failure'); END"
        );
        sqlx::query(sqlx::AssertSqlSafe(trigger.as_str()))
            .execute(&pool)
            .await
            .unwrap();

        let error = prepare(&pool, fast_simulator_args(), &test_config())
            .await
            .err()
            .expect("the injected metadata failure should abort preparation");
        assert!(error.to_string().contains("injected metadata failure"));

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Failed);
        assert!(
            runs[0]
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("injected metadata failure"))
        );
    }

    #[tokio::test]
    async fn every_prepare_metadata_failure_marks_the_run_terminal() {
        for column in [
            "effective_tuning_json",
            "allow_uncertain_quality",
            "request_json",
            "notes",
        ] {
            assert_prepare_metadata_failure_is_terminal(column).await;
        }
    }

    #[tokio::test]
    async fn prepare_deletes_the_run_when_terminalization_also_fails() {
        let pool = seeded_pool().await;
        sqlx::query(
            "CREATE TRIGGER fail_effective_tuning_update \
             BEFORE UPDATE OF effective_tuning_json ON tune_runs
             BEGIN SELECT RAISE(ABORT, 'injected metadata failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_terminal_update \
             BEFORE UPDATE OF outcome ON tune_runs
             BEGIN SELECT RAISE(ABORT, 'injected terminalization failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let error = prepare(&pool, fast_simulator_args(), &test_config())
            .await
            .err()
            .expect("the injected metadata failure should abort preparation");
        assert!(error.to_string().contains("injected metadata failure"));

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert!(
            runs.is_empty(),
            "the row must be removed when its failed outcome cannot be persisted"
        );
    }

    #[tokio::test]
    async fn finalize_preparation_failure_reports_a_failed_cleanup() {
        let pool = seeded_pool().await;
        pool.close().await;

        finalize_preparation_failure(&pool, 1, "injected preparation failure").await;
    }

    #[tokio::test]
    async fn execute_aborts_and_records_reason_when_a_relay_command_is_unconfirmed() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.noise_protection_secs = Some(0);
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            &pool,
            None,
            "actuation-abort",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        let driver =
            honeywell_driver_auto().distorting_first_writes(&tags.manipulated_variable, 1, -5.0);

        let outcome = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            RunTimeAnchor::now(),
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed { .. })
        ));
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stored.outcome, bhtune_db::models::TuneOutcome::Aborted);
        assert!(
            stored
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("MV actuation unconfirmed"))
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, MvActuationKind::Relay);
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
        assert_eq!(rows[1].kind, MvActuationKind::Restore);
        assert_eq!(rows[1].status, MvActuationStatus::Confirmed);
    }

    #[tokio::test]
    async fn execute_finalizes_pending_actuation_when_sample_persistence_fails() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.noise_protection_secs = Some(10);
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "actuation-db-failure",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
        )
        .await
        .unwrap();
        let initial = sample_initial_state();
        let engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            time_anchor.utc(),
            MrftCompat::default(),
        );
        TuneSampleRow::insert(
            &pool,
            run.id,
            0,
            Tick {
                time: time_anchor.utc(),
                pv: initial.pv_ini,
            },
            engine.state(),
            SampleQuality::Good,
        )
        .await
        .unwrap();

        let error = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &honeywell_driver_auto(),
            config,
            time_anchor,
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("UNIQUE constraint failed"));
        let rows = TuneMvActuationRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        assert!(!rows.is_empty());
        assert!(
            rows.iter()
                .all(|row| row.status != MvActuationStatus::Pending)
        );
    }

    #[tokio::test]
    async fn restore_incomplete_takes_precedence_over_actuation_failure() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.noise_protection_secs = Some(0);
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            &pool,
            None,
            "actuation-and-restore-fail",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        let driver = honeywell_driver_auto()
            .distorting_first_writes(&tags.manipulated_variable, 1, -5.0)
            .rejecting_write_after(&tags.manipulated_variable, 1);

        let outcome = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            RunTimeAnchor::now(),
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, RunOutcome::RestoreIncomplete { .. }));
        assert_eq!(
            tune_outcome_for_run(&outcome),
            TuneOutcome::RestoreIncomplete
        );
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stored.outcome, bhtune_db::models::TuneOutcome::Aborted);
        assert!(
            stored
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("MV actuation unconfirmed"))
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
    }

    #[tokio::test]
    async fn audit_cleanup_failure_does_not_override_restore_incomplete() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "actuation-cleanup-failure",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();
        TuneMvActuationRow::insert_pending(
            &pool,
            run.id,
            NewTuneMvActuation {
                sequence: 0,
                kind: MvActuationKind::Relay,
                commanded_at: started_at,
                target_mv: 55.0,
                previous_commanded_mv: Some(45.0),
                tolerance: 0.1,
                confirmation_due_at: started_at + chrono::Duration::seconds(4),
            },
        )
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_actuation_cleanup \
             BEFORE UPDATE OF status ON tune_mv_actuations \
             WHEN OLD.status = 'pending' AND NEW.status = 'unverified' \
             BEGIN SELECT RAISE(FAIL, 'forced actuation cleanup failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
        let driver = honeywell_driver_auto()
            .degrade_quality_after(&tags.process_variable, 1, bhtune_driver::Quality::Bad)
            .erroring_write(&tags.manipulated_variable);

        let outcome = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            RunTimeAnchor::now(),
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, RunOutcome::RestoreIncomplete { .. }));
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stored.outcome, bhtune_db::models::TuneOutcome::Aborted);
        assert_eq!(
            stored.restore_status,
            Some(bhtune_db::models::RestoreStatus::Incomplete)
        );
    }

    // --- `check_quality`: finding 5's single enforcement choke point ------------------------

    #[test]
    fn check_quality_accepts_good_regardless_of_the_quality_policy() {
        assert!(check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Good, false).is_ok());
        assert!(check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Good, true).is_ok());
    }

    #[test]
    fn check_quality_rejects_uncertain_unless_the_policy_allows_it() {
        let err =
            check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Uncertain, false).unwrap_err();
        assert!(err.to_string().contains("Uncertain"));
        assert!(err.to_string().contains("Unit1.LIC101.PV"));
        assert!(check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Uncertain, true).is_ok());
    }

    #[test]
    fn check_quality_never_accepts_bad_regardless_of_the_policy() {
        let err_without_flag =
            check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Bad, false).unwrap_err();
        assert!(err_without_flag.to_string().contains("Bad"));
        let err_with_flag =
            check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Bad, true).unwrap_err();
        assert!(err_with_flag.to_string().contains("Bad"));
    }

    // --- `pid_value_within_tolerance`: finding 6's write-back confirmation rule -------------

    #[test]
    fn pid_value_within_tolerance_accepts_an_exact_match() {
        assert!(pid_value_within_tolerance(10.0, 10.0));
        assert!(pid_value_within_tolerance(0.0, 0.0));
    }

    #[test]
    fn pid_value_within_tolerance_accepts_within_the_one_percent_relative_band() {
        // 1% of 10.0 is 0.1, so 10.09 is inside and 10.2 is outside.
        assert!(pid_value_within_tolerance(10.0, 10.09));
        assert!(pid_value_within_tolerance(10.0, 9.91));
        assert!(!pid_value_within_tolerance(10.0, 10.2));
        assert!(!pid_value_within_tolerance(10.0, 9.8));
    }

    #[test]
    fn pid_value_within_tolerance_uses_the_absolute_floor_near_zero() {
        // 1% of a requested 0.0 (or anything smaller than 0.1) would be a tolerance under
        // 1e-3, which would reject even a harmless floating-point rounding difference -- the
        // absolute 1e-3 floor exists precisely for a requested `D = 0` on a PI controller.
        assert!(pid_value_within_tolerance(0.0, 0.0009));
        assert!(!pid_value_within_tolerance(0.0, 0.002));
    }

    #[test]
    fn pid_value_within_tolerance_handles_negative_requested_values() {
        // The tolerance formula uses `requested.abs()`, so the relative band is symmetric
        // around a negative requested value too (relevant for reverse-acting controllers'
        // sign conventions).
        assert!(pid_value_within_tolerance(-10.0, -10.09));
        assert!(!pid_value_within_tolerance(-10.0, -10.2));
    }

    /// "Honeywell Experion" is the one built-in template with every optional tag (setpoint,
    /// mode, mode attribute, PID constants) configured, making it the right fixture for
    /// exercising every opcda-style branch in one place.
    fn honeywell_template() -> DcsTemplate {
        bhtune_core::built_in_templates()
            .into_iter()
            .find(|t| t.name == "Honeywell Experion")
            .expect("Honeywell Experion is a built-in template")
    }

    fn honeywell_tags() -> LoopTags {
        LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &honeywell_template())
    }

    fn yokogawa_template() -> DcsTemplate {
        bhtune_core::built_in_templates()
            .into_iter()
            .find(|t| t.name == "Yokogawa CentumVP")
            .expect("Yokogawa CentumVP is a built-in template")
    }

    fn yokogawa_tags() -> LoopTags {
        LoopTags::derive_from_pv_tag("FCS0217!204FC03010.PV", &yokogawa_template())
    }

    fn yokogawa_driver_auto() -> MockDriver {
        MockDriver::new(&[
            ("FCS0217!204FC03010.PV", "50.0"),
            ("FCS0217!204FC03010.MV", "45.0"),
            ("FCS0217!204FC03010.MODE", "AUT"),
            ("FCS0217!204FC03010.DR", "0"),
            ("FCS0217!204FC03010.SH", "100.0"),
            ("FCS0217!204FC03010.SL", "0.0"),
            ("FCS0217!204FC03010.MSH", "100.0"),
            ("FCS0217!204FC03010.MSL", "0.0"),
            ("FCS0217!204FC03010.SV", "55.0"),
            ("FCS0217!204FC03010.P", "10.0"),
            ("FCS0217!204FC03010.I", "2.0"),
            ("FCS0217!204FC03010.D", "0.5"),
        ])
    }

    /// A `MockDriver` pre-populated with every tag `honeywell_tags()` derives, using values
    /// that make the loop initially Auto (`MODE=1`) with its Mode Attribute not yet at the
    /// Program value (`MODEATTR=1`, program value is `"2"`) — the common starting point most
    /// of the tests below share before diverging.
    fn honeywell_driver_auto() -> MockDriver {
        MockDriver::new(&[
            ("Unit1.LIC101.PV", "50.0"),
            ("Unit1.LIC101.OP", "45.0"),
            ("Unit1.LIC101.MODE", "1"),
            ("Unit1.LIC101.MODEATTR", "1"),
            ("Unit1.LIC101.CTLACTN", "0"),
            ("Unit1.LIC101.PVEUHI", "100.0"),
            ("Unit1.LIC101.PVEULO", "0.0"),
            ("Unit1.LIC101.CVEUHI", "100.0"),
            ("Unit1.LIC101.CVEULO", "0.0"),
            ("Unit1.LIC101.SP", "55.0"),
            ("Unit1.LIC101.K", "10.0"),
            ("Unit1.LIC101.T1", "2.0"),
            ("Unit1.LIC101.T2", "0.5"),
        ])
    }

    #[tokio::test]
    async fn read_initial_values_batches_the_opcda_tag_set_before_auto_setpoint() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();

        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        assert_eq!(initial.pv_ini, 50.0);
        assert_eq!(initial.mv_ini, 45.0);
        assert_eq!(initial.pv_range_high, 100.0);
        assert_eq!(initial.pv_range_low, 0.0);
        assert_eq!(initial.mv_range_high, 100.0);
        assert_eq!(initial.mv_range_low, 0.0);
        assert_eq!(initial.direction, ControllerDirection::Direct);
        assert_eq!(initial.mode_raw.as_deref(), Some("1"));
        assert_eq!(initial.mode_attribute_raw.as_deref(), Some("1"));
        assert_eq!(
            driver.read_batches(),
            vec![
                vec![
                    "Unit1.LIC101.PV".to_string(),
                    "Unit1.LIC101.OP".to_string(),
                    "Unit1.LIC101.MODE".to_string(),
                    "Unit1.LIC101.MODEATTR".to_string(),
                    "Unit1.LIC101.CTLACTN".to_string(),
                    "Unit1.LIC101.PVEUHI".to_string(),
                    "Unit1.LIC101.PVEULO".to_string(),
                    "Unit1.LIC101.CVEUHI".to_string(),
                    "Unit1.LIC101.CVEULO".to_string(),
                ],
                vec!["Unit1.LIC101.SP".to_string()],
            ]
        );
    }

    #[tokio::test]
    async fn read_initial_values_batches_manual_tags_without_reading_setpoint() {
        let template = yokogawa_template();
        let tags = yokogawa_tags();
        let driver = MockDriver::new(&[
            ("FCS0217!204FC03010.PV", "50.0"),
            ("FCS0217!204FC03010.MV", "45.0"),
            ("FCS0217!204FC03010.MODE", "MAN"),
            ("FCS0217!204FC03010.DR", "0"),
            ("FCS0217!204FC03010.SH", "100.0"),
            ("FCS0217!204FC03010.SL", "0.0"),
            ("FCS0217!204FC03010.MSH", "100.0"),
            ("FCS0217!204FC03010.MSL", "0.0"),
        ]);

        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();

        assert_eq!(initial.mode_raw.as_deref(), Some("MAN"));
        assert_eq!(initial.setpoint_ini, None);
        assert_eq!(
            driver.read_batches(),
            vec![vec![
                "FCS0217!204FC03010.PV".to_string(),
                "FCS0217!204FC03010.MV".to_string(),
                "FCS0217!204FC03010.MODE".to_string(),
                "FCS0217!204FC03010.DR".to_string(),
                "FCS0217!204FC03010.SH".to_string(),
                "FCS0217!204FC03010.SL".to_string(),
                "FCS0217!204FC03010.MSH".to_string(),
                "FCS0217!204FC03010.MSL".to_string(),
            ]]
        );
    }

    #[tokio::test]
    async fn read_initial_values_deduplicates_tags_and_skips_fixed_overrides() {
        let template = honeywell_template();
        let mut tags = honeywell_tags();
        tags.manipulated_variable = tags.process_variable.clone();
        tags.controller_mode = None;
        tags.mode_attribute = None;
        tags.controller_direction = TagOrValue::Value(ControllerDirection::Reverse);
        tags.upper_pv_range = TagOrValue::Value(100.0);
        tags.lower_pv_range = TagOrValue::Value(0.0);
        tags.upper_mv_range = TagOrValue::Value(100.0);
        tags.lower_mv_range = TagOrValue::Value(0.0);
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "50.0")]);

        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();

        assert_eq!(initial.pv_ini, 50.0);
        assert_eq!(initial.mv_ini, 50.0);
        assert_eq!(initial.direction, ControllerDirection::Reverse);
        assert_eq!(
            driver.read_batches(),
            vec![vec!["Unit1.LIC101.PV".to_string()]]
        );
    }

    #[tokio::test]
    async fn read_initial_values_errors_when_a_tag_returns_no_value() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().empty_read("Unit1.LIC101.PV");

        let err = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no value"));
    }

    /// Matches `honeywell_driver_auto()`'s values -- a valid baseline for
    /// `validate_initial_state` tests to mutate one field at a time from. `setpoint_ini` is
    /// `Some(55.0)`, matching the fixture's `SP` tag, since `mode_raw` ("1") equals the
    /// template's `mode_auto_value` -- exactly the condition `read_initial_values` itself
    /// checks before reading the setpoint.
    fn sample_initial_state() -> InitialState {
        InitialState {
            pv_ini: 50.0,
            mv_ini: 45.0,
            pv_range_high: 100.0,
            pv_range_low: 0.0,
            mv_range_high: 100.0,
            mv_range_low: 0.0,
            direction: ControllerDirection::Direct,
            mode_raw: Some("1".to_string()),
            mode_attribute_raw: Some("1".to_string()),
            setpoint_ini: Some(55.0),
        }
    }

    #[test]
    fn validate_initial_state_accepts_a_typical_reading() {
        assert!(validate_initial_state(&sample_initial_state()).is_ok());
    }

    #[test]
    fn validate_initial_state_rejects_a_zero_span_pv_range() {
        let mut initial = sample_initial_state();
        initial.pv_range_high = 50.0;
        initial.pv_range_low = 50.0;
        let err = validate_initial_state(&initial).unwrap_err();
        assert!(err.to_string().contains("PV range"));
    }

    #[test]
    fn validate_initial_state_rejects_an_mv_range_with_low_not_below_high() {
        let mut initial = sample_initial_state();
        initial.mv_range_high = 0.0;
        initial.mv_range_low = 100.0;
        let err = validate_initial_state(&initial).unwrap_err();
        assert!(err.to_string().contains("MV range"));
    }

    #[test]
    fn validate_initial_state_rejects_equal_mv_range_bounds() {
        let mut initial = sample_initial_state();
        initial.mv_range_high = 50.0;
        initial.mv_range_low = 50.0;
        assert!(validate_initial_state(&initial).is_err());
    }

    #[test]
    fn validate_initial_state_rejects_an_initial_mv_outside_the_mv_range() {
        let mut initial = sample_initial_state();
        initial.mv_ini = 150.0;
        let err = validate_initial_state(&initial).unwrap_err();
        assert!(err.to_string().contains("outside the MV range"));
    }

    #[test]
    fn validate_initial_state_accepts_the_initial_mv_on_the_range_boundary() {
        let mut initial = sample_initial_state();
        initial.mv_ini = initial.mv_range_high;
        assert!(validate_initial_state(&initial).is_ok());
    }

    // --- finding 5, end to end: a poor-quality initial reading must fail `execute` before --
    // --- any mutation of the loop, exactly like finding 4's invalid-range checks below -----

    #[tokio::test]
    async fn execute_hard_fails_when_the_pv_tag_reports_bad_quality() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Bad);
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "bad-quality-initial",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
        )
        .await
        .unwrap();

        let err = execute(
            &pool,
            run.id,
            &fast_simulator_args(),
            &template,
            &tags,
            &driver,
            config,
            time_anchor,
            None,
            true,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains(&tags.process_variable));
        assert!(err.to_string().contains("Bad"));
        // Nothing was mutated -- the mode transition never ran, matching the invalid-range
        // test's own safety assertion below.
        assert!(driver.write_log().is_empty());
    }

    #[tokio::test]
    async fn execute_hard_fails_when_the_pv_tag_reports_uncertain_quality_policy_rejects_it() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Uncertain);
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "uncertain-quality-initial",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
        )
        .await
        .unwrap();

        let err = execute(
            &pool,
            run.id,
            &fast_simulator_args(),
            &template,
            &tags,
            &driver,
            config,
            time_anchor,
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains(&tags.process_variable));
        assert!(err.to_string().contains("Uncertain"));
        assert!(driver.write_log().is_empty());
    }

    #[tokio::test]
    async fn read_initial_values_and_transition_accept_uncertain_pv_quality_when_policy_allows_it()
    {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Uncertain);

        let initial = read_initial_values(&driver, &tags, &template, true)
            .await
            .unwrap();
        assert_eq!(initial.pv_ini, 50.0);

        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        // Proves the run actually proceeded to mutate the loop (the mode/mode-attribute
        // writes `transition_to_manual` performs), not just that `read_initial_values`
        // alone returned `Ok` -- the real proof the global quality policy has an effect,
        // not just that this specific error string disappeared.
        assert!(!driver.write_log().is_empty());
    }

    #[tokio::test]
    async fn read_initial_values_hard_fails_when_the_setpoint_tag_reports_bad_quality() {
        // The Honeywell fixture starts in Auto (`MODE=1` == `mode_auto_value`), so
        // `read_initial_values` reads the setpoint tag as part of computing
        // `InitialState::setpoint_ini` -- finding 5 applies to that read exactly as it does
        // to every other tuning-critical read. This read was hoisted out of
        // `transition_to_manual` (see `InitialState::setpoint_ini`'s doc comment) so it can
        // be persisted before any mutation is attempted; this test moved with it.
        let template = honeywell_template();
        let tags = honeywell_tags();
        let sp_tag = tags.setpoint_variable.clone().unwrap();
        let driver = honeywell_driver_auto().with_quality(&sp_tag, bhtune_driver::Quality::Bad);

        let err = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains(&sp_tag));
        assert!(err.to_string().contains("Bad"));
    }

    /// The end-to-end proof that finding 4's fix closes the actual safety gap, not just the
    /// isolated unit: a driver reporting an MV range with `low >= high` must fail `execute`
    /// before `transition_to_manual`'s first write -- i.e. before the loop is touched at
    /// all, not merely before the tuning math runs.
    #[tokio::test]
    async fn execute_rejects_an_invalid_mv_range_before_any_mutation_of_the_loop() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // Swaps CVEUHI/CVEULO so mv_range_high (0.0) < mv_range_low (100.0), violating
        // MvRange::new's low-strictly-below-high requirement.
        let driver = honeywell_driver_auto()
            .with_value("Unit1.LIC101.CVEUHI", "0.0")
            .with_value("Unit1.LIC101.CVEULO", "100.0");
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "invalid-mv-range",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
        )
        .await
        .unwrap();

        let err = execute(
            &pool,
            run.id,
            &fast_simulator_args(),
            &template,
            &tags,
            &driver,
            config,
            time_anchor,
            None,
            true,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("MV range"));
        // The real safety property: no write ever reached the driver -- not the mode
        // attribute, not the mode, not the MV. `transition_to_manual` never ran.
        assert!(driver.write_log().is_empty());
    }

    /// Covers `execute`'s first `restore_best_effort_then_propagate` call site: a failure
    /// from `transition_to_manual` itself, partway through (the mode-attribute write, the
    /// very first write it attempts) must still trigger a best-effort restore rather than
    /// propagating the error with the loop left half-mutated. `honeywell_driver_auto()`
    /// starts in Auto with `MODEATTR` not yet at the Program value, so `transition_to_manual`
    /// always attempts the mode-attribute write first.
    #[tokio::test]
    async fn execute_attempts_restore_when_transition_to_manual_fails_partway() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().erroring_write("Unit1.LIC101.MODEATTR");
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "transition-to-manual-fails",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
        )
        .await
        .unwrap();

        let err = execute(
            &pool,
            run.id,
            &fast_simulator_args(),
            &template,
            &tags,
            &driver,
            config,
            time_anchor,
            None,
            true,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        // `restore_best_effort_then_propagate` always returns the *original* error
        // unchanged -- this is that original `transition_to_manual` failure, not some
        // restore-side error masking it.
        assert!(err.to_string().contains("driver operation failed"));

        // `restore()`'s MV step is unconditional (never gated by the guard), so it still ran
        // despite `transition_to_manual` never getting anywhere near the MV -- proving the
        // restore was genuinely attempted, not skipped because "nothing was mutated yet".
        assert!(
            driver
                .write_log()
                .iter()
                .any(|(tag, _)| tag == "Unit1.LIC101.OP")
        );
        // `guard.mode_written` was correctly never armed: `transition_to_manual` failed on
        // the mode-attribute write, before it ever reached the mode write, so `restore()`
        // must not attempt to revert a mode change that was never made.
        assert!(
            driver
                .write_log()
                .iter()
                .all(|(tag, _)| tag != "Unit1.LIC101.MODE")
        );

        // The mode-attribute *restore* step, unlike mode/setpoint, retries the same
        // permanently-erroring tag and fails again -- so the restore is recorded as
        // incomplete, naming exactly which step could not be confirmed.
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(
            stored.restore_status,
            Some(bhtune_db::models::RestoreStatus::Incomplete)
        );
        assert!(
            stored
                .restore_detail
                .as_deref()
                .unwrap_or_default()
                .contains("mode attribute")
        );
    }

    async fn run_completed_result_persistence_conflict(
        successful_writes: u32,
    ) -> (anyhow::Result<RunOutcome>, TuneRunRow) {
        let pool = seeded_pool().await;
        let template = bhtune_core::built_in_templates().remove(0);
        let args = fast_simulator_args();
        let config = build_loop_config(&args).unwrap();
        let tags = build_loop_tags(&args, &template).unwrap();
        let simulator = bhtune_driver::SimulatorDriver::new(
            SIMULATOR_PV_TAG,
            SIMULATOR_MV_TAG,
            bhtune_driver::FopdtConfig::new(
                args.sim_gain,
                args.sim_tau,
                args.sim_dead_time,
                args.poll_interval_ms as f32 / 1000.0,
            ),
            args.sim_initial_pv,
            args.sim_initial_mv,
            args.sim_seed,
        );
        let driver = RestoreFailingSimulator {
            inner: simulator,
            writes: std::sync::Mutex::new(0),
            successful_writes,
        };
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "finish-completed-run-fails",
            TuneDriver::Simulator,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
        )
        .await
        .unwrap();

        // Pre-inserts a conflicting row so `persist_results`'s own insert of the same
        // `(run_id, Aggressive)` pair -- once the simulator tune genuinely completes --
        // collides with the `UNIQUE (run_id, response_level)` constraint and fails
        // deterministically, without needing to fake or corrupt anything about the tune
        // itself.
        TuneResultRow::insert(
            &pool,
            &TuneResultRow {
                id: 0,
                run_id: run.id,
                response_level: ResponseLevel::Aggressive,
                kp: Some(1.0),
                ti_minutes: Some(1.0),
                td_minutes: Some(1.0),
                proportional: Some(1.0),
                integral: Some(1.0),
                derivative: Some(1.0),
                status: TuningResultStatus::Valid,
                invalid_reason: None,
            },
        )
        .await
        .unwrap();

        let result = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            time_anchor,
            None,
            true,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await;
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        (result, stored)
    }

    /// Covers `execute`'s second `restore_best_effort_then_propagate` call site: a failure in
    /// `persist_completed_results` (here, `persist_results` colliding with the
    /// `UNIQUE (run_id, response_level)` constraint) *after* a real, successful MRFT
    /// completion must still trigger a best-effort restore. The restore is deliberately
    /// allowed to fail here so the combined result-persistence-failed/restore-incomplete
    /// branch remains covered.
    #[tokio::test]
    async fn execute_attempts_restore_when_persist_completed_results_fails() {
        let (result, stored) = run_completed_result_persistence_conflict(7).await;

        // Loose assertion by design, matching this codebase's existing convention for
        // constraint-violation tests (see `tests/schema.rs`): SQLite's exact wording for a
        // UNIQUE-constraint violation is version/implementation detail, not part of this
        // test's contract.
        assert!(result.is_err());

        // The simulator driver has no mode/setpoint/mode-attribute tags, so the rejected
        // MV write is the only incomplete restore step. The persistence error remains the
        // returned error while restore status is recorded independently.
        assert_eq!(
            stored.restore_status,
            Some(bhtune_db::models::RestoreStatus::Incomplete)
        );
        let timing = stored
            .timing_metrics
            .expect("cadence metrics should survive a post-poll persistence failure");
        assert!(timing.sample_gap_count > 0);
        assert_eq!(timing.measured_oscillation_period_ms, None);
        assert_eq!(timing.approximate_samples_per_period, None);
    }

    #[tokio::test]
    async fn execute_preserves_persistence_error_when_restore_is_confirmed() {
        let (result, stored) = run_completed_result_persistence_conflict(100).await;

        assert!(result.is_err());
        assert_eq!(
            stored.restore_status,
            Some(bhtune_db::models::RestoreStatus::Confirmed)
        );
        let timing = stored
            .timing_metrics
            .expect("cadence metrics should survive a post-poll persistence failure");
        assert!(timing.sample_gap_count > 0);
        assert_eq!(timing.measured_oscillation_period_ms, None);
        assert_eq!(timing.approximate_samples_per_period, None);
    }

    #[derive(Debug)]
    struct RestoreFailingSimulator {
        inner: bhtune_driver::SimulatorDriver,
        writes: std::sync::Mutex<u32>,
        successful_writes: u32,
    }

    #[async_trait::async_trait]
    impl Driver for RestoreFailingSimulator {
        async fn read(&self, tags: &[String]) -> bhtune_driver::DriverResult<Vec<TagValue>> {
            self.inner.read(tags).await
        }

        async fn write(
            &self,
            tag: &String,
            value: TagWrite,
        ) -> bhtune_driver::DriverResult<bhtune_driver::WriteOutcome> {
            let reject = {
                let mut writes = self.writes.lock().unwrap();
                *writes += 1;
                *writes > self.successful_writes
            };
            if reject {
                Ok(bhtune_driver::WriteOutcome::failure(
                    "restore intentionally rejected",
                ))
            } else {
                self.inner.write(tag, value).await
            }
        }

        async fn browse(
            &self,
            _request: bhtune_driver::BrowsePageRequest,
        ) -> bhtune_driver::DriverResult<bhtune_driver::BrowsePage> {
            Err(bhtune_driver::DriverError::Unsupported {
                operation: "browse",
            })
        }
    }

    #[tokio::test]
    async fn execute_reports_restore_incomplete_after_a_completed_run_cannot_restore_mv() {
        let pool = seeded_pool().await;
        let args = fast_simulator_args();
        let template = bhtune_core::built_in_templates().remove(0);
        let tags = build_loop_tags(&args, &template).unwrap();
        let config = build_loop_config(&args).unwrap();
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "completed-restore-incomplete",
            TuneDriver::Simulator,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
        )
        .await
        .unwrap();
        let simulator = bhtune_driver::SimulatorDriver::new(
            SIMULATOR_PV_TAG,
            SIMULATOR_MV_TAG,
            bhtune_driver::FopdtConfig::new(
                args.sim_gain,
                args.sim_tau,
                args.sim_dead_time,
                args.poll_interval_ms as f32 / 1000.0,
            ),
            args.sim_initial_pv,
            args.sim_initial_mv,
            args.sim_seed,
        );
        let driver = RestoreFailingSimulator {
            inner: simulator,
            writes: std::sync::Mutex::new(0),
            successful_writes: 7,
        };

        let outcome = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            time_anchor,
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, RunOutcome::RestoreIncomplete { .. }));
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(
            stored.restore_status,
            Some(bhtune_db::models::RestoreStatus::Incomplete)
        );
        assert!(matches!(
            driver
                .browse(bhtune_driver::BrowsePageRequest::root(20))
                .await,
            Err(bhtune_driver::DriverError::Unsupported {
                operation: "browse"
            })
        ));
    }

    /// Covers the `Aborted` branch's `RestoreAttempt::Incomplete` mapping -- the sibling of
    /// the `Completed` branch's equivalent (deliberately not separately covered; see
    /// AGENTS.md's `safety-restore-guard` notes) -- with a real, deterministic abort: the PV
    /// tag's quality degrades to `Bad` starting on the very first poll tick (its one
    /// `read_initial_values` read stays `Good`, via `degrade_quality_after`), and the MV tag
    /// is error-injected so the subsequent restore's unconditional MV step fails while
    /// mode/setpoint/mode-attribute all succeed normally. Deliberately plain `#[tokio::test]`
    /// rather than `start_paused = true`: pairing a paused clock with `seeded_pool()`'s real
    /// `sqlx` connection pool reliably deadlocks (`PoolTimedOut`), so -- matching every other
    /// `execute`-level test in this module that needs a real pool -- this test pays the real
    /// wall-clock cost of `transition_to_manual`/`restore`'s inter-write pacing sleeps
    /// instead (~3s: one in `transition_to_manual`, two more in `restore`).
    #[tokio::test]
    async fn execute_reports_restore_incomplete_after_a_poor_quality_abort() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .erroring_write("Unit1.LIC101.OP")
            .degrade_quality_after(&tags.process_variable, 1, bhtune_driver::Quality::Bad);
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "poor-quality-abort-restore-incomplete",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
        )
        .await
        .unwrap();

        let outcome = execute(
            &pool,
            run.id,
            &fast_simulator_args(),
            &template,
            &tags,
            &driver,
            config,
            time_anchor,
            None,
            true,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(&outcome, RunOutcome::RestoreIncomplete { .. }));
        let outcome_text = format!("{outcome:?}");
        assert!(outcome_text.contains("run aborted"));
        assert!(outcome_text.contains("PoorQuality"));
        assert!(outcome_text.contains("MV"));

        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(
            stored.restore_status,
            Some(bhtune_db::models::RestoreStatus::Incomplete)
        );
    }

    #[tokio::test]
    async fn read_f32_errors_on_a_non_numeric_value() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "not-a-number")]);
        let err = read_f32(&driver, "Unit1.LIC101.PV", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a number"));
    }

    #[test]
    fn parse_f32_value_accepts_trimmed_finite_numbers() {
        assert_eq!(parse_f32_value("Unit1.LIC101.PV", " 42.5 ").unwrap(), 42.5);
    }

    /// Rust's `f32::from_str` happily parses the literal strings `"nan"`/`"inf"` --
    /// confirming this gap is what motivated hardening `read_f32` (finding 4 of the
    /// live-plant safety review): a driver tag returning either string used to flow
    /// unchecked into the engine.
    #[tokio::test]
    async fn read_f32_rejects_nan() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "nan")]);
        let err = read_f32(&driver, "Unit1.LIC101.PV", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("finite"));
    }

    #[tokio::test]
    async fn read_f32_rejects_infinity() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "inf")]);
        let err = read_f32(&driver, "Unit1.LIC101.PV", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("finite"));
    }

    #[tokio::test]
    async fn resolve_f32_accepts_a_finite_tag_or_value() {
        let driver = MockDriver::new(&[]);
        let value = resolve_f32(&driver, &TagOrValue::Value(42.0), false)
            .await
            .unwrap();
        assert_eq!(value, 42.0);
    }

    #[tokio::test]
    async fn resolve_f32_reads_a_tag_backed_value() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "42.5")]);
        let value = resolve_f32(
            &driver,
            &TagOrValue::Tag("Unit1.LIC101.PV".to_string()),
            false,
        )
        .await
        .unwrap();
        assert_eq!(value, 42.5);
    }

    #[tokio::test]
    async fn resolve_f32_from_batch_reads_a_tag_from_the_batch() {
        let values = HashMap::from([(
            "Unit1.LIC101.PV".to_string(),
            TagValue {
                tag: "Unit1.LIC101.PV".to_string(),
                value: "42.5".to_string(),
                quality: bhtune_driver::Quality::Good,
                timestamp: None,
            },
        )]);
        let value = resolve_f32_from_batch(
            &MockDriver::default(),
            &values,
            &TagOrValue::Tag("Unit1.LIC101.PV".to_string()),
            false,
        )
        .await
        .unwrap();
        assert_eq!(value, 42.5);
    }

    #[tokio::test]
    async fn resolve_direction_from_batch_reads_and_maps_a_tag() {
        let template = honeywell_template();
        let tags = TagOrValue::Tag("Unit1.LIC101.CTLACTN".to_string());
        let direction_tag = "Unit1.LIC101.CTLACTN".to_string();
        let values = HashMap::from([(
            direction_tag.clone(),
            TagValue {
                tag: direction_tag,
                value: "0".to_string(),
                quality: bhtune_driver::Quality::Good,
                timestamp: None,
            },
        )]);
        let direction =
            resolve_direction_from_batch(&MockDriver::default(), &values, &tags, &template, false)
                .await
                .unwrap();
        assert_eq!(direction, ControllerDirection::Direct);
    }

    #[tokio::test]
    async fn resolve_direction_reads_and_maps_a_tag_directly() {
        let template = honeywell_template();
        let driver = MockDriver::new(&[("Unit1.LIC101.CTLACTN", "0")]);
        let direction = resolve_direction(
            &driver,
            &TagOrValue::Tag("Unit1.LIC101.CTLACTN".to_string()),
            &template,
            false,
        )
        .await
        .unwrap();
        assert_eq!(direction, ControllerDirection::Direct);
    }

    /// Defense in depth against a hypothetical future caller (e.g. a `bhtune-server` HTTP
    /// handler) constructing a `TagOrValue::Value` directly without going through clap's
    /// `finite_f32` parser at all -- see `args::finite_f32`.
    #[tokio::test]
    async fn resolve_f32_rejects_a_non_finite_direct_value() {
        let driver = MockDriver::new(&[]);
        let err = resolve_f32(&driver, &TagOrValue::Value(f32::NAN), false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("finite"));
    }

    #[tokio::test]
    async fn read_pv_sample_rejects_non_finite_values() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "nan")]);
        let err = read_pv_sample(&driver, "Unit1.LIC101.PV")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("finite"));
    }

    #[test]
    fn read_batch_f32_returns_a_good_numeric_value() {
        let values = HashMap::from([(
            "Unit1.LIC101.PV".to_string(),
            TagValue {
                tag: "Unit1.LIC101.PV".to_string(),
                value: "42.5".to_string(),
                quality: bhtune_driver::Quality::Good,
                timestamp: None,
            },
        )]);

        assert_eq!(
            read_batch_f32(&values, "Unit1.LIC101.PV", false).unwrap(),
            42.5
        );
    }

    #[tokio::test]
    async fn read_poll_batch_maps_reordered_responses_by_tag() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "42.5"), ("Unit1.LIC101.OP", "60.0")])
            .reversing_read_results();

        let values = read_poll_batch(&driver, "Unit1.LIC101.PV", Some("Unit1.LIC101.OP"))
            .await
            .unwrap();

        assert_eq!(
            read_numeric_from_batch(&values, "Unit1.LIC101.PV")
                .unwrap()
                .0,
            42.5
        );
        assert_eq!(
            read_numeric_from_batch(&values, "Unit1.LIC101.OP")
                .unwrap()
                .0,
            60.0
        );
    }

    #[test]
    fn sample_quality_mapping_covers_all_driver_qualities() {
        assert_eq!(
            sample_quality_from_driver(bhtune_driver::Quality::Good),
            SampleQuality::Good
        );
        assert_eq!(
            sample_quality_from_driver(bhtune_driver::Quality::Uncertain),
            SampleQuality::Uncertain
        );
        assert_eq!(
            sample_quality_from_driver(bhtune_driver::Quality::Bad),
            SampleQuality::Bad
        );
    }

    #[test]
    fn completed_oscillation_period_is_reported_for_a_successful_poll_result() {
        let start = Utc::now();
        let completion = PollOutcome::Completed(CompletedPoll {
            action: Action::Complete {
                peaks: vec![52.0, 48.0, 52.0],
                troughs: vec![46.0, 50.0],
                switch_times: vec![
                    start,
                    start + chrono::Duration::seconds(30),
                    start + chrono::Duration::seconds(60),
                    start + chrono::Duration::seconds(90),
                    start + chrono::Duration::seconds(120),
                ],
                mv_sign_init: 1,
            },
            state: MrftState {
                hysteresis: 0.0,
                mv_value_current: 45.0,
                mv_sign_next_step: 1,
                counter_all_switches: 5,
                cycles_completed: 2,
                cycles_remaining: 0,
            },
            next_tick_index: 0,
            tick_time: TickTimeSource::FixedStep {
                current: start,
                step: chrono::Duration::seconds(1),
            },
        });
        let result = completed_oscillation_period_ms(
            &Ok(completion),
            ControllerDirection::Reverse,
            build_loop_config(&fast_simulator_args()).unwrap(),
            PvRange {
                high: 100.0,
                low: 0.0,
            },
        );
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn read_raw_and_write_raw_propagate_a_hard_driver_error() {
        // Distinct from a *rejected* write (`WriteOutcome::success == false`, handled by
        // `write_raw`/`write_value`'s own "was rejected" message): this is the driver call
        // itself failing (`DriverError::Operation`), which `?` should propagate as-is.
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "50.0")])
            .erroring_read("Unit1.LIC101.PV")
            .erroring_write("Unit1.LIC101.OP");

        let read_err = read_raw(&driver, "Unit1.LIC101.PV", false)
            .await
            .unwrap_err();
        assert!(read_err.to_string().contains("driver operation failed"));

        let write_err = write_value(&driver, "Unit1.LIC101.OP", 45.0)
            .await
            .unwrap_err();
        assert!(write_err.to_string().contains("driver operation failed"));
    }

    #[tokio::test(start_paused = true)]
    async fn transition_to_manual_writes_program_value_and_mode_when_starting_in_auto() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        // The setpoint is captured here, in `read_initial_values`, before any mutation of
        // the loop -- see `InitialState::setpoint_ini`'s doc comment for why -- not during
        // `transition_to_manual` any more.
        assert_eq!(initial.setpoint_ini, Some(55.0));

        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();

        assert_eq!(
            driver.value_of("Unit1.LIC101.MODEATTR").as_deref(),
            Some("2")
        );
        assert_eq!(driver.value_of("Unit1.LIC101.MODE").as_deref(), Some("0"));
        assert!(guard.mode_attribute_written);
        assert!(guard.mode_written);
        // Order matters (mode attribute unlocked before the mode itself is switched), per
        // `ChangeControllerModeToMan`.
        let log = driver.write_log();
        let attr_index = log
            .iter()
            .position(|(t, _)| t == "Unit1.LIC101.MODEATTR")
            .unwrap();
        let mode_index = log
            .iter()
            .position(|(t, _)| t == "Unit1.LIC101.MODE")
            .unwrap();
        assert!(attr_index < mode_index);
    }

    #[tokio::test(start_paused = true)]
    async fn read_initial_values_skips_setpoint_capture_when_original_mode_is_not_auto() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        // "2" is neither the manual ("0") nor auto ("1") raw value — e.g. Cascade.
        let driver = MockDriver::new(&[
            ("Unit1.LIC101.PV", "50.0"),
            ("Unit1.LIC101.OP", "45.0"),
            ("Unit1.LIC101.MODE", "2"),
            ("Unit1.LIC101.MODEATTR", "2"),
            ("Unit1.LIC101.CTLACTN", "0"),
            ("Unit1.LIC101.PVEUHI", "100.0"),
            ("Unit1.LIC101.PVEULO", "0.0"),
            ("Unit1.LIC101.CVEUHI", "100.0"),
            ("Unit1.LIC101.CVEULO", "0.0"),
            ("Unit1.LIC101.SP", "55.0"),
        ]);
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();

        assert_eq!(initial.setpoint_ini, None);

        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();

        assert_eq!(driver.value_of("Unit1.LIC101.MODE").as_deref(), Some("0"));
    }

    #[tokio::test(start_paused = true)]
    async fn transition_to_manual_does_not_rewrite_mode_when_already_manual() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = MockDriver::new(&[
            ("Unit1.LIC101.PV", "50.0"),
            ("Unit1.LIC101.OP", "45.0"),
            ("Unit1.LIC101.MODE", "0"),
            ("Unit1.LIC101.MODEATTR", "2"),
            ("Unit1.LIC101.CTLACTN", "0"),
            ("Unit1.LIC101.PVEUHI", "100.0"),
            ("Unit1.LIC101.PVEULO", "0.0"),
            ("Unit1.LIC101.CVEUHI", "100.0"),
            ("Unit1.LIC101.CVEULO", "0.0"),
            ("Unit1.LIC101.SP", "55.0"),
        ]);
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        assert_eq!(initial.setpoint_ini, None);

        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();

        // The Mode Attribute write always fires unconditionally (there's no "already at the
        // program value" guard on it), but Mode itself is already Manual, so its own
        // conditional `write_raw` must not fire a second time.
        let log = driver.write_log();
        assert_eq!(
            log,
            vec![("Unit1.LIC101.MODEATTR".to_string(), "2".to_string())]
        );
        assert!(guard.mode_attribute_written);
        assert!(!guard.mode_written);
    }

    #[tokio::test(start_paused = true)]
    async fn restore_reverts_mode_setpoint_and_mode_attribute() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();

        let report = restore(&driver, &tags, &template, &initial, &guard).await;
        assert!(report.all_succeeded());

        assert_eq!(driver.value_of("Unit1.LIC101.OP").as_deref(), Some("45")); // mv_ini
        assert_eq!(driver.value_of("Unit1.LIC101.MODE").as_deref(), Some("1")); // original raw
        assert_eq!(driver.value_of("Unit1.LIC101.SP").as_deref(), Some("55")); // setpoint restored
        assert_eq!(
            driver.value_of("Unit1.LIC101.MODEATTR").as_deref(),
            Some("1")
        ); // reverted off the Program value
    }

    #[tokio::test(start_paused = true)]
    async fn restore_skips_mode_revert_when_template_disables_it() {
        let mut template = honeywell_template();
        template.revert_mode = false;
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let writes_before_restore = driver.write_log().len();

        let report = restore(&driver, &tags, &template, &initial, &guard).await;
        assert!(report.all_succeeded());

        // MV is always written back regardless of `revert_mode`; Mode/Setpoint are not.
        assert_eq!(driver.value_of("Unit1.LIC101.OP").as_deref(), Some("45"));
        assert_eq!(driver.value_of("Unit1.LIC101.MODE").as_deref(), Some("0")); // untouched
        let new_writes = &driver.write_log()[writes_before_restore..];
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.MODE"));
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.SP"));
    }

    #[tokio::test(start_paused = true)]
    async fn restore_skips_setpoint_revert_when_original_mode_was_not_auto() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = MockDriver::new(&[
            ("Unit1.LIC101.PV", "50.0"),
            ("Unit1.LIC101.OP", "45.0"),
            ("Unit1.LIC101.MODE", "2"),
            ("Unit1.LIC101.MODEATTR", "2"),
            ("Unit1.LIC101.CTLACTN", "0"),
            ("Unit1.LIC101.PVEUHI", "100.0"),
            ("Unit1.LIC101.PVEULO", "0.0"),
            ("Unit1.LIC101.CVEUHI", "100.0"),
            ("Unit1.LIC101.CVEULO", "0.0"),
            ("Unit1.LIC101.SP", "55.0"),
        ]);
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let writes_before_restore = driver.write_log().len();

        let report = restore(&driver, &tags, &template, &initial, &guard).await;
        assert!(report.all_succeeded());

        assert_eq!(driver.value_of("Unit1.LIC101.MODE").as_deref(), Some("2")); // reverted
        let new_writes = &driver.write_log()[writes_before_restore..];
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.SP"));
    }

    #[tokio::test(start_paused = true)]
    async fn restore_skips_mode_attribute_revert_when_already_at_program_value() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = MockDriver::new(&[
            ("Unit1.LIC101.PV", "50.0"),
            ("Unit1.LIC101.OP", "45.0"),
            ("Unit1.LIC101.MODE", "1"),
            ("Unit1.LIC101.MODEATTR", "2"), // already at the Program value
            ("Unit1.LIC101.CTLACTN", "0"),
            ("Unit1.LIC101.PVEUHI", "100.0"),
            ("Unit1.LIC101.PVEULO", "0.0"),
            ("Unit1.LIC101.CVEUHI", "100.0"),
            ("Unit1.LIC101.CVEULO", "0.0"),
            ("Unit1.LIC101.SP", "55.0"),
        ]);
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let writes_before_restore = driver.write_log().len();

        let report = restore(&driver, &tags, &template, &initial, &guard).await;
        assert!(report.all_succeeded());

        let new_writes = &driver.write_log()[writes_before_restore..];
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.MODEATTR"));
    }

    /// The heart of `safety-restore-guard`'s "aggregated best-effort restore" (Option C): one
    /// step failing must never prevent the others from being *attempted*, even in the
    /// pathological case where every single one of them also fails. Calls `restore` directly
    /// with a fully-armed `guard` (bypassing `transition_to_manual` entirely, since this test
    /// is only interested in `restore`'s own aggregation behavior in isolation, not in
    /// propagating any one mutation's own error -- that's covered by the `execute`-level
    /// tests around `restore_best_effort_then_propagate` instead).
    #[tokio::test(start_paused = true)]
    async fn restore_reports_each_step_failed_independently_without_short_circuiting() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .erroring_write("Unit1.LIC101.OP")
            .erroring_write("Unit1.LIC101.MODE")
            .erroring_write("Unit1.LIC101.SP")
            .erroring_write("Unit1.LIC101.MODEATTR");
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let guard = MutationGuard {
            mode_attribute_written: true,
            mode_written: true,
            mv_written: true,
        };

        let report = restore(&driver, &tags, &template, &initial, &guard).await;

        assert!(!report.all_succeeded());
        assert!(matches!(report.mv, RestoreStepOutcome::Failed(_)));
        assert!(matches!(report.mode, RestoreStepOutcome::Failed(_)));
        assert!(matches!(report.setpoint, RestoreStepOutcome::Failed(_)));
        assert!(matches!(
            report.mode_attribute,
            RestoreStepOutcome::Failed(_)
        ));

        // Every step's own failure is independently attributable in the summary -- an
        // operator reading this must be able to tell all four apart, not just "something
        // failed".
        let summary = report.failure_summary().unwrap();
        assert!(summary.contains("MV:"));
        assert!(summary.contains("mode:"));
        assert!(summary.contains("setpoint:"));
        assert!(summary.contains("mode attribute:"));
    }

    #[test]
    fn restore_report_failure_summary_is_none_when_nothing_failed() {
        // The complement of the all-failed test above: a report where every step is
        // `NotNeeded` (the `Default`) has nothing to summarize.
        let report = RestoreReport::default();
        assert!(report.all_succeeded());
        assert!(report.failure_summary().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn write_raw_and_write_value_error_when_the_driver_rejects_the_write() {
        let driver = MockDriver::new(&[("Unit1.LIC101.MODE", "1")])
            .rejecting_write("Unit1.LIC101.MODE")
            .rejecting_write("Unit1.LIC101.OP");

        let raw_err = write_raw(&driver, "Unit1.LIC101.MODE", "0".to_string())
            .await
            .unwrap_err();
        assert!(raw_err.to_string().contains("rejected"));

        let value_err = write_value(&driver, "Unit1.LIC101.OP", 45.0)
            .await
            .unwrap_err();
        assert!(value_err.to_string().contains("rejected"));
    }

    #[tokio::test]
    async fn persist_results_bails_on_a_non_complete_action() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let err = persist_results(
            &pool,
            1,
            Action::WriteMv(0.0),
            ControllerDirection::Direct,
            build_loop_config(&fast_simulator_args()).unwrap(),
            PvRange {
                high: 100.0,
                low: 0.0,
            },
            &template,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("internal error"));
    }

    /// Sets up a run with 3 recorded `TuneResultRow`s (matching a real completed tune) using
    /// the Honeywell template/tags, whose PID constant tags are all configured — the
    /// precondition for `maybe_write_back` to prompt at all rather than skip immediately.
    async fn run_with_recorded_results() -> (SqlitePool, i64) {
        let pool = seeded_pool().await;
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let run = TuneRunRow::start(
            &pool,
            None,
            "write-back-test",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &honeywell_template(),
            &honeywell_tags(),
            Utc::now(),
        )
        .await
        .unwrap();
        for (level, kp, ti, td, p, i, d) in [
            (ResponseLevel::Aggressive, 1.0, 0.5, 0.1, 10.0, 2.0, 0.5),
            (ResponseLevel::Moderate, 1.5, 0.7, 0.15, 12.0, 2.5, 0.6),
            (ResponseLevel::Sluggish, 2.0, 0.9, 0.2, 14.0, 3.0, 0.7),
        ] {
            TuneResultRow::insert(
                &pool,
                &TuneResultRow {
                    id: 0,
                    run_id: run.id,
                    response_level: level,
                    kp: Some(kp),
                    ti_minutes: Some(ti),
                    td_minutes: Some(td),
                    proportional: Some(p),
                    integral: Some(i),
                    derivative: Some(d),
                    status: TuningResultStatus::Valid,
                    invalid_reason: None,
                },
            )
            .await
            .unwrap();
        }
        (pool, run.id)
    }

    #[tokio::test]
    async fn maybe_write_back_skips_when_no_pid_constant_tags_are_configured() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let mut tags = honeywell_tags();
        tags.proportional_constant = None;
        let driver = honeywell_driver_auto();

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Skipped);
        assert_eq!(
            write_back_detail.as_deref(),
            Some("no PID constant tags configured for this run's driver/template")
        );
        assert!(
            TuneWriteRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn maybe_write_back_skips_when_no_results_were_recorded() {
        let pool = seeded_pool().await;
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            &pool,
            None,
            "no-results",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        let driver = honeywell_driver_auto();

        let (outcome, write_back_detail) = maybe_write_back(
            &pool, run.id, &tags, &template, &driver, config, None, false, None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Skipped);
        assert_eq!(
            write_back_detail.as_deref(),
            Some("no calculated results were recorded for this run")
        );
        assert!(
            TuneWriteRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// Runs `maybe_write_back` against `run_with_recorded_results()`'s fixture with the
    /// requested response level, returning both the outcome and the recorded
    /// write-back audit rows (0 or 1).
    async fn write_back_with_level(
        response_level: ResponseLevel,
    ) -> (WriteBackOutcome, Vec<bhtune_db::models::TuneWriteRow>) {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(response_level),
            false,
            None,
        )
        .await
        .unwrap();

        (
            outcome,
            TuneWriteRow::list_for_run(&pool, run_id).await.unwrap(),
        )
    }
    #[tokio::test]
    async fn maybe_write_back_writes_and_confirms_a_valid_selection() {
        let (outcome, writes) = write_back_with_level(ResponseLevel::Moderate).await;
        assert_eq!(
            outcome,
            WriteBackOutcome::Written {
                response_level: ResponseLevel::Moderate
            }
        );
        assert_eq!(writes.len(), 1);
        let write = &writes[0];
        assert!(write.success);
        assert_eq!(write.response_level, ResponseLevel::Moderate);
        assert!(write.error_message.is_none());
        // A full success pre-reads, writes, and verifies all three constants, and never
        // rolls anything back.
        assert!(write.previous.is_some());
        assert!(write.proportional_written.is_some());
        assert!(write.integral_written.is_some());
        assert!(write.derivative_written.is_some());
        assert!(write.proportional_readback.is_some());
        assert!(write.integral_readback.is_some());
        assert!(write.derivative_readback.is_some());
        assert_eq!(write.rollback_state, None);
        assert!(write.rollback_error.is_none());
    }

    #[tokio::test]
    async fn maybe_write_back_records_failure_when_the_pre_read_fails() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // Every read of the P tag fails, including the very first (pre-read) one -- nothing
        // is ever written.
        let driver = honeywell_driver_auto().erroring_read("Unit1.LIC101.K");

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(ResponseLevel::Aggressive),
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        assert!(
            write_back_detail
                .as_deref()
                .unwrap_or_default()
                .starts_with("pre-read failed:")
        );
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        let write = &writes[0];
        assert!(!write.success);
        // The pre-read never produced a known-good value, so there is nothing to record as
        // "previous", and nothing was ever attempted.
        assert!(write.previous.is_none());
        assert!(write.proportional_written.is_none());
        assert!(write.integral_written.is_none());
        assert!(write.derivative_written.is_none());
        assert!(
            write
                .error_message
                .as_deref()
                .unwrap_or_default()
                .starts_with("pre-read of Proportional")
        );
        assert_eq!(write.rollback_state, None);
        // Nothing was written to the driver at all -- confirms the pre-read is a genuine
        // hard stop, not just a reported failure alongside attempted writes.
        assert!(driver.write_log().is_empty());
    }

    #[tokio::test]
    async fn maybe_write_back_rolls_back_a_confirmed_write_when_a_later_constant_fails() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // P writes and verifies successfully; I's write is then rejected. P was already
        // confirmed, so it must be rolled back to its pre-read value.
        let driver = honeywell_driver_auto().rejecting_write("Unit1.LIC101.T1");

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(ResponseLevel::Aggressive),
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        assert!(
            write_back_detail
                .as_deref()
                .unwrap_or_default()
                .ends_with("(rolled back)")
        );
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        let write = &writes[0];
        assert!(!write.success);
        assert!(write.previous.is_some());
        // P was confirmed (written + read back), I was attempted but never confirmed, D was
        // never attempted at all.
        assert!(write.proportional_written.is_some());
        assert!(write.proportional_readback.is_some());
        assert!(write.integral_written.is_some());
        assert!(write.integral_readback.is_none());
        assert!(write.derivative_written.is_none());
        assert!(write.derivative_readback.is_none());
        assert_eq!(write.rollback_state, Some(RollbackState::Succeeded));
        assert!(write.rollback_error.is_none());
        // The rollback actually put P's original value back on the driver, not just in the
        // audit row.
        let p_previous = write.previous.as_ref().unwrap().proportional;
        assert_eq!(
            driver
                .value_of("Unit1.LIC101.K")
                .and_then(|v| v.parse::<f32>().ok()),
            Some(p_previous)
        );
    }

    #[tokio::test]
    async fn maybe_write_back_records_a_failed_rollback_when_the_rollback_write_is_also_rejected() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // P's forward write succeeds (1st write to the tag), but its rollback write (2nd
        // write to the same tag, once I fails) is rejected -- "wrote some and could not put
        // it back" must be a distinguishable, clearly reported outcome.
        let driver = honeywell_driver_auto()
            .rejecting_write("Unit1.LIC101.T1")
            .rejecting_write_after("Unit1.LIC101.K", 1);

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(ResponseLevel::Aggressive),
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        let detail = write_back_detail.unwrap_or_default();
        assert!(detail.contains("rollback also failed"));
        assert!(detail.contains("history revert"));
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        let write = &writes[0];
        assert!(!write.success);
        assert_eq!(write.rollback_state, Some(RollbackState::Failed));
        let rollback_error = write.rollback_error.as_deref().unwrap_or_default();
        assert!(rollback_error.contains("Proportional"));
        assert!(rollback_error.contains("rollback"));
    }

    #[tokio::test]
    async fn maybe_write_back_records_failure_when_the_readback_is_outside_tolerance() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // The write itself succeeds and the readback parses fine at Good quality, but the
        // DCS silently stored a value far outside tolerance of what was requested -- a
        // distinct failure mode from an erroring or poor-quality readback.
        let driver = honeywell_driver_auto().distorting_write("Unit1.LIC101.K", 5.0);

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(ResponseLevel::Aggressive),
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        let write = &writes[0];
        assert!(!write.success);
        let message = write.error_message.as_deref().unwrap_or_default();
        assert!(message.contains("outside tolerance"));
        assert!(message.contains("Proportional"));
        // Nothing was confirmed before the tolerance rejection, so there is nothing to roll
        // back.
        assert_eq!(write.rollback_state, None);
    }

    #[tokio::test]
    async fn maybe_write_back_records_failure_when_a_write_is_rejected() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().rejecting_write("Unit1.LIC101.K");

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(ResponseLevel::Aggressive),
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        assert!(!writes[0].success);
        assert!(writes[0].error_message.is_some());
    }

    #[tokio::test]
    async fn maybe_write_back_records_failure_when_the_readback_fails() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // The pre-read of P succeeds (the tag's 1st read), the write itself succeeds, but
        // the confirmation re-read of the P tag (its 2nd read) then errors.
        let driver = honeywell_driver_auto().erroring_read_after("Unit1.LIC101.K", 1);

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(ResponseLevel::Aggressive),
            false,
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, WriteBackOutcome::Failed);
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();

        assert_eq!(writes.len(), 1);
        assert!(!writes[0].success);
        // The pre-read succeeded, so the audit row still records the previous values, even
        // though the write-and-verify step for P failed.
        assert!(writes[0].previous.is_some());
        let message = writes[0].error_message.as_deref().unwrap_or_default();
        assert!(message.starts_with("Proportional readback from"));
        assert!(!message.starts_with("pre-read of"));
        // Nothing was confirmed before the failure, so there is nothing to roll back.
        assert_eq!(writes[0].rollback_state, None);
    }

    #[tokio::test]
    async fn maybe_write_back_records_failure_when_the_readback_reports_poor_quality() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // The pre-read of P (its 1st read) succeeds at the default `Good` quality; only the
        // confirmation re-read after the write (its 2nd read) reports a poor OPC quality --
        // finding 5's rule applies to this readback exactly as it does to any other
        // tuning-critical read, so a stale/clamped value must not be mistaken for proof the
        // write actually landed.
        let driver = honeywell_driver_auto().degrade_quality_after(
            "Unit1.LIC101.K",
            1,
            bhtune_driver::Quality::Bad,
        );

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(ResponseLevel::Aggressive),
            false,
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, WriteBackOutcome::Failed);

        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        assert!(!writes[0].success);
        assert!(writes[0].previous.is_some());
        let message = writes[0].error_message.as_deref().unwrap_or_default();
        assert!(message.contains("quality"));
        assert!(message.contains("Unit1.LIC101.K"));
        // Confirms this is the *readback* failing, not the pre-read -- the pre-read's
        // message format is "pre-read of ... failed", distinct from "... readback from ...
        // failed", so an operator (or the history explorer) can tell a poor-quality
        // confirmation apart from a pre-read failure or an outright transport read failure.
        assert!(message.starts_with("Proportional readback from"));
        assert!(!message.starts_with("pre-read of"));
        assert_eq!(writes[0].rollback_state, None);
    }

    #[tokio::test]
    async fn maybe_write_back_accepts_an_uncertain_readback_when_the_flag_is_set() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .with_quality("Unit1.LIC101.K", bhtune_driver::Quality::Uncertain);

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(ResponseLevel::Aggressive),
            true,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, WriteBackOutcome::Written { .. }));
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        assert!(writes[0].success);
    }

    #[tokio::test]
    async fn maybe_write_back_writes_non_interactively_via_write_pid() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(ResponseLevel::Aggressive),
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            WriteBackOutcome::Written {
                response_level: ResponseLevel::Aggressive
            }
        );
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        assert!(writes[0].success);
        assert_eq!(writes[0].response_level, ResponseLevel::Aggressive);
    }

    #[tokio::test]
    async fn maybe_write_back_fails_when_write_pid_names_a_level_with_no_recorded_result() {
        let pool = seeded_pool().await;
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            &pool,
            None,
            "partial-results",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        // Deliberately only record Aggressive and Moderate -- Sluggish is missing, which
        // should never actually happen (`calculate_all` always computes all 3), but
        // `maybe_write_back` must still fail safely rather than panic on an out-of-bounds
        // index or silently write the wrong level.
        for (level, kp, ti, td, p, i, d) in [
            (ResponseLevel::Aggressive, 1.0, 0.5, 0.1, 10.0, 2.0, 0.5),
            (ResponseLevel::Moderate, 1.5, 0.7, 0.15, 12.0, 2.5, 0.6),
        ] {
            TuneResultRow::insert(
                &pool,
                &TuneResultRow {
                    id: 0,
                    run_id: run.id,
                    response_level: level,
                    kp: Some(kp),
                    ti_minutes: Some(ti),
                    td_minutes: Some(td),
                    proportional: Some(p),
                    integral: Some(i),
                    derivative: Some(d),
                    status: TuningResultStatus::Valid,
                    invalid_reason: None,
                },
            )
            .await
            .unwrap();
        }
        let driver = honeywell_driver_auto();

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run.id,
            &tags,
            &template,
            &driver,
            config,
            Some(ResponseLevel::Sluggish),
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        assert_eq!(
            write_back_detail.as_deref(),
            Some("no calculated result recorded for response level Sluggish")
        );
        // Nothing was attempted at the driver at all, so no audit row exists either.
        assert!(
            TuneWriteRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn delayed_live_timing_emits_the_observational_warning() {
        warn_on_missed_poll_opportunities(42, &delayed_live_timing_metrics());
    }

    #[tokio::test]
    async fn timing_persistence_failure_does_not_replace_the_run_outcome() {
        let pool = seeded_pool().await;
        pool.close().await;

        record_timing_metrics_best_effort(&pool, 42, delayed_live_timing_metrics()).await;
    }

    #[tokio::test]
    async fn present_timing_metrics_are_persisted_by_the_optional_wrapper() {
        let (pool, run_id) = run_with_recorded_results().await;
        let metrics = delayed_live_timing_metrics();

        record_timing_metrics_if_present(&pool, run_id, Some(metrics)).await;

        let stored = TuneRunRow::get(&pool, run_id).await.unwrap().unwrap();
        assert_eq!(stored.timing_metrics, Some(metrics));
    }

    #[test]
    fn restore_incomplete_warning_message_names_the_reason_and_mv_restore_target() {
        let message = restore_incomplete_warning_message(
            &honeywell_tags(),
            &sample_initial_state(),
            "restore timed out",
        );

        assert!(message.contains("restore timed out"));
        assert!(message.contains("Unit1.LIC101.OP"));
        assert!(message.contains("45"));
        assert!(message.contains("loop's mode"));
    }

    #[test]
    fn warn_restore_incomplete_returns_the_message_it_emits() {
        let tags = honeywell_tags();
        let initial = sample_initial_state();
        let message = warn_restore_incomplete(&tags, &initial, "restore timed out");

        assert_eq!(
            message,
            restore_incomplete_warning_message(&tags, &initial, "restore timed out")
        );
    }

    #[test]
    fn tune_outcome_for_run_maps_every_run_outcome_variant() {
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Completed {
                write_back: WriteBackOutcome::Skipped,
                write_back_detail: None,
            }),
            TuneOutcome::Completed
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Completed {
                write_back: WriteBackOutcome::Written {
                    response_level: ResponseLevel::Moderate
                },
                write_back_detail: None,
            }),
            TuneOutcome::Completed
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Completed {
                write_back: WriteBackOutcome::Failed,
                write_back_detail: None,
            }),
            TuneOutcome::WriteBackFailed
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Aborted(AbortReason::UserInterrupt)),
            TuneOutcome::Aborted
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Aborted(AbortReason::Timeout {
                timeout_secs: 3600
            })),
            TuneOutcome::TimedOut
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Aborted(AbortReason::PoorQuality {
                tag: "Unit1.LIC101.PV".to_string(),
                quality: bhtune_driver::Quality::Bad,
            })),
            TuneOutcome::PoorQuality
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
                tag: "Unit1.LIC101.OP".to_string(),
                target: 55.0,
                readback: Some(50.0),
                tolerance: 0.1,
                elapsed_ms: 4_000,
                deadline_secs: 4,
            })),
            TuneOutcome::ActuationFailed
        );
    }
    #[tokio::test]
    async fn run_rejects_write_pid_without_yes_before_starting_the_tune() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.write_pid = Some(ResponseLevel::Aggressive);
        args.yes = false;

        let err = run(&pool, args, &test_config()).await.unwrap_err();
        assert!(err.to_string().contains("--write-pid requires --yes"));

        // The check happens before any driver/database I/O, so no run row should exist.
        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn a_full_simulator_tune_with_write_pid_and_yes_still_skips_write_back() {
        // The built-in simulator driver has no PID constant tags at all (see
        // `build_loop_tags`), so `--write-pid`/`--yes` must be accepted but remain a no-op
        // -- not an error, and not `TuneOutcome::WriteBackFailed`.
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.write_pid = Some(ResponseLevel::Aggressive);
        args.yes = true;

        let outcome = run(&pool, args, &test_config()).await.unwrap();
        assert_eq!(outcome, TuneOutcome::Completed);
    }

    #[tokio::test]
    async fn run_times_out_and_aborts_when_timeout_secs_elapses_before_completion() {
        // Real (unpaused) time: `start_paused` was tried here first but interacts badly with
        // the real sqlx `SqlitePool` -- pausing tokio's clock also fast-forwards the pool's
        // own internal connection-acquire timeout, which fires instantly and turns every
        // query into a spurious `PoolTimedOut` error. So this test pays a real ~1s wall-clock
        // cost instead, matching `tests/ctrlc_abort.rs`'s existing precedent of a similar
        // real-time cost for the same reason (an actual signal/timeout has to actually
        // elapse). `poll_interval_ms: 3` is not a divisor of `timeout_secs: 1`'s 1000ms, so
        // the timeout can never land exactly on a tick boundary. `cycles_count: 100_000`
        // makes it impossible for the MRFT test to legitimately finish within the handful of
        // ticks that occur in one real second at this poll rate (a real oscillation cycle
        // needs at least 2 ticks, so 100,000 cycles needs at least 200,000 -- nowhere near
        // reachable in ~333 ticks).
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.poll_interval_ms = 3;
        args.timeout_secs = 1;
        args.cycles_count = Some(100_000);

        let outcome = run(&pool, args, &test_config()).await.unwrap();
        assert_eq!(outcome, TuneOutcome::TimedOut);

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        // A timeout-triggered abort reuses the exact same DB outcome as Ctrl+C -- only the
        // CLI-level `TuneOutcome`/exit code/printed message distinguish *why*.
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Aborted);

        // Ticks were actually recorded before the timeout fired, proving the loop really ran
        // rather than aborting instantly with nothing sampled.
        let samples = TuneSampleRow::list_for_run(&pool, runs[0].id)
            .await
            .unwrap();
        assert!(!samples.is_empty());
    }

    // --- `bounded_driver_call` / `TickOperation`: the four possible race outcomes, tested --
    // --- directly and in isolation from the polling loop that's the only real caller --------

    #[tokio::test]
    async fn bounded_driver_call_returns_completed_when_the_call_finishes_first() {
        let mut ctrl_c = CtrlC::never();
        let result = bounded_driver_call(30, &mut ctrl_c, async { Ok::<_, anyhow::Error>(42) })
            .await
            .unwrap();
        assert!(matches!(result, TickOperation::Completed(42)));
    }

    #[tokio::test]
    async fn bounded_driver_call_propagates_a_genuine_error_from_the_call() {
        // A real failure from the call itself (a rejected write, a malformed value, a
        // transport error) must still propagate through `?` at the call site -- it is not
        // "gave up waiting", so it has no `TickOperation` variant of its own.
        let mut ctrl_c = CtrlC::never();
        let err = bounded_driver_call(30, &mut ctrl_c, async {
            Err::<(), _>(anyhow::anyhow!("boom"))
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    #[tokio::test]
    async fn bounded_driver_call_returns_cancelled_when_ctrl_c_fires_first() {
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();
        let result: TickOperation<()> = bounded_driver_call(
            30,
            &mut ctrl_c,
            std::future::pending::<anyhow::Result<()>>(),
        )
        .await
        .unwrap();
        assert!(matches!(result, TickOperation::Cancelled));
    }

    #[tokio::test(start_paused = true)]
    async fn bounded_driver_call_returns_timed_out_when_the_driver_call_stalls() {
        // No `SqlitePool` involved here (unlike the `run_polling_loop`-level tests), so
        // `start_paused` is safe -- see the precedent/caveat noted on the timeout test above.
        let mut ctrl_c = CtrlC::never();
        let result: TickOperation<()> =
            bounded_driver_call(1, &mut ctrl_c, std::future::pending::<anyhow::Result<()>>())
                .await
                .unwrap();
        assert!(matches!(result, TickOperation::TimedOut));
    }

    #[tokio::test]
    async fn a_stalled_pv_read_during_a_tick_is_cancelled_without_recording_a_sample() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_read(&tags.process_variable);
        let mut args = fast_simulator_args();
        args.op_timeout_secs = 30;
        let config = build_loop_config(&args).unwrap();
        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "stalled-pv-read",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();
        let initial = sample_initial_state();
        let beta = lookup(
            config.process_type,
            config.controller_type,
            ResponseLevel::Aggressive,
        )
        .beta;
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(1);
        });

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut ctrl_c,
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut None,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::UserInterrupt)
        ));
        assert!(
            TuneSampleRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_stalled_mv_write_during_a_tick_times_out_after_recording_the_sample() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_write(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.op_timeout_secs = 1;
        let config = build_loop_config(&args).unwrap();
        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "stalled-mv-write-timeout",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();
        let initial = sample_initial_state();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut None,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::OperationTimedOut { ref tag, op_timeout_secs })
                if tag == &tags.manipulated_variable && op_timeout_secs == 1
        ));
        assert_eq!(
            TuneSampleRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    // --- `attempt_restore` / `RestoreAttempt`: confirmed vs. incomplete, and both ways to ---
    // --- become incomplete (a second Ctrl+C, and `[tuning].restore_timeout_secs` elapsing) ---

    #[tokio::test(start_paused = true)]
    async fn attempt_restore_confirms_a_normal_restore() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let args = fast_simulator_args();

        let outcome = attempt_restore_with_actuation(
            &pool,
            0,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut CtrlC::never(),
            &mut None,
        )
        .await;

        assert!(matches!(outcome, RestoreAttempt::Confirmed));
        assert_eq!(
            driver.value_of(&tags.manipulated_variable).as_deref(),
            Some("45")
        );
    }

    #[tokio::test]
    async fn record_restore_status_best_effort_swallows_database_errors() {
        let pool = seeded_pool().await;
        record_restore_status_best_effort(&pool, i64::MAX, &RestoreAttempt::Confirmed).await;
    }

    #[tokio::test(start_paused = true)]
    async fn attempt_restore_reports_incomplete_when_restore_timeout_secs_elapses() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        // `restore`'s very first step writes the MV -- hanging it means `restore()` itself
        // can never resolve on its own, so only the timeout branch can win this race.
        let driver = honeywell_driver_auto().hanging_write(&tags.manipulated_variable);
        let initial = sample_initial_state();
        let guard = MutationGuard::default();
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let mut args = fast_simulator_args();
        args.restore_timeout_secs = MV_ACTUATION_CONFIRMATION_SECS;
        args.op_timeout_secs = 30;

        let outcome = attempt_restore_with_actuation(
            &pool,
            0,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut CtrlC::never(),
            &mut None,
        )
        .await;

        match outcome {
            RestoreAttempt::Incomplete { reason } => {
                assert!(reason.contains("[tuning].restore_timeout_secs"));
            }
            RestoreAttempt::Confirmed => panic!("expected RestoreAttempt::Incomplete"),
        }
    }

    #[tokio::test]
    async fn attempt_restore_reports_incomplete_on_a_second_ctrl_c() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_write(&tags.manipulated_variable);
        let initial = sample_initial_state();
        let guard = MutationGuard::default();
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();
        let pool = seeded_pool().await;
        let args = fast_simulator_args();

        let outcome = attempt_restore_with_actuation(
            &pool,
            0,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut ctrl_c,
            &mut None,
        )
        .await;

        match outcome {
            RestoreAttempt::Incomplete { reason } => {
                assert!(reason.contains("second Ctrl+C"));
            }
            RestoreAttempt::Confirmed => panic!("expected RestoreAttempt::Incomplete"),
        }
    }

    #[tokio::test]
    async fn restore_wrapper_handles_ctrl_c_after_mv_restore_completes() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_write(tags.controller_mode.as_ref().unwrap());
        let initial = sample_initial_state();
        let guard = MutationGuard {
            mode_written: true,
            ..MutationGuard::default()
        };
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let _ = tx.send(1);
        });

        let outcome = attempt_restore_with_actuation(
            &pool,
            0,
            &fast_simulator_args(),
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut ctrl_c,
            &mut None,
        )
        .await;

        assert!(matches!(
            outcome,
            RestoreAttempt::Incomplete { ref reason } if reason.contains("second Ctrl+C")
        ));
        assert_eq!(
            driver.value_of(&tags.manipulated_variable).as_deref(),
            Some("45")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn restore_wrapper_handles_deadline_after_mv_restore_completes() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_write(tags.controller_mode.as_ref().unwrap());
        let initial = sample_initial_state();
        let guard = MutationGuard {
            mode_written: true,
            ..MutationGuard::default()
        };
        let mut args = fast_simulator_args();
        args.restore_timeout_secs = 1;
        args.op_timeout_secs = 30;

        let outcome = attempt_restore_with_actuation(
            &pool,
            0,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut CtrlC::never(),
            &mut None,
        )
        .await;

        assert!(matches!(
            outcome,
            RestoreAttempt::Incomplete { ref reason }
                if reason.contains("[tuning].restore_timeout_secs")
        ));
        assert_eq!(
            driver.value_of(&tags.manipulated_variable).as_deref(),
            Some("45")
        );
    }

    #[tokio::test]
    async fn accepted_mv_restore_gets_a_full_confirmation_window_before_remaining_restore_steps() {
        let pool = seeded_pool().await;
        let (run_id, _config, template, tags) =
            start_opc_test_run(&pool, "actuation-restore-deadline-extension").await;
        let driver = honeywell_driver_auto()
            .delaying_write(&tags.manipulated_variable, Duration::from_millis(2_300))
            .delaying_read(&tags.manipulated_variable, Duration::from_millis(900))
            .delaying_write(
                tags.controller_mode.as_ref().unwrap(),
                Duration::from_millis(500),
            );
        let initial = sample_initial_state();
        let guard = MutationGuard {
            mode_written: true,
            ..MutationGuard::default()
        };
        let mut args = fast_simulator_args();
        args.driver = DriverKind::Opcda;
        args.restore_timeout_secs = MV_ACTUATION_CONFIRMATION_SECS;
        args.op_timeout_secs = 30;
        let mut mv_actuations = Some(MvActuationTracker::for_run(&args, &initial).unwrap());

        let outcome = attempt_restore_with_actuation(
            &pool,
            run_id,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut CtrlC::never(),
            &mut mv_actuations,
        )
        .await;

        assert!(matches!(outcome, RestoreAttempt::Confirmed));
        assert_eq!(
            driver.value_of(&tags.manipulated_variable).as_deref(),
            Some("45")
        );
        assert_eq!(
            driver
                .value_of(tags.controller_mode.as_ref().unwrap())
                .as_deref(),
            Some("1")
        );
        assert!(!driver.delayed_write_was_cancelled(&tags.manipulated_variable));
        assert!(!driver.delayed_read_was_cancelled(&tags.manipulated_variable));
    }

    #[tokio::test]
    async fn restore_failure_wrapper_preserves_error_when_status_and_cleanup_writes_fail() {
        let pool = seeded_pool().await;
        pool.close().await;
        let original = anyhow::anyhow!("original polling failure");
        let error = restore_best_effort_then_propagate(
            &pool,
            42,
            &honeywell_driver_auto(),
            &honeywell_tags(),
            &honeywell_template(),
            &sample_initial_state(),
            &MutationGuard::default(),
            &fast_simulator_args(),
            false,
            &mut CtrlC::never(),
            &mut None,
            original,
        )
        .await;

        assert_eq!(error.to_string(), "original polling failure");
    }

    // --- `run_with_ctrl_c`: the real, ctrl-c-aware entry point, exercised end to end with ---
    // --- a simulated signal rather than only through the `CtrlC::never()`-backed `run` above

    /// The one test exercising `run_with_ctrl_c` itself (every other test in this module goes
    /// through the `#[cfg(test)]`-only `run` wrapper, which always passes `CtrlC::never()` --
    /// see its doc comment) -- proving this is what `lib.rs::run_with_cli_and_ctrl_c` actually
    /// calls in production correctly reacts to a signalled `CtrlC` end to end: dispatch, the
    /// poll loop, the restore, and the final `TuneOutcome`/DB row.
    #[tokio::test]
    async fn run_with_ctrl_c_aborts_the_run_when_signalled_during_the_poll() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        // Impossible to legitimately finish within the ~50ms before the signal below fires,
        // matching the timeout test's own precedent for making a real completion race moot.
        args.cycles_count = Some(100_000);
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(1);
        });

        let outcome = run_with_ctrl_c(&pool, args, &test_config(), &mut ctrl_c)
            .await
            .unwrap();
        assert_eq!(outcome, TuneOutcome::Aborted);

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Aborted);
    }
}
