//! Timestamp sources for MRFT samples.
//!
//! The simulator advances its FOPDT process by one exact configured step per PV read, so its
//! MRFT timestamps must advance by that same step rather than by however long the host happened
//! to take to schedule the read. Live OPC DA timestamps instead project monotonic elapsed time
//! onto the run's UTC start anchor: real scheduling and driver delays remain visible to the
//! engine and persisted samples, but an NTP correction or manual calendar-clock change cannot
//! move MRFT time backward or forward after the run starts.

use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::time::Instant;

use crate::args::DriverKindArg;

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
}
