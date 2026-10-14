use super::*;
use crate::{
    constants::{ResponseLevel, lookup},
    controller_type::ControllerType,
    process_type::ProcessType,
};

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

fn t_ms(offset_ms: i64) -> DateTime<Utc> {
    DateTime::UNIX_EPOCH + Duration::milliseconds(offset_ms)
}

mod clamp_relay_amplitude_tests;

#[test]
fn action_multiplier_is_negative_one_for_direct() {
    let engine = MrftEngine::new(config(), ControllerDirection::Direct, 0.0, initial(), t(0));
    assert_eq!(engine.action_multiplier, -1);
}

#[test]
fn action_multiplier_is_positive_one_for_reverse() {
    let engine = MrftEngine::new(config(), ControllerDirection::Reverse, 0.0, initial(), t(0));
    assert_eq!(engine.action_multiplier, 1);
}

/// Full 3-switch run to completion, beta=0.3 (a realistic hysteresis multiplier),
/// Reverse action. Expected values cross-checked against an independent Python
/// transcription of the same C# formulas rather than hand-derived, since MRFT's
/// peak/trough/hysteresis interaction is easy to get subtly wrong by inspection alone.
#[test]
fn full_run_reverse_action_completes_with_expected_peaks_troughs_and_snap_back() {
    let mut engine = MrftEngine::new(config(), ControllerDirection::Reverse, 0.3, initial(), t(0));

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
                peaks: vec![55.0],
                troughs: vec![50.0, 45.0],
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
    let mut engine = MrftEngine::new(config(), ControllerDirection::Direct, 0.0, initial(), t(0));

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
                peaks: vec![50.0, 60.0],
                troughs: vec![40.0],
                switch_times: vec![t(1), t(2), t(3)],
                mv_sign_init: -1,
            },
        ]
    );
}

#[test]
fn run_7_like_trace_includes_switch_endpoints_in_recorded_extrema() {
    let config = LoopConfig {
        process_type: ProcessType::Flow,
        controller_type: ControllerType::Pi,
        relay_amp_percent: 10.0,
        num_cycles_skip: 1,
        num_cycles_count: 2,
        noise_protection_secs: 3,
        mrft_delay_secs: 0,
    };
    let initial = InitialReadings {
        pv_ini: 2.25,
        mv_ini: 50.0,
        mv_range_low: 12.0,
        mv_range_high: 100.0,
    };
    let beta = lookup(
        ProcessType::Flow,
        ControllerType::Pi,
        ResponseLevel::Aggressive,
    )
    .beta;
    let samples = [
        (0, 2.25),
        (2001, 2.49),
        (2999, 2.67),
        (3999, 2.72),
        (5999, 2.25),
        (6999, 1.89),
        (8999, 2.26),
        (10001, 2.61),
        (12002, 2.24),
        (12999, 1.89),
        (13999, 1.80),
        (15999, 2.25),
        (17999, 2.61),
        (19000, 2.24),
        (20000, 1.89),
        (21000, 1.80),
    ];
    let mut engine = MrftEngine::new(config, ControllerDirection::Reverse, beta, initial, t_ms(0));

    let completion = samples.iter().find_map(|(offset_ms, pv)| {
        engine
            .step(Tick {
                time: t_ms(*offset_ms),
                pv: *pv,
            })
            .into_iter()
            .find_map(|action| match action {
                Action::Complete {
                    peaks,
                    troughs,
                    switch_times,
                    mv_sign_init,
                } => Some((peaks, troughs, switch_times, mv_sign_init)),
                Action::WriteMv(_) => None,
            })
    });

    let (peaks, troughs, switch_times, mv_sign_init) =
        completion.expect("run 7-like trace must complete");
    assert_eq!(mv_sign_init, 1);
    assert_eq!(switch_times.len(), 5);
    assert_eq!(peaks, vec![2.72, 2.61, 2.61]);
    assert_eq!(troughs, vec![1.89, 1.80]);
}

#[test]
fn run_10_like_trace_keeps_the_final_switch_pv_as_a_trough() {
    let config = LoopConfig {
        process_type: ProcessType::PressureLine,
        controller_type: ControllerType::Pi,
        relay_amp_percent: 10.0,
        num_cycles_skip: 1,
        num_cycles_count: 2,
        noise_protection_secs: 3,
        mrft_delay_secs: 0,
    };
    let initial = InitialReadings {
        pv_ini: 189.94,
        mv_ini: 50.0,
        mv_range_low: 0.0,
        mv_range_high: 100.0,
    };
    let beta = lookup(
        ProcessType::PressureLine,
        ControllerType::Pi,
        ResponseLevel::Aggressive,
    )
    .beta;
    let samples = [
        (0, 189.96),
        (2001, 189.97),
        (3003, 181.07),
        (5989, 174.54),
        (6989, 187.07),
        (8989, 204.0),
        (10990, 204.43),
        (11988, 192.14),
        (12990, 182.73),
        (15990, 175.28),
        (17989, 197.09),
        (20003, 200.38),
        (20988, 189.04),
        (22991, 173.70),
        (24991, 174.22),
        (25990, 186.82),
        (27989, 203.86),
    ];
    let mut engine = MrftEngine::new(config, ControllerDirection::Reverse, beta, initial, t_ms(0));

    let completion = samples.iter().find_map(|(offset_ms, pv)| {
        engine
            .step(Tick {
                time: t_ms(*offset_ms),
                pv: *pv,
            })
            .into_iter()
            .find_map(|action| match action {
                Action::Complete {
                    peaks,
                    troughs,
                    switch_times,
                    mv_sign_init,
                } => Some((peaks, troughs, switch_times, mv_sign_init)),
                Action::WriteMv(_) => None,
            })
    });

    let (peaks, troughs, switch_times, mv_sign_init) =
        completion.expect("run 10-like trace must complete");
    assert_eq!(mv_sign_init, -1);
    assert_eq!(switch_times.len(), 5);
    assert_eq!(peaks, vec![204.43, 200.38]);
    assert_eq!(troughs, vec![174.54, 175.28, 173.70]);
}

#[test]
fn direct_action_hysteresis_uses_previous_sign_and_subtracts() {
    let mut engine = MrftEngine::new(config(), ControllerDirection::Direct, 0.3, initial(), t(0));

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
    let mut engine = MrftEngine::new(config, ControllerDirection::Reverse, 0.0, initial(), t(0));

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
                peaks: vec![60.0],
                troughs: vec![40.0, 40.0],
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
    let mut engine = MrftEngine::new(config, ControllerDirection::Reverse, 0.0, initial(), t(0));

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
    let mut engine = MrftEngine::new(config(), ControllerDirection::Reverse, 0.3, initial(), t(0));

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
