#![allow(rustdoc::broken_intra_doc_links)]

use std::future::Future;
use std::time::Duration;

use bhtune_core::{
    Action, ControllerDirection, DcsTemplate, LoopConfig, LoopTags, MrftEngine, MrftState, PvRange,
    Tick, TuningMathCompat, calculate_all_checked,
};
use bhtune_db::SqlitePool;
use bhtune_db::models::{MvActuationKind, SampleQuality, TuneResultRow, TuneSampleRow};
use bhtune_driver::Driver;
use chrono::{DateTime, Utc};
use tokio::time::Instant;

use super::request::TuneRequest;
use crate::cancel::CtrlC;
use crate::timing::{PollTimingAccumulator, RunTimeAnchor, TickTimeSource};

use super::RequireInvariant;
use super::actuation::{
    ActuationAuditPolicy, MvActuationTracker, MvVerificationCallLimit, checked_at_for_pending,
    mv_actuation_tolerance, record_relay_actuation, reject_replacement_for_pending_actuation,
    resolve_pending_mv_poll, verification_trigger, verify_pending_mv_actuation_with_timing,
    wait_for_mv_verification,
};
use super::config::EffectiveTiming;
#[cfg(test)]
use super::config::test_effective_timing;
use super::outcome::AbortReason;
use super::prepare::MutationGuard;
use super::quality::{
    check_quality, read_numeric_from_batch, read_poll_batch, sample_quality_from_driver,
    write_value,
};

/// The outcome of racing one driver call (a poll batch or [`write_value`], during a poll tick)
/// against Ctrl+C and `[tuning].op_timeout_secs` -- see [`bounded_driver_call`]. Distinct from a
/// genuine `Err` from the call itself (a rejected write, a malformed value, a transport error),
/// which [`bounded_driver_call`] still propagates via `?` rather than wrapping here, since those
/// are real failures, not "gave up waiting".
#[derive(Debug)]
pub(super) enum TickOperation<T> {
    /// `fut` resolved before either interrupt source.
    Completed(T),
    /// Ctrl+C (or a second Ctrl+C) fired first; `fut` was dropped, abandoning it in flight.
    Cancelled,
    /// `[tuning].op_timeout_secs` elapsed first; `fut` was dropped, abandoning it in flight.
    TimedOut,
}
/// Races one driver call against `ctrl_c` and a fresh `op_timeout_secs` sleep, so a single
/// stalled read/write (gateway down, DCOM wedged, network black-holed) can never make the
/// polling loop -- or the restore, via [`attempt_restore_with_actuation`] -- uninterruptible.
/// This is what
/// fixes finding 2 of the live-plant safety review: previously, `run_polling_loop`'s Ctrl+C
/// and `[tuning].timeout_secs` listeners only ran *between* tick-body awaits, so a hung call inside
/// one was invisible to both. `fut` is taken by value (not `&mut`) and is simply dropped,
/// abandoning the in-flight operation, on the losing branches -- there is no cancellation
/// signal sent to the driver itself, only to this call's own wait for it. A genuine `Err`
/// from `fut` resolving still propagates through the `?` here, distinct from either
/// [`TickOperation`] interrupt case.
pub(super) async fn bounded_driver_call<T>(
    op_timeout_secs: u64,
    ctrl_c: &mut CtrlC,
    fut: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<TickOperation<T>> {
    tokio::select! {
        result = fut => result.map(TickOperation::Completed),
        () = ctrl_c.signalled() => Ok(TickOperation::Cancelled),
        () = tokio::time::sleep(Duration::from_secs(op_timeout_secs)) => Ok(TickOperation::TimedOut),
    }
}
/// Distinguishes *why* [`run_polling_loop`] ended without a normal engine completion, so
/// `execute` can record and report the right [`AbortReason`].
pub(super) enum PollOutcome {
    /// The engine reported [`Action::Complete`] and any post-completion
    /// `[tuning].mrft_delay_secs`
    /// padding has elapsed.
    Completed(CompletedPoll),
    /// Ctrl+C, `[tuning].timeout_secs`, `[tuning].op_timeout_secs`, or a poor-quality PV sample ended the
    /// run before that.
    Aborted(AbortReason),
}
pub(super) struct CompletedPoll {
    pub(super) action: Action,
    pub(super) state: MrftState,
    pub(super) next_tick_index: i64,
    pub(super) tick_time: TickTimeSource,
}
pub(super) async fn insert_tune_sample_with_timing(
    pool: &SqlitePool,
    run_id: i64,
    tick_index: i64,
    tick: Tick,
    state: bhtune_core::MrftState,
    sample_quality: SampleQuality,
    timing: &mut PollTimingAccumulator,
) -> anyhow::Result<()> {
    let started = Instant::now();
    TuneSampleRow::insert(pool, run_id, tick_index, tick, state, sample_quality).await?;
    timing.observe_sample_persist(started.elapsed());
    Ok(())
}
/// Polls the driver on the frozen global polling interval, driving `engine` once the pre-test
/// `[tuning].mrft_delay_secs` padding period has elapsed, and continuing to record (but not evaluate)
/// samples for the same padding period after completion. Returns `Ok(PollOutcome::Completed`
/// on a normal finish, `Ok(PollOutcome::Aborted)` if interrupted by Ctrl+C, by
/// the frozen whole-run timeout elapsing, or by a single driver call exceeding the frozen
/// operation timeout -- the last of these via [`bounded_driver_call`], which wraps
/// every driver read/write in the tick body so a stalled call is abandoned rather than
/// awaited forever, keeping Ctrl+C and both timeouts effective even mid-hung-read/write. The
/// outer `tokio::select!` below still separately covers the *idle* wait between ticks (via
/// `ctrl_c`, shared with every `bounded_driver_call` inside the winning tick body -- see
/// that function's doc comment for why reusing it across nested `select!`s is safe) and the
/// whole-run `[tuning].timeout_secs` deadline.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_polling_loop_with_timing(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneRequest,
    effective_timing: EffectiveTiming,
    tags: &LoopTags,
    driver: &dyn Driver,
    engine: &mut MrftEngine,
    time_anchor: RunTimeAnchor,
    ctrl_c: &mut CtrlC,
    guard: &mut MutationGuard,
    allow_uncertain_quality: bool,
    timing: &mut PollTimingAccumulator,
    mv_actuations: &mut Option<MvActuationTracker>,
    config: LoopConfig,
) -> anyhow::Result<PollOutcome> {
    let start_time = time_anchor.utc();
    let mut tick_time =
        TickTimeSource::for_driver(args.driver, time_anchor, effective_timing.poll_interval_ms)?;
    let poll_interval = Duration::from_millis(effective_timing.poll_interval_ms);
    let mut next_poll_at = Instant::now();

    let pre_delay_end =
        start_time + chrono::Duration::seconds(i64::from(effective_timing.mrft_delay_secs));
    let mut tick_index: i64 = 0;
    let mut completion: Option<Action> = None;
    let mut post_delay_end: Option<DateTime<Utc>> = None;

    // A mandatory safety net for unattended operation: an unattended run must never be able
    // to perturb a live process indefinitely (a stuck relay, a misconfigured tag mapping that
    // never crosses hysteresis, a stalled driver read). Created once and raced via
    // `tokio::select!` on every iteration below, rather than checked only after each
    // completed tick, so it fires even if a single `read_f32` call itself hangs.
    let timeout = tokio::time::sleep(Duration::from_secs(effective_timing.timeout_secs));
    tokio::pin!(timeout);

    loop {
        let verification_wakeup = mv_actuations
            .as_ref()
            .and_then(MvActuationTracker::next_verification_wakeup);
        tokio::select! {
            biased;
            _ = wait_for_mv_verification(verification_wakeup) => {
                let trigger = mv_actuations
                    .as_ref()
                    .and_then(|tracker| tracker.pending.as_ref())
                    .and_then(|pending| verification_trigger(pending, Instant::now()))
                    .require_invariant("a verification wakeup requires a due pending actuation")?;
                let tracker = mv_actuations
                    .as_mut()
                    .require_invariant("a verification trigger requires an OPC DA tracker")?;
                let reason = verify_pending_mv_actuation_with_timing(
                    pool,
                    args,
                    effective_timing,
                    &tags.manipulated_variable,
                    driver,
                    ctrl_c,
                    allow_uncertain_quality,
                    tracker,
                    trigger,
                    MvVerificationCallLimit::None,
                    ActuationAuditPolicy::Required,
                    Some(timing),
                )
                .await?;
                return Ok(match reason {
                    Some(reason) => PollOutcome::Aborted(reason),
                    None => continue,
                });
            }
            _ = tokio::time::sleep_until(next_poll_at) => {
                let tick_started = Instant::now();
                next_poll_at = Instant::now() + poll_interval;
                let pending_actuation = mv_actuations
                    .as_ref()
                    .is_some_and(|tracker| tracker.pending.is_some());
                let pv_read_started = Instant::now();
                let (pv, quality, poll_provided_mv_evidence, batched_mv_abort_reason) = match bounded_driver_call(
                    effective_timing.op_timeout_secs,
                    ctrl_c,
                    read_poll_batch(
                        driver,
                        &tags.process_variable,
                        pending_actuation.then_some(tags.manipulated_variable.as_str()),
                    ),
                )
                .await?
                {
                    TickOperation::Completed(values) => {
                        timing.observe_pv_read(pv_read_started.elapsed());
                        let completed_at = Instant::now();
                        let (batched_mv_abort_reason, poll_provided_mv_evidence) =
                            if pending_actuation {
                                let pending = mv_actuations
                                    .as_ref()
                                    .and_then(|tracker| tracker.pending.as_ref())
                                    .require_invariant("pending actuation existed for the batched poll")?;
                                let checked_at = checked_at_for_pending(pending, completed_at)?;
                                let tracker = mv_actuations
                                    .as_mut()
                                    .require_invariant("pending actuation requires an OPC DA tracker")?;
                                resolve_pending_mv_poll(
                                    pool,
                                    effective_timing,
                                    TickOperation::Completed(values.clone()),
                                    &tags.manipulated_variable,
                                    checked_at,
                                    completed_at,
                                    pv_read_started.elapsed(),
                                    allow_uncertain_quality,
                                    tracker,
                                    timing,
                                )
                                .await?
                            } else {
                                (None, false)
                            };
                        let (pv, quality) = read_numeric_from_batch(&values, &tags.process_variable)?;
                        (
                            pv,
                            quality,
                            poll_provided_mv_evidence,
                            batched_mv_abort_reason,
                        )
                    }
                    TickOperation::Cancelled => {
                        if let Some(tracker) = mv_actuations.as_mut()
                            && tracker.pending.is_some()
                        {
                            let completed_at = Instant::now();
                            let pending = tracker
                                .pending
                                .as_ref()
                                .require_invariant("pending actuation existed for the cancelled poll")?;
                            let checked_at = checked_at_for_pending(pending, completed_at)?;
                            let (reason, _) = resolve_pending_mv_poll(
                                pool,
                                effective_timing,
                                TickOperation::Cancelled,
                                &tags.manipulated_variable,
                                checked_at,
                                completed_at,
                                pv_read_started.elapsed(),
                                allow_uncertain_quality,
                                tracker,
                                timing,
                            )
                            .await?;
                            let reason =
                                reason.require_invariant("a cancelled pending MV poll must abort the run")?;
                            return Ok(PollOutcome::Aborted(reason));
                        }
                        tracing::warn!(run_id, tick_index, "Ctrl+C received while reading the PV; aborting run");
                        return Ok(PollOutcome::Aborted(AbortReason::UserInterrupt));
                    }
                    TickOperation::TimedOut => {
                        if let Some(tracker) = mv_actuations.as_mut()
                            && tracker.pending.is_some()
                        {
                            let completed_at = Instant::now();
                            let pending = tracker
                                .pending
                                .as_ref()
                                .require_invariant("pending actuation existed for the timed-out poll")?;
                            let checked_at = checked_at_for_pending(pending, completed_at)?;
                            let (reason, _) = resolve_pending_mv_poll(
                                pool,
                                effective_timing,
                                TickOperation::TimedOut,
                                &tags.manipulated_variable,
                                checked_at,
                                completed_at,
                                pv_read_started.elapsed(),
                                allow_uncertain_quality,
                                tracker,
                                timing,
                            )
                            .await?;
                            let reason =
                                reason.require_invariant("a timed-out pending MV poll must abort the run")?;
                            return Ok(PollOutcome::Aborted(reason));
                        }
                        tracing::warn!(
                            run_id,
                            tick_index,
                            op_timeout_secs = effective_timing.op_timeout_secs,
                            tag = %tags.process_variable,
                            "[tuning].op_timeout_secs elapsed reading the PV; aborting run"
                        );
                        return Ok(PollOutcome::Aborted(AbortReason::OperationTimedOut {
                            tag: tags.process_variable.clone(),
                            op_timeout_secs: effective_timing.op_timeout_secs,
                        }));
                    }
                };
                // Timestamp the value after it is actually read. For OPC DA this includes the
                // read's real monotonic latency; for the simulator it advances the logical
                // process clock by exactly one fixed poll step per successful PV sample.
                let tick_observed_instant = Instant::now();
                let now = tick_time.next_timestamp()?;
                timing.observe(now)?;
                let tick = Tick { time: now, pv };
                let sample_quality = sample_quality_from_driver(quality);

                if let Err(e) = check_quality(&tags.process_variable, quality, allow_uncertain_quality) {
                    tracing::warn!(
                        run_id,
                        tick_index,
                        tag = %tags.process_variable,
                        quality = ?quality,
                        error = %e,
                        "PV quality check failed; aborting run"
                    );
                    // Record the triggering sample (with its real quality) before aborting, so
                    // the history explorer can show exactly what was seen when the run gave up.
                    insert_tune_sample_with_timing(
                        pool,
                        run_id,
                        tick_index,
                        tick,
                        engine.state(),
                        sample_quality,
                        timing,
                    )
                    .await?;
                    timing.observe_tick_work(tick_started.elapsed());
                    return Ok(PollOutcome::Aborted(AbortReason::PoorQuality {
                        tag: tags.process_variable.clone(),
                        quality,
                    }));
                }

                if let Some(reason) = batched_mv_abort_reason {
                    insert_tune_sample_with_timing(
                        pool,
                        run_id,
                        tick_index,
                        tick,
                        engine.state(),
                        sample_quality,
                        timing,
                    )
                    .await?;
                    timing.observe_tick_work(tick_started.elapsed());
                    return Ok(PollOutcome::Aborted(reason));
                }

                if completion.is_none() && now < pre_delay_end {
                    insert_tune_sample_with_timing(
                        pool,
                        run_id,
                        tick_index,
                        tick,
                        engine.state(),
                        sample_quality,
                        timing,
                    )
                    .await?;
                    timing.observe_tick_work(tick_started.elapsed());
                    tick_index += 1;
                    continue;
                }

                // When an earlier command is still pending, evaluate this tick against a
                // clone. The real engine is committed only if the preview does not request a
                // replacement command. This keeps the engine's counters/switch timestamp and
                // final-completion state causally behind physical MV confirmation.
                let state_before_step = engine.state();
                let actions = if mv_actuations
                    .as_ref()
                    .is_some_and(|tracker| tracker.pending.is_some())
                {
                    let mut preview = engine.clone();
                    let actions = preview.step(tick);
                    if actions.iter().any(|action| matches!(action, Action::WriteMv(_))) {
                        let tracker = mv_actuations
                            .as_mut()
                            .require_invariant("a pending actuation requires an OPC DA tracker")?;
                        assert!(
                            poll_provided_mv_evidence,
                            "a pending batched poll must provide MV evidence before replacement preview"
                        );
                        let reason = reject_replacement_for_pending_actuation(
                            pool,
                            &tags.manipulated_variable,
                            tracker,
                        )
                        .await?;
                        insert_tune_sample_with_timing(
                            pool,
                            run_id,
                            tick_index,
                            tick,
                            state_before_step,
                            sample_quality,
                            timing,
                        )
                        .await?;
                        timing.observe_tick_work(tick_started.elapsed());
                        return Ok(PollOutcome::Aborted(reason));
                    }
                    *engine = preview;
                    actions
                } else {
                    engine.step(tick)
                };
                for action in actions {
                    match action {
                        Action::WriteMv(v) => {
                            let tolerance = mv_actuations
                                .as_ref()
                                .map(|tracker| {
                                    mv_actuation_tolerance(
                                        MvActuationKind::Relay,
                                        v,
                                        tracker.previous_commanded_mv,
                                        tracker.mv_span,
                                    )
                                })
                                .transpose()?;
                            guard.mv_written = true;
                                let mv_write_started = Instant::now();
                                match bounded_driver_call(
                                effective_timing.op_timeout_secs,
                                ctrl_c,
                                write_value(driver, &tags.manipulated_variable, v),
                            )
                            .await?
                            {
                                TickOperation::Completed(()) => {
                                    timing.observe_mv_write(mv_write_started.elapsed());
                                    if let (Some(tracker), Some(tolerance)) =
                                        (mv_actuations.as_mut(), tolerance)
                                    {
                                        let commanded_instant = Instant::now();
                                        record_relay_actuation(
                                            tracker,
                                            pool,
                                            run_id,
                                            v,
                                            now,
                                            tick_observed_instant,
                                            tick_observed_instant
                                                + Duration::from_secs(u64::from(
                                                    config.noise_protection_secs,
                                                )),
                                            commanded_instant
                                                .saturating_duration_since(tick_observed_instant),
                                            commanded_instant,
                                            tolerance,
                                        )
                                        .await?;
                                    }
                                }
                                TickOperation::Cancelled => {
                                    // A valid sample/tick is already in hand for this iteration
                                    // (unlike the PV-read timeout/cancel case above), so record
                                    // it before aborting -- same rationale as the quality-check
                                    // abort above.
                                    insert_tune_sample_with_timing(
                                        pool,
                                        run_id,
                                        tick_index,
                                        tick,
                                        state_before_step,
                                        sample_quality,
                                        timing,
                                    )
                                    .await?;
                                    timing.observe_tick_work(tick_started.elapsed());
                                    tracing::warn!(run_id, tick_index, "Ctrl+C received while writing the MV; aborting run");
                                    return Ok(PollOutcome::Aborted(AbortReason::UserInterrupt));
                                }
                                TickOperation::TimedOut => {
                                    insert_tune_sample_with_timing(
                                        pool,
                                        run_id,
                                        tick_index,
                                        tick,
                                        engine.state(),
                                        sample_quality,
                                        timing,
                                    )
                                    .await?;
                                    timing.observe_tick_work(tick_started.elapsed());
                                    tracing::warn!(
                                        run_id,
                                        tick_index,
                                        op_timeout_secs = effective_timing.op_timeout_secs,
                                        tag = %tags.manipulated_variable,
                                        "[tuning].op_timeout_secs elapsed writing the MV; aborting run"
                                    );
                                    return Ok(PollOutcome::Aborted(AbortReason::OperationTimedOut {
                                        tag: tags.manipulated_variable.clone(),
                                        op_timeout_secs: effective_timing.op_timeout_secs,
                                    }));
                                }
                            }
                        }
                        Action::Complete { .. } => {
                            tracing::info!(
                                run_id,
                                tick_index,
                                "MRFT engine reported completion; recording post-test padding"
                            );
                            completion = Some(action);
                            post_delay_end = Some(
                                now + chrono::Duration::seconds(i64::from(
                                    effective_timing.mrft_delay_secs,
                                )),
                            );
                        }
                    }
                }
                tracing::trace!(run_id, tick_index, pv, "recorded tune sample");
                insert_tune_sample_with_timing(
                    pool,
                    run_id,
                    tick_index,
                    tick,
                    engine.state(),
                    sample_quality,
                    timing,
                )
                .await?;
                timing.observe_tick_work(tick_started.elapsed());
                tick_index += 1;

                if let Some(end) = post_delay_end
                    && now >= end
                {
                    break;
                }
            }
            () = ctrl_c.signalled() => {
                tracing::warn!(run_id, tick_index, "Ctrl+C received; aborting run");
                return Ok(PollOutcome::Aborted(AbortReason::UserInterrupt));
            }
            _ = &mut timeout => {
                tracing::warn!(
                    run_id,
                    tick_index,
                    timeout_secs = effective_timing.timeout_secs,
                    "[tuning].timeout_secs elapsed before completion; aborting run"
                );
                return Ok(PollOutcome::Aborted(AbortReason::Timeout {
                    timeout_secs: effective_timing.timeout_secs,
                }));
            }
        }
    }

    Ok(PollOutcome::Completed(CompletedPoll {
        action: completion.require_invariant("the loop only `break`s after `completion` is set")?,
        state: engine.state(),
        next_tick_index: tick_index,
        tick_time,
    }))
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_polling_loop(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneRequest,
    tags: &LoopTags,
    driver: &dyn Driver,
    engine: &mut MrftEngine,
    time_anchor: RunTimeAnchor,
    ctrl_c: &mut CtrlC,
    guard: &mut MutationGuard,
    allow_uncertain_quality: bool,
    timing: &mut PollTimingAccumulator,
    mv_actuations: &mut Option<MvActuationTracker>,
    config: LoopConfig,
) -> anyhow::Result<PollOutcome> {
    run_polling_loop_with_timing(
        pool,
        run_id,
        args,
        test_effective_timing(args),
        tags,
        driver,
        engine,
        time_anchor,
        ctrl_c,
        guard,
        allow_uncertain_quality,
        timing,
        mv_actuations,
        config,
    )
    .await
}
pub(super) async fn persist_results(
    pool: &SqlitePool,
    run_id: i64,
    action: Action,
    direction: ControllerDirection,
    config: LoopConfig,
    pv_range: PvRange,
    template: &DcsTemplate,
) -> anyhow::Result<()> {
    let Action::Complete {
        peaks,
        troughs,
        switch_times,
        mv_sign_init,
    } = action
    else {
        anyhow::bail!("internal error: persist_results called with a non-Complete action");
    };

    let results = calculate_all_checked(
        &peaks,
        &troughs,
        &switch_times,
        mv_sign_init,
        direction,
        config,
        pv_range,
        template,
        TuningMathCompat::default(),
    );

    for result in results {
        let row = TuneResultRow::from_checked(run_id, result);
        TuneResultRow::insert(pool, &row).await?;
    }

    Ok(())
}
