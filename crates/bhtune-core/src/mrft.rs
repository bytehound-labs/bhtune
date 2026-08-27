//! The MRFT (Modified Relay Feedback Test) engine: the pure, I/O-free relay-switching state
//! machine at the heart of bhtune. See `AGENTS.md`'s "Key architectural decisions" for why
//! this must never read a clock, perform network I/O, or touch a UI.
//!
//! Scope is deliberately narrow: this module decides *when to switch the MV and when the
//! test is complete*. It does not read or write OPC tags (`bhtune-driver`'s job) and it does
//! not calculate PID constants from the collected peaks/troughs (`core-tuning-math`'s job) —
//! it only hands them off via [`Action::Complete`].

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::{direction::ControllerDirection, loop_config::LoopConfig};

/// One PV sample fed into the engine. The engine has no clock of its own — every timestamp
/// it ever reasons about arrives through a `Tick`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Tick {
    pub time: DateTime<Utc>,
    pub pv: f32,
}

/// A side effect the caller must perform in response to a [`MrftEngine::step`] call.
///
/// Uses adjacent tagging (`{"kind": "...", "data": ...}`), like
/// [`crate::tags::TagOrValue`]: serde cannot internally tag a newtype variant holding a bare
/// number, which `WriteMv(f32)` is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Action {
    /// Write this value to the MV tag.
    WriteMv(f32),
    /// The test is complete. Carries everything `core-tuning-math` needs to calculate PID
    /// constants: the recorded peak/trough PV values, the timestamps of every switch after
    /// the skip period, and which direction the very first switch went.
    Complete {
        peaks: Vec<f32>,
        troughs: Vec<f32>,
        switch_times: Vec<DateTime<Utc>>,
        mv_sign_init: i8,
    },
}

/// Initial readings the engine needs at construction time, taken once before the relay test
/// starts (`ReadInitialOPCvalues` in the legacy app).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct InitialReadings {
    pub pv_ini: f32,
    pub mv_ini: f32,
    /// MV range floor (`MvMSL`). Zero for an uncascaded, 0-100% loop.
    pub mv_range_low: f32,
    /// MV range ceiling (`MvMSH`).
    pub mv_range_high: f32,
}

/// Legacy-bug replication flags, for bug-for-bug replay validation against captured legacy
/// traces (see `core-bug-register`). Every field defaults to `false`: the fixed, correct
/// behavior. Set a field `true` only to intentionally reproduce that specific legacy defect,
/// e.g. when asserting parity against a captured trace that has the bug baked in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MrftCompat {
    /// Replicates `CheckMVboundaries`'s lower-clamp bug: clamps to `mv_range_low + mv_ini`
    /// instead of the dimensionally-correct `mv_ini - mv_range_low`. Silently masked whenever
    /// `mv_range_low == 0` (the common 0-100% case); only visibly wrong for cascaded loops
    /// with a nonzero MV range floor.
    pub replicate_lower_clamp_bug: bool,
}

/// Computes the raw engineering-unit relay amplitude from a percentage of the MV range, and
/// clamps it so neither relay step would drive the MV outside `[mv_range_low,
/// mv_range_high]`. Pure port of `CheckMVboundaries`.
pub fn clamp_relay_amplitude(
    relay_amp_percent: f32,
    mv_ini: f32,
    mv_range_low: f32,
    mv_range_high: f32,
    compat: MrftCompat,
) -> f32 {
    let mut relay_amp_raw = relay_amp_percent / 100.0 * (mv_range_high - mv_range_low);

    if mv_ini + relay_amp_raw > mv_range_high {
        relay_amp_raw = mv_range_high - mv_ini;
    } else if mv_ini - relay_amp_raw < mv_range_low {
        relay_amp_raw = if compat.replicate_lower_clamp_bug {
            mv_range_low + mv_ini
        } else {
            mv_ini - mv_range_low
        };
    }

    relay_amp_raw
}

/// A snapshot of the engine's observable state after a `step()` call — the fields a
/// golden-master trace comparison checks against the legacy CSV log's per-tick columns
/// (`Hysteresis`, `MvValueCurrent`, `MvSignNextStep`, `CounterAllSwitches`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct MrftState {
    pub hysteresis: f32,
    pub mv_value_current: f32,
    pub mv_sign_next_step: i8,
    pub counter_all_switches: u32,
    /// Whole relay cycles completed so far. Mirrors `GetCyclesCompleted`.
    pub cycles_completed: i32,
    /// Whole relay cycles remaining. Mirrors `GetCyclesRemaining`; reaches `0` on the tick of
    /// the final switch, which is what triggers the snap-back to `mv_ini` instead of a full
    /// relay step.
    pub cycles_remaining: i32,
}

/// The MRFT relay-feedback state machine. Construct with [`MrftEngine::new`], then call
/// [`MrftEngine::step`] once per PV sample.
///
/// Deliberately has no knowledge of `--mrftDelayTime` pre/post padding: the caller decides
/// when to start calling `step()` at all, rather than the engine accepting an internal
/// "delay active" no-op flag. That keeps this type's only job "given a PV sample, decide
/// whether to switch" — nothing else.
#[derive(Debug, Clone)]
pub struct MrftEngine {
    // Fixed for the life of the engine.
    beta: f32,
    action_multiplier: i8,
    relay_amp_raw: f32,
    pv_value_ini: f32,
    mv_value_ini: f32,
    num_switches_skip: u32,
    num_cycles_count: u32,
    noise_protection_secs: u32,

    // Running state, updated by `step`.
    mv_value_current: f32,
    mv_value_next_step: f32,
    max_pv_cycle: f32,
    min_pv_cycle: f32,
    hysteresis: f32,
    mv_sign_next_step: i8,
    mv_sign_init: i8,
    time_previous_switch: DateTime<Utc>,
    counter_all_switches: u32,
    peaks: Vec<f32>,
    troughs: Vec<f32>,
    switch_times: Vec<DateTime<Utc>>,
    completed: bool,
}

impl MrftEngine {
    /// Builds a new engine and performs the one-time setup `MRFTstart` does before the
    /// relay-switching loop begins: resolves the action multiplier from `direction`, clamps
    /// the relay amplitude to the MV range, and seeds the peak/trough trackers from the
    /// initial PV reading.
    ///
    /// `start_time` seeds `time_previous_switch` (`TimePreviousSwitch = DateTime.Now` in the
    /// legacy `MRFTinitializeVariables`) — pass the timestamp of the first [`Tick`] that will
    /// be given to `step`, or whatever timestamp the caller considers "test start".
    ///
    /// `beta` is the hysteresis multiplier for this (process type, controller type)
    /// combination — see [`crate::constants::lookup`]. It is response-level-invariant, so
    /// any [`crate::constants::ResponseLevel`] passed to `lookup` yields the same `beta`.
    pub fn new(
        config: LoopConfig,
        direction: ControllerDirection,
        beta: f32,
        initial: InitialReadings,
        start_time: DateTime<Utc>,
        compat: MrftCompat,
    ) -> MrftEngine {
        let action_multiplier = direction.action_multiplier();

        let relay_amp_raw = clamp_relay_amplitude(
            config.relay_amp_percent,
            initial.mv_ini,
            initial.mv_range_low,
            initial.mv_range_high,
            compat,
        );

        MrftEngine {
            beta,
            action_multiplier,
            relay_amp_raw,
            pv_value_ini: initial.pv_ini,
            mv_value_ini: initial.mv_ini,
            num_switches_skip: config.num_cycles_skip * 2 + 1,
            num_cycles_count: config.num_cycles_count,
            noise_protection_secs: config.noise_protection_secs,

            mv_value_current: initial.mv_ini,
            mv_value_next_step: initial.mv_ini,
            max_pv_cycle: initial.pv_ini,
            min_pv_cycle: initial.pv_ini,
            hysteresis: 0.0,
            mv_sign_next_step: 1,
            mv_sign_init: 0,
            time_previous_switch: start_time,
            counter_all_switches: 0,
            peaks: Vec::new(),
            troughs: Vec::new(),
            switch_times: Vec::new(),
            completed: false,
        }
    }

    /// Feeds one PV sample through the engine. Returns the actions the caller must perform:
    /// zero or one [`Action::WriteMv`] (mirrors `MRFTswitchIsNeeded` + `MRFTperformSwitch`),
    /// followed by [`Action::Complete`] at most once, on the tick that satisfies the
    /// completion condition.
    ///
    /// Calling `step` again after `Complete` has been returned is a no-op that returns an
    /// empty `Vec` — the engine has nothing left to do.
    pub fn step(&mut self, tick: Tick) -> Vec<Action> {
        if self.completed {
            return Vec::new();
        }

        let mut actions = Vec::new();

        if self.switch_is_needed(tick) {
            actions.push(Action::WriteMv(self.perform_switch(tick)));
        }

        if self.is_complete() {
            self.completed = true;
            actions.push(Action::Complete {
                peaks: self.peaks.clone(),
                troughs: self.troughs.clone(),
                switch_times: self.switch_times.clone(),
                mv_sign_init: self.mv_sign_init,
            });
        }

        actions
    }

    /// A snapshot of the fields a golden-master comparison checks per tick.
    pub fn state(&self) -> MrftState {
        MrftState {
            hysteresis: self.hysteresis,
            mv_value_current: self.mv_value_current,
            mv_sign_next_step: self.mv_sign_next_step,
            counter_all_switches: self.counter_all_switches,
            cycles_completed: self.cycles_completed(),
            cycles_remaining: self.cycles_remaining(),
        }
    }

    /// Whether the completion condition has been reached. Pure port of `MRFTisComplete`.
    pub fn is_complete(&self) -> bool {
        self.counter_all_switches >= self.num_switches_skip + self.num_cycles_count * 2
    }

    /// Whole relay cycles completed so far. Pure port of `GetCyclesCompleted`. Integer
    /// division truncates toward zero (matching C#'s `int` division), so this is `0` for
    /// every tick before the first switch.
    fn cycles_completed(&self) -> i32 {
        (self.counter_all_switches as i32 - 1) / 2
    }

    /// Whole relay cycles remaining. Pure port of `GetCyclesRemaining`. Reaches exactly `0`
    /// on the tick of the final switch — see `perform_switch`'s snap-back to `mv_value_ini`.
    fn cycles_remaining(&self) -> i32 {
        (self.num_switches_skip as i32 + self.num_cycles_count as i32 * 2
            - self.counter_all_switches as i32
            + 1)
            / 2
    }

    /// Decides whether a switch is needed on this tick, updating peak/trough tracking,
    /// hysteresis, and the next-step sign/value along the way. Pure port of
    /// `MRFTswitchIsNeeded` — like the original, this both answers the question and mutates
    /// state that `perform_switch` depends on; it is not a side-effect-free predicate.
    fn switch_is_needed(&mut self, tick: Tick) -> bool {
        let sp_pv_diff = self.pv_value_ini - tick.pv;

        self.max_pv_cycle = self.max_pv_cycle.max(tick.pv);
        self.min_pv_cycle = self.min_pv_cycle.min(tick.pv);

        self.hysteresis = self.beta
            * (self.max_pv_cycle - self.pv_value_ini).max(self.pv_value_ini - self.min_pv_cycle);

        let mv_sign_previous: i8 = if self.mv_value_current >= self.mv_value_ini {
            1
        } else {
            -1
        };

        let valve_switch: f32;
        if self.action_multiplier == 1 {
            valve_switch = sp_pv_diff + mv_sign_previous as f32 * self.hysteresis;
            self.mv_sign_next_step = if valve_switch >= 0.0 { 1 } else { -1 };
        } else {
            valve_switch = sp_pv_diff - mv_sign_previous as f32 * self.hysteresis;
            self.mv_sign_next_step = if valve_switch <= 0.0 { 1 } else { -1 };
        }

        // Freezes at whatever mv_sign_next_step evaluates to on the tick of the very first
        // switch: this branch runs every tick while counter_all_switches is still 0, so it
        // keeps being overwritten right up until perform_switch increments the counter past
        // 0 on the tick the first switch actually happens.
        if self.counter_all_switches == 0 {
            self.mv_sign_init = self.mv_sign_next_step;
        }

        self.mv_value_next_step =
            self.mv_value_ini + self.mv_sign_next_step as f32 * self.relay_amp_raw;

        // Compared as f64, matching the legacy `Convert.ToDouble` widening before the
        // subtraction, to keep rounding behavior identical for values near the threshold.
        let mv_switch_required =
            (self.mv_value_next_step as f64 - self.mv_value_current as f64).abs() >= 0.01;

        let enable_mv_switch = self.time_previous_switch
            + Duration::seconds(self.noise_protection_secs as i64)
            <= tick.time
            || self.counter_all_switches == 0;

        mv_switch_required && enable_mv_switch
    }

    /// Performs a switch: advances the switch counters, records a peak or trough once the
    /// skip period has elapsed, and computes the new MV (snapping back to `mv_value_ini`
    /// instead of taking a full relay step on the final switch). Returns the new MV value to
    /// write.
    ///
    /// Pure port of `MRFTperformSwitch`, with the wall-clock re-read bug structurally
    /// impossible here: this reuses `tick.time`, the same timestamp `switch_is_needed` just
    /// reasoned about, rather than reading a fresh clock value.
    fn perform_switch(&mut self, tick: Tick) -> f32 {
        self.time_previous_switch = tick.time;

        self.counter_all_switches += 1;
        if self.counter_all_switches >= self.num_switches_skip {
            self.switch_times.push(tick.time);

            if self.mv_sign_next_step as i32 * self.action_multiplier as i32 == 1 {
                self.peaks.push(self.max_pv_cycle);
            } else {
                self.troughs.push(self.min_pv_cycle);
            }
        }

        self.max_pv_cycle = self.pv_value_ini;
        self.min_pv_cycle = self.pv_value_ini;

        self.mv_value_current = if self.cycles_remaining() == 0 {
            self.mv_value_ini
        } else {
            self.mv_value_next_step
        };

        self.mv_value_current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{controller_type::ControllerType, process_type::ProcessType};

    fn t(secs: i64) -> DateTime<Utc> {
        DateTime::UNIX_EPOCH + Duration::seconds(secs)
    }

    /// `relay_amp_percent=10` against a 0-100 MV range gives `relay_amp_raw=10`, unclamped.
    /// `noise_protection_secs` and cycle skip/count are overridden per test via `LoopConfig`
    /// spread syntax where a scenario needs something other than the `skip=0, count=1`
    /// default (which completes after exactly 3 total switches).
    fn config() -> LoopConfig {
        LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 10.0,
            num_cycles_skip: 0,
            num_cycles_count: 1,
            noise_protection_secs: 0,
            mrft_delay_secs: 0,
        }
    }

    fn initial() -> InitialReadings {
        InitialReadings {
            pv_ini: 50.0,
            mv_ini: 50.0,
            mv_range_low: 0.0,
            mv_range_high: 100.0,
        }
    }

    mod clamp_relay_amplitude_tests {
        use super::*;

        #[test]
        fn no_clamp_needed_when_within_range() {
            let amp = clamp_relay_amplitude(10.0, 50.0, 0.0, 100.0, MrftCompat::default());
            assert_eq!(amp, 10.0); // 10% of (100-0)
        }

        #[test]
        fn unclamped_amplitude_uses_the_mv_span() {
            let amp = clamp_relay_amplitude(10.0, 50.0, 20.0, 100.0, MrftCompat::default());
            assert_eq!(amp, 8.0); // 10% of (100-20)
        }

        #[test]
        fn upper_clamp_engages_near_ceiling() {
            let amp = clamp_relay_amplitude(10.0, 95.0, 0.0, 100.0, MrftCompat::default());
            assert_eq!(amp, 5.0); // mv_range_high - mv_ini
        }

        #[test]
        fn lower_clamp_uses_dimensionally_correct_formula_by_default() {
            // mv_ini=10, mv_range_low=5: naive 10% of (100-5) = 9.5 would drive MV to 0.5,
            // below the floor of 5, so it must clamp to mv_ini - mv_range_low = 5.
            let amp = clamp_relay_amplitude(10.0, 10.0, 5.0, 100.0, MrftCompat::default());
            assert_eq!(amp, 5.0);
        }

        #[test]
        fn lower_clamp_replicates_legacy_bug_when_compat_flag_set() {
            let compat = MrftCompat {
                replicate_lower_clamp_bug: true,
            };
            let amp = clamp_relay_amplitude(10.0, 10.0, 5.0, 100.0, compat);
            assert_eq!(amp, 15.0); // legacy: mv_range_low + mv_ini
        }

        #[test]
        fn fixed_and_buggy_formulas_agree_when_mv_range_low_is_zero() {
            // This is exactly why the legacy bug went unnoticed: with the common 0-100%
            // range, `mv_ini - 0` and `0 + mv_ini` are the same value.
            let fixed = clamp_relay_amplitude(50.0, 10.0, 0.0, 100.0, MrftCompat::default());
            let buggy = clamp_relay_amplitude(
                50.0,
                10.0,
                0.0,
                100.0,
                MrftCompat {
                    replicate_lower_clamp_bug: true,
                },
            );
            assert_eq!(fixed, buggy);
        }

        #[test]
        fn cascade_case_with_nonzero_floor() {
            // mv_ini=10, mv_msl=5, mv_msh=100: naive 10% of (100-5)=9.5 drives MV to 0.5,
            // below the floor, so the fixed clamp gives mv_ini - mv_range_low = 5.
            let amp = clamp_relay_amplitude(10.0, 10.0, 5.0, 100.0, MrftCompat::default());
            assert_eq!(amp, 5.0);
        }

        #[test]
        fn lower_clamp_boundary_uses_subtraction() {
            let relay_amp_percent = 100.0 * 1.1 / 91.0;
            let amp =
                clamp_relay_amplitude(relay_amp_percent, 10.0, 9.0, 100.0, MrftCompat::default());
            assert_eq!(amp, 1.0);
        }
    }

    #[test]
    fn action_multiplier_is_negative_one_for_direct() {
        let engine = MrftEngine::new(
            config(),
            ControllerDirection::Direct,
            0.0,
            initial(),
            t(0),
            MrftCompat::default(),
        );
        assert_eq!(engine.action_multiplier, -1);
    }

    #[test]
    fn action_multiplier_is_positive_one_for_reverse() {
        let engine = MrftEngine::new(
            config(),
            ControllerDirection::Reverse,
            0.0,
            initial(),
            t(0),
            MrftCompat::default(),
        );
        assert_eq!(engine.action_multiplier, 1);
    }

    /// Full 3-switch run to completion, beta=0.3 (a realistic hysteresis multiplier),
    /// Reverse action. Expected values cross-checked against an independent Python
    /// transcription of the same C# formulas (see the `core-mrft` task notes) rather than
    /// hand-derived, since MRFT's peak/trough/hysteresis interaction is easy to get subtly
    /// wrong by inspection alone.
    #[test]
    fn full_run_reverse_action_completes_with_expected_peaks_troughs_and_snap_back() {
        let mut engine = MrftEngine::new(
            config(),
            ControllerDirection::Reverse,
            0.3,
            initial(),
            t(0),
            MrftCompat::default(),
        );

        let actions = engine.step(Tick {
            time: t(1),
            pv: 55.0,
        });
        assert_eq!(actions, vec![Action::WriteMv(40.0)]);
        assert_eq!(
            engine.state(),
            MrftState {
                hysteresis: 1.5,
                mv_value_current: 40.0,
                mv_sign_next_step: -1,
                counter_all_switches: 1,
                cycles_completed: 0,
                cycles_remaining: 1,
            }
        );

        let actions = engine.step(Tick {
            time: t(2),
            pv: 45.0,
        });
        assert_eq!(actions, vec![Action::WriteMv(60.0)]);
        assert_eq!(
            engine.state(),
            MrftState {
                hysteresis: 1.5,
                mv_value_current: 60.0,
                mv_sign_next_step: 1,
                counter_all_switches: 2,
                cycles_completed: 0,
                cycles_remaining: 1,
            }
        );

        // Final switch: snaps MV back to mv_value_ini (50) instead of taking a full relay
        // step, and emits Complete in the same tick.
        let actions = engine.step(Tick {
            time: t(3),
            pv: 55.0,
        });
        assert_eq!(
            actions,
            vec![
                Action::WriteMv(50.0),
                Action::Complete {
                    peaks: vec![50.0],
                    troughs: vec![50.0, 50.0],
                    switch_times: vec![t(1), t(2), t(3)],
                    mv_sign_init: -1,
                },
            ]
        );
        assert_eq!(
            engine.state(),
            MrftState {
                hysteresis: 1.5,
                mv_value_current: 50.0,
                mv_sign_next_step: -1,
                counter_all_switches: 3,
                cycles_completed: 1,
                cycles_remaining: 0,
            }
        );
        assert!(engine.is_complete());
    }

    #[test]
    fn direct_action_mirrors_reverse_with_peaks_and_troughs_swapped() {
        let mut engine = MrftEngine::new(
            config(),
            ControllerDirection::Direct,
            0.0,
            initial(),
            t(0),
            MrftCompat::default(),
        );

        let mut last_actions = Vec::new();
        for (i, pv) in [40.0, 60.0, 40.0].into_iter().enumerate() {
            last_actions = engine.step(Tick {
                time: t(i as i64 + 1),
                pv,
            });
        }

        assert_eq!(
            last_actions,
            vec![
                Action::WriteMv(50.0),
                Action::Complete {
                    // Swapped vs. the Reverse scenario: Direct flips which sign counts as
                    // a peak vs. a trough (`MvSignNextStep * ActionMultiplier`).
                    peaks: vec![50.0, 50.0],
                    troughs: vec![50.0],
                    switch_times: vec![t(1), t(2), t(3)],
                    mv_sign_init: -1,
                },
            ]
        );
    }

    #[test]
    fn direct_action_hysteresis_uses_previous_sign_and_subtracts() {
        let mut engine = MrftEngine::new(
            config(),
            ControllerDirection::Direct,
            0.3,
            initial(),
            t(0),
            MrftCompat::default(),
        );

        assert_eq!(
            engine.step(Tick {
                time: t(1),
                pv: 40.0,
            }),
            vec![Action::WriteMv(40.0)]
        );
        assert_eq!(engine.mv_sign_init, -1);

        // Keep the previous sign at -1 while building a nonzero hysteresis from a
        // second sample after the switch.
        assert!(
            engine
                .step(Tick {
                    time: t(2),
                    pv: 40.0,
                })
                .is_empty()
        );

        // The previous MV is still below the initial MV, so its sign is -1. With
        // the subtraction and multiplication intact, the next relay target
        // remains 40 and no switch is needed.
        assert!(
            engine
                .step(Tick {
                    time: t(3),
                    pv: 49.0,
                })
                .is_empty()
        );
    }

    #[test]
    fn direct_hysteresis_switch_sign_uses_multiplication() {
        let mut engine = MrftEngine::new(
            LoopConfig {
                noise_protection_secs: 100,
                ..config()
            },
            ControllerDirection::Direct,
            0.3,
            initial(),
            t(0),
            MrftCompat::default(),
        );

        assert_eq!(
            engine.step(Tick {
                time: t(1),
                pv: 40.0,
            }),
            vec![Action::WriteMv(40.0)]
        );
        assert!(
            engine
                .step(Tick {
                    time: t(2),
                    pv: 60.0,
                })
                .is_empty()
        );
        assert!(
            engine
                .step(Tick {
                    time: t(3),
                    pv: 53.0,
                })
                .is_empty()
        );

        assert_eq!(engine.hysteresis, 3.0);
        assert_eq!(engine.mv_sign_next_step, 1);
        assert_eq!(engine.mv_value_next_step, 60.0);
    }

    /// With `num_cycles_skip=1`, the first `NumSwitchesSkip = 1*2+1 = 3` switches must not
    /// be recorded as peaks/troughs — only the switches after the skip period count toward
    /// the returned arrays, even though `cycles_completed`/`cycles_remaining` (a total
    /// skip+test progress indicator) advance from switch 1 onward.
    #[test]
    fn skip_cycles_are_excluded_from_recorded_peaks_and_troughs() {
        let config = LoopConfig {
            num_cycles_skip: 1,
            num_cycles_count: 1,
            ..config()
        };
        let mut engine = MrftEngine::new(
            config,
            ControllerDirection::Reverse,
            0.0,
            initial(),
            t(0),
            MrftCompat::default(),
        );

        let mut last_actions = Vec::new();
        for (i, pv) in [60.0, 40.0, 60.0, 40.0, 60.0].into_iter().enumerate() {
            last_actions = engine.step(Tick {
                time: t(i as i64 + 1),
                pv,
            });
        }

        assert_eq!(
            last_actions,
            vec![
                Action::WriteMv(50.0),
                Action::Complete {
                    peaks: vec![50.0],
                    troughs: vec![50.0, 50.0],
                    switch_times: vec![t(3), t(4), t(5)],
                    mv_sign_init: -1,
                },
            ]
        );
        assert_eq!(engine.state().counter_all_switches, 5);
    }

    /// A switch that becomes due too soon after the previous one must be suppressed until
    /// `noise_protection_secs` has elapsed, then fire on the tick it finally allows it
    /// (inclusive of the exact boundary).
    #[test]
    fn noise_protection_suppresses_and_then_allows_a_switch() {
        let config = LoopConfig {
            noise_protection_secs: 5,
            ..config()
        };
        let mut engine = MrftEngine::new(
            config,
            ControllerDirection::Reverse,
            0.0,
            initial(),
            t(0),
            MrftCompat::default(),
        );

        let actions = engine.step(Tick {
            time: t(1),
            pv: 60.0,
        });
        assert_eq!(actions, vec![Action::WriteMv(40.0)]);

        // Only 1s after the switch (needs 5s): must be suppressed even though a switch
        // would otherwise be required (mv_sign_next_step flips to 1 here).
        let actions = engine.step(Tick {
            time: t(2),
            pv: 40.0,
        });
        assert!(actions.is_empty());
        assert_eq!(engine.state().mv_sign_next_step, 1);
        assert_eq!(engine.state().counter_all_switches, 1);

        // Exactly 5s after the switch (the inclusive boundary): now allowed.
        let actions = engine.step(Tick {
            time: t(6),
            pv: 40.0,
        });
        assert_eq!(actions, vec![Action::WriteMv(60.0)]);
        assert_eq!(engine.state().counter_all_switches, 2);
    }

    #[test]
    fn step_after_completion_is_a_no_op() {
        let mut engine = MrftEngine::new(
            config(),
            ControllerDirection::Reverse,
            0.3,
            initial(),
            t(0),
            MrftCompat::default(),
        );

        for (i, pv) in [55.0, 45.0, 55.0].into_iter().enumerate() {
            engine.step(Tick {
                time: t(i as i64 + 1),
                pv,
            });
        }
        assert!(engine.is_complete());

        let actions = engine.step(Tick {
            time: t(100),
            pv: 0.0,
        });
        assert!(actions.is_empty());
    }

    #[test]
    fn tick_serde_round_trip() {
        let tick = Tick {
            time: t(42),
            pv: 12.5,
        };
        let json = serde_json::to_string(&tick).unwrap();
        let back: Tick = serde_json::from_str(&json).unwrap();
        assert_eq!(tick, back);
    }

    #[test]
    fn action_serde_round_trip() {
        for action in [
            Action::WriteMv(12.5),
            Action::Complete {
                peaks: vec![1.0, 2.0],
                troughs: vec![3.0],
                switch_times: vec![t(1), t(2)],
                mv_sign_init: 1,
            },
        ] {
            let json = serde_json::to_string(&action).unwrap();
            let back: Action = serde_json::from_str(&json).unwrap();
            assert_eq!(action, back);
        }
    }
}
