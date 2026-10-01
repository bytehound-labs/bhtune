#![allow(rustdoc::broken_intra_doc_links)]

use std::collections::HashMap;
use std::time::Duration;

use bhtune_core::mrft::clamp_relay_amplitude;
use bhtune_core::{LoopConfig, MrftCompat};
use bhtune_db::SqlitePool;
use bhtune_db::models::{
    MvActuationKind, MvActuationStatus, NewTuneMvActuation, SampleQuality, TuneMvActuationRow,
};
use bhtune_driver::{Driver, TagValue};
use chrono::{DateTime, Utc};
use tokio::time::Instant;

use super::request::{DriverKind, TuneRequest};
use crate::cancel::CtrlC;
use crate::timing::PollTimingAccumulator;

use super::RequireInvariant;
use super::config::EffectiveTiming;
#[cfg(test)]
use super::config::test_effective_timing;
use super::outcome::AbortReason;
use super::poll::{TickOperation, bounded_driver_call};
use super::prepare::InitialState;
use super::quality::{
    check_quality, read_numeric_from_batch, read_numeric_sample, sample_quality_from_driver,
};

/// Maximum interval from an accepted OPC DA MV write to its mandatory confirmation check.
///
/// Public so HTTP and other non-clap adapters can expose the same validation policy without
/// copying the safety-critical literal.
pub const MV_ACTUATION_CONFIRMATION_SECS: u64 = 4;
pub(super) const MV_ACTUATION_RETRY_INTERVAL: Duration = Duration::from_millis(100);
pub(super) const MV_ACTUATION_DEADLINE_READ_MAX: Duration = Duration::from_secs(1);
pub(super) const MV_ACTUATION_FALLBACK_HEADROOM: Duration = Duration::from_secs(1);
pub(super) const MV_RESTORE_HANDOFF_READ_MAX: Duration = Duration::from_secs(1);
pub(super) const MV_SPAN_TOLERANCE_FRACTION: f32 = 0.001;
pub(super) const RELAY_STEP_TOLERANCE_FRACTION: f32 = 0.25;
pub(super) const MIN_RELAY_STEP: f32 = 0.01;
#[derive(Debug)]
pub(super) struct PendingMvActuation {
    pub(super) id: Option<i64>,
    pub(super) kind: MvActuationKind,
    pub(super) target: f32,
    pub(super) tolerance: f32,
    pub(super) switch_tick: DateTime<Utc>,
    pub(super) switch_instant: Instant,
    pub(super) accepted_instant: Instant,
    pub(super) first_check_at: Instant,
    pub(super) deadline: Instant,
    pub(super) last_readback: Option<f32>,
}
#[derive(Debug)]
pub(super) struct MvActuationTracker {
    pub(super) next_sequence: i64,
    pub(super) previous_commanded_mv: f32,
    pub(super) confirmed_mv: Option<f32>,
    pub(super) pending: Option<PendingMvActuation>,
    pub(super) mv_span: f32,
}
impl MvActuationTracker {
    pub(super) fn for_run(args: &TuneRequest, initial: &InitialState) -> Option<Self> {
        (args.driver == DriverKind::Opcda).then_some(Self {
            next_sequence: 0,
            previous_commanded_mv: initial.mv_ini,
            confirmed_mv: None,
            pending: None,
            mv_span: initial.mv_range_high - initial.mv_range_low,
        })
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn record_accepted(
        &mut self,
        pool: &SqlitePool,
        run_id: i64,
        kind: MvActuationKind,
        target: f32,
        first_check_at: Instant,
        accepted_at: DateTime<Utc>,
        accepted_instant: Instant,
        tolerance: f32,
    ) -> anyhow::Result<()> {
        self.record_accepted_at_switch(
            pool,
            run_id,
            kind,
            target,
            accepted_at,
            accepted_instant,
            first_check_at,
            accepted_at,
            accepted_instant,
            tolerance,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn record_accepted_at_switch(
        &mut self,
        pool: &SqlitePool,
        run_id: i64,
        kind: MvActuationKind,
        target: f32,
        switch_tick: DateTime<Utc>,
        switch_instant: Instant,
        first_check_at: Instant,
        accepted_at: DateTime<Utc>,
        accepted_instant: Instant,
        tolerance: f32,
    ) -> anyhow::Result<()> {
        let deadline = accepted_instant + Duration::from_secs(MV_ACTUATION_CONFIRMATION_SECS);
        let confirmation_due_at =
            accepted_at + chrono::Duration::seconds(MV_ACTUATION_CONFIRMATION_SECS as i64);
        let previous_commanded_mv = Some(self.previous_commanded_mv);
        let row = TuneMvActuationRow::insert_pending(
            pool,
            run_id,
            NewTuneMvActuation {
                sequence: self.next_sequence,
                kind,
                commanded_at: accepted_at,
                target_mv: target,
                previous_commanded_mv,
                tolerance,
                confirmation_due_at,
            },
        )
        .await?;
        self.accept_pending(PendingMvActuation {
            id: Some(row.id),
            kind,
            target,
            tolerance,
            switch_tick,
            switch_instant,
            accepted_instant,
            first_check_at: first_check_at.min(deadline),
            deadline,
            last_readback: None,
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn record_restore_accepted_best_effort(
        &mut self,
        pool: &SqlitePool,
        run_id: i64,
        target: f32,
        accepted_at: DateTime<Utc>,
        accepted_instant: Instant,
        tolerance: f32,
    ) {
        let deadline = accepted_instant + Duration::from_secs(MV_ACTUATION_CONFIRMATION_SECS);
        let confirmation_due_at =
            accepted_at + chrono::Duration::seconds(MV_ACTUATION_CONFIRMATION_SECS as i64);
        let pending = PendingMvActuation {
            id: None,
            kind: MvActuationKind::Restore,
            target,
            tolerance,
            switch_tick: accepted_at,
            switch_instant: accepted_instant,
            accepted_instant,
            first_check_at: accepted_instant,
            deadline,
            last_readback: None,
        };
        let row = TuneMvActuationRow::insert_pending(
            pool,
            run_id,
            NewTuneMvActuation {
                sequence: self.next_sequence,
                kind: MvActuationKind::Restore,
                commanded_at: accepted_at,
                target_mv: target,
                previous_commanded_mv: Some(self.previous_commanded_mv),
                tolerance,
                confirmation_due_at,
            },
        )
        .await;
        let mut pending = pending;
        match row {
            Ok(row) => pending.id = Some(row.id),
            Err(error) => {
                tracing::error!(
                    run_id,
                    error = %error,
                    "failed to record accepted restore MV command; continuing physical restore"
                );
            }
        }
        self.accept_pending(pending);
    }

    pub(super) fn accept_pending(&mut self, pending: PendingMvActuation) {
        self.next_sequence += 1;
        self.previous_commanded_mv = pending.target;
        self.confirmed_mv = None;
        self.pending = Some(pending);
    }

    pub(super) fn next_verification_wakeup(&self) -> Option<Instant> {
        self.pending.as_ref().map(|pending| {
            if pending.last_readback.is_some() {
                pending.deadline
            } else {
                pending
                    .first_check_at
                    .max(pending.deadline - MV_ACTUATION_FALLBACK_HEADROOM)
            }
        })
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn record_relay_actuation(
    tracker: &mut MvActuationTracker,
    pool: &SqlitePool,
    run_id: i64,
    target: f32,
    switch_tick: DateTime<Utc>,
    switch_instant: Instant,
    first_check_at: Instant,
    elapsed_since_observation: Duration,
    accepted_instant: Instant,
    tolerance: f32,
) -> anyhow::Result<()> {
    let accepted_at = utc_after_elapsed(switch_tick, elapsed_since_observation)?;
    tracker
        .record_accepted_at_switch(
            pool,
            run_id,
            MvActuationKind::Relay,
            target,
            switch_tick,
            switch_instant,
            first_check_at,
            accepted_at,
            accepted_instant,
            tolerance,
        )
        .await
}
pub(super) fn f32_precision_floor(target: f32, previous: f32) -> f32 {
    4.0 * f32::EPSILON * target.abs().max(previous.abs()).max(1.0)
}
pub(super) fn mv_actuation_tolerance(
    kind: MvActuationKind,
    target: f32,
    previous: f32,
    mv_span: f32,
) -> anyhow::Result<f32> {
    let uncapped = mv_actuation_uncapped_tolerance(target, previous, mv_span);
    if kind == MvActuationKind::Restore {
        return Ok(uncapped);
    }

    let step = (target - previous).abs();
    let relay_cap = step * RELAY_STEP_TOLERANCE_FRACTION;
    if !step.is_finite()
        || step < MIN_RELAY_STEP
        || relay_cap <= f32_precision_floor(target, previous)
    {
        let minimum_step = MIN_RELAY_STEP
            .max(f32_precision_floor(target, previous) / RELAY_STEP_TOLERANCE_FRACTION);
        anyhow::bail!(
            "the effective relay step {step} is too small to verify safely (minimum {minimum_step})"
        );
    }
    Ok(uncapped.min(relay_cap))
}
pub(super) fn mv_actuation_uncapped_tolerance(target: f32, previous: f32, mv_span: f32) -> f32 {
    let precision_floor = f32_precision_floor(target, previous);
    let span_tolerance = mv_span.abs() * MV_SPAN_TOLERANCE_FRACTION;
    precision_floor + span_tolerance
}
pub(super) fn validate_relay_actuation_step(
    args: &TuneRequest,
    config: LoopConfig,
    initial: &InitialState,
) -> anyhow::Result<()> {
    if args.driver != DriverKind::Opcda {
        return Ok(());
    }
    let relay_step = clamp_relay_amplitude(
        config.relay_amp_percent,
        initial.mv_ini,
        initial.mv_range_low,
        initial.mv_range_high,
        MrftCompat::default(),
    );
    mv_actuation_tolerance(
        MvActuationKind::Relay,
        initial.mv_ini + relay_step,
        initial.mv_ini,
        initial.mv_range_high - initial.mv_range_low,
    )
    .map(|_| ())
}
pub(super) fn actuation_abort_reason(
    tag: &str,
    pending: &PendingMvActuation,
    readback: Option<f32>,
    now: Instant,
) -> AbortReason {
    let elapsed_ms = now
        .saturating_duration_since(pending.accepted_instant)
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    AbortReason::MvActuationUnconfirmed {
        tag: tag.to_string(),
        target: pending.target,
        readback,
        tolerance: pending.tolerance,
        elapsed_ms,
        deadline_secs: MV_ACTUATION_CONFIRMATION_SECS,
    }
}
pub(super) fn actuation_matches(target: f32, readback: f32, tolerance: f32) -> bool {
    (target - readback).abs() <= tolerance
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MvVerificationTrigger {
    Scheduled,
    Deadline,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActuationAuditPolicy {
    Required,
    BestEffort,
}
#[derive(Debug, Clone, Copy)]
pub(super) enum MvVerificationCallLimit {
    None,
    Restore(Instant),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MvVerificationLimitKind {
    Confirmation,
    Deadline,
    Restore,
}
pub(super) async fn record_actuation_observation(
    pool: &SqlitePool,
    pending: &PendingMvActuation,
    checked_at: DateTime<Utc>,
    readback: Option<f32>,
    quality: Option<SampleQuality>,
    policy: ActuationAuditPolicy,
) -> anyhow::Result<Option<i64>> {
    let Some(id) = pending.id else {
        return Ok(None);
    };
    match TuneMvActuationRow::record_observation(pool, id, checked_at, readback, quality).await {
        Ok(row) => Ok(Some(row.attempt_count)),
        Err(error) if policy == ActuationAuditPolicy::BestEffort => {
            tracing::error!(
                actuation_id = id,
                error = %error,
                "failed to record MV verification observation; continuing physical restore"
            );
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn record_final_actuation_observation(
    pool: &SqlitePool,
    pending: &PendingMvActuation,
    checked_at: DateTime<Utc>,
    readback: Option<f32>,
    quality: Option<SampleQuality>,
    status: MvActuationStatus,
    detail: &str,
) -> Option<i64> {
    let id = pending.id?;
    match TuneMvActuationRow::record_final_observation(
        pool,
        id,
        checked_at,
        readback,
        quality,
        status,
        (!detail.is_empty()).then_some(detail),
    )
    .await
    {
        Ok(row) => Some(row.attempt_count),
        Err(error) => {
            tracing::error!(
                actuation_id = id,
                error = %error,
                "failed to record terminal MV verification observation"
            );
            None
        }
    }
}
pub(super) async fn finalize_actuation_best_effort(
    pool: &SqlitePool,
    pending: &PendingMvActuation,
    status: MvActuationStatus,
    detail: &str,
) {
    let Some(id) = pending.id else {
        return;
    };
    if let Err(error) = TuneMvActuationRow::finalize(pool, id, status, Some(detail)).await {
        tracing::error!(
            actuation_id = id,
            error = %error,
            "failed to finalize MV actuation audit row"
        );
    }
}
pub(super) async fn reject_replacement_for_pending_actuation(
    pool: &SqlitePool,
    tag: &str,
    tracker: &mut MvActuationTracker,
) -> anyhow::Result<AbortReason> {
    let pending = tracker
        .pending
        .take()
        .require_invariant("called only when an MV actuation is pending")?;
    let (status, detail) = if pending.last_readback.is_some() {
        (
            MvActuationStatus::Failed,
            "a replacement relay command was requested while the prior MV readback remained outside tolerance",
        )
    } else {
        (
            MvActuationStatus::Unverified,
            "a replacement relay command was requested before an acceptable prior MV readback was available",
        )
    };
    finalize_actuation_best_effort(pool, &pending, status, detail).await;
    Ok(actuation_abort_reason(
        tag,
        &pending,
        pending.last_readback,
        Instant::now(),
    ))
}
pub(super) fn verification_trigger(
    pending: &PendingMvActuation,
    now: Instant,
) -> Option<MvVerificationTrigger> {
    if now >= pending.deadline {
        Some(MvVerificationTrigger::Deadline)
    } else if pending.last_readback.is_none() && now >= pending.first_check_at {
        Some(MvVerificationTrigger::Scheduled)
    } else {
        None
    }
}
pub(super) async fn wait_for_mv_verification(wakeup: Option<Instant>) {
    match wakeup {
        Some(wakeup) => tokio::time::sleep_until(wakeup).await,
        None => std::future::pending::<()>().await,
    }
}
pub(super) fn mv_verification_read_limit(
    trigger: MvVerificationTrigger,
    pending: &PendingMvActuation,
    call_limit: MvVerificationCallLimit,
) -> (Instant, MvVerificationLimitKind) {
    let external = match call_limit {
        MvVerificationCallLimit::None => None,
        MvVerificationCallLimit::Restore(deadline) => {
            Some((deadline, MvVerificationLimitKind::Restore))
        }
    };
    if trigger == MvVerificationTrigger::Deadline {
        let deadline_read_limit = (
            Instant::now() + MV_ACTUATION_DEADLINE_READ_MAX,
            MvVerificationLimitKind::Deadline,
        );
        return match external {
            Some(external) if external.0 < deadline_read_limit.0 => external,
            _ => deadline_read_limit,
        };
    }
    match external {
        Some(external @ (deadline, _)) if deadline < pending.deadline => external,
        _ => (pending.deadline, MvVerificationLimitKind::Confirmation),
    }
}
/// Checks the accepted OPC DA MV command without producing a tune sample. Transport failures
/// remain ordinary failed-run errors, an individual operation timeout remains
/// [`AbortReason::OperationTimedOut`], and rejected quality remains [`AbortReason::PoorQuality`].
/// Only a finite, acceptable-quality mismatch can become
/// [`AbortReason::MvActuationUnconfirmed`]. Successful verification-read duration is optionally
/// recorded when this is called from the polling loop.
#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_pending_mv_actuation_with_timing(
    pool: &SqlitePool,
    _args: &TuneRequest,
    effective_timing: EffectiveTiming,
    tag: &str,
    driver: &dyn Driver,
    ctrl_c: &mut CtrlC,
    allow_uncertain_quality: bool,
    tracker: &mut MvActuationTracker,
    trigger: MvVerificationTrigger,
    call_limit: MvVerificationCallLimit,
    audit_policy: ActuationAuditPolicy,
    mut timing: Option<&mut PollTimingAccumulator>,
) -> anyhow::Result<Option<AbortReason>> {
    let Some(pending) = tracker.pending.as_ref() else {
        return Ok(None);
    };
    let now = Instant::now();
    if trigger == MvVerificationTrigger::Scheduled && now < pending.first_check_at {
        return Ok(None);
    }

    match read_pending_mv_verification_with_timing(
        effective_timing,
        tag,
        driver,
        ctrl_c,
        tracker,
        trigger,
        call_limit,
    )
    .await?
    {
        PendingMvVerificationRead::DeadlineTimedOut {
            pending,
            checked_at,
            checked_instant,
        } => {
            record_final_actuation_observation(
                pool,
                &pending,
                checked_at,
                pending.last_readback,
                None,
                MvActuationStatus::Unverified,
                "the fresh MV read at the confirmation deadline did not finish within its bounded verification window",
            )
            .await;
            Ok(Some(actuation_abort_reason(
                tag,
                &pending,
                pending.last_readback,
                checked_instant,
            )))
        }
        PendingMvVerificationRead::RestoreTimedOut {
            pending,
            checked_at,
            checked_instant,
        } => {
            record_final_actuation_observation(
                pool,
                &pending,
                checked_at,
                pending.last_readback,
                None,
                MvActuationStatus::Unverified,
                "the restore timeout elapsed before MV confirmation completed",
            )
            .await;
            Ok(Some(actuation_abort_reason(
                tag,
                &pending,
                pending.last_readback,
                checked_instant,
            )))
        }
        PendingMvVerificationRead::Ready {
            operation,
            checked_at,
            checked_instant,
            read_duration,
        } => match resolve_pending_mv_read(
            pool,
            effective_timing,
            tag,
            tracker,
            operation,
            checked_at,
            checked_instant,
            allow_uncertain_quality,
        )
        .await?
        {
            PendingMvVerificationResult::Abort(reason) => Ok(Some(reason)),
            PendingMvVerificationResult::Value(value) => {
                if let Some(timing) = timing.as_mut() {
                    timing.observe_mv_verification(read_duration);
                }
                finalize_pending_mv_verification(pool, tag, tracker, value, trigger, audit_policy)
                    .await
            }
        },
    }
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_pending_mv_actuation_with(
    pool: &SqlitePool,
    args: &TuneRequest,
    tag: &str,
    driver: &dyn Driver,
    ctrl_c: &mut CtrlC,
    allow_uncertain_quality: bool,
    tracker: &mut MvActuationTracker,
    trigger: MvVerificationTrigger,
    call_limit: MvVerificationCallLimit,
    audit_policy: ActuationAuditPolicy,
) -> anyhow::Result<Option<AbortReason>> {
    verify_pending_mv_actuation_with_timing(
        pool,
        args,
        test_effective_timing(args),
        tag,
        driver,
        ctrl_c,
        allow_uncertain_quality,
        tracker,
        trigger,
        call_limit,
        audit_policy,
        None,
    )
    .await
}
pub(super) enum PendingMvVerificationRead {
    Ready {
        operation: anyhow::Result<TickOperation<(f32, bhtune_driver::Quality)>>,
        checked_at: DateTime<Utc>,
        checked_instant: Instant,
        read_duration: Duration,
    },
    DeadlineTimedOut {
        pending: PendingMvActuation,
        checked_at: DateTime<Utc>,
        checked_instant: Instant,
    },
    RestoreTimedOut {
        pending: PendingMvActuation,
        checked_at: DateTime<Utc>,
        checked_instant: Instant,
    },
}
pub(super) struct PendingMvVerificationValue {
    pub(super) readback: f32,
    pub(super) sample_quality: SampleQuality,
    pub(super) checked_at: DateTime<Utc>,
    pub(super) checked_instant: Instant,
}
pub(super) enum PendingMvVerificationResult {
    Value(PendingMvVerificationValue),
    Abort(AbortReason),
}
pub(super) fn pending_verification_ready(
    tracker: &MvActuationTracker,
    operation: anyhow::Result<TickOperation<(f32, bhtune_driver::Quality)>>,
    read_duration: Duration,
) -> anyhow::Result<PendingMvVerificationRead> {
    let checked_instant = Instant::now();
    let pending = tracker
        .pending
        .as_ref()
        .require_invariant("pending actuation existed after the bounded read")?;
    Ok(PendingMvVerificationRead::Ready {
        operation,
        checked_at: checked_at_for_pending(pending, checked_instant)?,
        checked_instant,
        read_duration,
    })
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn read_pending_mv_verification_with_timing(
    effective_timing: EffectiveTiming,
    tag: &str,
    driver: &dyn Driver,
    ctrl_c: &mut CtrlC,
    tracker: &mut MvActuationTracker,
    mut trigger: MvVerificationTrigger,
    call_limit: MvVerificationCallLimit,
) -> anyhow::Result<PendingMvVerificationRead> {
    loop {
        let limit = {
            let pending = tracker
                .pending
                .as_ref()
                .require_invariant("pending actuation existed before the bounded read")?;
            mv_verification_read_limit(trigger, pending, call_limit)
        };
        let read_started = Instant::now();
        let read = bounded_driver_call(
            effective_timing.op_timeout_secs,
            ctrl_c,
            read_numeric_sample(driver, tag),
        );
        let (deadline, limit_kind) = limit;
        match tokio::time::timeout_at(deadline, read).await {
            Ok(operation) => {
                return pending_verification_ready(tracker, operation, read_started.elapsed());
            }
            Err(_) => match limit_kind {
                MvVerificationLimitKind::Confirmation => {
                    trigger = MvVerificationTrigger::Deadline;
                }

                MvVerificationLimitKind::Deadline => {
                    const MSG: &str = "pending actuation existed before the deadline read";
                    let pending = tracker.pending.take().require_invariant(MSG)?;
                    let checked_instant = Instant::now();
                    let checked_at = checked_at_for_pending(&pending, checked_instant)?;
                    return Ok(PendingMvVerificationRead::DeadlineTimedOut {
                        pending,
                        checked_at,
                        checked_instant,
                    });
                }
                MvVerificationLimitKind::Restore => {
                    const MSG: &str = "pending actuation existed before the bounded read";
                    let pending = tracker.pending.take().require_invariant(MSG)?;
                    let checked_instant = Instant::now();
                    let checked_at = checked_at_for_pending(&pending, checked_instant)?;
                    return Ok(PendingMvVerificationRead::RestoreTimedOut {
                        pending,
                        checked_at,
                        checked_instant,
                    });
                }
            },
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn resolve_pending_mv_read(
    pool: &SqlitePool,
    effective_timing: EffectiveTiming,
    tag: &str,
    tracker: &mut MvActuationTracker,
    operation: anyhow::Result<TickOperation<(f32, bhtune_driver::Quality)>>,
    checked_at: DateTime<Utc>,
    checked_instant: Instant,
    allow_uncertain_quality: bool,
) -> anyhow::Result<PendingMvVerificationResult> {
    let operation = match operation {
        Ok(operation) => operation,
        Err(error) => {
            let pending = tracker
                .pending
                .take()
                .require_invariant("pending actuation existed before the verification read")?;
            let detail = format!("MV verification read failed: {error}");
            record_final_actuation_observation(
                pool,
                &pending,
                checked_at,
                None,
                None,
                MvActuationStatus::Unverified,
                &detail,
            )
            .await;
            return Err(error);
        }
    };

    let (readback, quality) = match operation {
        TickOperation::Completed(value) => value,
        TickOperation::Cancelled => {
            let pending = tracker
                .pending
                .take()
                .require_invariant("pending actuation existed before the verification read")?;
            finalize_actuation_best_effort(
                pool,
                &pending,
                MvActuationStatus::Unverified,
                "MV verification was interrupted before confirmation completed",
            )
            .await;
            return Ok(PendingMvVerificationResult::Abort(
                AbortReason::UserInterrupt,
            ));
        }
        TickOperation::TimedOut => {
            let pending = tracker
                .pending
                .take()
                .require_invariant("pending actuation existed before the verification read")?;
            let detail = format!(
                "MV verification read did not complete within {} seconds",
                effective_timing.op_timeout_secs
            );
            record_final_actuation_observation(
                pool,
                &pending,
                checked_at,
                None,
                None,
                MvActuationStatus::Unverified,
                &detail,
            )
            .await;
            return Ok(PendingMvVerificationResult::Abort(
                AbortReason::OperationTimedOut {
                    tag: tag.to_string(),
                    op_timeout_secs: effective_timing.op_timeout_secs,
                },
            ));
        }
    };
    let sample_quality = sample_quality_from_driver(quality);
    if check_quality(tag, quality, allow_uncertain_quality).is_err() {
        let pending = tracker
            .pending
            .take()
            .require_invariant("pending actuation existed before the verification read")?;
        let detail = format!("MV verification read reported OPC quality {quality:?}");
        record_final_actuation_observation(
            pool,
            &pending,
            checked_at,
            Some(readback),
            Some(sample_quality),
            MvActuationStatus::Unverified,
            &detail,
        )
        .await;
        return Ok(PendingMvVerificationResult::Abort(
            AbortReason::PoorQuality {
                tag: tag.to_string(),
                quality,
            },
        ));
    }

    Ok(PendingMvVerificationResult::Value(
        PendingMvVerificationValue {
            readback,
            sample_quality,
            checked_at,
            checked_instant,
        },
    ))
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn resolve_pending_mv_poll(
    pool: &SqlitePool,
    effective_timing: EffectiveTiming,
    pv_mv_values: TickOperation<HashMap<String, TagValue>>,
    mv_tag: &str,
    checked_at: DateTime<Utc>,
    checked_instant: Instant,
    read_duration: Duration,
    allow_uncertain_quality: bool,
    tracker: &mut MvActuationTracker,
    timing: &mut PollTimingAccumulator,
) -> anyhow::Result<(Option<AbortReason>, bool)> {
    let operation = match pv_mv_values {
        TickOperation::Completed(values) => match read_numeric_from_batch(&values, mv_tag) {
            Ok(value) => Ok(TickOperation::Completed(value)),
            Err(error) => Err(error),
        },
        TickOperation::Cancelled => Ok(TickOperation::Cancelled),
        TickOperation::TimedOut => Ok(TickOperation::TimedOut),
    };

    match resolve_pending_mv_read(
        pool,
        effective_timing,
        mv_tag,
        tracker,
        operation,
        checked_at,
        checked_instant,
        allow_uncertain_quality,
    )
    .await?
    {
        PendingMvVerificationResult::Abort(reason) => Ok((Some(reason), false)),
        PendingMvVerificationResult::Value(value) => {
            timing.observe_mv_verification(read_duration);
            let outcome = finalize_pending_mv_verification(
                pool,
                mv_tag,
                tracker,
                value,
                MvVerificationTrigger::Scheduled,
                ActuationAuditPolicy::Required,
            )
            .await?;
            Ok((outcome, true))
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn confirm_pending_mv_actuation(
    pool: &SqlitePool,
    tracker: &mut MvActuationTracker,
    pending: PendingMvActuation,
    checked_at: DateTime<Utc>,
    readback: f32,
    sample_quality: SampleQuality,
    audit_policy: ActuationAuditPolicy,
) -> anyhow::Result<()> {
    let recorded_attempts = match pending.id {
        Some(id) => {
            let result = TuneMvActuationRow::record_final_observation(
                pool,
                id,
                checked_at,
                Some(readback),
                Some(sample_quality),
                MvActuationStatus::Confirmed,
                None,
            )
            .await;
            match result {
                Ok(row) => Some(row.attempt_count),
                Err(error) if audit_policy == ActuationAuditPolicy::BestEffort => {
                    tracing::error!(
                        actuation_id = id,
                        error = %error,
                        "failed to record confirmed MV restore observation"
                    );
                    None
                }
                Err(error) => {
                    tracker.pending = Some(pending);
                    return Err(error.into());
                }
            }
        }
        None => None,
    };
    if let Some(attempt_count) = recorded_attempts.filter(|attempt_count| *attempt_count > 1) {
        tracing::warn!(
            actuation_id = ?pending.id,
            attempt_count,
            target = pending.target,
            readback,
            tolerance = pending.tolerance,
            "MV actuation confirmed after earlier unsuccessful observations"
        );
    }
    tracker.confirmed_mv = Some(pending.target);
    Ok(())
}
pub(super) async fn finalize_pending_mv_verification(
    pool: &SqlitePool,
    tag: &str,
    tracker: &mut MvActuationTracker,
    value: PendingMvVerificationValue,
    trigger: MvVerificationTrigger,
    audit_policy: ActuationAuditPolicy,
) -> anyhow::Result<Option<AbortReason>> {
    let pending = tracker
        .pending
        .take()
        .require_invariant("pending actuation existed before the verification result")?;
    if value.checked_instant > pending.deadline {
        let detail = if actuation_matches(pending.target, value.readback, pending.tolerance) {
            "MV readback matched the target only after the confirmation deadline"
        } else {
            "MV readback remained outside tolerance after the confirmation deadline"
        };
        record_final_actuation_observation(
            pool,
            &pending,
            value.checked_at,
            Some(value.readback),
            Some(value.sample_quality),
            MvActuationStatus::Failed,
            detail,
        )
        .await;
        return Ok(Some(actuation_abort_reason(
            tag,
            &pending,
            Some(value.readback),
            value.checked_instant,
        )));
    }
    if actuation_matches(pending.target, value.readback, pending.tolerance) {
        confirm_pending_mv_actuation(
            pool,
            tracker,
            pending,
            value.checked_at,
            value.readback,
            value.sample_quality,
            audit_policy,
        )
        .await?;
        return Ok(None);
    }

    let deadline_reached =
        trigger == MvVerificationTrigger::Deadline || value.checked_instant >= pending.deadline;
    if deadline_reached {
        record_final_actuation_observation(
            pool,
            &pending,
            value.checked_at,
            Some(value.readback),
            Some(value.sample_quality),
            MvActuationStatus::Failed,
            "MV readback remained outside tolerance at the confirmation deadline",
        )
        .await;
        return Ok(Some(actuation_abort_reason(
            tag,
            &pending,
            Some(value.readback),
            value.checked_instant,
        )));
    }

    let attempt_count = record_actuation_observation(
        pool,
        &pending,
        value.checked_at,
        Some(value.readback),
        Some(value.sample_quality),
        audit_policy,
    )
    .await?;
    tracing::warn!(
        actuation_id = ?pending.id,
        attempt_count,
        target = pending.target,
        readback = value.readback,
        tolerance = pending.tolerance,
        "MV readback is outside tolerance; confirmation remains pending"
    );
    let mut pending = pending;
    pending.last_readback = Some(value.readback);
    tracker.pending = Some(pending);
    Ok(None)
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_pending_mv_actuation(
    pool: &SqlitePool,
    args: &TuneRequest,
    tag: &str,
    driver: &dyn Driver,
    ctrl_c: &mut CtrlC,
    allow_uncertain_quality: bool,
    tracker: &mut MvActuationTracker,
    call_deadline: Option<Instant>,
) -> anyhow::Result<Option<AbortReason>> {
    let trigger = tracker
        .pending
        .as_ref()
        .and_then(|pending| verification_trigger(pending, Instant::now()))
        .unwrap_or(MvVerificationTrigger::Scheduled);
    verify_pending_mv_actuation_with_timing(
        pool,
        args,
        test_effective_timing(args),
        tag,
        driver,
        ctrl_c,
        allow_uncertain_quality,
        tracker,
        trigger,
        call_deadline.map_or(
            MvVerificationCallLimit::None,
            MvVerificationCallLimit::Restore,
        ),
        ActuationAuditPolicy::Required,
        None,
    )
    .await
}
pub(super) async fn finalize_pending_for_run_best_effort(
    pool: &SqlitePool,
    run_id: i64,
    detail: &str,
) {
    if let Err(error) = TuneMvActuationRow::finalize_pending_for_run(
        pool,
        run_id,
        MvActuationStatus::Unverified,
        Some(detail),
    )
    .await
    {
        tracing::error!(
            run_id,
            error = %error,
            "failed to finalize pending MV actuation rows"
        );
    }
}
pub(super) async fn supersede_pending_actuation_best_effort(
    pool: &SqlitePool,
    tracker: &mut MvActuationTracker,
    detail: &str,
) {
    let Some(pending) = tracker.pending.take() else {
        return;
    };
    finalize_actuation_best_effort(pool, &pending, MvActuationStatus::Superseded, detail).await;
}
pub(super) async fn record_handoff_observation_best_effort(
    pool: &SqlitePool,
    pending: &PendingMvActuation,
    checked_at: DateTime<Utc>,
    readback: Option<f32>,
    quality: Option<SampleQuality>,
    detail: &str,
) {
    record_final_actuation_observation(
        pool,
        pending,
        checked_at,
        readback,
        quality,
        MvActuationStatus::Superseded,
        detail,
    )
    .await;
}
pub(super) fn checked_at_for_pending(
    pending: &PendingMvActuation,
    checked_instant: Instant,
) -> anyhow::Result<DateTime<Utc>> {
    Ok(pending.switch_tick
        + chrono::Duration::from_std(
            checked_instant.saturating_duration_since(pending.switch_instant),
        )
        .map_err(|_| anyhow::anyhow!("MV actuation observation time exceeded chrono's range"))?)
}
pub(super) fn utc_after_elapsed(
    now: DateTime<Utc>,
    elapsed: Duration,
) -> anyhow::Result<DateTime<Utc>> {
    Ok(now
        + chrono::Duration::from_std(elapsed)
            .map_err(|_| anyhow::anyhow!("MV command time exceeded chrono's range"))?)
}
