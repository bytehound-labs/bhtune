#![allow(rustdoc::broken_intra_doc_links)]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use bhtune_core::{
    Action, ControllerDirection, ControllerType, DcsTemplate, InitialReadings, LoopConfig,
    LoopTags, MrftCompat, MrftEngine, MvRange, ProcessType, PvRange, ResponseLevel, TagOrValue,
    TagOverrides, lookup,
};
use bhtune_db::SqlitePool;
use bhtune_db::models::{
    DcsTemplateRow, LiveOperationKind, TimingBasis, TimingMetrics, TuneDriver,
    TuneRunInitialReadings, TuneRunRow,
};
use bhtune_driver::{Driver, TagValue};
use chrono::Utc;

use crate::cancel::CtrlC;
use crate::live_ownership::{AuditedDriver, LiveOperationGuard};
use crate::timing::{PollTimingAccumulator, RunTimeAnchor};

use super::actuation::{
    MvActuationTracker, finalize_pending_for_run_best_effort, validate_relay_actuation_step,
};
#[cfg(test)]
use super::config::test_effective_timing;
use super::config::{EffectiveTiming, build_loop_config_with_timing, build_loop_tags};
use super::outcome::{
    AbortReason, RunOutcome, TuneOutcome, TuneRunReport, format_mv_actuation_abort_reason,
    tune_outcome_for_run,
};
use super::poll::{CompletedPoll, PollOutcome, persist_results, run_polling_loop_with_timing};
use super::quality::{
    read_batch_f32, read_batch_raw, read_f32, resolve_direction_from_batch, resolve_f32_from_batch,
    write_raw,
};
use super::request::{DriverKind, TuneRequest, ValidatedTuneRequest};
use super::restore::{
    RestoreAttempt, attempt_restore_with_actuation_with_timing, record_restore_status_best_effort,
    restore_best_effort_then_propagate_with_timing,
};
use super::timing::{
    completed_oscillation_period_ms, record_timing_metrics_if_present,
    warn_on_missed_poll_opportunities,
};
use super::writeback::{PidWritePreview, WriteBackHandler, maybe_write_back_with_owner};

/// Everything [`prepare`] resolves before a tune's long-running polling phase can start:
/// the already-validated [`ValidatedTuneRequest`], the resolved template and derived tags, a
/// connected driver, the built [`LoopConfig`], the run's start time, and the response level
/// (if any) to write back at the end -- plus the `tune_runs` row's assigned id.
///
/// The first phase (template lookup, tag derivation, driver connect, and the `tune_runs`
/// insert that assigns [`PreparedTune::run_id`]) is fast enough to await in an HTTP request
/// handler, which can then spawn [`drive`] and return the assigned id before the tune ends.
///
/// Every field but `run_id` is private: a caller that isn't this module has no legitimate
/// reason to inspect a template/tags/driver/config mid-flight, only to hand the whole
/// prepared bundle to [`drive`] unchanged.
pub struct PreparedTune {
    pub(super) run_id: i64,
    pub(super) args: TuneRequest,
    pub(super) template: DcsTemplate,
    pub(super) tags: LoopTags,
    pub(super) driver: Box<dyn Driver>,
    pub(super) config: LoopConfig,
    pub(super) timing: EffectiveTiming,
    pub(super) time_anchor: RunTimeAnchor,
    pub(super) write_pid: Option<ResponseLevel>,
    pub(super) allow_uncertain_quality: bool,
    pub(super) ownership: Option<LiveOperationGuard>,
}
/// Prepare a simulator tune and bind its history to a demo session before the background
/// execution is started. Live OPC DA preparation is intentionally rejected by this helper.
pub async fn prepare_owned<R>(
    pool: &SqlitePool,
    args: R,
    app_config: &crate::config::BhtuneConfig,
    demo_session_id: i64,
) -> anyhow::Result<PreparedTune>
where
    R: TryInto<ValidatedTuneRequest>,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let args = args.try_into()?;
    if args.as_request().driver != DriverKind::Simulator {
        anyhow::bail!("demo sessions may only start simulator runs");
    }
    prepare_internal(pool, args, app_config, Some(demo_session_id)).await
}
impl PreparedTune {
    /// The `tune_runs` row id assigned to this run -- returned to an HTTP caller immediately
    /// (before the run has necessarily finished, or even started polling) so it can be used
    /// to poll `GET /api/runs/{id}` or issue `POST /api/runs/{id}/cancel`.
    pub fn run_id(&self) -> i64 {
        self.run_id
    }
}
/// The shape persisted into `tune_runs.request_json`.
/// Its stable fields correspond to the transport-neutral [`TuneRequest`] shared by the CLI
/// and HTTP adapters, so either adapter persists the same snapshot shape.
///
/// Built from `args` *before* [`prepare`]'s own `bridge_host`/`server` resolution mutates
/// them, so a field left unset by the caller stays absent here rather than silently baking
/// in a resolved default -- this lets the last-run form prefill show blanks where the user
/// relied on a default, instead of freezing resolved values into the next
/// form. `output` is deliberately excluded: it's a CLI/HTTP-transport concern with no
/// meaning as a "setting" to remember or duplicate.
#[derive(serde::Serialize)]
pub(super) struct RequestSnapshot<'a> {
    pub(super) tagname: &'a str,
    pub(super) template: &'a str,
    pub(super) process_type: ProcessType,
    pub(super) controller_type: ControllerType,
    pub(super) relay_amp: f32,
    pub(super) cycles_skip: Option<u32>,
    pub(super) cycles_count: Option<u32>,
    pub(super) noise_protection_secs: Option<u32>,
    pub(super) driver: TuneDriver,
    pub(super) bridge_host: Option<&'a str>,
    pub(super) server: Option<&'a str>,
    pub(super) sim_gain: f32,
    pub(super) sim_tau: f32,
    pub(super) sim_dead_time: f32,
    pub(super) sim_noise: f32,
    pub(super) sim_seed: u64,
    pub(super) sim_initial_pv: f32,
    pub(super) sim_initial_mv: f32,
    pub(super) pv_range_high: Option<f32>,
    pub(super) pv_range_low: Option<f32>,
    pub(super) mv_range_high: Option<f32>,
    pub(super) mv_range_low: Option<f32>,
    pub(super) direction: Option<ControllerDirection>,
    pub(super) tag_overrides: Option<&'a TagOverrides>,
    pub(super) notes: Option<&'a str>,
    pub(super) yes: bool,
    pub(super) write_pid: Option<ResponseLevel>,
}
/// Validates a tune request and prepares the driver and persisted run record.
///
/// The unattended-write guard and timing/config validation run before driver I/O. The
/// caller's request is snapshotted before effective connection defaults are resolved, and
/// the resolved connection and tuning context are persisted before polling starts.
pub async fn prepare<R>(
    pool: &SqlitePool,
    args: R,
    app_config: &crate::config::BhtuneConfig,
) -> anyhow::Result<PreparedTune>
where
    R: TryInto<ValidatedTuneRequest>,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let args = args.try_into()?;
    prepare_internal(pool, args, app_config, None).await
}
pub(super) async fn prepare_internal(
    pool: &SqlitePool,
    args: ValidatedTuneRequest,
    app_config: &crate::config::BhtuneConfig,
    demo_session_id: Option<i64>,
) -> anyhow::Result<PreparedTune> {
    let mut args = args.into_request();
    // Fails before any driver/database I/O at all: an unattended write-back must be an
    // explicit, deliberate choice, not something a stray `--write-pid` without `--yes` can
    // trigger by accident.
    if args.write_pid.is_some() && !args.yes {
        anyhow::bail!(
            "--write-pid requires --yes: writing PID constants back to the DCS with no \
             human present to confirm must be an explicit, deliberate choice"
        );
    }
    let timing: EffectiveTiming = crate::config::resolve_and_validate_tuning_config(
        &app_config.tuning,
        args.driver == DriverKind::Opcda,
    )?
    .into();
    let allow_uncertain_quality = app_config.allow_uncertain_quality;

    let db_driver = args.driver.into();

    // Snapshotted before `bridge_host`/`server` are resolved to their effective values just
    // below, so a field the caller left unset stays absent here instead of silently baking
    // in a resolved default -- see `RequestSnapshot`'s doc comment.
    #[allow(
        clippy::expect_used,
        reason = "RequestSnapshot is plain enums and finite scalars, serialized before any driver I/O"
    )]
    let request_json = serde_json::to_string(&RequestSnapshot {
        tagname: &args.tagname,
        template: &args.template,
        process_type: args.process_type,
        controller_type: args.controller_type,
        relay_amp: args.relay_amp,
        cycles_skip: args.cycles_skip,
        cycles_count: args.cycles_count,
        noise_protection_secs: args.noise_protection_secs,
        driver: db_driver,
        bridge_host: args.bridge_host.as_deref(),
        server: args.server.as_deref(),
        sim_gain: args.sim_gain,
        sim_tau: args.sim_tau,
        sim_dead_time: args.sim_dead_time,
        sim_noise: args.sim_noise,
        sim_seed: args.sim_seed,
        sim_initial_pv: args.sim_initial_pv,
        sim_initial_mv: args.sim_initial_mv,
        pv_range_high: args.pv_range_high,
        pv_range_low: args.pv_range_low,
        mv_range_high: args.mv_range_high,
        mv_range_low: args.mv_range_low,
        direction: args.direction,
        tag_overrides: args.tag_overrides.as_ref(),
        notes: args.notes.as_deref(),
        yes: args.yes,
        write_pid: args.write_pid,
    })
    .expect(
        "RequestSnapshot serialization is infallible: plain enum/scalar fields, no maps and \
         no floats that JSON can't represent (every f32 here is validated finite before \
         reaching this call, per safety-validation)",
    );

    args.bridge_host = Some(crate::config::resolve_bridge_host(
        args.bridge_host.take(),
        app_config,
    ));
    if matches!(args.driver, DriverKind::Opcda) {
        args.server = Some(crate::config::resolve_server(
            args.server.take(),
            app_config,
        )?);
    }

    let template_row = DcsTemplateRow::get_by_name(pool, &args.template)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no template named '{}'", args.template))?;
    let template_origin = template_row.origin;
    let template = template_row.template;

    let config = build_loop_config_with_timing(&args, timing)?;
    let tags = build_loop_tags(&args, &template)?;
    // This gateway-only preflight is read-only and rejects unsupported peers before a run
    // row exists. The live driver connection remains after ownership is acquired below.
    let gateway_compatibility = if let (DriverKind::Opcda, Some(bridge_host), Some(server)) = (
        args.driver,
        args.bridge_host.as_deref(),
        args.server.as_deref(),
    ) {
        Some(crate::gateway::require_live_gateway_compatible(bridge_host, Some(server)).await?)
    } else {
        None
    };
    let time_anchor = RunTimeAnchor::now();
    let started_at = time_anchor.utc();
    let run = TuneRunRow::start_with_demo_session(
        pool,
        demo_session_id,
        None,
        &args.tagname,
        db_driver,
        config,
        template_origin,
        &template,
        &tags,
        started_at,
    )
    .await?;
    let metadata_result = async {
        TuneRunRow::record_effective_tuning(pool, run.id, timing.into()).await?;
        TuneRunRow::record_allow_uncertain_quality(pool, run.id, allow_uncertain_quality).await?;

        // The *resolved, effective* connection this run actually used -- `None`/`None` for a
        // non-opcda run even though `args.bridge_host` was just unconditionally resolved to a
        // default above (a pre-existing, harmless quirk of that resolution call). Forcing both
        // to `None` here is what makes a simulator/replay run correctly show "no connection"
        // rather than an OPC DA gateway it never actually touched -- load-bearing for
        // `history revert`'s connection safety fix.
        let (opc_server, bridge_host) = if db_driver == TuneDriver::Opcda {
            (args.server.as_deref(), args.bridge_host.as_deref())
        } else {
            (None, None)
        };
        TuneRunRow::record_connection(pool, run.id, opc_server, bridge_host, &request_json).await?;
        if let Some(report) = gateway_compatibility.as_ref() {
            TuneRunRow::record_gateway_compatibility(
                pool,
                run.id,
                &crate::gateway::compatibility_json(report),
            )
            .await?;
        }
        let notes = args
            .notes
            .as_deref()
            .map(str::trim)
            .filter(|notes| !notes.is_empty());
        TuneRunRow::update_notes(pool, run.id, notes).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(error) = metadata_result {
        finalize_preparation_failure(pool, run.id, &error.to_string()).await;
        return Err(error);
    }

    let ownership = if args.driver == DriverKind::Opcda {
        let ownership_result = async {
            let bridge_host = args
                .bridge_host
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("resolved OPC bridge host is missing"))?;
            let server = args
                .server
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("resolved OPC server is missing"))?;
            let intent = serde_json::to_string(&serde_json::json!({
                "version": 1,
                "kind": "tune_restore",
                "run_id": run.id,
                "state": "awaiting_initial_readings",
                "mv_tag": tags.manipulated_variable,
            }))?;
            LiveOperationGuard::acquire(
                pool,
                Some(run.id),
                LiveOperationKind::Tune,
                bridge_host,
                server,
                &tags.manipulated_variable,
                Some(intent),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        .await;
        match ownership_result {
            Ok(ownership) => Some(ownership),
            Err(error) => {
                finalize_preparation_failure(pool, run.id, &error.to_string()).await;
                return Err(error);
            }
        }
    } else {
        None
    };

    let setup_result =
        crate::driver::build_with_poll_interval(&args, timing.poll_interval_ms).await;
    let (driver, ownership) = complete_driver_setup(pool, run.id, setup_result, ownership).await?;

    tracing::info!(
        run_id = run.id,
        template = %args.template,
        process_type = ?config.process_type,
        controller_type = ?config.controller_type,
        driver = ?db_driver,
        allow_uncertain_quality,
        "starting tune run"
    );

    let write_pid = args.write_pid;

    Ok(PreparedTune {
        run_id: run.id,
        args,
        template,
        tags,
        driver,
        config,
        timing,
        time_anchor,
        write_pid,
        allow_uncertain_quality,
        ownership,
    })
}
pub(super) async fn finalize_preparation_failure(pool: &SqlitePool, run_id: i64, reason: &str) {
    if let Err(error) = TuneRunRow::fail(pool, run_id, Utc::now(), reason).await {
        tracing::error!(
            run_id,
            error = %error,
            "could not mark a failed preparation run terminal; attempting to delete it"
        );
        if let Err(delete_error) = TuneRunRow::delete(pool, run_id).await {
            tracing::error!(
                run_id,
                error = %delete_error,
                "could not remove a failed preparation run"
            );
        }
    }
}
pub(super) async fn complete_driver_setup(
    pool: &SqlitePool,
    run_id: i64,
    setup_result: anyhow::Result<Box<dyn Driver>>,
    ownership: Option<LiveOperationGuard>,
) -> anyhow::Result<(Box<dyn Driver>, Option<LiveOperationGuard>)> {
    match setup_result {
        Ok(driver) => Ok((driver, ownership)),
        Err(error) => {
            let failure_persisted =
                match TuneRunRow::fail(pool, run_id, Utc::now(), &error.to_string()).await {
                    Ok(_) => true,
                    Err(persist_error) => {
                        tracing::error!(
                            run_id,
                            error = %persist_error,
                            "failed to persist live tune preparation failure"
                        );
                        false
                    }
                };
            if failure_persisted && let Some(ownership) = ownership {
                ownership.release().await?;
            }
            if !failure_persisted {
                tracing::error!(
                    run_id,
                    "retaining live ownership so startup can export this orphaned running row"
                );
            }
            Err(error)
        }
    }
}
/// Runs an already-prepared tune without an interactive selector and returns its coarse
/// outcome. An unrequested write-back is skipped.
pub async fn drive(
    pool: &SqlitePool,
    prepared: PreparedTune,
    ctrl_c: &mut CtrlC,
) -> anyhow::Result<TuneOutcome> {
    let report = drive_report(pool, prepared, ctrl_c, None).await?;
    Ok(tune_outcome_for_run(&report.outcome))
}

/// Runs an already-prepared tune and returns the detailed runtime outcome for adapters that
/// report it to a user. An optional handler may select an interactive PID response level and
/// observe write-back progress; it never replaces runtime validation or safety checks.
pub async fn drive_report(
    pool: &SqlitePool,
    prepared: PreparedTune,
    ctrl_c: &mut CtrlC,
    handler: Option<&mut dyn WriteBackHandler>,
) -> anyhow::Result<TuneRunReport> {
    let PreparedTune {
        run_id,
        args,
        template,
        tags,
        driver,
        config,
        timing,
        time_anchor,
        write_pid,
        allow_uncertain_quality,
        ownership,
    } = prepared;
    let mut ownership = ownership;
    let audited_driver = ownership
        .as_ref()
        .map(|owner| AuditedDriver::new(driver.as_ref(), pool, owner, Some(run_id)));
    let execution_driver: &dyn Driver = audited_driver
        .as_ref()
        .map_or(driver.as_ref(), |audited| audited);

    let outcome = execute_with_timing(
        pool,
        run_id,
        &args,
        &template,
        &tags,
        execution_driver,
        config,
        timing,
        time_anchor,
        write_pid,
        allow_uncertain_quality,
        ctrl_c,
        handler,
        ownership.as_ref(),
    )
    .await;

    match outcome {
        Ok(run_outcome) => {
            let tune_outcome = tune_outcome_for_run(&run_outcome);
            tracing::info!(run_id, outcome = tune_outcome.label(), "tune run finished");
            if let Some(ownership) = ownership.take() {
                ownership.release().await?;
            }
            Ok(TuneRunReport {
                run_id,
                outcome: run_outcome,
            })
        }
        Err(e) => {
            tracing::error!(run_id, error = %e, "tune run failed");
            finalize_pending_for_run_best_effort(
                pool,
                run_id,
                "the run failed before MV confirmation completed",
            )
            .await;
            let failure_persistence =
                TuneRunRow::fail(pool, run_id, Utc::now(), &e.to_string()).await;
            if let Err(persist_error) = failure_persistence {
                return Err(anyhow::anyhow!(
                    "{e}; additionally failed to persist the terminal run failure: \
                     {persist_error}"
                ));
            }
            let ownership_release = match ownership.take() {
                Some(ownership) => ownership.release().await,
                None => Ok(()),
            };
            if let Err(release_error) = ownership_release {
                return Err(anyhow::anyhow!(
                    "{e}; additionally failed to persist live ownership release: \
                     {release_error}"
                ));
            }
            Err(e)
        }
    }
}
/// Everything read from the driver before any mode transition is attempted — mirrors
/// `ReadInitialOPCvalues`.
#[derive(Debug)]
pub(super) struct InitialState {
    pub(super) pv_ini: f32,
    pub(super) mv_ini: f32,
    pub(super) pv_range_high: f32,
    pub(super) pv_range_low: f32,
    pub(super) mv_range_high: f32,
    pub(super) mv_range_low: f32,
    pub(super) direction: ControllerDirection,
    pub(super) mode_raw: Option<String>,
    pub(super) mode_attribute_raw: Option<String>,
    /// The setpoint, captured here -- before any mutation of the loop -- whenever the loop's
    /// original mode is Auto and both a mode and a setpoint tag are configured. Persisting
    /// this through [`TuneRunRow::record_initial_readings`] before
    /// `transition_to_manual`'s first mutating write lets a crashed run recover its restore
    /// intent. Note that this field being `Some(..)` only proves the loop *was* in Auto --
    /// not that a mode transition was actually attempted, since `read_initial_values` runs
    /// unconditionally, before any such decision is made -- so `restore`'s setpoint-revert
    /// step is additionally gated on [`MutationGuard::mode_written`], not on this field
    /// alone.
    pub(super) setpoint_ini: Option<f32>,
}
/// Tracks which of `transition_to_manual`'s mutations were actually *attempted* -- armed
/// immediately before each write is issued, not after it succeeds -- so `restore` can
/// independently decide what's safe/necessary to revert even when `transition_to_manual`
/// itself returns partway through with an error. The captured setpoint lives on
/// [`InitialState`], where it can be persisted before any mutation.
#[derive(Debug, Default)]
pub(super) struct MutationGuard {
    /// The mode-attribute tag's "put in program/computer mode" write was attempted.
    pub(super) mode_attribute_written: bool,
    /// The mode tag's "put in Manual" write was attempted.
    pub(super) mode_written: bool,
    /// A relay-test MV write was attempted at least once during `run_polling_loop`. Tracked
    /// for completeness/audit parity with the other two flags -- `restore`'s MV-revert step
    /// is unconditional and never gated by this flag, since a redundant write-back is always
    /// harmless (idempotent if the pre-test value is already there, or safely rejected by
    /// the DCS if the loop never actually left Auto), so there is no correctness reason to
    /// skip attempting it even while this is still `false`.
    pub(super) mv_written: bool,
}
/// Persists the three calculated response levels after a completed MRFT test. The run's
/// terminal outcome is deliberately recorded later, after restoration, together with its
/// timing snapshot; this keeps SSE-triggered readers from seeing a terminal row before those
/// diagnostics are visible.
pub(super) async fn persist_completed_results(
    pool: &SqlitePool,
    run_id: i64,
    completion: Action,
    direction: ControllerDirection,
    config: LoopConfig,
    pv_range: PvRange,
    template: &DcsTemplate,
) -> anyhow::Result<Vec<PidWritePreview>> {
    persist_results(
        pool, run_id, completion, direction, config, pv_range, template,
    )
    .await
}
/// The runtime owns tune execution and delegates any interactive write-back choice to an
/// adapter-provided handler. A missing handler means no unrequested write-back; no terminal
/// or transport I/O is performed here.
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_with_timing(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneRequest,
    template: &DcsTemplate,
    tags: &LoopTags,
    driver: &dyn Driver,
    config: LoopConfig,
    effective_timing: EffectiveTiming,
    time_anchor: RunTimeAnchor,
    write_pid: Option<ResponseLevel>,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    handler: Option<&mut dyn WriteBackHandler>,
    ownership: Option<&LiveOperationGuard>,
) -> anyhow::Result<RunOutcome> {
    let started_at = time_anchor.utc();
    let initial = read_initial_values(driver, tags, template, allow_uncertain_quality).await?;
    validate_initial_state(&initial)?;
    validate_relay_actuation_step(args, config, &initial)?;

    // Persisted before any mutation is attempted: a crash between here and a confirmed
    // restore still leaves a durable record of the mode/mode-attribute/
    // setpoint as they were *before* anything was written, so `bhtune restore-loop` can
    // reconstruct and restore the loop later even if the process never gets to do so itself.
    TuneRunRow::record_initial_readings(
        pool,
        run_id,
        TuneRunInitialReadings {
            pv_ini: initial.pv_ini,
            mv_ini: initial.mv_ini,
            mv_range_low: initial.mv_range_low,
            mv_range_high: initial.mv_range_high,
            pv_range_high: initial.pv_range_high,
            pv_range_low: initial.pv_range_low,
            controller_direction: initial.direction,
            mode_raw: initial.mode_raw.clone(),
            mode_attribute_raw: initial.mode_attribute_raw.clone(),
            setpoint_ini: initial.setpoint_ini,
        },
    )
    .await?;
    if let Some(ownership) = ownership {
        let restore_intent = serde_json::to_string(&serde_json::json!({
            "version": 1,
            "kind": "tune_restore",
            "run_id": run_id,
            "state": "ready_to_restore",
            "mv_tag": tags.manipulated_variable,
            "initial_readings": {
                "pv_ini": initial.pv_ini,
                "mv_ini": initial.mv_ini,
                "mv_range_low": initial.mv_range_low,
                "mv_range_high": initial.mv_range_high,
                "pv_range_low": initial.pv_range_low,
                "pv_range_high": initial.pv_range_high,
                "controller_direction": initial.direction,
                "mode_raw": initial.mode_raw,
                "mode_attribute_raw": initial.mode_attribute_raw,
                "setpoint_ini": initial.setpoint_ini,
            },
            "template_policy": {
                "revert_mode": template.revert_mode,
                "mode_auto_value": template.mode_auto_value,
                "mode_manual_value": template.mode_manual_value,
                "mode_attribute_program_value": template.mode_attribute_program_value,
            },
        }))?;
        ownership.persist_restore_intent(&restore_intent).await?;
    }

    let mut guard = MutationGuard::default();
    let mut mv_actuations = MvActuationTracker::for_run(args, &initial);
    if let Err(e) = transition_to_manual(driver, tags, template, &initial, &mut guard).await {
        return Err(restore_best_effort_then_propagate_with_timing(
            pool,
            run_id,
            driver,
            tags,
            template,
            &initial,
            &guard,
            args,
            effective_timing,
            allow_uncertain_quality,
            ctrl_c,
            &mut mv_actuations,
            e,
        )
        .await);
    }

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
    let timing_basis = match args.driver {
        DriverKind::Opcda => TimingBasis::LiveMonotonic,
        DriverKind::Simulator => TimingBasis::SimulatedFixedStep,
    };
    let mut timing = PollTimingAccumulator::new(timing_basis, effective_timing.poll_interval_ms);

    let poll_result = run_polling_loop_with_timing(
        pool,
        run_id,
        args,
        effective_timing,
        tags,
        driver,
        &mut engine,
        time_anchor,
        ctrl_c,
        &mut guard,
        allow_uncertain_quality,
        &mut timing,
        &mut mv_actuations,
        config,
    )
    .await;
    let measured_oscillation_period_ms = completed_oscillation_period_ms(
        &poll_result,
        initial.direction,
        config,
        PvRange {
            high: initial.pv_range_high,
            low: initial.pv_range_low,
        },
    );
    let timing_metrics_without_period = timing.finish(None);
    if let Some(timing_metrics) = timing_metrics_without_period.as_ref() {
        warn_on_missed_poll_opportunities(run_id, timing_metrics);
    }

    match poll_result {
        Ok(PollOutcome::Completed(completion)) => {
            finish_completed_run(
                pool,
                run_id,
                args,
                effective_timing,
                template,
                tags,
                driver,
                config,
                &initial,
                &guard,
                write_pid,
                allow_uncertain_quality,
                ctrl_c,
                handler,
                ownership,
                &mut mv_actuations,
                completion,
                &mut timing,
                measured_oscillation_period_ms,
            )
            .await
        }
        Ok(PollOutcome::Aborted(reason)) => {
            finish_aborted_run(
                pool,
                run_id,
                args,
                effective_timing,
                template,
                tags,
                driver,
                &initial,
                &guard,
                allow_uncertain_quality,
                ctrl_c,
                &mut mv_actuations,
                reason,
                timing_metrics_without_period,
            )
            .await
        }
        Err(error) => {
            finish_failed_run(
                pool,
                run_id,
                template,
                tags,
                driver,
                &initial,
                &guard,
                args,
                effective_timing,
                allow_uncertain_quality,
                ctrl_c,
                &mut mv_actuations,
                error,
                timing_metrics_without_period,
            )
            .await
        }
    }
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute<R: std::io::BufRead>(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneRequest,
    template: &DcsTemplate,
    tags: &LoopTags,
    driver: &dyn Driver,
    config: LoopConfig,
    time_anchor: RunTimeAnchor,
    write_pid: Option<ResponseLevel>,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    _reader: &mut R,
) -> anyhow::Result<RunOutcome> {
    execute_with_timing(
        pool,
        run_id,
        args,
        template,
        tags,
        driver,
        config,
        test_effective_timing(args),
        time_anchor,
        write_pid,
        allow_uncertain_quality,
        ctrl_c,
        None,
        None,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn attempt_and_record_restore(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneRequest,
    effective_timing: EffectiveTiming,
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
) -> RestoreAttempt {
    attempt_and_record_restore_with_settling(
        pool,
        run_id,
        args,
        effective_timing,
        driver,
        tags,
        template,
        initial,
        guard,
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
        None,
        None,
        None,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn attempt_and_record_restore_with_settling(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneRequest,
    effective_timing: EffectiveTiming,
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
    completion: Option<&mut CompletedPoll>,
    measured_oscillation_period_ms: Option<f64>,
    timing: Option<&mut PollTimingAccumulator>,
) -> RestoreAttempt {
    let restore_attempt = attempt_restore_with_actuation_with_timing(
        pool,
        run_id,
        args,
        effective_timing,
        driver,
        tags,
        template,
        initial,
        guard,
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
        completion,
        measured_oscillation_period_ms,
        timing,
    )
    .await;
    record_restore_status_best_effort(pool, run_id, &restore_attempt).await;
    finalize_pending_for_run_best_effort(
        pool,
        run_id,
        "the run ended before MV confirmation completed",
    )
    .await;
    restore_attempt
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn finish_completed_run(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneRequest,
    effective_timing: EffectiveTiming,
    template: &DcsTemplate,
    tags: &LoopTags,
    driver: &dyn Driver,
    config: LoopConfig,
    initial: &InitialState,
    guard: &MutationGuard,
    write_pid: Option<ResponseLevel>,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mut handler: Option<&mut dyn WriteBackHandler>,
    ownership: Option<&LiveOperationGuard>,
    mv_actuations: &mut Option<MvActuationTracker>,
    mut completion: CompletedPoll,
    timing: &mut PollTimingAccumulator,
    measured_oscillation_period_ms: Option<f64>,
) -> anyhow::Result<RunOutcome> {
    let pv_range = PvRange {
        high: initial.pv_range_high,
        low: initial.pv_range_low,
    };
    let previews = match persist_completed_results(
        pool,
        run_id,
        completion.action.clone(),
        initial.direction,
        config,
        pv_range,
        template,
    )
    .await
    {
        Ok(previews) => previews,
        Err(error) => {
            let restore_attempt = attempt_and_record_restore_with_settling(
                pool,
                run_id,
                args,
                effective_timing,
                driver,
                tags,
                template,
                initial,
                guard,
                allow_uncertain_quality,
                ctrl_c,
                mv_actuations,
                Some(&mut completion),
                measured_oscillation_period_ms,
                Some(timing),
            )
            .await;
            let error = match restore_attempt {
                RestoreAttempt::Confirmed => error,
                RestoreAttempt::Incomplete { reason } => {
                    tracing::warn!(
                        run_id,
                        reason = %reason,
                        "completed MRFT result persistence failed and restore was incomplete"
                    );
                    error
                }
            };
            record_timing_metrics_if_present(pool, run_id, timing.finish(None)).await;
            return Err(error);
        }
    };
    let restore_attempt = attempt_and_record_restore_with_settling(
        pool,
        run_id,
        args,
        effective_timing,
        driver,
        tags,
        template,
        initial,
        guard,
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
        Some(&mut completion),
        measured_oscillation_period_ms,
        Some(timing),
    )
    .await;
    TuneRunRow::complete_with_timing_metrics(
        pool,
        run_id,
        Utc::now(),
        timing.finish(measured_oscillation_period_ms),
    )
    .await?;
    if let Some(handler) = handler.as_deref_mut() {
        handler.pid_results_ready(&previews);
    }
    match restore_attempt {
        RestoreAttempt::Confirmed => {
            let (write_back, write_back_detail) = maybe_write_back_with_owner(
                pool,
                run_id,
                tags,
                template,
                driver,
                config,
                write_pid,
                allow_uncertain_quality,
                handler,
                ownership,
            )
            .await?;
            Ok(RunOutcome::Completed {
                write_back,
                write_back_detail,
            })
        }
        RestoreAttempt::Incomplete { reason } => Ok(RunOutcome::RestoreIncomplete { reason }),
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn finish_aborted_run(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneRequest,
    effective_timing: EffectiveTiming,
    template: &DcsTemplate,
    tags: &LoopTags,
    driver: &dyn Driver,
    initial: &InitialState,
    guard: &MutationGuard,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
    reason: AbortReason,
    timing_metrics_without_period: Option<TimingMetrics>,
) -> anyhow::Result<RunOutcome> {
    let restore_attempt = attempt_and_record_restore(
        pool,
        run_id,
        args,
        effective_timing,
        driver,
        tags,
        template,
        initial,
        guard,
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
    )
    .await;
    if matches!(reason, AbortReason::MvActuationUnconfirmed { .. }) {
        TuneRunRow::abort_with_timing_metrics_and_reason(
            pool,
            run_id,
            Utc::now(),
            timing_metrics_without_period,
            &format_mv_actuation_abort_reason(&reason),
        )
        .await?;
    } else {
        TuneRunRow::abort_with_timing_metrics(
            pool,
            run_id,
            Utc::now(),
            timing_metrics_without_period,
        )
        .await?;
    }
    match restore_attempt {
        RestoreAttempt::Confirmed => Ok(RunOutcome::Aborted(reason)),
        RestoreAttempt::Incomplete {
            reason: restore_reason,
        } => Ok(RunOutcome::RestoreIncomplete {
            reason: format!("run aborted ({reason:?}); {restore_reason}"),
        }),
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn finish_failed_run(
    pool: &SqlitePool,
    run_id: i64,
    template: &DcsTemplate,
    tags: &LoopTags,
    driver: &dyn Driver,
    initial: &InitialState,
    guard: &MutationGuard,
    args: &TuneRequest,
    effective_timing: EffectiveTiming,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
    error: anyhow::Error,
    timing_metrics_without_period: Option<TimingMetrics>,
) -> anyhow::Result<RunOutcome> {
    // Best-effort: a failed test still stroked the valve, so try to put it back even
    // though the overall run is going to be reported as failed regardless. Still
    // bounded/interruptible (a second Ctrl+C or `[tuning].restore_timeout_secs` still cuts
    // it short) and still warns loudly on an incomplete restore.
    let error = restore_best_effort_then_propagate_with_timing(
        pool,
        run_id,
        driver,
        tags,
        template,
        initial,
        guard,
        args,
        effective_timing,
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
        error,
    )
    .await;
    record_timing_metrics_if_present(pool, run_id, timing_metrics_without_period).await;
    Err(error)
}
/// Pure port of `ReadInitialOPCvalues`: everything read before any mode transition.
pub(super) async fn read_initial_values(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    allow_uncertain: bool,
) -> anyhow::Result<InitialState> {
    let mut requested_tags = Vec::new();
    let mut seen_tags = HashSet::new();
    let mut request_tag = |tag: &str| {
        if seen_tags.insert(tag.to_string()) {
            requested_tags.push(tag.to_string());
        }
    };

    request_tag(&tags.process_variable);
    request_tag(&tags.manipulated_variable);
    if let Some(tag) = &tags.controller_mode {
        request_tag(tag);
    }
    if let Some(tag) = &tags.mode_attribute {
        request_tag(tag);
    }
    if let TagOrValue::Tag(tag) = &tags.controller_direction {
        request_tag(tag.as_str());
    }
    for tag_or_value in [
        &tags.upper_pv_range,
        &tags.lower_pv_range,
        &tags.upper_mv_range,
        &tags.lower_mv_range,
    ] {
        if let TagOrValue::Tag(tag) = tag_or_value {
            request_tag(tag.as_str());
        }
    }

    let values_by_tag: HashMap<String, TagValue> = driver
        .read(&requested_tags)
        .await?
        .into_iter()
        .map(|value| (value.tag.clone(), value))
        .collect();

    let pv_ini = read_batch_f32(&values_by_tag, &tags.process_variable, allow_uncertain)?;
    let mv_ini = read_batch_f32(&values_by_tag, &tags.manipulated_variable, allow_uncertain)?;

    let mode_raw = match &tags.controller_mode {
        Some(tag) => Some(read_batch_raw(&values_by_tag, tag, allow_uncertain)?),
        None => None,
    };
    let mode_attribute_raw = match &tags.mode_attribute {
        Some(tag) => Some(read_batch_raw(&values_by_tag, tag, allow_uncertain)?),
        None => None,
    };

    // Captured here, before any mutation, whenever the loop's original mode is Auto -- see
    // `InitialState::setpoint_ini`'s doc comment for why this read is hoisted out of
    // `transition_to_manual`, where the legacy app captures the analogous `SvValueIni`.
    let setpoint_ini = match (&tags.setpoint_variable, &mode_raw) {
        (Some(sv_tag), Some(mode_raw)) if mode_raw == &template.mode_auto_value => {
            Some(read_f32(driver, sv_tag, allow_uncertain).await?)
        }
        _ => None,
    };

    let direction = resolve_direction_from_batch(
        driver,
        &values_by_tag,
        &tags.controller_direction,
        template,
        allow_uncertain,
    )
    .await?;
    let pv_range_high = resolve_f32_from_batch(
        driver,
        &values_by_tag,
        &tags.upper_pv_range,
        allow_uncertain,
    )
    .await?;
    let pv_range_low = resolve_f32_from_batch(
        driver,
        &values_by_tag,
        &tags.lower_pv_range,
        allow_uncertain,
    )
    .await?;
    let mv_range_high = resolve_f32_from_batch(
        driver,
        &values_by_tag,
        &tags.upper_mv_range,
        allow_uncertain,
    )
    .await?;
    let mv_range_low = resolve_f32_from_batch(
        driver,
        &values_by_tag,
        &tags.lower_mv_range,
        allow_uncertain,
    )
    .await?;

    Ok(InitialState {
        pv_ini,
        mv_ini,
        pv_range_high,
        pv_range_low,
        mv_range_high,
        mv_range_low,
        direction,
        mode_raw,
        mode_attribute_raw,
        setpoint_ini,
    })
}
/// The single choke point validating an `InitialState` -- from live driver tags and/or CLI
/// flag overrides alike -- before any mutation of the loop happens (i.e. called between
/// `read_initial_values` and `transition_to_manual` in `execute`, never after). `read_f32`/
/// `resolve_f32` already reject a non-finite individual value as it's read; this additionally
/// checks values *together*: range ordering, zero span, and that the initial MV actually
/// lies inside its own reported range.
pub(super) fn validate_initial_state(initial: &InitialState) -> anyhow::Result<()> {
    PvRange::new(initial.pv_range_high, initial.pv_range_low)
        .map_err(|e| anyhow::anyhow!("invalid PV range: {e}"))?;
    let mv_range = MvRange::new(initial.mv_range_high, initial.mv_range_low)
        .map_err(|e| anyhow::anyhow!("invalid MV range: {e}"))?;
    if !mv_range.contains(initial.mv_ini) {
        anyhow::bail!(
            "initial MV {} is outside the MV range [{}, {}]",
            initial.mv_ini,
            initial.mv_range_low,
            initial.mv_range_high
        );
    }
    Ok(())
}
/// Pure port of `ChangeControllerModeToMan`. No-ops entirely when `tags.controller_mode` and
/// `tags.mode_attribute` are both `None` (the simulator's case). `guard`'s flags are armed
/// immediately before each write is attempted, not after it succeeds -- see
/// [`MutationGuard`]'s doc comment for why that ordering matters -- so a caller still knows
/// exactly what was attempted even if this function returns partway through with an error.
pub(super) async fn transition_to_manual(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &mut MutationGuard,
) -> anyhow::Result<()> {
    if let (Some(attr_tag), Some(program_value)) =
        (&tags.mode_attribute, &template.mode_attribute_program_value)
    {
        guard.mode_attribute_written = true;
        write_raw(driver, attr_tag, program_value.clone()).await?;
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }

    if let Some(mode_tag) = &tags.controller_mode {
        let mode_raw = initial.mode_raw.as_deref().unwrap_or_default();
        if mode_raw != template.mode_manual_value {
            guard.mode_written = true;
            write_raw(driver, mode_tag, template.mode_manual_value.clone()).await?;
        }
    }

    Ok(())
}
