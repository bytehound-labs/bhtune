//! `ReplayDriver`: feeds a previously captured golden-master trace back through the
//! [`Driver`] trait, tick by tick, instead of a live OPC DA connection or the in-process
//! FOPDT simulator.
//!
//! `core-replay-harness` (`crates/bhtune-core/tests/golden_replay.rs`) already proves the
//! *pure* `MrftEngine` reproduces the legacy C# application's behavior exactly, by feeding a
//! fixture's recorded ticks directly into `engine.step(Tick { time, pv })`. That test cannot
//! exercise anything in this crate at all -- `bhtune-core` cannot depend on `bhtune-driver`,
//! which depends on it -- so it says nothing about whether the *real* async `Driver`
//! abstraction a live run actually goes through (tag-based indirection, `TagValue`/
//! `TagWrite` conversions, `Quality`, `Send`-across-`.await` dispatch behind `Box<dyn
//! Driver>`/`Arc<dyn Driver>`) introduces its own bugs on top of a provably-correct engine.
//! `ReplayDriver` closes that gap: it serves the exact same recorded trace through the
//! genuine trait boundary, so a validation test can drive a real `MrftEngine` through it and
//! confirm the same golden trace still reaches the same answer -- see this module's own
//! `mrft_engine_replays_the_golden_trace_through_the_real_driver_trait` test.

use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{
    driver::Driver,
    error::{DriverError, DriverResult},
    types::{
        BrowsePage, BrowsePageRequest, DriverCapabilities, Quality, SearchEvent, SearchRequest,
        TagId, TagValue, TagWrite, WriteOutcome,
    },
};

/// One recorded `(time, PV)` sample from a captured trace -- the two fields
/// [`ReplayDriver`] actually needs to serve a PV read.
///
/// Deliberately not `bhtune-core`'s `Tick`: this crate stays free of a `bhtune-core`
/// dependency in production code (matching `driver-trait`/`driver-opcda`/
/// `driver-simulator`'s "reading/writing named string tags has no domain meaning by
/// itself" rule -- see `driver-trait`'s design notes in `AGENTS.md`). Also deliberately not
/// the full golden-fixture JSON schema `crates/bhtune-core/tests/golden_replay.rs` owns
/// (`config`, `direction`, `initial`, `pv_range`, `template_name`, each tick's `expected`
/// block, `expected_final`) -- none of that is needed to *serve* a replay, only to
/// *validate* one, which stays the calling test's job.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplaySample {
    pub time: DateTime<Utc>,
    pub pv: f32,
}

/// A single MV write [`ReplayDriver`] observed, in the order [`Driver::write`] was called
/// -- what a validation test inspects afterward (via [`ReplayDriver::writes`]) to see
/// exactly what an engine driven through the real `Driver` trait chose to write, without
/// needing its own separate bookkeeping alongside the driver's.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedWrite {
    pub tag: TagId,
    pub value: f32,
}

/// The subset of a golden-master fixture's JSON shape [`ReplayDriver::from_fixture_json`]
/// parses -- every other top-level or per-tick field is silently ignored by `serde`'s
/// default "unknown fields are fine" behavior, since none of it is needed to serve PV
/// samples. See [`ReplaySample`]'s doc comment for why this is a deliberate subset rather
/// than reusing `bhtune-core`'s test-only `Fixture` type.
#[derive(Debug, Deserialize)]
struct FixtureFile {
    ticks: Vec<FixtureTick>,
}

#[derive(Debug, Deserialize)]
struct FixtureTick {
    time: DateTime<Utc>,
    pv: f32,
}

/// The error [`Driver::read`] wraps in [`DriverError::Operation`] when the configured PV
/// tag is read after every recorded sample has already been consumed.
///
/// A correctly captured trace paired with a correctly behaving engine should never reach
/// this: `MrftEngine::step` is a documented no-op once it has returned `Action::Complete`
/// once (see `crates/bhtune-core/tests/golden_replay.rs`'s own note on this), so a caller
/// that stops polling as soon as completion is observed -- exactly the shape that test and
/// this module's own end-to-end test both use -- never triggers it. Seeing this in practice
/// means either the trace's tick count doesn't actually cover a real MRFT completion, or the
/// engine driving this driver has a genuine regression that fails to complete in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayTraceExhausted {
    /// How many samples this driver was constructed with.
    pub recorded: usize,
    /// The 1-based read attempt number that failed (`recorded + 1`, `recorded + 2`, ...).
    pub attempted: usize,
}

impl std::fmt::Display for ReplayTraceExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "replay trace exhausted: only {} sample(s) recorded, but PV read attempt #{} was \
             made -- the driving engine never reported completion within the recorded trace",
            self.recorded, self.attempted
        )
    }
}

impl std::error::Error for ReplayTraceExhausted {}

#[derive(Debug)]
struct ReplayState {
    next_index: usize,
    last_mv: f32,
    writes: Vec<RecordedWrite>,
}

/// Serves a captured `(time, PV)` trace through the real [`Driver`] trait, for validating
/// that a live `MrftEngine` run reproduces a golden-master trace's result when driven
/// through the actual async abstraction -- not just when fed the trace directly, as
/// `core-replay-harness` already does at the pure-engine level.
///
/// Reading the configured PV tag returns the next unconsumed sample's PV value, with its
/// *real* recorded time in `TagValue.timestamp`. This is the one [`Driver`] implementation
/// in this crate where that field is genuinely meaningful rather than a diagnostic-only
/// extra: [`crate::types::TagValue`]'s own doc comment is clear that a live driver's
/// timestamp must never become "the tick time the tuning engine itself runs on, which comes
/// from the caller's own polling clock instead" -- true and load-bearing for
/// `OpcDaDriver`/`SimulatorDriver`, whose reported (or absent) timestamps cannot be
/// trusted to reconstruct a control loop's real tick cadence. `ReplayDriver` is a
/// deliberate, narrow exception to that rule, not a violation of it: it is not a live
/// driver at all, its entire purpose is exact historical replay, and the recorded time
/// *is* the tick time a validation test needs -- reading it straight back out of the trait
/// boundary is simpler and less redundant than threading the same value through some
/// separate side channel the test would otherwise have to keep in lockstep with this
/// driver's own internal cursor.
///
/// Reading the configured MV tag returns the last written value (or the seeded initial MV
/// before any write), matching [`crate::SimulatorDriver`]'s convention exactly. Running a
/// PV read past the last recorded sample is [`DriverError::Operation`] (wrapping
/// [`ReplayTraceExhausted`]) rather than panicking or silently repeating/holding the last
/// value, since either of those would let a real regression (the engine failing to
/// complete) masquerade as a passing test.
///
/// Uses `std::sync::Mutex`, matching [`crate::SimulatorDriver`]: every operation here is
/// synchronous index/vec bookkeeping with no `.await` point in the critical section.
#[derive(Debug)]
pub struct ReplayDriver {
    pv_tag: TagId,
    mv_tag: TagId,
    samples: Vec<ReplaySample>,
    state: Mutex<ReplayState>,
}

impl ReplayDriver {
    /// Builds a replay driver from an already-parsed sample sequence. `initial_mv` is what
    /// the MV tag reads as before the first write -- mirroring
    /// [`crate::SimulatorDriver::new`]'s `initial_mv` parameter, and matching the fact that
    /// a real trace's MV convention (see `crates/bhtune-core/tests/golden_replay.rs`'s
    /// `FixtureInitial::mv_ini`) is likewise supplied out of band from the tick sequence
    /// itself.
    pub fn new(
        pv_tag: impl Into<TagId>,
        mv_tag: impl Into<TagId>,
        samples: Vec<ReplaySample>,
        initial_mv: f32,
    ) -> ReplayDriver {
        ReplayDriver {
            pv_tag: pv_tag.into(),
            mv_tag: mv_tag.into(),
            samples,
            state: Mutex::new(ReplayState {
                next_index: 0,
                last_mv: initial_mv,
                writes: Vec::new(),
            }),
        }
    }

    /// Parses a golden-master fixture JSON document's `ticks[].time`/`ticks[].pv` fields
    /// (see [`FixtureFile`]) into the sample sequence [`ReplayDriver::new`] expects, so a
    /// validation test can point this driver directly at the same fixture file
    /// `core-replay-harness` already validates against (`tests/golden/fixtures/*.json`)
    /// rather than hand-transcribing the tick sequence a second time. A parse failure is
    /// [`DriverError::Operation`], matching this crate's error-model doc comment's own
    /// forward-looking note that golden-trace parse errors belong there.
    pub fn from_fixture_json(
        pv_tag: impl Into<TagId>,
        mv_tag: impl Into<TagId>,
        json: &str,
        initial_mv: f32,
    ) -> DriverResult<ReplayDriver> {
        let file: FixtureFile =
            serde_json::from_str(json).map_err(|e| DriverError::Operation(Box::new(e)))?;
        let samples = file
            .ticks
            .into_iter()
            .map(|t| ReplaySample {
                time: t.time,
                pv: t.pv,
            })
            .collect();
        Ok(ReplayDriver::new(pv_tag, mv_tag, samples, initial_mv))
    }

    /// Every MV write observed so far, in call order -- for a validation test to compare
    /// against a golden fixture's own expected per-tick MV sequence.
    pub fn writes(&self) -> Vec<RecordedWrite> {
        self.state.lock().unwrap().writes.clone()
    }

    /// How many configured samples have not yet been consumed by a PV read.
    pub fn remaining(&self) -> usize {
        let state = self.state.lock().unwrap();
        self.samples.len() - state.next_index
    }
}

#[async_trait]
impl Driver for ReplayDriver {
    async fn read(&self, tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
        let mut state = self.state.lock().unwrap();
        tags.iter()
            .map(|tag| {
                if *tag == self.pv_tag {
                    let index = state.next_index;
                    let sample = self.samples.get(index).ok_or_else(|| {
                        DriverError::Operation(Box::new(ReplayTraceExhausted {
                            recorded: self.samples.len(),
                            attempted: index + 1,
                        }))
                    })?;
                    state.next_index += 1;
                    Ok(TagValue {
                        tag: tag.clone(),
                        value: sample.pv.to_string(),
                        quality: Quality::Good,
                        timestamp: Some(sample.time),
                    })
                } else if *tag == self.mv_tag {
                    Ok(TagValue {
                        tag: tag.clone(),
                        value: state.last_mv.to_string(),
                        quality: Quality::Good,
                        timestamp: None,
                    })
                } else {
                    Err(DriverError::InvalidTagValue {
                        tag: tag.clone(),
                        message: "ReplayDriver only knows its configured PV/MV tags".to_string(),
                    })
                }
            })
            .collect()
    }

    async fn write(&self, tag: &TagId, value: TagWrite) -> DriverResult<WriteOutcome> {
        if *tag != self.mv_tag {
            return Err(DriverError::InvalidTagValue {
                tag: tag.clone(),
                message: "ReplayDriver only accepts writes to its configured MV tag".to_string(),
            });
        }
        let mv = match value {
            TagWrite::Float(f) => f,
            TagWrite::Raw(s) => match s.parse::<f32>() {
                Ok(f) => f,
                Err(_) => {
                    return Ok(WriteOutcome::failure(format!(
                        "'{s}' is not a valid numeric MV value"
                    )));
                }
            },
        };
        let mut state = self.state.lock().unwrap();
        state.last_mv = mv;
        state.writes.push(RecordedWrite {
            tag: tag.clone(),
            value: mv,
        });
        Ok(WriteOutcome::success())
    }

    async fn capabilities(&self) -> DriverResult<DriverCapabilities> {
        Err(DriverError::Unsupported {
            operation: "capabilities",
        })
    }

    async fn browse(&self, _request: BrowsePageRequest) -> DriverResult<BrowsePage> {
        Err(DriverError::Unsupported {
            operation: "browse",
        })
    }

    async fn close_browse_session(&self, _session_id: &str) -> DriverResult<()> {
        Err(DriverError::Unsupported {
            operation: "browse-session close",
        })
    }

    async fn search(&self, _request: SearchRequest) -> DriverResult<Vec<SearchEvent>> {
        Err(DriverError::Unsupported {
            operation: "search",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap() + chrono::Duration::seconds(secs)
    }

    fn samples() -> Vec<ReplaySample> {
        vec![
            ReplaySample {
                time: t(0),
                pv: 10.0,
            },
            ReplaySample {
                time: t(1),
                pv: 11.0,
            },
            ReplaySample {
                time: t(2),
                pv: 12.0,
            },
        ]
    }

    fn expect_trace_exhaustion(error: DriverError) -> Box<ReplayTraceExhausted> {
        match error {
            DriverError::Operation(source) => source.downcast::<ReplayTraceExhausted>().unwrap(),
            other => panic!("expected DriverError::Operation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unsupported_namespace_operations_are_reported() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 0.0);
        assert!(matches!(
            driver.capabilities().await,
            Err(DriverError::Unsupported {
                operation: "capabilities"
            })
        ));
        assert!(matches!(
            driver.browse(BrowsePageRequest::root(1)).await,
            Err(DriverError::Unsupported {
                operation: "browse"
            })
        ));
        assert!(matches!(
            driver.close_browse_session("s").await,
            Err(DriverError::Unsupported {
                operation: "browse-session close"
            })
        ));
        assert!(matches!(
            driver
                .search(SearchRequest {
                    query: "PV".into(),
                    match_mode: crate::types::SearchMatchMode::Exact,
                    session_id: None,
                    scope_node_key: None,
                    max_results: 1,
                    include_branches: false,
                    refresh: false,
                })
                .await,
            Err(DriverError::Unsupported {
                operation: "search"
            })
        ));
    }

    // --- construction / basic read-back ---------------------------------------------------

    #[tokio::test]
    async fn reads_pv_samples_in_order_with_their_recorded_timestamps() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 50.0);

        for (i, expected) in samples().iter().enumerate() {
            let read = driver.read(&["PV".to_string()]).await.unwrap();
            assert_eq!(read.len(), 1, "tick {i}");
            assert_eq!(read[0].tag, "PV");
            assert_eq!(read[0].value, expected.pv.to_string(), "tick {i}");
            assert_eq!(read[0].quality, Quality::Good, "tick {i}");
            assert_eq!(read[0].timestamp, Some(expected.time), "tick {i}");
        }
    }

    #[tokio::test]
    async fn mv_read_before_any_write_returns_the_seeded_initial_value() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 42.5);
        let read = driver.read(&["MV".to_string()]).await.unwrap();
        assert_eq!(read[0].value, "42.5");
        assert_eq!(read[0].timestamp, None, "MV reads have no recorded time");
    }

    #[tokio::test]
    async fn mv_read_reflects_the_most_recent_write() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 0.0);
        driver
            .write(&"MV".to_string(), TagWrite::Float(37.0))
            .await
            .unwrap();
        let read = driver.read(&["MV".to_string()]).await.unwrap();
        assert_eq!(read[0].value, "37");
    }

    #[tokio::test]
    async fn reading_pv_does_not_advance_a_subsequent_mv_read_and_vice_versa() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 0.0);
        driver.read(&["MV".to_string()]).await.unwrap();
        driver.read(&["MV".to_string()]).await.unwrap();
        assert_eq!(
            driver.remaining(),
            3,
            "MV reads must not consume PV samples"
        );
        driver.read(&["PV".to_string()]).await.unwrap();
        assert_eq!(driver.remaining(), 2);
    }

    #[tokio::test]
    async fn reading_multiple_tags_in_one_call_resolves_each_independently() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 5.0);
        let read = driver
            .read(&["PV".to_string(), "MV".to_string()])
            .await
            .unwrap();
        assert_eq!(read[0].tag, "PV");
        assert_eq!(read[0].value, "10");
        assert_eq!(read[1].tag, "MV");
        assert_eq!(read[1].value, "5");
        assert_eq!(
            driver.remaining(),
            2,
            "the one PV tag in the batch consumed one sample"
        );
    }

    // --- writes / recording ----------------------------------------------------------------

    #[tokio::test]
    async fn writes_are_recorded_in_call_order() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 0.0);
        driver
            .write(&"MV".to_string(), TagWrite::Float(1.0))
            .await
            .unwrap();
        driver
            .write(&"MV".to_string(), TagWrite::Float(2.0))
            .await
            .unwrap();
        driver
            .write(&"MV".to_string(), TagWrite::Raw("3".to_string()))
            .await
            .unwrap();
        let writes = driver.writes();
        assert_eq!(
            writes,
            vec![
                RecordedWrite {
                    tag: "MV".to_string(),
                    value: 1.0
                },
                RecordedWrite {
                    tag: "MV".to_string(),
                    value: 2.0
                },
                RecordedWrite {
                    tag: "MV".to_string(),
                    value: 3.0
                },
            ]
        );
    }

    #[tokio::test]
    async fn raw_write_with_unparseable_value_is_a_rejected_outcome_not_an_error() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 0.0);
        let outcome = driver
            .write(&"MV".to_string(), TagWrite::Raw("not-a-number".to_string()))
            .await
            .unwrap();
        assert!(!outcome.success);
        assert!(outcome.error_message.unwrap().contains("not-a-number"));
        assert!(
            driver.writes().is_empty(),
            "a rejected write must not be recorded"
        );
    }

    // --- error paths -------------------------------------------------------------------------

    #[tokio::test]
    async fn reading_an_unknown_tag_is_invalid_tag_value() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 0.0);
        let err = driver
            .read(&["SomeOtherTag".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            DriverError::InvalidTagValue { tag, .. } if tag == "SomeOtherTag"
        ));
    }

    #[tokio::test]
    async fn writing_an_unknown_tag_is_invalid_tag_value() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 0.0);
        let err = driver
            .write(&"SomeOtherTag".to_string(), TagWrite::Float(1.0))
            .await
            .unwrap_err();
        assert!(matches!(err, DriverError::InvalidTagValue { .. }));
    }

    #[tokio::test]
    async fn browse_is_unsupported() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 0.0);
        let err = driver
            .browse(BrowsePageRequest::root(20))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            DriverError::Unsupported {
                operation: "browse"
            }
        ));
    }

    #[tokio::test]
    async fn reading_pv_past_the_last_sample_is_operation_error_not_a_panic() {
        let driver = ReplayDriver::new("PV", "MV", samples(), 0.0);
        for _ in 0..3 {
            driver.read(&["PV".to_string()]).await.unwrap();
        }
        assert_eq!(driver.remaining(), 0);
        let err = driver.read(&["PV".to_string()]).await.unwrap_err();
        let exhausted = expect_trace_exhaustion(err);
        assert_eq!(exhausted.recorded, 3);
        assert_eq!(exhausted.attempted, 4);
        assert!(exhausted.to_string().contains("exhausted"));
    }

    #[tokio::test]
    async fn an_empty_trace_reports_exhaustion_on_the_very_first_read() {
        let driver = ReplayDriver::new("PV", "MV", Vec::new(), 0.0);
        let err = driver.read(&["PV".to_string()]).await.unwrap_err();
        let exhausted = expect_trace_exhaustion(err);
        assert_eq!(exhausted.recorded, 0);
        assert_eq!(exhausted.attempted, 1);
    }

    #[test]
    fn trace_exhaustion_assertion_fails_clearly_for_a_non_operation_error() {
        let panic = std::panic::catch_unwind(|| {
            expect_trace_exhaustion(DriverError::Unsupported {
                operation: "browse",
            })
        })
        .unwrap_err();
        assert!(
            panic
                .downcast_ref::<String>()
                .is_some_and(|message| message.contains("DriverError::Operation"))
        );
    }

    // --- from_fixture_json -------------------------------------------------------------------

    #[test]
    fn from_fixture_json_parses_ticks_and_ignores_every_other_field() {
        let json = r#"{
            "name": "example",
            "description": "irrelevant prose",
            "source": { "static_log": "x", "dynamic_log": "y", "captured": "2026-01-01" },
            "config": { "process_type": "flow", "controller_type": "pi" },
            "direction": "reverse",
            "initial": { "pv_ini": 1.0 },
            "pv_range": { "high": 100.0, "low": 0.0 },
            "template_name": "Yokogawa CentumVP",
            "ticks": [
                { "time": "2024-01-01T00:00:00Z", "pv": 40.0, "expected": { "hysteresis": 0.1 } },
                { "time": "2024-01-01T00:00:01Z", "pv": 41.5, "expected": { "hysteresis": 0.2 } }
            ]
        }"#;

        let driver = ReplayDriver::from_fixture_json("PV", "MV", json, 40.0).unwrap();
        assert_eq!(driver.remaining(), 2);
    }

    #[test]
    fn from_fixture_json_rejects_malformed_json_as_operation_error() {
        let err = ReplayDriver::from_fixture_json("PV", "MV", "not json", 0.0).unwrap_err();
        assert!(matches!(err, DriverError::Operation(_)));
    }

    #[test]
    fn from_fixture_json_rejects_a_document_with_no_ticks_field() {
        let err = ReplayDriver::from_fixture_json("PV", "MV", "{}", 0.0).unwrap_err();
        assert!(matches!(err, DriverError::Operation(_)));
    }

    #[test]
    fn trace_exhaustion_error_reports_the_recorded_and_attempted_counts() {
        let error = ReplayTraceExhausted {
            recorded: 3,
            attempted: 4,
        };
        assert!(error.to_string().contains("3 sample(s)"));
        assert!(error.to_string().contains("#4"));
        let _: Box<dyn std::error::Error> = Box::new(error);
    }

    // --- object safety -----------------------------------------------------------------------

    #[tokio::test]
    async fn is_usable_as_a_boxed_dyn_driver() {
        let driver: Box<dyn Driver> = Box::new(ReplayDriver::new("PV", "MV", samples(), 0.0));
        let read = driver.read(&["PV".to_string()]).await.unwrap();
        assert_eq!(read[0].value, "10");
    }

    /// End-to-end: a real `MrftEngine` (from `bhtune-core`) drives `ReplayDriver` through
    /// the actual `Driver` trait, fed from the *same* captured golden-master fixture
    /// `core-replay-harness` (`crates/bhtune-core/tests/golden_replay.rs`) already validates
    /// at the pure-engine level, and reaches the same final tuning result. This is
    /// deliberately not a re-run of that test's exhaustive per-tick assertions (hysteresis,
    /// `mv_sign_next_step`, cycle counters, ...) at every tick -- that would just duplicate
    /// already-proven engine correctness. What this test adds is proof that the *real* async
    /// `Driver` abstraction this trace is now served through -- tag lookup, `TagValue`/
    /// `TagWrite` conversions, the `timestamp` field carrying the tick time -- introduces no
    /// bugs of its own on top of an already-correct engine.
    #[tokio::test]
    async fn mrft_engine_replays_the_golden_trace_through_the_real_driver_trait() {
        use std::{fs, path::Path};

        use bhtune_core::{
            Action, ControllerDirection, ControllerType, InitialReadings, LoopConfig, MrftCompat,
            MrftEngine, ProcessType, PvRange, ResponseLevel, Tick, TuningMathCompat,
            built_in_templates, calculate_all, lookup,
        };

        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/golden/fixtures/flow_pi_direct.json");
        let json = fs::read_to_string(&fixture_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", fixture_path.display()));

        let pv_tag = "Loop.PV".to_string();
        let mv_tag = "Loop.MV".to_string();
        // This fixture's own `initial.mv_ini` (see `crates/bhtune-core/tests/
        // golden_replay.rs`'s `FixtureInitial`) -- kept as a literal here since this test
        // deliberately only parses `ticks[].time`/`ticks[].pv` via `from_fixture_json`, not
        // the fixture's other fields.
        let initial_mv = 40.0;
        let driver =
            ReplayDriver::from_fixture_json(pv_tag.clone(), mv_tag.clone(), &json, initial_mv)
                .expect("flow_pi_direct.json should parse");
        let total_samples = driver.remaining();

        // This fixture's own config/direction/initial-readings/pv-range, matching
        // `golden_replay.rs`'s hardcoded transcription of the same fixture exactly (that
        // test asserts the fixture's `config`/`direction` enums decode to these values, so
        // duplicating the literals here rather than re-parsing them is a deliberate,
        // already-covered redundancy, not a risk of silent drift).
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 2.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 3,
            mrft_delay_secs: 0,
        };
        config.validate().expect("fixture config must be valid");
        let direction = ControllerDirection::Reverse;
        let initial = InitialReadings {
            pv_ini: 40.00012,
            mv_ini: initial_mv,
            mv_range_low: 0.0,
            mv_range_high: 100.0,
        };
        let pv_range = PvRange {
            high: 100.0,
            low: 0.0,
        };
        let beta = lookup(
            config.process_type,
            config.controller_type,
            ResponseLevel::Aggressive,
        )
        .beta;
        let template = built_in_templates()
            .into_iter()
            .find(|t| t.name == "Yokogawa CentumVP")
            .expect("built-in template must exist");

        // The first read's timestamp is this trace's own start time -- read here (rather
        // than hardcoded) purely to seed `MrftEngine::new`, which needs a start time before
        // the first `step` call.
        let first = driver.read(std::slice::from_ref(&pv_tag)).await.unwrap();
        let start_time = first[0]
            .timestamp
            .expect("ReplayDriver always sets a timestamp on PV reads");
        let mut pending_tick = Some(Tick {
            time: start_time,
            pv: first[0].value.parse().unwrap(),
        });

        let mut engine = MrftEngine::new(
            config,
            direction,
            beta,
            initial,
            start_time,
            MrftCompat::default(),
        );

        let mut completion = None;
        for _ in 0..total_samples {
            let tick = match pending_tick.take() {
                Some(tick) => tick,
                None => {
                    let read = driver.read(std::slice::from_ref(&pv_tag)).await.unwrap();
                    let time = read[0]
                        .timestamp
                        .expect("ReplayDriver always sets a timestamp on PV reads");
                    Tick {
                        time,
                        pv: read[0].value.parse().unwrap(),
                    }
                }
            };

            for action in engine.step(tick) {
                match action {
                    Action::WriteMv(mv) => {
                        driver.write(&mv_tag, TagWrite::Float(mv)).await.unwrap();
                    }
                    Action::Complete {
                        peaks,
                        troughs,
                        switch_times,
                        mv_sign_init,
                    } => {
                        completion = Some((peaks, troughs, switch_times, mv_sign_init));
                    }
                }
            }
            if completion.is_some() {
                break;
            }
        }

        let (peaks, troughs, switch_times, mv_sign_init) =
            completion.expect("engine should complete within the recorded trace");

        let results = calculate_all(
            &peaks,
            &troughs,
            &switch_times,
            mv_sign_init,
            direction,
            config,
            pv_range,
            &template,
            TuningMathCompat::default(),
        );

        // The aggressive-response proportional band, in the fixture's own DCS units --
        // matching the same "PB=157.7" figure already confirmed control-theory-consistent
        // and recorded against this exact capture in AGENTS.md's `capture-traces` notes.
        // Same tolerance shape `golden_replay.rs` uses for its own final numbers -- see that
        // test for the full rationale (float rounding, and the period-truncation-bug-driven
        // `ti_minutes`/`integral` slack in particular, which is why this test doesn't also
        // re-check those two fields as tightly).
        let expected_aggressive_pb = 157.7088_f32;
        let (_tuning, pid) = results
            .iter()
            .find(|(r, _)| r.response_level == ResponseLevel::Aggressive)
            .expect("aggressive result must be present");
        let tolerance = 1e-3 + expected_aggressive_pb.abs() * 1e-2;
        assert!(
            (pid.proportional - expected_aggressive_pb).abs() <= tolerance,
            "aggressive proportional band: expected ~{expected_aggressive_pb}, got {} \
             (tolerance {tolerance})",
            pid.proportional
        );

        assert!(
            !driver.writes().is_empty(),
            "the engine should have written at least one relay step through the real \
             Driver trait"
        );
        // The engine stops driving reads the instant completion is observed (matching
        // `core-replay-harness`'s own "any remaining fixture ticks are exactly this harmless
        // trailing data and are not replayed" behavior -- see that test's comment), so this
        // trace's trailing padding ticks are expected to remain unconsumed; the meaningful
        // assertion is that real consumption happened at all, not that every recorded tick
        // was read.
        assert!(
            driver.remaining() < total_samples,
            "expected at least one sample to be consumed before completion"
        );
    }
}
