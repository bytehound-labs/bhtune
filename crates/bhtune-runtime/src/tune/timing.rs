#![allow(rustdoc::broken_intra_doc_links)]

use bhtune_core::{
    Action, ControllerDirection, LoopConfig, PvRange, TuningMathCompat, measure_oscillation,
};
use bhtune_db::SqlitePool;
use bhtune_db::models::{TimingBasis, TimingMetrics, TuneRunRow};

use super::poll::{CompletedPoll, PollOutcome};

pub(super) fn completed_oscillation_period_ms(
    poll_result: &anyhow::Result<PollOutcome>,
    direction: ControllerDirection,
    config: LoopConfig,
    pv_range: PvRange,
) -> Option<f64> {
    let Ok(PollOutcome::Completed(CompletedPoll {
        action:
            Action::Complete {
                peaks,
                troughs,
                switch_times,
                mv_sign_init,
            },
        ..
    })) = poll_result
    else {
        return None;
    };

    let oscillation = measure_oscillation(
        peaks,
        troughs,
        switch_times,
        *mv_sign_init,
        direction,
        config,
        pv_range,
        TuningMathCompat::default(),
    );
    Some(f64::from(oscillation.period_minutes) * 60_000.0)
}
pub(super) fn warn_on_missed_poll_opportunities(run_id: i64, metrics: &TimingMetrics) {
    if metrics.basis != TimingBasis::LiveMonotonic || metrics.missed_poll_opportunity_count == 0 {
        return;
    }

    tracing::warn!(
        run_id,
        requested_interval_ms = metrics.requested_interval_ms,
        sample_gap_count = metrics.sample_gap_count,
        mean_sample_gap_ms = metrics.mean_sample_gap_ms,
        max_sample_gap_ms = metrics.max_sample_gap_ms,
        missed_poll_opportunity_count = metrics.missed_poll_opportunity_count,
        "live tune missed at least one complete polling opportunity"
    );
}
/// Timing diagnostics are observational: every call site defers this database write until
/// after the safety-critical restore attempt, and a failure here must never replace the
/// tune's actual completion, abort, or driver-error outcome.
pub(super) async fn record_timing_metrics_best_effort(
    pool: &SqlitePool,
    run_id: i64,
    metrics: TimingMetrics,
) {
    if let Err(e) = TuneRunRow::record_timing_metrics(pool, run_id, metrics).await {
        tracing::error!(run_id, error = %e, "failed to record tune timing metrics");
    }
}
pub(super) async fn record_timing_metrics_if_present(
    pool: &SqlitePool,
    run_id: i64,
    metrics: Option<TimingMetrics>,
) {
    let Some(metrics) = metrics else { return };
    record_timing_metrics_best_effort(pool, run_id, metrics).await;
}
