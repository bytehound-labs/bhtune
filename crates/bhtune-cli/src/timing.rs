//! Timestamp sources for MRFT samples.
//!
//! The simulator advances its FOPDT process by one exact configured step per PV read, so its
//! MRFT timestamps must advance by that same step rather than by however long the host happened
//! to take to schedule the read. Live OPC DA timestamps instead project monotonic elapsed time
//! onto the run's UTC start anchor: real scheduling and driver delays remain visible to the
//! engine and persisted samples, but an NTP correction or manual calendar-clock change cannot
//! move MRFT time backward or forward after the run starts.

use std::time::Duration;

use bhtune_db::models::{
    PollLatencyMetrics, SamplingAdequacy, TimingBasis, TimingMetrics, TimingSummary,
};
use chrono::{DateTime, Utc};
use tokio::time::Instant;

use crate::args::DriverKindArg;

/// Six samples per measured period is an advisory minimum for interpreting extrema.
pub(crate) const MIN_SAMPLES_PER_PERIOD: f64 = 6.0;

/// A UTC timestamp paired with the monotonic instant at which it was observed.
///
/// UTC remains the externally readable representation stored in SQLite and passed to
/// `bhtune-core`; elapsed live-run time is measured only from `monotonic`, which cannot jump
/// because of NTP or a manual system-clock adjustment.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunTimeAnchor {
    utc: DateTime<Utc>,
    monotonic: Instant,
}

impl RunTimeAnchor {
    pub(crate) fn now() -> Self {
        let monotonic = Instant::now();
        let utc = Utc::now();
        Self { utc, monotonic }
    }

    #[cfg(test)]
    pub(crate) fn from_parts(utc: DateTime<Utc>, monotonic: Instant) -> Self {
        Self { utc, monotonic }
    }

    pub(crate) fn utc(self) -> DateTime<Utc> {
        self.utc
    }
}

#[derive(Debug)]
pub(crate) enum TickTimeSource {
    LiveMonotonic {
        anchor: RunTimeAnchor,
    },
    FixedStep {
        current: DateTime<Utc>,
        step: chrono::Duration,
    },
}

/// Accumulates adjacent successful-sample gaps without influencing tune control flow.
///
/// Initial readings and presentation-only trend boundary points are deliberately excluded:
/// only timestamps produced by [`TickTimeSource`] for real polling samples are observed.
#[derive(Debug)]
pub(crate) struct PollTimingAccumulator {
    basis: TimingBasis,
    requested_interval_ms: u64,
    previous_timestamp: Option<DateTime<Utc>>,
    sample_gap_count: u64,
    total_gap_micros: u128,
    max_gap_micros: u128,
    missed_poll_opportunity_count: u64,
    latency: PollLatencyAccumulator,
}

#[derive(Debug, Default)]
struct DurationAccumulator {
    count: u64,
    total_ms: f64,
    max_ms: Option<f64>,
}

impl DurationAccumulator {
    fn observe(&mut self, duration: Duration) {
        let elapsed_ms = duration.as_secs_f64() * 1_000.0;
        self.count = self.count.saturating_add(1);
        self.total_ms += elapsed_ms;
        self.max_ms = Some(self.max_ms.map_or(elapsed_ms, |max| max.max(elapsed_ms)));
    }

    fn finish(&self) -> TimingSummary {
        TimingSummary {
            count: self.count,
            mean_ms: (self.count > 0).then(|| self.total_ms / self.count as f64),
            max_ms: self.max_ms,
        }
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }
}

#[derive(Debug, Default)]
struct PollLatencyAccumulator {
    pv_read: DurationAccumulator,
    mv_write: DurationAccumulator,
    mv_verification: DurationAccumulator,
    sample_persist: DurationAccumulator,
    tick_work: DurationAccumulator,
}

impl PollLatencyAccumulator {
    fn is_empty(&self) -> bool {
        self.pv_read.is_empty()
            && self.mv_write.is_empty()
            && self.mv_verification.is_empty()
            && self.sample_persist.is_empty()
            && self.tick_work.is_empty()
    }

    fn finish(&self) -> PollLatencyMetrics {
        PollLatencyMetrics {
            pv_read: self.pv_read.finish(),
            mv_write: self.mv_write.finish(),
            mv_verification: self.mv_verification.finish(),
            sample_persist: self.sample_persist.finish(),
            tick_work: self.tick_work.finish(),
        }
    }
}

impl PollTimingAccumulator {
    pub(crate) fn new(basis: TimingBasis, requested_interval_ms: u64) -> Self {
        Self {
            basis,
            requested_interval_ms: requested_interval_ms.max(1),
            previous_timestamp: None,
            sample_gap_count: 0,
            total_gap_micros: 0,
            max_gap_micros: 0,
            missed_poll_opportunity_count: 0,
            latency: PollLatencyAccumulator::default(),
        }
    }

    pub(crate) fn observe(&mut self, timestamp: DateTime<Utc>) -> anyhow::Result<()> {
        if let Some(previous) = self.previous_timestamp {
            let gap = timestamp.signed_duration_since(previous);
            let gap_micros = gap.num_microseconds().ok_or_else(|| {
                anyhow::anyhow!("sample gap exceeded chrono's supported microsecond range")
            })?;
            let gap_micros = u128::try_from(gap_micros)
                .map_err(|_| anyhow::anyhow!("sample timestamps moved backward"))?;

            self.sample_gap_count = self
                .sample_gap_count
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("sample gap count exceeded the supported range"))?;
            self.total_gap_micros = self
                .total_gap_micros
                .checked_add(gap_micros)
                .ok_or_else(|| anyhow::anyhow!("accumulated sample gaps overflowed"))?;
            self.max_gap_micros = self.max_gap_micros.max(gap_micros);

            let missed_threshold_micros =
                u128::from(self.requested_interval_ms).saturating_mul(2_000);
            if gap_micros >= missed_threshold_micros {
                self.missed_poll_opportunity_count = self
                    .missed_poll_opportunity_count
                    .checked_add(1)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "missed poll opportunity count exceeded the supported range"
                        )
                    })?;
            }
        }

        self.previous_timestamp = Some(timestamp);
        Ok(())
    }

    pub(crate) fn observe_pv_read(&mut self, duration: Duration) {
        self.latency.pv_read.observe(duration);
    }

    pub(crate) fn observe_mv_write(&mut self, duration: Duration) {
        self.latency.mv_write.observe(duration);
    }

    pub(crate) fn observe_mv_verification(&mut self, duration: Duration) {
        self.latency.mv_verification.observe(duration);
    }

    pub(crate) fn observe_sample_persist(&mut self, duration: Duration) {
        self.latency.sample_persist.observe(duration);
    }

    pub(crate) fn observe_tick_work(&mut self, duration: Duration) {
        self.latency.tick_work.observe(duration);
    }

    pub(crate) fn finish(
        &self,
        measured_oscillation_period_ms: Option<f64>,
    ) -> Option<TimingMetrics> {
        self.previous_timestamp?;

        let mean_sample_gap_ms = (self.sample_gap_count > 0)
            .then(|| self.total_gap_micros as f64 / self.sample_gap_count as f64 / 1_000.0);
        let max_sample_gap_ms =
            (self.sample_gap_count > 0).then(|| self.max_gap_micros as f64 / 1_000.0);
        let measured_oscillation_period_ms =
            measured_oscillation_period_ms.filter(|period| period.is_finite() && *period > 0.0);
        let approximate_samples_per_period = measured_oscillation_period_ms.and_then(|period| {
            mean_sample_gap_ms
                .filter(|mean| *mean > 0.0)
                .map(|mean| period / mean)
        });
        let sampling_adequacy = classify_sampling_adequacy(approximate_samples_per_period);

        Some(TimingMetrics {
            basis: self.basis,
            requested_interval_ms: self.requested_interval_ms,
            sample_gap_count: self.sample_gap_count,
            mean_sample_gap_ms,
            max_sample_gap_ms,
            missed_poll_opportunity_count: self.missed_poll_opportunity_count,
            measured_oscillation_period_ms,
            approximate_samples_per_period,
            sampling_adequacy,
            poll_latency: (!self.latency.is_empty()).then(|| self.latency.finish()),
        })
    }
}

fn classify_sampling_adequacy(samples_per_period: Option<f64>) -> SamplingAdequacy {
    match samples_per_period {
        Some(value) if value.is_finite() && value >= MIN_SAMPLES_PER_PERIOD => {
            SamplingAdequacy::Adequate
        }
        Some(value) if value.is_finite() && value > 0.0 => SamplingAdequacy::Marginal,
        _ => SamplingAdequacy::NotAssessed,
    }
}

impl TickTimeSource {
    pub(crate) fn for_driver(
        driver: DriverKindArg,
        anchor: RunTimeAnchor,
        poll_interval_ms: u64,
    ) -> anyhow::Result<Self> {
        match driver {
            DriverKindArg::Opcda => Ok(Self::LiveMonotonic { anchor }),
            DriverKindArg::Simulator => {
                let poll_interval = Duration::from_millis(poll_interval_ms.max(1));
                let step = chrono::Duration::from_std(poll_interval).map_err(|_| {
                    anyhow::anyhow!(
                        "poll interval {poll_interval_ms} ms is too large for simulator timestamps"
                    )
                })?;
                Ok(Self::FixedStep {
                    current: anchor.utc,
                    step,
                })
            }
        }
    }

    pub(crate) fn next_timestamp(&mut self) -> anyhow::Result<DateTime<Utc>> {
        self.next_timestamp_at(Instant::now())
    }

    fn next_timestamp_at(&mut self, monotonic_now: Instant) -> anyhow::Result<DateTime<Utc>> {
        match self {
            Self::LiveMonotonic { anchor } => {
                let elapsed = monotonic_now
                    .checked_duration_since(anchor.monotonic)
                    .ok_or_else(|| {
                        anyhow::anyhow!("live monotonic clock moved before the run's time anchor")
                    })?;
                project_elapsed(*anchor, elapsed)
            }
            Self::FixedStep { current, step } => {
                let next = current.checked_add_signed(*step).ok_or_else(|| {
                    anyhow::anyhow!("simulator timestamp exceeded chrono's supported range")
                })?;
                *current = next;
                Ok(next)
            }
        }
    }
}

fn project_elapsed(anchor: RunTimeAnchor, elapsed: Duration) -> anyhow::Result<DateTime<Utc>> {
    let elapsed = chrono::Duration::from_std(elapsed)
        .map_err(|_| anyhow::anyhow!("live elapsed time exceeds chrono's supported range"))?;
    anchor
        .utc
        .checked_add_signed(elapsed)
        .ok_or_else(|| anyhow::anyhow!("live timestamp exceeded chrono's supported range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor_at(utc: DateTime<Utc>) -> RunTimeAnchor {
        RunTimeAnchor::from_parts(utc, Instant::now())
    }

    #[test]
    fn simulator_first_tick_is_one_process_step_after_start() {
        let start = DateTime::UNIX_EPOCH;
        let mut source =
            TickTimeSource::for_driver(DriverKindArg::Simulator, anchor_at(start), 5).unwrap();

        assert_eq!(
            source.next_timestamp().unwrap(),
            start + chrono::Duration::milliseconds(5)
        );
    }

    #[test]
    fn simulator_ticks_advance_by_the_exact_poll_interval() {
        let start = DateTime::UNIX_EPOCH;
        let mut source =
            TickTimeSource::for_driver(DriverKindArg::Simulator, anchor_at(start), 50).unwrap();

        let first = source.next_timestamp().unwrap();
        let second = source.next_timestamp().unwrap();
        let third = source.next_timestamp().unwrap();

        assert_eq!((second - first).num_milliseconds(), 50);
        assert_eq!((third - second).num_milliseconds(), 50);
    }

    #[test]
    fn simulator_matches_the_poll_loop_minimum_interval() {
        let start = DateTime::UNIX_EPOCH;
        let mut source =
            TickTimeSource::for_driver(DriverKindArg::Simulator, anchor_at(start), 0).unwrap();

        assert_eq!(
            source.next_timestamp().unwrap(),
            start + chrono::Duration::milliseconds(1)
        );
    }

    #[test]
    fn simulator_rejects_an_unrepresentable_poll_interval() {
        let error = TickTimeSource::for_driver(
            DriverKindArg::Simulator,
            anchor_at(DateTime::UNIX_EPOCH),
            u64::MAX,
        )
        .unwrap_err();

        assert!(error.to_string().contains("too large"));
    }

    #[test]
    fn simulator_rejects_a_timestamp_that_exceeds_chronos_range() {
        let mut source = TickTimeSource::FixedStep {
            current: DateTime::<Utc>::MAX_UTC,
            step: chrono::Duration::milliseconds(1),
        };

        let error = source.next_timestamp().unwrap_err();

        assert!(error.to_string().contains("supported range"));
    }

    #[test]
    fn live_timestamp_projects_monotonic_elapsed_onto_the_utc_anchor() {
        let monotonic = Instant::now();
        let anchor = RunTimeAnchor::from_parts(DateTime::UNIX_EPOCH, monotonic);
        let mut source = TickTimeSource::for_driver(DriverKindArg::Opcda, anchor, 800).unwrap();

        assert_eq!(
            source
                .next_timestamp_at(monotonic + Duration::from_millis(1_234))
                .unwrap(),
            DateTime::UNIX_EPOCH + chrono::Duration::milliseconds(1_234)
        );
    }

    #[test]
    fn live_timestamp_preserves_a_real_delayed_poll_gap() {
        let monotonic = Instant::now();
        let anchor = RunTimeAnchor::from_parts(DateTime::UNIX_EPOCH, monotonic);
        let mut source = TickTimeSource::for_driver(DriverKindArg::Opcda, anchor, 800).unwrap();

        let first = source
            .next_timestamp_at(monotonic + Duration::from_millis(800))
            .unwrap();
        let delayed = source
            .next_timestamp_at(monotonic + Duration::from_millis(2_750))
            .unwrap();

        assert_eq!(delayed - first, chrono::Duration::milliseconds(1_950));
    }

    #[test]
    fn live_timestamp_rejects_an_instant_before_the_anchor() {
        let monotonic = Instant::now();
        let anchor = RunTimeAnchor::from_parts(DateTime::UNIX_EPOCH, monotonic);
        let mut source = TickTimeSource::for_driver(DriverKindArg::Opcda, anchor, 800).unwrap();

        let error = source
            .next_timestamp_at(monotonic - Duration::from_millis(1))
            .unwrap_err();

        assert!(error.to_string().contains("before"));
    }

    #[test]
    fn live_timestamp_rejects_unrepresentable_elapsed_time() {
        let error = project_elapsed(
            RunTimeAnchor::from_parts(DateTime::UNIX_EPOCH, Instant::now()),
            Duration::MAX,
        )
        .unwrap_err();

        assert!(error.to_string().contains("supported range"));
    }

    #[test]
    fn live_timestamp_rejects_utc_overflow() {
        let error = project_elapsed(
            RunTimeAnchor::from_parts(DateTime::<Utc>::MAX_UTC, Instant::now()),
            Duration::from_millis(1),
        )
        .unwrap_err();

        assert!(error.to_string().contains("supported range"));
    }

    #[test]
    fn timing_metrics_are_empty_until_two_samples_exist() {
        let empty = PollTimingAccumulator::new(TimingBasis::LiveMonotonic, 800);
        assert_eq!(empty.finish(None), None);

        let mut timing = PollTimingAccumulator::new(TimingBasis::LiveMonotonic, 800);
        timing.observe(DateTime::UNIX_EPOCH).unwrap();

        let metrics = timing.finish(None).unwrap();

        assert_eq!(metrics.sample_gap_count, 0);
        assert_eq!(metrics.mean_sample_gap_ms, None);
        assert_eq!(metrics.max_sample_gap_ms, None);
        assert_eq!(metrics.missed_poll_opportunity_count, 0);
        assert_eq!(metrics.measured_oscillation_period_ms, None);
        assert_eq!(metrics.approximate_samples_per_period, None);
        assert_eq!(metrics.sampling_adequacy, SamplingAdequacy::NotAssessed);
        assert_eq!(metrics.poll_latency, None);
    }

    #[test]
    fn timing_metrics_measure_mean_max_and_missed_opportunities() {
        let start = DateTime::UNIX_EPOCH;
        let mut timing = PollTimingAccumulator::new(TimingBasis::LiveMonotonic, 10);
        for offset_ms in [0, 10, 30, 61] {
            timing
                .observe(start + chrono::Duration::milliseconds(offset_ms))
                .unwrap();
        }

        let metrics = timing.finish(Some(100.0)).unwrap();

        assert_eq!(metrics.sample_gap_count, 3);
        assert_eq!(metrics.mean_sample_gap_ms, Some(61.0 / 3.0));
        assert_eq!(metrics.max_sample_gap_ms, Some(31.0));
        assert_eq!(metrics.missed_poll_opportunity_count, 2);
        assert_eq!(metrics.measured_oscillation_period_ms, Some(100.0));
        assert_eq!(
            metrics.approximate_samples_per_period,
            Some(100.0 / (61.0 / 3.0))
        );
        assert_eq!(metrics.sampling_adequacy, SamplingAdequacy::Marginal);
    }

    #[test]
    fn timing_metrics_count_a_gap_at_exactly_twice_the_requested_interval() {
        let start = DateTime::UNIX_EPOCH;
        let mut timing = PollTimingAccumulator::new(TimingBasis::LiveMonotonic, 50);
        timing.observe(start).unwrap();
        timing
            .observe(start + chrono::Duration::milliseconds(100))
            .unwrap();

        assert_eq!(
            timing.finish(None).unwrap().missed_poll_opportunity_count,
            1
        );
    }

    #[test]
    fn sampling_adequacy_is_adequate_at_the_advisory_threshold() {
        let start = DateTime::UNIX_EPOCH;
        let mut timing = PollTimingAccumulator::new(TimingBasis::SimulatedFixedStep, 100);
        timing.observe(start).unwrap();
        timing
            .observe(start + chrono::Duration::milliseconds(100))
            .unwrap();

        let marginal = timing.finish(Some(500.0)).unwrap();
        assert_eq!(marginal.sampling_adequacy, SamplingAdequacy::Marginal);

        let adequate = timing.finish(Some(600.0)).unwrap();
        assert_eq!(adequate.sampling_adequacy, SamplingAdequacy::Adequate);
    }

    #[test]
    fn latency_metrics_accumulate_each_operation_category() {
        let mut timing = PollTimingAccumulator::new(TimingBasis::LiveMonotonic, 800);
        timing.observe(DateTime::UNIX_EPOCH).unwrap();
        timing.observe_pv_read(Duration::from_millis(4));
        timing.observe_pv_read(Duration::from_millis(8));
        timing.observe_mv_write(Duration::from_millis(10));
        timing.observe_mv_verification(Duration::from_millis(12));
        timing.observe_sample_persist(Duration::from_millis(14));
        timing.observe_tick_work(Duration::from_millis(20));

        let latency = timing
            .finish(None)
            .unwrap()
            .poll_latency
            .expect("observed operation timings should be present");

        assert_eq!(latency.pv_read.count, 2);
        assert_eq!(latency.pv_read.mean_ms, Some(6.0));
        assert_eq!(latency.pv_read.max_ms, Some(8.0));
        assert_eq!(latency.mv_write.mean_ms, Some(10.0));
        assert_eq!(latency.mv_verification.max_ms, Some(12.0));
        assert_eq!(latency.sample_persist.count, 1);
        assert_eq!(latency.tick_work.count, 1);
    }

    #[test]
    fn timing_metrics_reject_timestamps_that_move_backward() {
        let start = DateTime::UNIX_EPOCH;
        let mut timing = PollTimingAccumulator::new(TimingBasis::LiveMonotonic, 50);
        timing.observe(start).unwrap();

        let error = timing
            .observe(start - chrono::Duration::milliseconds(1))
            .unwrap_err();

        assert!(error.to_string().contains("backward"));
    }

    #[test]
    fn timing_metrics_reject_a_gap_outside_the_supported_microsecond_range() {
        let mut timing = PollTimingAccumulator::new(TimingBasis::LiveMonotonic, 50);
        timing.observe(DateTime::<Utc>::MIN_UTC).unwrap();

        let error = timing.observe(DateTime::<Utc>::MAX_UTC).unwrap_err();

        assert!(error.to_string().contains("microsecond range"));
    }

    #[test]
    fn timing_metrics_reject_a_sample_gap_count_overflow() {
        let start = DateTime::UNIX_EPOCH;
        let mut timing = PollTimingAccumulator::new(TimingBasis::LiveMonotonic, 50);
        timing.observe(start).unwrap();
        timing.sample_gap_count = u64::MAX;

        let error = timing
            .observe(start + chrono::Duration::milliseconds(1))
            .unwrap_err();

        assert!(error.to_string().contains("sample gap count"));
    }

    #[test]
    fn timing_metrics_reject_an_accumulated_gap_overflow() {
        let start = DateTime::UNIX_EPOCH;
        let mut timing = PollTimingAccumulator::new(TimingBasis::LiveMonotonic, 50);
        timing.observe(start).unwrap();
        timing.total_gap_micros = u128::MAX;

        let error = timing
            .observe(start + chrono::Duration::microseconds(1))
            .unwrap_err();

        assert!(error.to_string().contains("accumulated sample gaps"));
    }

    #[test]
    fn timing_metrics_reject_a_missed_opportunity_count_overflow() {
        let start = DateTime::UNIX_EPOCH;
        let mut timing = PollTimingAccumulator::new(TimingBasis::LiveMonotonic, 1);
        timing.observe(start).unwrap();
        timing.missed_poll_opportunity_count = u64::MAX;

        let error = timing
            .observe(start + chrono::Duration::milliseconds(2))
            .unwrap_err();

        assert!(error.to_string().contains("missed poll opportunity count"));
    }
}
