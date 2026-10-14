use super::*;
use crate::{process_type::ProcessType, template};

fn t(offset_secs: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::seconds(offset_secs)
}

fn flow_pi_config() -> LoopConfig {
    LoopConfig {
        process_type: ProcessType::Flow,
        controller_type: ControllerType::Pi,
        relay_amp_percent: 5.0,
        num_cycles_skip: 1,
        num_cycles_count: 2,
        noise_protection_secs: 3,
        mrft_delay_secs: 0,
    }
}

fn assert_approx(actual: f32, expected: f32, epsilon: f32) {
    let diff = (actual - expected).abs();
    let message = format!("expected {expected}, got {actual} (diff {diff})");
    assert!(diff <= epsilon, "{message}");
}

// --- measure_oscillation ---------------------------------------------------------------

/// Verified against an independent Python re-implementation of `TuningConstantsCalc`'s
/// period/frequency/amplitude math (`peaks=[52,49], troughs=[48], mv_sign_init=1,
/// direction=Reverse` => first_switch_is_peak, matching `perform_switch`'s convention).
#[test]
fn measure_oscillation_reverse_first_switch_is_peak() {
    let osc = measure_oscillation(
        &[999.0, 52.0, 48.0],
        &[50.0, 46.0],
        &[t(0), t(30), t(60), t(90), t(120)],
        1,
        ControllerDirection::Reverse,
        flow_pi_config(),
        PvRange {
            high: 100.0,
            low: 0.0,
        },
    );
    // period = (120s / 60) / 2 cycles = 1.0 minute
    assert_approx(osc.period_minutes, 1.0, 1e-6);
    // frequency = 2*pi / 1.0
    assert_approx(osc.frequency, 2.0 * std::f32::consts::PI, 1e-5);
    // peaks_sum excludes index 0 (999.0, a deliberately distinct junk value proving
    // exclusion): 52.0 + 48.0 = 100.0. troughs_sum = 50.0 + 46.0 = 96.0 (all of it).
    // pv_amp_raw = (100.0 - 96.0) / (2*2) = 1.0
    assert_approx(osc.pv_amp_raw, 1.0, 1e-5);
    // pv_amp_percent = 1.0 / (100-0) * 100 = 1.0
    assert_approx(osc.pv_amp_percent, 1.0, 1e-5);
}

/// Same shape but with the discriminant flipped to select the "troughs is long" branch:
/// `mv_sign_init=-1, direction=Reverse` => `mv_sign_init * action_multiplier == -1`, so the
/// first recorded switch is a trough.
#[test]
fn measure_oscillation_first_switch_is_trough() {
    let osc = measure_oscillation(
        &[52.0, 48.0],
        &[999.0, 50.0, 46.0],
        &[t(0), t(30), t(60), t(90), t(120)],
        -1,
        ControllerDirection::Reverse,
        flow_pi_config(),
        PvRange {
            high: 100.0,
            low: 0.0,
        },
    );
    // peaks_sum = 52.0 + 48.0 = 100.0 (all of it, short array). troughs_sum excludes
    // index 0 (999.0, junk): 50.0 + 46.0 = 96.0.
    // pv_amp_raw = (100.0 - 96.0) / 4 = 1.0
    assert_approx(osc.pv_amp_raw, 1.0, 1e-5);
}

/// Direct action flips which sign of `mv_sign_init` selects the "peak" branch, since
/// `action_multiplier` itself flips sign — matching `perform_switch`'s
/// `mv_sign_next_step * action_multiplier == 1` peak/trough discriminant.
#[test]
fn measure_oscillation_direct_action_flips_discriminant() {
    let osc = measure_oscillation(
        &[999.0, 52.0, 48.0],
        &[50.0, 46.0],
        &[t(0), t(30), t(60), t(90), t(120)],
        -1, // Direct's action_multiplier is -1, so mv_sign_init=-1 gives the same
        // product (+1) that mv_sign_init=1 gave with Reverse above.
        ControllerDirection::Direct,
        flow_pi_config(),
        PvRange {
            high: 100.0,
            low: 0.0,
        },
    );
    assert_approx(osc.pv_amp_raw, 1.0, 1e-5);
}

#[test]
#[should_panic(expected = "switch_times must have at least 2 entries")]
fn measure_oscillation_panics_on_too_few_switch_times() {
    measure_oscillation(
        &[52.0],
        &[48.0],
        &[t(0)], // only 1 entry, can't measure a period
        1,
        ControllerDirection::Reverse,
        flow_pi_config(),
        PvRange {
            high: 100.0,
            low: 0.0,
        },
    );
}

#[test]
#[should_panic(expected = "switch_times.len() must equal 2 * num_cycles_count + 1")]
fn measure_oscillation_panics_on_mismatched_switch_times_length() {
    measure_oscillation(
        &[52.0, 49.0],
        &[48.0],
        &[t(0), t(30), t(60), t(90)], // 4, not 5
        1,
        ControllerDirection::Reverse,
        flow_pi_config(),
        PvRange {
            high: 100.0,
            low: 0.0,
        },
    );
}

#[test]
#[should_panic(expected = "peaks.len() must be num_cycles_count + 1")]
fn measure_oscillation_panics_on_mismatched_peaks_length() {
    measure_oscillation(
        &[52.0], // should be length 3 (num_cycles_count + 1 = 2 + 1)
        &[48.0, 47.0],
        &[t(0), t(30), t(60), t(90), t(120)],
        1,
        ControllerDirection::Reverse,
        flow_pi_config(),
        PvRange {
            high: 100.0,
            low: 0.0,
        },
    );
}

/// A run lasting >= 24 hours uses its full duration rather than wrapping at 24 hours.
/// Verified against the independent Python oracle: 90000s total / 2 cycles => 750.0 minutes.
#[test]
fn measure_oscillation_period_includes_full_elapsed_time() {
    let switch_times = [t(0), t(20_000), t(40_000), t(60_000), t(90_000)];

    let oscillation = measure_oscillation(
        &[999.0, 52.0, 48.0],
        &[50.0, 46.0],
        &switch_times,
        1,
        ControllerDirection::Reverse,
        flow_pi_config(),
        PvRange {
            high: 100.0,
            low: 0.0,
        },
    );
    assert_approx(oscillation.period_minutes, 750.0, 1e-3);
}

/// A run lasting well under one second verifies that millisecond precision keeps short
/// periods meaningful for fast loops with short poll intervals;
/// `bhtune-cli/tests/e2e_simulator.rs` also exercises real switch times.
#[test]
fn measure_oscillation_keeps_sub_second_precision() {
    let t_ms =
        |offset_ms: i64| DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::milliseconds(offset_ms);
    // Same shape as `measure_oscillation_reverse_first_switch_is_peak` (2 cycles, first
    // switch is a peak), but 300ms total elapsed instead of 120s.
    let switch_times = [t_ms(0), t_ms(75), t_ms(150), t_ms(225), t_ms(300)];

    let osc = measure_oscillation(
        &[999.0, 52.0, 48.0],
        &[50.0, 46.0],
        &switch_times,
        1,
        ControllerDirection::Reverse,
        flow_pi_config(),
        PvRange {
            high: 100.0,
            low: 0.0,
        },
    );

    // period = (0.3s / 60) / 2 cycles = 0.0025 minutes -- small, but the bug this guards
    // against collapsed it all the way to exactly 0.0, not just imprecisely small.
    assert_approx(osc.period_minutes, 0.3 / 60.0 / 2.0, 1e-8);
    assert!(
        osc.period_minutes > 0.0,
        "a sub-second run's period must not collapse to exactly zero"
    );
}

#[test]
fn measure_oscillation_scales_amplitude_by_cycle_count() {
    let config = LoopConfig {
        num_cycles_count: 3,
        ..flow_pi_config()
    };
    let osc = measure_oscillation(
        &[999.0, 52.0, 52.0, 52.0],
        &[50.0, 50.0, 50.0],
        &[t(0), t(10), t(20), t(30), t(40), t(50), t(60)],
        1,
        ControllerDirection::Reverse,
        config,
        PvRange {
            high: 100.0,
            low: 0.0,
        },
    );
    assert_approx(osc.pv_amp_raw, 1.0, 1e-6);
}

#[test]
fn measure_oscillation_uses_nonzero_range_floor_in_amplitude_percent() {
    let osc = measure_oscillation(
        &[999.0, 52.0, 48.0],
        &[50.0, 46.0],
        &[t(0), t(30), t(60), t(90), t(120)],
        1,
        ControllerDirection::Reverse,
        flow_pi_config(),
        PvRange {
            high: 100.0,
            low: 20.0,
        },
    );
    assert_approx(osc.pv_amp_raw, 1.0, 1e-6);
    assert_approx(osc.pv_amp_percent, 1.25, 1e-6);
}

// --- calculate_tuning_result -------------------------------------------------------------

/// Verified against the independent Python oracle for Flow/PI/Aggressive
/// (c1=0.451, c2=0.331, c3=0.0, from `constants::lookup`).
#[test]
fn calculate_tuning_result_flow_pi_aggressive() {
    let osc = Oscillation {
        period_minutes: 1.0,
        frequency: 2.0 * std::f32::consts::PI,
        pv_amp_raw: 0.25,
        pv_amp_percent: 0.25,
    };
    let result = calculate_tuning_result(osc, flow_pi_config(), ResponseLevel::Aggressive);
    // kp = 0.451 * 4 * 5.0 / (pi * 0.25) = 9.02 / 0.7853981... = 11.4854...
    assert_approx(result.kp, 11.4854, 1e-2);
    // ti_minutes = 0.331 * 2*pi / (2*pi) = 0.331
    assert_approx(result.ti_minutes, 0.331, 1e-5);
    // td_minutes = 0.0 (c3 is 0 for PI)
    assert_approx(result.td_minutes, 0.0, 1e-6);
}

/// `c2`/`c3` don't vary by response level, so `ti_minutes`/`td_minutes` must be identical
/// across all three levels for the same `Oscillation` — only `kp` (driven by `c1`) varies.
#[test]
fn calculate_tuning_result_ti_td_invariant_across_response_levels() {
    let osc = Oscillation {
        period_minutes: 1.0,
        frequency: 2.0 * std::f32::consts::PI,
        pv_amp_raw: 0.25,
        pv_amp_percent: 0.25,
    };
    let config = flow_pi_config();
    let aggressive = calculate_tuning_result(osc, config, ResponseLevel::Aggressive);
    let moderate = calculate_tuning_result(osc, config, ResponseLevel::Moderate);
    let sluggish = calculate_tuning_result(osc, config, ResponseLevel::Sluggish);

    assert_eq!(aggressive.ti_minutes, moderate.ti_minutes);
    assert_eq!(moderate.ti_minutes, sluggish.ti_minutes);
    assert_eq!(aggressive.td_minutes, moderate.td_minutes);
    assert!(aggressive.kp > moderate.kp);
    assert!(moderate.kp > sluggish.kp);
}

#[test]
fn calculate_tuning_result_pid_derivative_uses_frequency_division() {
    let osc = Oscillation {
        period_minutes: 1.0,
        frequency: 3.0,
        pv_amp_raw: 1.0,
        pv_amp_percent: 1.0,
    };
    let config = LoopConfig {
        process_type: ProcessType::TemperatureMixing,
        controller_type: ControllerType::Pid,
        ..flow_pi_config()
    };
    let result = calculate_tuning_result(osc, config, ResponseLevel::Moderate);
    let expected = 0.14 * 2.0 * std::f32::consts::PI / 3.0;
    assert_approx(result.td_minutes, expected, 1e-6);
}

// --- calculate_pid_parameters -------------------------------------------------------------

fn sample_result() -> TuningResult {
    TuningResult {
        response_level: ResponseLevel::Moderate,
        kp: 2.0,
        ti_minutes: 4.0,
        td_minutes: 0.5,
    }
}

#[test]
fn proportional_gain_passes_through_kp_unchanged() {
    let mut template = template::built_in_templates().remove(1); // Honeywell: Kp
    template.proportional_type = ProportionalType::Gain;
    let pid = calculate_pid_parameters(sample_result(), &template);
    assert_approx(pid.proportional, 2.0, 1e-6);
}

#[test]
fn proportional_band_is_100_over_kp() {
    let mut template = template::built_in_templates().remove(0); // Yokogawa: PB
    template.proportional_type = ProportionalType::Band;
    let pid = calculate_pid_parameters(sample_result(), &template);
    assert_approx(pid.proportional, 50.0, 1e-6); // 100 / 2.0
}

#[test]
fn reset_time_minutes_passes_through_unchanged() {
    let mut template = template::built_in_templates().remove(1);
    template.integral_type = IntegralType::ResetTime;
    template.integral_unit = TimeUnit::Minutes;
    let pid = calculate_pid_parameters(sample_result(), &template);
    assert_approx(pid.integral, 4.0, 1e-6);
}

#[test]
fn reset_time_seconds_converts_minutes_to_seconds() {
    let mut template = template::built_in_templates().remove(1);
    template.integral_type = IntegralType::ResetTime;
    template.integral_unit = TimeUnit::Seconds;
    let pid = calculate_pid_parameters(sample_result(), &template);
    assert_approx(pid.integral, 240.0, 1e-4); // 4.0 * 60
}

/// The order-of-operations subtlety: unit conversion happens *before* the reset-gain
/// transform, so with `integral_unit = Seconds`, Ki is computed from Ti-in-seconds
/// (`kp / (ti_minutes * 60)`), not from Ti-in-minutes (`kp / ti_minutes`) — these give
/// very different numeric results (2.0/240.0 = 0.008333... vs 2.0/4.0 = 0.5), so this
/// test would fail if the conversion order were swapped.
#[test]
fn reset_gain_uses_the_already_unit_converted_integral_time() {
    let mut template = template::built_in_templates().remove(1);
    template.integral_type = IntegralType::ResetGain;
    template.integral_unit = TimeUnit::Seconds;
    let pid = calculate_pid_parameters(sample_result(), &template);
    assert_approx(pid.integral, 2.0 / 240.0, 1e-6);
}

#[test]
fn reset_rate_is_reciprocal_of_unit_converted_integral_time() {
    let mut template = template::built_in_templates().remove(1);
    template.integral_type = IntegralType::ResetRate;
    template.integral_unit = TimeUnit::Minutes;
    let pid = calculate_pid_parameters(sample_result(), &template);
    assert_approx(pid.integral, 1.0 / 4.0, 1e-6);
}

#[test]
fn derivative_time_seconds_converts_minutes_to_seconds() {
    let mut template = template::built_in_templates().remove(1);
    template.derivative_type = DerivativeType::DerivativeTime;
    template.derivative_unit = TimeUnit::Seconds;
    let pid = calculate_pid_parameters(sample_result(), &template);
    assert_approx(pid.derivative, 30.0, 1e-4); // 0.5 * 60
}

#[test]
fn derivative_gain_uses_the_already_unit_converted_derivative_time() {
    let mut template = template::built_in_templates().remove(1);
    template.derivative_type = DerivativeType::DerivativeGain;
    template.derivative_unit = TimeUnit::Seconds;
    let pid = calculate_pid_parameters(sample_result(), &template);
    assert_approx(pid.derivative, 2.0 * 30.0, 1e-4); // kp * (0.5*60)
}

// --- opc_write_values ----------------------------------------------------------------

fn sample_pid() -> PidParameters {
    PidParameters {
        response_level: ResponseLevel::Moderate,
        proportional: 2.0,
        integral: 0.25,
        derivative: 1.5,
    }
}

#[test]
fn pi_and_pid_write_the_real_integral_value() {
    for controller_type in [ControllerType::Pi, ControllerType::Pid] {
        let values = opc_write_values(sample_pid(), controller_type, IntegralType::ResetTime);
        assert_approx(values.integral, 0.25, 1e-6);
    }
}

#[test]
fn p_only_reset_time_writes_9999_sentinel_for_integral() {
    let values = opc_write_values(sample_pid(), ControllerType::P, IntegralType::ResetTime);
    assert_approx(values.integral, 9999.0, 1e-6);
}

#[test]
fn p_only_reset_rate_or_gain_writes_zero_for_integral() {
    for integral_type in [IntegralType::ResetRate, IntegralType::ResetGain] {
        let values = opc_write_values(sample_pid(), ControllerType::P, integral_type);
        assert_approx(values.integral, 0.0, 1e-6);
    }
}

#[test]
fn only_pid_writes_the_real_derivative_value() {
    let values = opc_write_values(sample_pid(), ControllerType::Pid, IntegralType::ResetTime);
    assert_approx(values.derivative, 1.5, 1e-6);
}

#[test]
fn p_and_pi_write_zero_for_derivative() {
    for controller_type in [ControllerType::P, ControllerType::Pi] {
        let values = opc_write_values(sample_pid(), controller_type, IntegralType::ResetTime);
        assert_approx(values.derivative, 0.0, 1e-6);
    }
}

#[test]
fn opc_write_values_always_passes_through_proportional_and_response_level() {
    let values = opc_write_values(sample_pid(), ControllerType::P, IntegralType::ResetTime);
    assert_approx(values.proportional, 2.0, 1e-6);
    assert_eq!(values.response_level, ResponseLevel::Moderate);
}

#[test]
fn controller_pid_rounds_only_active_final_unit_values() {
    for mut template in template::built_in_templates() {
        let raw = calculate_pid_parameters(sample_result(), &template);
        let target = controller_pid_values(raw, ControllerType::Pid, &template).unwrap();
        assert_eq!(target.response_level, raw.response_level);
        for (value, rounded) in [
            (raw.proportional, &target.proportional),
            (raw.integral, &target.integral),
            (raw.derivative, &target.derivative),
        ] {
            assert_eq!(template.pid_rounding.round_value(value).unwrap(), *rounded);
        }

        template.integral_type = IntegralType::ResetGain;
        template.derivative_type = DerivativeType::DerivativeGain;
        template.pid_rounding = PidRounding::default();
        let raw = calculate_pid_parameters(sample_result(), &template);
        let target = controller_pid_values(raw, ControllerType::Pid, &template).unwrap();
        assert_eq!(
            target.integral,
            template.pid_rounding.round_value(raw.integral).unwrap()
        );
        assert_eq!(
            target.derivative,
            template.pid_rounding.round_value(raw.derivative).unwrap()
        );
    }
}

#[test]
fn controller_pid_preserves_disabled_9999_and_zero_sentinels() {
    for mut template in template::built_in_templates() {
        for integral_type in [
            IntegralType::ResetTime,
            IntegralType::ResetRate,
            IntegralType::ResetGain,
        ] {
            template.integral_type = integral_type;
            let target = controller_pid_values(sample_pid(), ControllerType::P, &template).unwrap();
            let sentinel = if integral_type == IntegralType::ResetTime {
                9999.0
            } else {
                0.0
            };
            assert_eq!(target.integral.value, sentinel);
            assert_eq!(target.integral.display.parse::<f32>().unwrap(), sentinel);
            assert!(!target.integral.display.contains("10000"));
            assert_eq!(target.derivative.value, 0.0);

            let target =
                controller_pid_values(sample_pid(), ControllerType::Pi, &template).unwrap();
            assert_eq!(target.derivative.value, 0.0);
            assert_eq!(target.derivative.display.parse::<f32>().unwrap(), 0.0);
        }
    }
}

#[test]
fn controller_pid_rejects_active_terms_erased_by_rounding() {
    let template = template::built_in_templates().remove(0);
    for (pid, label) in [
        (
            PidParameters {
                proportional: 0.04,
                ..sample_pid()
            },
            "proportional",
        ),
        (
            PidParameters {
                integral: 0.04,
                ..sample_pid()
            },
            "integral",
        ),
        (
            PidParameters {
                derivative: 0.04,
                ..sample_pid()
            },
            "derivative",
        ),
    ] {
        let error = controller_pid_values(pid, ControllerType::Pid, &template).unwrap_err();
        assert!(error.to_string().contains(label));
        assert!(error.to_string().contains("zero"));
        assert!(std::error::Error::source(&error).is_none());
    }
    for value in [0.0, -1.0] {
        let error = controller_pid_values(
            PidParameters {
                proportional: value,
                ..sample_pid()
            },
            ControllerType::Pid,
            &template,
        )
        .unwrap_err();
        assert!(error.to_string().contains("positive"));
    }
}

#[test]
fn controller_pid_rejects_invalid_precision_and_non_finite_targets() {
    let mut template = template::built_in_templates().remove(0);
    let error = controller_pid_values(
        PidParameters {
            proportional: f32::INFINITY,
            ..sample_pid()
        },
        ControllerType::Pid,
        &template,
    )
    .unwrap_err();
    assert!(error.to_string().contains("finite"));
    template.pid_rounding.digits = 8;
    assert!(controller_pid_values(sample_pid(), ControllerType::P, &template).is_err());
}

// --- calculate_all + integration with a real MrftEngine run -----------------------------

#[test]
fn calculate_all_produces_three_distinct_kp_with_shared_ti_td() {
    let template = template::built_in_templates().remove(1); // Honeywell: Kp, Ti/Td minutes
    let results = calculate_all(
        &[999.0, 52.0, 48.0],
        &[50.0, 46.0],
        &[t(0), t(30), t(60), t(90), t(120)],
        1,
        ControllerDirection::Reverse,
        flow_pi_config(),
        PvRange {
            high: 100.0,
            low: 0.0,
        },
        &template,
    );

    assert_eq!(results[0].1.response_level, ResponseLevel::Aggressive);
    assert_eq!(results[1].1.response_level, ResponseLevel::Moderate);
    assert_eq!(results[2].1.response_level, ResponseLevel::Sluggish);
    assert!(results[0].1.proportional > results[1].1.proportional);
    assert!(results[1].1.proportional > results[2].1.proportional);
    // Ti (here: `integral`, since Honeywell uses ResetTime/Minutes) is shared.
    assert_eq!(results[0].1.integral, results[1].1.integral);
    assert_eq!(results[1].1.integral, results[2].1.integral);
    // The intermediate TuningResult (Kp/Ti/Td) is also present alongside PidParameters.
    assert_eq!(results[0].0.response_level, ResponseLevel::Aggressive);
    assert!(results[0].0.kp > 0.0);
}

fn checked_fixture(
    peaks: &[f32],
    troughs: &[f32],
    switch_times: &[DateTime<Utc>],
    config: LoopConfig,
    template: &DcsTemplate,
) -> [CheckedTuningResult; 3] {
    calculate_all_checked(
        peaks,
        troughs,
        switch_times,
        1,
        ControllerDirection::Reverse,
        config,
        PvRange {
            high: 100.0,
            low: 0.0,
        },
        template,
    )
}

#[test]
fn checked_calculation_rejects_zero_pv_amplitude_for_every_response_level() {
    let config = flow_pi_config();
    let template = template::built_in_templates().remove(0);
    let results = checked_fixture(
        &[2.25, 2.25, 2.25],
        &[2.25, 2.25],
        &[t(0), t(30), t(60), t(90), t(120)],
        config,
        &template,
    );

    for result in results {
        assert_eq!(result.status, TuningResultStatus::Invalid);
        assert_eq!(
            result.invalid_reason,
            Some(TuningResultInvalidReason::NonPositivePvAmplitude)
        );
        assert!(result.tuning.is_none());
        assert!(result.pid.is_none());
        assert!(result.usable_values().is_none());
    }
}

#[test]
fn checked_calculation_rejects_negative_pv_amplitude() {
    let config = flow_pi_config();
    let template = template::built_in_templates().remove(0);
    let results = checked_fixture(
        &[1.0, 1.0, 1.0],
        &[2.0, 2.0],
        &[t(0), t(30), t(60), t(90), t(120)],
        config,
        &template,
    );

    assert!(results.iter().all(|result| {
        result.status == TuningResultStatus::Invalid
            && result.invalid_reason == Some(TuningResultInvalidReason::NonPositivePvAmplitude)
    }));
}

#[test]
fn checked_calculation_rejects_non_finite_pv_amplitude() {
    let config = flow_pi_config();
    let template = template::built_in_templates().remove(0);
    let results = checked_fixture(
        &[1.0, f32::NAN, 1.0],
        &[0.0, 0.0],
        &[t(0), t(30), t(60), t(90), t(120)],
        config,
        &template,
    );

    assert!(results.iter().all(|result| {
        result.status == TuningResultStatus::Invalid
            && result.invalid_reason == Some(TuningResultInvalidReason::NonFinitePvAmplitude)
    }));
}

#[test]
fn checked_calculation_rejects_a_zero_period_before_non_finite_frequency() {
    let config = flow_pi_config();
    let template = template::built_in_templates().remove(0);
    let results = checked_fixture(
        &[1.0, 2.0, 1.5],
        &[0.0, 0.0],
        &[t(0), t(0), t(0), t(0), t(0)],
        config,
        &template,
    );

    assert!(results.iter().all(|result| {
        result.status == TuningResultStatus::Invalid
            && result.invalid_reason == Some(TuningResultInvalidReason::NonPositivePeriod)
    }));
}

#[test]
fn checked_calculation_accepts_finite_zero_terms_for_a_p_only_result() {
    let config = LoopConfig {
        controller_type: ControllerType::P,
        ..flow_pi_config()
    };
    let mut template = template::built_in_templates().remove(0);
    template.integral_type = IntegralType::ResetTime;
    let results = checked_fixture(
        &[60.0, 52.0, 48.0],
        &[50.0, 46.0],
        &[t(0), t(30), t(60), t(90), t(120)],
        config,
        &template,
    );

    for result in results {
        assert_eq!(result.status, TuningResultStatus::Valid);
        assert!(result.invalid_reason.is_none());
        let (tuning, pid) = result.usable_values().expect("result should be usable");
        assert!(tuning.kp.is_finite() && tuning.kp > 0.0);
        assert!(pid.proportional.is_finite() && pid.proportional > 0.0);
        assert_eq!(pid.integral, 0.0);
        assert_eq!(pid.derivative, 0.0);
    }
}

#[test]
fn checked_calculation_rejects_non_finite_kp() {
    let mut config = flow_pi_config();
    config.relay_amp_percent = f32::INFINITY;
    let template = template::built_in_templates().remove(0);
    let results = checked_fixture(
        &[60.0, 52.0, 48.0],
        &[50.0, 46.0],
        &[t(0), t(30), t(60), t(90), t(120)],
        config,
        &template,
    );

    assert!(results.iter().all(|result| {
        result.status == TuningResultStatus::Invalid
            && result.invalid_reason == Some(TuningResultInvalidReason::NonFiniteKp)
    }));
}

#[test]
fn invalid_result_reasons_have_stable_operator_messages() {
    let cases = [
        (
            TuningResultInvalidReason::NonFinitePvAmplitude,
            "PV amplitude is not finite",
        ),
        (
            TuningResultInvalidReason::NonPositivePvAmplitude,
            "PV amplitude is not positive",
        ),
        (
            TuningResultInvalidReason::NonFinitePeriod,
            "oscillation period is not finite",
        ),
        (
            TuningResultInvalidReason::NonPositivePeriod,
            "oscillation period is not positive",
        ),
        (
            TuningResultInvalidReason::NonFiniteFrequency,
            "oscillation frequency is not finite",
        ),
        (
            TuningResultInvalidReason::NonPositiveFrequency,
            "oscillation frequency is not positive",
        ),
        (TuningResultInvalidReason::NonFiniteKp, "Kp is not finite"),
        (
            TuningResultInvalidReason::NonFiniteTiMinutes,
            "Ti is not finite",
        ),
        (
            TuningResultInvalidReason::NonFiniteTdMinutes,
            "Td is not finite",
        ),
        (
            TuningResultInvalidReason::NonFiniteProportional,
            "proportional value is not finite",
        ),
        (
            TuningResultInvalidReason::NonFiniteIntegral,
            "integral value is not finite",
        ),
        (
            TuningResultInvalidReason::NonFiniteDerivative,
            "derivative value is not finite",
        ),
    ];

    for (reason, expected) in cases {
        assert_eq!(reason.to_string(), expected);
    }
}

#[test]
fn checked_calculation_classifies_every_oscillation_guard() {
    let valid = Oscillation {
        period_minutes: 1.0,
        frequency: 1.0,
        pv_amp_raw: 1.0,
        pv_amp_percent: 1.0,
    };
    let cases = [
        (
            Oscillation {
                pv_amp_raw: f32::NAN,
                ..valid
            },
            Some(TuningResultInvalidReason::NonFinitePvAmplitude),
        ),
        (
            Oscillation {
                pv_amp_raw: 0.0,
                ..valid
            },
            Some(TuningResultInvalidReason::NonPositivePvAmplitude),
        ),
        (
            Oscillation {
                period_minutes: f32::NAN,
                ..valid
            },
            Some(TuningResultInvalidReason::NonFinitePeriod),
        ),
        (
            Oscillation {
                period_minutes: 0.0,
                ..valid
            },
            Some(TuningResultInvalidReason::NonPositivePeriod),
        ),
        (
            Oscillation {
                frequency: f32::NAN,
                ..valid
            },
            Some(TuningResultInvalidReason::NonFiniteFrequency),
        ),
        (
            Oscillation {
                frequency: 0.0,
                ..valid
            },
            Some(TuningResultInvalidReason::NonPositiveFrequency),
        ),
        (valid, None),
    ];

    for (oscillation, expected) in cases {
        assert_eq!(invalid_oscillation_reason(oscillation), expected);
    }
}

fn sample_tuning() -> TuningResult {
    TuningResult {
        response_level: ResponseLevel::Moderate,
        kp: 2.0,
        ti_minutes: 4.0,
        td_minutes: 0.5,
    }
}

fn assert_invalid_checked_result(
    tuning: TuningResult,
    template: &DcsTemplate,
    reason: TuningResultInvalidReason,
) {
    let result = checked_result_from_tuning(tuning, template);
    assert_eq!(result.status, TuningResultStatus::Invalid);
    assert_eq!(result.invalid_reason, Some(reason));
    assert!(result.tuning.is_none());
    assert!(result.pid.is_none());
}

#[test]
fn checked_calculation_classifies_every_calculated_value_guard() {
    let gain_template = template::built_in_templates().remove(1);
    assert_invalid_checked_result(
        TuningResult {
            kp: f32::INFINITY,
            ..sample_tuning()
        },
        &gain_template,
        TuningResultInvalidReason::NonFiniteKp,
    );
    assert_invalid_checked_result(
        TuningResult {
            ti_minutes: f32::INFINITY,
            ..sample_tuning()
        },
        &gain_template,
        TuningResultInvalidReason::NonFiniteTiMinutes,
    );
    assert_invalid_checked_result(
        TuningResult {
            td_minutes: f32::INFINITY,
            ..sample_tuning()
        },
        &gain_template,
        TuningResultInvalidReason::NonFiniteTdMinutes,
    );

    let band_template = template::built_in_templates().remove(0);
    assert_invalid_checked_result(
        TuningResult {
            kp: 0.0,
            ..sample_tuning()
        },
        &band_template,
        TuningResultInvalidReason::NonFiniteProportional,
    );

    let mut reset_rate_template = gain_template.clone();
    reset_rate_template.integral_type = IntegralType::ResetRate;
    assert_invalid_checked_result(
        TuningResult {
            ti_minutes: 0.0,
            ..sample_tuning()
        },
        &reset_rate_template,
        TuningResultInvalidReason::NonFiniteIntegral,
    );

    let mut derivative_gain_template = gain_template;
    derivative_gain_template.derivative_type = DerivativeType::DerivativeGain;
    assert_invalid_checked_result(
        TuningResult {
            kp: f32::MAX,
            td_minutes: f32::MAX,
            ..sample_tuning()
        },
        &derivative_gain_template,
        TuningResultInvalidReason::NonFiniteDerivative,
    );

    let valid = checked_result_from_tuning(sample_tuning(), &template::built_in_templates()[1]);
    assert_eq!(valid.status, TuningResultStatus::Valid);
    assert!(valid.usable_values().is_some());
}

/// Fields unpacked from a real engine's `Action::Complete`, named here purely to keep
/// clippy's `type_complexity` lint happy for the one test that needs them.
type CompletionFields = (Vec<f32>, Vec<f32>, Vec<DateTime<Utc>>, i8);

/// End-to-end: run a real `MrftEngine` to completion and feed its `Action::Complete`
/// straight into `calculate_all`, proving the two modules compose without needing any
/// glue beyond what `Action::Complete` already carries.
#[test]
fn calculate_all_consumes_a_real_mrft_engine_completion() {
    use crate::mrft::{Action, InitialReadings, MrftEngine, Tick};
    use std::collections::VecDeque;

    let config = flow_pi_config();
    let tc = lookup(
        config.process_type,
        config.controller_type,
        ResponseLevel::Aggressive,
    );
    let initial = InitialReadings {
        pv_ini: 50.0,
        mv_ini: 50.0,
        mv_range_low: 0.0,
        mv_range_high: 100.0,
    };
    let mut engine = MrftEngine::new(config, ControllerDirection::Reverse, tc.beta, initial, t(0));

    // A minimal first-order-plus-dead-time process reacting to the engine's own
    // relay-driven MV: pv exponentially approaches a target set by a *delayed* view of
    // whatever MV the engine last wrote (gain 1.0, i.e. mv_ini +/- relay_amp_raw maps
    // directly to the pv target). The dead time is essential: without it, the relay's
    // own hysteresis is computed from the same tick's pv that decides the switch, so a
    // memoryless (zero-delay) process causes it to flip every single tick (chattering),
    // and every recorded "peak"/"trough" degenerates to exactly `pv_ini` (never having
    // had more than one, wrongly-signed sample to update the tracked extremum before the
    // next reset). Five ticks of dead time plus a mild lag reproduces genuine relay
    // feedback behavior — a real, sustained square-wave-driven oscillation with distinct
    // peaks/troughs — matching the classic Åström-Hägglund relay auto-tuning method,
    // which fundamentally relies on process phase lag to sustain oscillation.
    const DELAY_TICKS: usize = 5;
    const LAG: f32 = 0.2;
    let mut pv = initial.pv_ini;
    let mut mv_value_current = initial.mv_ini;
    let mut mv_history: VecDeque<f32> = std::iter::repeat_n(initial.mv_ini, DELAY_TICKS).collect();
    let mut completion: Option<CompletionFields> = None;
    for i in 1..=200 {
        let delayed_mv = *mv_history.front().expect("fixed-size, never empty");
        let target = initial.pv_ini + (delayed_mv - initial.mv_ini);
        pv += (target - pv) * LAG;
        mv_history.pop_front();
        mv_history.push_back(mv_value_current);

        let actions = engine.step(Tick { time: t(i), pv });
        for action in actions {
            match action {
                Action::WriteMv(new_mv) => mv_value_current = new_mv,
                Action::Complete {
                    peaks,
                    troughs,
                    switch_times,
                    mv_sign_init,
                } => completion = Some((peaks, troughs, switch_times, mv_sign_init)),
            }
        }
        if completion.is_some() {
            break;
        }
    }

    let (peaks, troughs, switch_times, mv_sign_init) =
        completion.expect("engine should complete within 200 ticks");

    let template = template::built_in_templates().remove(1);
    let results = calculate_all(
        &peaks,
        &troughs,
        &switch_times,
        mv_sign_init,
        ControllerDirection::Reverse,
        config,
        PvRange {
            high: 100.0,
            low: 0.0,
        },
        &template,
    );

    // Just confirm the pipeline produced finite, sane-signed output — the precise
    // numeric values are already covered by the synthetic-array tests above.
    for (tuning, pid) in &results {
        assert!(tuning.kp.is_finite() && tuning.kp > 0.0);
        assert!(pid.proportional.is_finite() && pid.proportional > 0.0);
        assert!(pid.integral.is_finite() && pid.integral > 0.0);
    }
}

// --- serde round trips -------------------------------------------------------------------

#[test]
fn pv_range_serde_round_trip() {
    let range = PvRange {
        high: 100.0,
        low: 0.0,
    };
    let json = serde_json::to_string(&range).unwrap();
    let back: PvRange = serde_json::from_str(&json).unwrap();
    assert_eq!(range, back);
}

#[test]
fn oscillation_serde_round_trip() {
    let osc = Oscillation {
        period_minutes: 1.0,
        frequency: 7.5,
        pv_amp_raw: 0.25,
        pv_amp_percent: 0.25,
    };
    let json = serde_json::to_string(&osc).unwrap();
    let back: Oscillation = serde_json::from_str(&json).unwrap();
    assert_eq!(osc, back);
}

#[test]
fn tuning_result_serde_round_trip() {
    let result = sample_result();
    let json = serde_json::to_string(&result).unwrap();
    let back: TuningResult = serde_json::from_str(&json).unwrap();
    assert_eq!(result, back);
}

#[test]
fn pid_parameters_serde_round_trip() {
    let pid = PidParameters {
        response_level: ResponseLevel::Aggressive,
        proportional: 2.0,
        integral: 4.0,
        derivative: 0.5,
    };
    let json = serde_json::to_string(&pid).unwrap();
    let back: PidParameters = serde_json::from_str(&json).unwrap();
    assert_eq!(pid, back);
}

#[test]
fn opc_write_values_serde_round_trip() {
    let values = OpcWriteValues {
        response_level: ResponseLevel::Aggressive,
        proportional: 2.0,
        integral: 9999.0,
        derivative: 0.0,
    };
    let json = serde_json::to_string(&values).unwrap();
    let back: OpcWriteValues = serde_json::from_str(&json).unwrap();
    assert_eq!(values, back);
}
