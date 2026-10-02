#![allow(rustdoc::broken_intra_doc_links)]

use std::time::Duration;

use bhtune_core::{DcsTemplate, LoopTags, Tick};
use bhtune_db::SqlitePool;
use bhtune_db::models::{MvActuationKind, MvActuationStatus, TuneMvActuationRow, TuneRunRow};
use bhtune_driver::Driver;
use chrono::Utc;
use tokio::time::Instant;

use super::request::{DriverKind, TuneRequest};
use crate::cancel::CtrlC;
use crate::timing::PollTimingAccumulator;

use super::RequireInvariant;
use super::actuation::{
    ActuationAuditPolicy, MV_ACTUATION_CONFIRMATION_SECS, MV_ACTUATION_RETRY_INTERVAL,
    MV_RESTORE_HANDOFF_READ_MAX, MvActuationTracker, MvVerificationCallLimit,
    MvVerificationTrigger, actuation_matches, checked_at_for_pending,
    finalize_actuation_best_effort, mv_actuation_uncapped_tolerance,
    record_handoff_observation_best_effort, supersede_pending_actuation_best_effort,
    verification_trigger, verify_pending_mv_actuation_with_timing,
};
use super::config::EffectiveTiming;
#[cfg(test)]
use super::config::test_effective_timing;
use super::outcome::AbortReason;
use super::poll::{
    CompletedPoll, TickOperation, bounded_driver_call, insert_tune_sample_with_timing,
};
use super::prepare::{InitialState, MutationGuard};
use super::quality::{
    check_quality, read_numeric_from_batch, read_numeric_sample, read_poll_batch,
    sample_quality_from_driver, write_raw, write_value,
};

/// One step of a [`RestoreReport`]. `NotNeeded` means the step's precondition wasn't met --
/// the loop wasn't in a state requiring reverting that aspect, or the corresponding mutation
/// was never attempted per [`MutationGuard`] -- not that it failed.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) enum RestoreStepOutcome {
    #[default]
    NotNeeded,
    Succeeded,
    Failed(String),
}
/// The result of one [`restore`] call: each of the (up to) four independent revert steps is
/// attempted regardless of whether an earlier one failed, so a rejected MV write can never
/// prevent the mode from also being put back.
#[derive(Debug, Clone, Default)]
pub(super) struct RestoreReport {
    pub(super) mv: RestoreStepOutcome,
    pub(super) mode: RestoreStepOutcome,
    pub(super) setpoint: RestoreStepOutcome,
    pub(super) mode_attribute: RestoreStepOutcome,
}
impl RestoreReport {
    /// `true` only if every step that was attempted succeeded -- a step that was
    /// [`RestoreStepOutcome::NotNeeded`] doesn't count against this, since nothing needed
    /// doing there in the first place.
    pub(super) fn all_succeeded(&self) -> bool {
        [&self.mv, &self.mode, &self.setpoint, &self.mode_attribute]
            .into_iter()
            .all(|step| !matches!(step, RestoreStepOutcome::Failed(_)))
    }

    /// A human-readable summary of every step that failed, or `None` if none did. Used both
    /// for [`bhtune_db::models::RestoreStatus::Incomplete`]'s persisted `detail` and the
    /// operator-facing warning.
    pub(super) fn failure_summary(&self) -> Option<String> {
        let labelled = [
            ("MV", &self.mv),
            ("mode", &self.mode),
            ("setpoint", &self.setpoint),
            ("mode attribute", &self.mode_attribute),
        ];
        let failures: Vec<String> = labelled
            .into_iter()
            .filter_map(|(label, step)| match step {
                RestoreStepOutcome::Failed(e) => Some(format!("{label}: {e}")),
                _ => None,
            })
            .collect();
        if failures.is_empty() {
            None
        } else {
            Some(failures.join("; "))
        }
    }
}
/// Every applicable step is attempted independently, via a per-step `match` rather than `?`,
/// so one step failing can never prevent the others from being tried. The MV write-back is
/// unconditional and never gated by `guard` at all -- proven safe by the fact that a
/// no-op MV write-back is always harmless (idempotent if the pre-test value is already
/// there, or safely rejected by the DCS if the loop never actually left Auto) -- while the
/// mode/setpoint/mode-attribute reverts are each gated by both their original value-based
/// condition (as before) *and* the matching `guard` flag, so nothing is "restored" that was
/// never actually mutated in the first place.
pub(super) async fn restore_after_mv_with_policy(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    mv: RestoreStepOutcome,
    mode_policy: RestoreModePolicy,
) -> RestoreReport {
    let mode = match mode_policy {
        RestoreModePolicy::ReleaseToInitial => {
            restore_mode_step(driver, tags, template, initial, guard).await
        }
        RestoreModePolicy::KeepManual => RestoreStepOutcome::NotNeeded,
    };
    let setpoint = restore_setpoint_step(driver, tags, template, initial, guard).await;
    let mode_attribute = restore_mode_attribute_step(driver, tags, template, initial, guard).await;

    RestoreReport {
        mv,
        mode,
        setpoint,
        mode_attribute,
    }
}
pub(super) async fn restore_value_step(
    driver: &dyn Driver,
    tag: &str,
    value: f32,
) -> RestoreStepOutcome {
    match write_value(driver, tag, value).await {
        Ok(()) => RestoreStepOutcome::Succeeded,
        Err(e) => RestoreStepOutcome::Failed(e.to_string()),
    }
}
#[cfg(test)]
pub(super) async fn restore(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
) -> RestoreReport {
    let mv = restore_value_step(driver, &tags.manipulated_variable, initial.mv_ini).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    restore_after_mv_with_policy(
        driver,
        tags,
        template,
        initial,
        guard,
        mv,
        RestoreModePolicy::ReleaseToInitial,
    )
    .await
}
pub(super) async fn restore_raw_step(
    driver: &dyn Driver,
    tag: &str,
    value: &str,
) -> RestoreStepOutcome {
    match write_raw(driver, tag, value.to_string()).await {
        Ok(()) => RestoreStepOutcome::Succeeded,
        Err(e) => RestoreStepOutcome::Failed(e.to_string()),
    }
}
pub(super) async fn restore_mode_step(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
) -> RestoreStepOutcome {
    let Some(mode_tag) = &tags.controller_mode else {
        return RestoreStepOutcome::NotNeeded;
    };
    let mode_raw = initial.mode_raw.as_deref().unwrap_or_default();
    if !guard.mode_written || !template.revert_mode || mode_raw == template.mode_manual_value {
        return RestoreStepOutcome::NotNeeded;
    }
    restore_raw_step(driver, mode_tag, mode_raw).await
}
pub(super) async fn restore_setpoint_step(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
) -> RestoreStepOutcome {
    if !guard.mode_written || !template.revert_mode {
        return RestoreStepOutcome::NotNeeded;
    }
    let (Some(sv_tag), Some(sv_ini)) = (&tags.setpoint_variable, initial.setpoint_ini) else {
        return RestoreStepOutcome::NotNeeded;
    };
    tokio::time::sleep(Duration::from_millis(1000)).await;
    restore_value_step(driver, sv_tag, sv_ini).await
}
pub(super) async fn restore_mode_attribute_step(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
) -> RestoreStepOutcome {
    let Some(attr_tag) = &tags.mode_attribute else {
        return RestoreStepOutcome::NotNeeded;
    };
    let attr_raw = initial.mode_attribute_raw.as_deref().unwrap_or_default();
    let program_value = template
        .mode_attribute_program_value
        .as_deref()
        .unwrap_or_default();
    if !guard.mode_attribute_written || attr_raw == program_value {
        return RestoreStepOutcome::NotNeeded;
    }
    restore_raw_step(driver, attr_tag, attr_raw).await
}
/// The outcome of [`attempt_restore_with_actuation`] -- whether the restore was confirmed to run
/// every applicable step to completion, or was abandoned/only partially successful because a
/// second Ctrl+C arrived, `[tuning].restore_timeout_secs` elapsed, or one or more individual restore
/// steps themselves failed.
pub(super) enum RestoreAttempt {
    /// The restore ran to completion and [`RestoreReport::all_succeeded`] was `true`. A
    /// per-step failure is not a separate `Err` case -- it is folded into
    /// [`RestoreAttempt::Incomplete`]
    /// below via the report's own failure summary.
    Confirmed,
    /// The restore could not be confirmed: a second Ctrl+C arrived, `[tuning].restore_timeout_secs`
    /// elapsed, or one or more restore steps failed. `reason` is a
    /// human-readable description of which, for composing into the final
    /// [`RunOutcome::RestoreIncomplete`] message and the stderr warning already printed by
    /// [`warn_restore_incomplete`] before this variant is returned.
    Incomplete { reason: String },
}
pub(super) enum RestoreMvOutcome {
    Continue(RestoreStepOutcome),
    Interrupted(String),
}
pub(super) fn restore_mv_outcome_or_failed(
    result: anyhow::Result<RestoreMvOutcome>,
) -> RestoreMvOutcome {
    match result {
        Ok(outcome) => outcome,
        Err(error) => RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(error.to_string())),
    }
}
pub(super) enum RestoreHandoffOutcome {
    Confirmed,
    Rewrite,
    Interrupted(String),
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn try_confirm_final_snapback_handoff_with_timing(
    pool: &SqlitePool,
    _args: &TuneRequest,
    effective_timing: EffectiveTiming,
    driver: &dyn Driver,
    tag: &str,
    initial_mv: f32,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    tracker: &mut MvActuationTracker,
    restore_deadline: Instant,
) -> anyhow::Result<Option<RestoreHandoffOutcome>> {
    let is_final_snapback = tracker.pending.as_ref().is_some_and(|pending| {
        pending.kind == MvActuationKind::Relay && pending.target == initial_mv
    });
    if !is_final_snapback {
        return Ok(None);
    }

    let pending = tracker
        .pending
        .take()
        .require_invariant("the final-snapback predicate required a pending actuation")?;
    let now = Instant::now();
    let reserved_restore_window = Duration::from_secs(MV_ACTUATION_CONFIRMATION_SECS);
    let latest_handoff_finish = restore_deadline
        .checked_sub(reserved_restore_window)
        .unwrap_or(now);
    if now >= latest_handoff_finish {
        finalize_actuation_best_effort(
            pool,
            &pending,
            MvActuationStatus::Superseded,
            "the authoritative restore skipped the final-snapback handoff read to preserve its full MV confirmation budget",
        )
        .await;
        return Ok(Some(RestoreHandoffOutcome::Rewrite));
    }
    let handoff_deadline = (now + MV_RESTORE_HANDOFF_READ_MAX).min(latest_handoff_finish);
    let read = tokio::time::timeout_at(
        handoff_deadline,
        bounded_driver_call(
            effective_timing.op_timeout_secs,
            ctrl_c,
            read_numeric_sample(driver, tag),
        ),
    )
    .await;
    let checked_instant = Instant::now();
    let checked_at = checked_at_for_pending(&pending, checked_instant)?;

    let operation = match read {
        Err(_) => {
            finalize_actuation_best_effort(
                pool,
                &pending,
                MvActuationStatus::Superseded,
                "the authoritative restore superseded the final MRFT snapback when its tightly bounded handoff read did not finish promptly",
            )
            .await;
            return Ok(Some(RestoreHandoffOutcome::Rewrite));
        }
        Ok(Ok(operation)) => operation,
        Ok(Err(error)) => {
            let detail = format!(
                "the authoritative restore superseded the final MRFT snapback after its handoff read failed: {error}"
            );
            record_handoff_observation_best_effort(pool, &pending, checked_at, None, None, &detail)
                .await;
            tracing::warn!(error = %error, "final MRFT snapback handoff read failed; issuing authoritative restore write");
            return Ok(Some(RestoreHandoffOutcome::Rewrite));
        }
    };

    let (readback, quality) = match operation {
        TickOperation::Completed(value) => value,
        TickOperation::Cancelled => {
            tracker.pending = Some(pending);
            return Ok(Some(RestoreHandoffOutcome::Interrupted(
                "a second Ctrl+C was received while confirming the final MRFT snapback".to_string(),
            )));
        }
        TickOperation::TimedOut => {
            let detail = format!(
                "the authoritative restore superseded the final MRFT snapback after its handoff read exceeded the {}s operation timeout",
                effective_timing.op_timeout_secs
            );
            record_handoff_observation_best_effort(pool, &pending, checked_at, None, None, &detail)
                .await;
            return Ok(Some(RestoreHandoffOutcome::Rewrite));
        }
    };
    let sample_quality = sample_quality_from_driver(quality);
    if check_quality(tag, quality, allow_uncertain_quality).is_err() {
        let detail = format!(
            "the authoritative restore superseded the final MRFT snapback after its handoff read reported OPC quality {quality:?}"
        );
        record_handoff_observation_best_effort(
            pool,
            &pending,
            checked_at,
            Some(readback),
            Some(sample_quality),
            &detail,
        )
        .await;
        return Ok(Some(RestoreHandoffOutcome::Rewrite));
    }

    if actuation_matches(pending.target, readback, pending.tolerance) {
        record_handoff_observation_best_effort(
            pool,
            &pending,
            checked_at,
            Some(readback),
            Some(sample_quality),
            "the authoritative restore adopted and confirmed the final MRFT snapback; no duplicate MV write was issued",
        )
        .await;
        tracker.confirmed_mv = Some(initial_mv);
        return Ok(Some(RestoreHandoffOutcome::Confirmed));
    }

    record_handoff_observation_best_effort(
        pool,
        &pending,
        checked_at,
        Some(readback),
        Some(sample_quality),
        "the authoritative restore superseded an unconfirmed final MRFT snapback and issued a replacement MV write",
    )
    .await;
    Ok(Some(RestoreHandoffOutcome::Rewrite))
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn restore_mv_with_verification_with_timing(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneRequest,
    effective_timing: EffectiveTiming,
    driver: &dyn Driver,
    tag: &str,
    initial_mv: f32,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    tracker: &mut Option<MvActuationTracker>,
    restore_deadline: &mut Instant,
) -> anyhow::Result<RestoreMvOutcome> {
    let Some(tracker) = tracker.as_mut() else {
        let write = tokio::time::timeout_at(
            *restore_deadline,
            bounded_driver_call(
                effective_timing.op_timeout_secs,
                ctrl_c,
                write_value(driver, tag, initial_mv),
            ),
        )
        .await;
        return Ok(match write {
            Err(_) => RestoreMvOutcome::Interrupted(format!(
                "the restore did not complete within the {}s [tuning].restore_timeout_secs limit",
                effective_timing.restore_timeout_secs
            )),
            Ok(Err(error)) => {
                RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(error.to_string()))
            }
            Ok(Ok(operation)) => RestoreMvOutcome::Continue(match operation {
                TickOperation::Completed(()) => RestoreStepOutcome::Succeeded,
                TickOperation::Cancelled => {
                    return Ok(RestoreMvOutcome::Interrupted(
                        "a second Ctrl+C was received while restoring the MV".to_string(),
                    ));
                }
                TickOperation::TimedOut => RestoreStepOutcome::Failed(format!(
                    "MV restore write did not complete within {}s",
                    effective_timing.op_timeout_secs
                )),
            }),
        });
    };

    if tracker.pending.is_none() && tracker.confirmed_mv == Some(initial_mv) {
        return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded));
    }

    match try_confirm_final_snapback_handoff_with_timing(
        pool,
        args,
        effective_timing,
        driver,
        tag,
        initial_mv,
        allow_uncertain_quality,
        ctrl_c,
        tracker,
        *restore_deadline,
    )
    .await?
    {
        Some(RestoreHandoffOutcome::Confirmed) => {
            return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded));
        }
        Some(RestoreHandoffOutcome::Interrupted(reason)) => {
            return Ok(RestoreMvOutcome::Interrupted(reason));
        }
        Some(RestoreHandoffOutcome::Rewrite) | None => {}
    }

    let tolerance =
        mv_actuation_uncapped_tolerance(initial_mv, tracker.previous_commanded_mv, tracker.mv_span);
    let write = tokio::time::timeout_at(
        *restore_deadline,
        bounded_driver_call(
            effective_timing.op_timeout_secs,
            ctrl_c,
            write_value(driver, tag, initial_mv),
        ),
    )
    .await;
    match write {
        Err(_) => {
            return Ok(RestoreMvOutcome::Interrupted(format!(
                "the restore did not complete within the {}s [tuning].restore_timeout_secs limit",
                effective_timing.restore_timeout_secs
            )));
        }
        Ok(Ok(TickOperation::Completed(()))) => {}
        Ok(Ok(TickOperation::Cancelled)) => {
            return Ok(RestoreMvOutcome::Interrupted(
                "a second Ctrl+C was received while restoring the MV".to_string(),
            ));
        }
        Ok(Ok(TickOperation::TimedOut)) => {
            return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(
                format!(
                    "MV restore write did not complete within {}s",
                    effective_timing.op_timeout_secs
                ),
            )));
        }
        Ok(Err(error)) => {
            return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(
                error.to_string(),
            )));
        }
    }

    if tracker.pending.is_some() {
        supersede_pending_actuation_best_effort(
            pool,
            tracker,
            "the authoritative restore write replaced this pending relay command",
        )
        .await;
    }
    let accepted_instant = Instant::now();
    let accepted_at = Utc::now();
    *restore_deadline = (*restore_deadline)
        .max(accepted_instant + Duration::from_secs(MV_ACTUATION_CONFIRMATION_SECS));
    tracker
        .record_restore_accepted_best_effort(
            pool,
            run_id,
            initial_mv,
            accepted_at,
            accepted_instant,
            tolerance,
        )
        .await;

    loop {
        let trigger = tracker
            .pending
            .as_ref()
            .and_then(|pending| verification_trigger(pending, Instant::now()))
            .unwrap_or(MvVerificationTrigger::Scheduled);
        let verification = verify_pending_mv_actuation_with_timing(
            pool,
            args,
            effective_timing,
            tag,
            driver,
            ctrl_c,
            allow_uncertain_quality,
            tracker,
            trigger,
            MvVerificationCallLimit::Restore(*restore_deadline),
            ActuationAuditPolicy::BestEffort,
            None,
        )
        .await;
        match verification {
            Ok(None) if tracker.pending.is_none() => {
                return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded));
            }
            Ok(None) => {
                let pending = tracker
                    .pending
                    .as_ref()
                    .require_invariant("pending state was checked above")?;
                let remaining_confirmation =
                    pending.deadline.saturating_duration_since(Instant::now());
                let remaining_restore =
                    (*restore_deadline).saturating_duration_since(Instant::now());
                tokio::time::sleep(
                    MV_ACTUATION_RETRY_INTERVAL
                        .min(remaining_confirmation)
                        .min(remaining_restore),
                )
                .await;
            }
            Ok(Some(AbortReason::UserInterrupt)) => {
                return Ok(RestoreMvOutcome::Interrupted(
                    "a second Ctrl+C was received while confirming the restored MV".to_string(),
                ));
            }
            Ok(Some(_)) if Instant::now() >= *restore_deadline => {
                return Ok(RestoreMvOutcome::Interrupted(format!(
                    "the restore did not complete within the {}s [tuning].restore_timeout_secs limit",
                    effective_timing.restore_timeout_secs
                )));
            }
            Ok(Some(reason)) => {
                return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(
                    format!("MV restore could not be confirmed: {reason:?}"),
                )));
            }
            Err(error) => {
                return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(
                    format!("MV restore verification failed: {error}"),
                )));
            }
        }
    }
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn try_confirm_final_snapback_handoff(
    pool: &SqlitePool,
    args: &TuneRequest,
    driver: &dyn Driver,
    tag: &str,
    initial_mv: f32,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    tracker: &mut MvActuationTracker,
    restore_deadline: Instant,
) -> anyhow::Result<Option<RestoreHandoffOutcome>> {
    try_confirm_final_snapback_handoff_with_timing(
        pool,
        args,
        test_effective_timing(args),
        driver,
        tag,
        initial_mv,
        allow_uncertain_quality,
        ctrl_c,
        tracker,
        restore_deadline,
    )
    .await
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn restore_mv_with_verification(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneRequest,
    driver: &dyn Driver,
    tag: &str,
    initial_mv: f32,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    tracker: &mut Option<MvActuationTracker>,
    restore_deadline: Instant,
) -> anyhow::Result<RestoreMvOutcome> {
    let mut restore_deadline = restore_deadline;
    restore_mv_with_verification_with_timing(
        pool,
        run_id,
        args,
        test_effective_timing(args),
        driver,
        tag,
        initial_mv,
        allow_uncertain_quality,
        ctrl_c,
        tracker,
        &mut restore_deadline,
    )
    .await
}
/// Restores the loop, bounded by `restore_timeout_secs` and a second Ctrl+C, so a restore
/// that itself hangs (the same class of stalled-driver-call risk `bounded_driver_call`
/// guards the polling loop against) can never block the process indefinitely. Unlike
/// [`bounded_driver_call`], this takes `restore`'s exact parameters directly rather than a
/// generic `impl Future` -- there is only one real call shape (one `restore(...)` call per
/// run), so a generic signature would add no value. Infallible: [`restore`] itself no longer
/// returns a `Result` (every step is now attempted independently and reported via
/// [`RestoreReport`] instead of short-circuiting), so this function's only remaining
/// "failure" shapes are the two abandonment cases already covered by
/// [`RestoreAttempt::Incomplete`], plus a completed-but-not-fully-successful report, which
/// maps to that same variant. On [`RestoreAttempt::Incomplete`], calls
/// [`warn_restore_incomplete`] before returning, so the operator-facing warning is printed
/// exactly once, at the one place that decides a restore could not be confirmed -- callers
/// only need to fold the returned `reason` into their own context (e.g. the original
/// [`AbortReason`], if any) for the final [`RunOutcome`].
#[allow(clippy::too_many_arguments)]
pub(super) async fn attempt_restore_with_actuation_with_timing(
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
    let mut restore_deadline =
        Instant::now() + Duration::from_secs(effective_timing.restore_timeout_secs);
    let mv = match restore_mv_outcome_or_failed(
        restore_mv_with_verification_with_timing(
            pool,
            run_id,
            args,
            effective_timing,
            driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            allow_uncertain_quality,
            ctrl_c,
            mv_actuations,
            &mut restore_deadline,
        )
        .await,
    ) {
        RestoreMvOutcome::Continue(outcome) => outcome,
        RestoreMvOutcome::Interrupted(reason) => {
            let _ = warn_restore_incomplete(tags, initial, &reason);
            return RestoreAttempt::Incomplete { reason };
        }
    };

    let mode_policy = if should_settle_before_auto_release(args, tags, template, initial, guard)
        && !matches!(mv, RestoreStepOutcome::Succeeded)
    {
        RestoreModePolicy::KeepManual
    } else {
        RestoreModePolicy::ReleaseToInitial
    };
    if should_settle_before_auto_release(args, tags, template, initial, guard)
        && matches!(mv, RestoreStepOutcome::Succeeded)
        && let Some(duration) = settling_duration(measured_oscillation_period_ms)
    {
        let Some(completion) = completion else {
            tracing::warn!(
                run_id,
                "skipping live Auto-release settling because completion state was unavailable"
            );
            return restore_after_mv_with_deadline(
                driver,
                tags,
                template,
                initial,
                guard,
                mv,
                RestoreModePolicy::ReleaseToInitial,
                ctrl_c,
                restore_deadline,
                effective_timing,
            )
            .await;
        };
        let Some(timing) = timing else {
            tracing::warn!(
                run_id,
                "skipping live Auto-release settling because timing state was unavailable"
            );
            return restore_after_mv_with_deadline(
                driver,
                tags,
                template,
                initial,
                guard,
                mv,
                RestoreModePolicy::ReleaseToInitial,
                ctrl_c,
                restore_deadline,
                effective_timing,
            )
            .await;
        };

        let duration_ms = duration.as_millis();
        tracing::info!(
            run_id,
            duration_ms,
            measured_oscillation_period_ms = ?measured_oscillation_period_ms,
            "holding the loop in Manual for post-MRFT Auto-release settling"
        );
        if let Err(error) = run_auto_release_settling(
            pool,
            run_id,
            effective_timing,
            tags,
            driver,
            ctrl_c,
            allow_uncertain_quality,
            completion,
            timing,
            duration,
            restore_deadline,
        )
        .await
        {
            let reason = format!("post-MRFT Auto-release settling failed: {error}");
            let restore_result = restore_after_mv_with_deadline(
                driver,
                tags,
                template,
                initial,
                guard,
                mv,
                RestoreModePolicy::KeepManual,
                ctrl_c,
                restore_deadline,
                effective_timing,
            )
            .await;
            return match restore_result {
                RestoreAttempt::Confirmed => {
                    let _ = warn_restore_incomplete(tags, initial, &reason);
                    RestoreAttempt::Incomplete { reason }
                }
                RestoreAttempt::Incomplete {
                    reason: restore_reason,
                } => RestoreAttempt::Incomplete {
                    reason: format!("{reason}; {restore_reason}"),
                },
            };
        }
    }

    restore_after_mv_with_deadline(
        driver,
        tags,
        template,
        initial,
        guard,
        mv,
        mode_policy,
        ctrl_c,
        restore_deadline,
        effective_timing,
    )
    .await
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RestoreModePolicy {
    ReleaseToInitial,
    KeepManual,
}
pub(super) fn should_settle_before_auto_release(
    args: &TuneRequest,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
) -> bool {
    matches!(args.driver, DriverKind::Opcda)
        && tags.controller_mode.is_some()
        && template.revert_mode
        && guard.mode_written
        && initial.mode_raw.as_deref() == Some(template.mode_auto_value.as_str())
}
pub(super) fn settling_duration(measured_oscillation_period_ms: Option<f64>) -> Option<Duration> {
    let period_ms =
        measured_oscillation_period_ms.filter(|period| period.is_finite() && *period > 0.0)?;
    let seconds = period_ms / 3_000.0;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    Duration::try_from_secs_f64(seconds).ok()
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_auto_release_settling(
    pool: &SqlitePool,
    run_id: i64,
    effective_timing: EffectiveTiming,
    tags: &LoopTags,
    driver: &dyn Driver,
    ctrl_c: &mut CtrlC,
    allow_uncertain_quality: bool,
    completion: &mut CompletedPoll,
    timing: &mut PollTimingAccumulator,
    duration: Duration,
    restore_deadline: Instant,
) -> anyhow::Result<()> {
    let settling_deadline = Instant::now()
        .checked_add(duration)
        .ok_or_else(|| anyhow::anyhow!("post-MRFT settling deadline exceeded the clock range"))?;
    let poll_interval = Duration::from_millis(effective_timing.poll_interval_ms.max(1));
    let mut next_poll_at = Instant::now();
    let mut tick_index = completion.next_tick_index;

    loop {
        let now = Instant::now();
        if now >= settling_deadline {
            break;
        }
        tokio::select! {
            biased;
            () = ctrl_c.signalled() => {
                anyhow::bail!("Ctrl+C was received during post-MRFT Auto-release settling");
            }
            () = tokio::time::sleep_until(restore_deadline) => {
                anyhow::bail!(
                    "the post-MRFT Auto-release settling interval exceeded the {}s [tuning].restore_timeout_secs limit",
                    effective_timing.restore_timeout_secs
                );
            }
            () = tokio::time::sleep_until(next_poll_at) => {}
        }

        let tick_started = Instant::now();
        if tick_started >= settling_deadline {
            break;
        }
        next_poll_at = tick_started + poll_interval;

        let pv_read_started = Instant::now();
        let (pv, quality) = match bounded_driver_call(
            effective_timing.op_timeout_secs,
            ctrl_c,
            read_poll_batch(driver, &tags.process_variable, None),
        )
        .await?
        {
            TickOperation::Completed(values) => {
                timing.observe_pv_read(pv_read_started.elapsed());
                read_numeric_from_batch(&values, &tags.process_variable)?
            }
            TickOperation::Cancelled => {
                anyhow::bail!("Ctrl+C was received while reading the settling PV");
            }
            TickOperation::TimedOut => {
                anyhow::bail!(
                    "[tuning].op_timeout_secs elapsed reading the settling PV tag '{}'",
                    tags.process_variable
                );
            }
        };

        let now = completion.tick_time.next_timestamp()?;
        timing.observe(now)?;
        let tick = Tick { time: now, pv };
        let sample_quality = sample_quality_from_driver(quality);
        if let Err(error) = check_quality(&tags.process_variable, quality, allow_uncertain_quality)
        {
            insert_tune_sample_with_timing(
                pool,
                run_id,
                tick_index,
                tick,
                completion.state,
                sample_quality,
                timing,
            )
            .await?;
            timing.observe_tick_work(tick_started.elapsed());
            anyhow::bail!("{error}");
        }

        insert_tune_sample_with_timing(
            pool,
            run_id,
            tick_index,
            tick,
            completion.state,
            sample_quality,
            timing,
        )
        .await?;
        timing.observe_tick_work(tick_started.elapsed());
        tick_index = tick_index
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("settling sample tick index exceeded i64 range"))?;
    }

    completion.next_tick_index = tick_index;
    Ok(())
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn restore_after_mv_with_deadline(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    mv: RestoreStepOutcome,
    mode_policy: RestoreModePolicy,
    ctrl_c: &mut CtrlC,
    restore_deadline: Instant,
    effective_timing: EffectiveTiming,
) -> RestoreAttempt {
    tokio::select! {
        report = restore_after_mv_with_policy(driver, tags, template, initial, guard, mv, mode_policy) => {
            if report.all_succeeded() {
                RestoreAttempt::Confirmed
            } else {
                let reason = report
                    .failure_summary()
                    .unwrap_or_else(|| "one or more restore steps failed".to_string());
                let _ = warn_restore_incomplete(tags, initial, &reason);
                RestoreAttempt::Incomplete { reason }
            }
        }
        () = ctrl_c.signalled() => {
            let reason = "a second Ctrl+C was received while restoring the loop".to_string();
            let _ = warn_restore_incomplete(tags, initial, &reason);
            RestoreAttempt::Incomplete { reason }
        }
        () = tokio::time::sleep_until(restore_deadline) => {
            let reason = format!(
                "the restore did not complete within the {}s [tuning].restore_timeout_secs limit",
                effective_timing.restore_timeout_secs
            );
            let _ = warn_restore_incomplete(tags, initial, &reason);
            RestoreAttempt::Incomplete { reason }
        }
    }
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn attempt_restore_with_actuation(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneRequest,
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
) -> RestoreAttempt {
    attempt_restore_with_actuation_with_timing(
        pool,
        run_id,
        args,
        test_effective_timing(args),
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
/// Prints a loud, operator-facing warning (to stderr, so it survives `--output json` and any
/// stdout redirection) naming the MV tag and the pre-test value it may not have been
/// restored to, plus a matching `tracing::error!` for anyone mining logs rather than watching
/// the terminal. The loop's mode may also not have been reverted -- see `restore`'s own
/// mode/setpoint/mode-attribute steps -- but the MV is called out specifically since it is
/// the one value every template has and the one most directly consequential if left at a
/// relay-test extreme.
pub(super) fn restore_incomplete_warning_message(
    tags: &LoopTags,
    initial: &InitialState,
    reason: &str,
) -> String {
    format!(
        "WARNING: could not confirm the loop was fully restored ({reason}). Tag '{}' may still be at its last relay-test value instead of its pre-test value {}. Check it -- and the loop's mode -- by hand.",
        tags.manipulated_variable, initial.mv_ini
    )
}
pub(super) fn warn_restore_incomplete(
    tags: &LoopTags,
    initial: &InitialState,
    reason: &str,
) -> String {
    let message = restore_incomplete_warning_message(tags, initial, reason);
    eprintln!("{message}");
    tracing::error!(
        mv_tag = %tags.manipulated_variable,
        mv_ini = initial.mv_ini,
        reason,
        "loop restore could not be confirmed"
    );
    message
}
/// Best-effort records a restore attempt's outcome on the run -- logs and swallows its own
/// failure rather than propagating, since failing to *record* that a restore was attempted
/// must never itself change what error (if any) a run reports.
pub(super) async fn record_restore_status_best_effort(
    pool: &SqlitePool,
    run_id: i64,
    attempt: &RestoreAttempt,
) {
    let (status, detail) = match attempt {
        RestoreAttempt::Confirmed => (bhtune_db::models::RestoreStatus::Confirmed, None),
        RestoreAttempt::Incomplete { reason } => (
            bhtune_db::models::RestoreStatus::Incomplete,
            Some(reason.as_str()),
        ),
    };
    if let Err(e) = TuneRunRow::record_restore_status(pool, run_id, status, detail).await {
        tracing::error!(run_id, error = %e, "failed to record restore status");
    }
}
/// Attempts a best-effort restore, records its outcome, then returns `err` **unchanged** --
/// the single choke point every early-return error path in `execute` funnels through, so a
/// partial mutation is never left un-restored just because the step that failed came before
/// `attempt_restore` was reached -- including failures during `transition_to_manual`,
/// `record_initial_readings`/`persist_results`, or `run_polling_loop` itself. Always returns
/// the *original* `err`:
/// neither an incomplete restore nor a failure recording its status should ever mask the
/// real reason the run is failing.
#[allow(clippy::too_many_arguments)]
pub(super) async fn restore_best_effort_then_propagate_with_timing(
    pool: &SqlitePool,
    run_id: i64,
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    args: &TuneRequest,
    effective_timing: EffectiveTiming,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
    err: anyhow::Error,
) -> anyhow::Error {
    let attempt = attempt_restore_with_actuation_with_timing(
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
    .await;
    record_restore_status_best_effort(pool, run_id, &attempt).await;
    if let Err(finalize_error) = TuneMvActuationRow::finalize_pending_for_run(
        pool,
        run_id,
        MvActuationStatus::Unverified,
        Some("the run failed before MV confirmation completed"),
    )
    .await
    {
        tracing::error!(
            run_id,
            error = %finalize_error,
            "failed to finalize pending MV actuation rows"
        );
    }
    err
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn restore_best_effort_then_propagate(
    pool: &SqlitePool,
    run_id: i64,
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    args: &TuneRequest,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
    err: anyhow::Error,
) -> anyhow::Error {
    restore_best_effort_then_propagate_with_timing(
        pool,
        run_id,
        driver,
        tags,
        template,
        initial,
        guard,
        args,
        test_effective_timing(args),
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
        err,
    )
    .await
}
