//! Timestamp sources for MRFT samples.
//!
//! The simulator advances its FOPDT process by one exact configured step per PV read, so its
//! MRFT timestamps must advance by that same step rather than by however long the host happened
//! to take to schedule the read. OPC DA runs use the live wall clock here; live monotonic timing
//! is handled separately from the simulator's fixed-step time domain.

use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::args::DriverKindArg;

#[derive(Debug)]
pub(crate) enum TickTimeSource {
    WallClock,
    FixedStep {
        current: DateTime<Utc>,
        step: chrono::Duration,
    },
}

impl TickTimeSource {
    pub(crate) fn for_driver(
        driver: DriverKindArg,
        start_time: DateTime<Utc>,
        poll_interval_ms: u64,
    ) -> anyhow::Result<Self> {
        match driver {
            DriverKindArg::Opcda => Ok(Self::WallClock),
            DriverKindArg::Simulator => {
                let poll_interval = Duration::from_millis(poll_interval_ms.max(1));
                let step = chrono::Duration::from_std(poll_interval).map_err(|_| {
                    anyhow::anyhow!(
                        "poll interval {poll_interval_ms} ms is too large for simulator timestamps"
                    )
                })?;
                Ok(Self::FixedStep {
                    current: start_time,
                    step,
                })
            }
        }
    }

    pub(crate) fn next_timestamp(&mut self) -> anyhow::Result<DateTime<Utc>> {
        match self {
            Self::WallClock => Ok(Utc::now()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulator_first_tick_is_one_process_step_after_start() {
        let start = DateTime::UNIX_EPOCH;
        let mut source = TickTimeSource::for_driver(DriverKindArg::Simulator, start, 5).unwrap();

        assert_eq!(
            source.next_timestamp().unwrap(),
            start + chrono::Duration::milliseconds(5)
        );
    }

    #[test]
    fn simulator_ticks_advance_by_the_exact_poll_interval() {
        let start = DateTime::UNIX_EPOCH;
        let mut source = TickTimeSource::for_driver(DriverKindArg::Simulator, start, 50).unwrap();

        let first = source.next_timestamp().unwrap();
        let second = source.next_timestamp().unwrap();
        let third = source.next_timestamp().unwrap();

        assert_eq!((second - first).num_milliseconds(), 50);
        assert_eq!((third - second).num_milliseconds(), 50);
    }

    #[test]
    fn simulator_matches_the_poll_loop_minimum_interval() {
        let start = DateTime::UNIX_EPOCH;
        let mut source = TickTimeSource::for_driver(DriverKindArg::Simulator, start, 0).unwrap();

        assert_eq!(
            source.next_timestamp().unwrap(),
            start + chrono::Duration::milliseconds(1)
        );
    }

    #[test]
    fn simulator_rejects_an_unrepresentable_poll_interval() {
        let error =
            TickTimeSource::for_driver(DriverKindArg::Simulator, DateTime::UNIX_EPOCH, u64::MAX)
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
}
