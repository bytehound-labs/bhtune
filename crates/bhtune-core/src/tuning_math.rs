//! Tuning-constant math: turns a completed MRFT run's peaks/troughs/switch-times into Kp/Ti/Td
//! for all three [`ResponseLevel`]s, then into the PID parameters in whatever
//! representation/units a DCS/PLC template expects.
//!
//! Pure port of `TuningConstantsCalc` ([`measure_oscillation`] + [`calculate_tuning_result`])
//! and `CalculatePIDparameters` ([`calculate_pid_parameters`]), split the same way the legacy
//! app split them. Like `core-mrft`, this module does no I/O and reads no clock — every
//! timestamp it reasons about is already inside `switch_times`, taken from a completed
//! [`crate::mrft::Action::Complete`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    constants::{ResponseLevel, lookup},
    controller_type::ControllerType,
    direction::ControllerDirection,
    loop_config::LoopConfig,
    pid_config::{DerivativeType, IntegralType, ProportionalType, TimeUnit},
    range::PvRange,
    template::DcsTemplate,
};

/// Legacy-bug replication flags for this module, mirroring [`crate::mrft::MrftCompat`]'s
/// pattern (see `core-bug-register`). Every field defaults to `false`: the fixed, correct
/// behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TuningMathCompat {
    /// Replicate `TuningConstantsCalc`'s period calculation, which reconstructs elapsed time
    /// from a `TimeSpan`'s `.Hours`/`.Minutes`/`.Seconds` *component* properties (each
    /// wrapping at 24/60/60) instead of its total duration — silently dropping whole days
    /// for any MRFT run lasting 24 hours or longer. Real relay-test runs are essentially
    /// never that long, so this is a latent defect rather than one likely to bite in
    /// practice, but it is fixed by default; set this `true` only to reproduce it bit-for-bit
    /// against a captured legacy trace.
    pub replicate_period_truncation_bug: bool,
}

/// Oscillation measurements derived from a completed MRFT run, before applying any
/// response-level-specific Kp multiplier. Pure port of the period/frequency/amplitude portion
/// of `TuningConstantsCalc`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Oscillation {
    pub period_minutes: f32,
    pub frequency: f32,
    pub pv_amp_raw: f32,
    pub pv_amp_percent: f32,
}

/// Calculated Kp/Ti/Td for one [`ResponseLevel`], before DCS-specific unit conversion. Pure
/// port of the Kp/Ti/Td portion of `TuningConstantsCalc`. `ti_minutes`/`td_minutes` are
/// identical across all three response levels for a given run — only `kp` varies — since
/// `C2`/`C3` don't vary by response level (see [`crate::constants::lookup`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TuningResult {
    pub response_level: ResponseLevel,
    pub kp: f32,
    pub ti_minutes: f32,
    pub td_minutes: f32,
}

/// The final PID parameters in a DCS/PLC template's own representation (e.g. proportional
/// band instead of gain, reset rate instead of reset time, seconds instead of minutes) — pure
/// port of `CalculatePIDparameters`, applied to one [`TuningResult`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PidParameters {
    pub response_level: ResponseLevel,
    pub proportional: f32,
    pub integral: f32,
    pub derivative: f32,
}

/// The literal values to write back to the DCS/PLC for one [`PidParameters`] — see
/// [`opc_write_values`]. Distinct from [`PidParameters`] because integral/derivative may
/// differ from the calculated values for controller types that don't use one or both terms.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OpcWriteValues {
    pub response_level: ResponseLevel,
    pub proportional: f32,
    pub integral: f32,
    pub derivative: f32,
}

/// Computes oscillation measurements (period, frequency, PV amplitude) from a completed MRFT
/// run's recorded peaks/troughs/switch-times. Pure port of `TuningConstantsCalc`'s
/// period/frequency/amplitude math — everything before the per-response-level Kp/Ti/Td split,
/// which [`calculate_tuning_result`] handles.
///
/// `peaks`/`troughs`/`switch_times`/`mv_sign_init` come directly from a completed
/// [`crate::mrft::Action::Complete`]; `direction` and `config.num_cycles_count` must be the
/// same values the [`crate::mrft::MrftEngine`] that produced them was built with.
///
/// # Panics
/// Panics if `switch_times` has fewer than 2 entries, or if `peaks`/`troughs` don't have the
/// lengths a real `Action::Complete` for `config.num_cycles_count` cycles always has (one of
/// them `num_cycles_count`, the other `num_cycles_count + 1`, alternating from the first
/// recorded switch's direction). Both are impossible outputs from [`crate::mrft::MrftEngine`]
/// and indicate a caller bug, not a runtime data problem.
#[allow(clippy::too_many_arguments)]
pub fn measure_oscillation(
    peaks: &[f32],
    troughs: &[f32],
    switch_times: &[DateTime<Utc>],
    mv_sign_init: i8,
    direction: ControllerDirection,
    config: LoopConfig,
    pv_range: PvRange,
    compat: TuningMathCompat,
) -> Oscillation {
    let num_cycles_count = config.num_cycles_count;
    assert!(
        switch_times.len() >= 2,
        "switch_times must have at least 2 entries to measure a period, got {}",
        switch_times.len()
    );
    assert_eq!(
        switch_times.len(),
        num_cycles_count as usize * 2 + 1,
        "switch_times.len() must equal 2 * num_cycles_count + 1"
    );

    // Whichever of peaks/troughs holds the first recorded switch's value has one extra
    // (excluded-from-the-average) entry at index 0 — see the comment below.
    let first_switch_is_peak = mv_sign_init as i32 * direction.action_multiplier() as i32 == 1;
    let (peaks_sum, troughs_sum) = if first_switch_is_peak {
        assert_eq!(
            peaks.len(),
            num_cycles_count as usize + 1,
            "peaks.len() must be num_cycles_count + 1 when the first recorded switch is a peak"
        );
        assert_eq!(
            troughs.len(),
            num_cycles_count as usize,
            "troughs.len() must equal num_cycles_count when the first recorded switch is a peak"
        );
        // The very first recorded peak is excluded from the average — the same choice
        // `TuningConstantsCalc` makes (its summation loop starts at index 1, not 0), on the
        // theory that the first post-skip oscillation may not yet be a "clean" full-amplitude
        // cycle.
        (peaks[1..].iter().sum::<f32>(), troughs.iter().sum::<f32>())
    } else {
        assert_eq!(
            troughs.len(),
            num_cycles_count as usize + 1,
            "troughs.len() must be num_cycles_count + 1 when the first recorded switch is a trough"
        );
        assert_eq!(
            peaks.len(),
            num_cycles_count as usize,
            "peaks.len() must equal num_cycles_count when the first recorded switch is a trough"
        );
        (peaks.iter().sum::<f32>(), troughs[1..].iter().sum::<f32>())
    };

    // `TuningConstantsCalc` reconstructs elapsed time from a `TimeSpan`'s `.Hours`/`.Minutes`/
    // `.Seconds` component properties (each wrapping at 24/60/60) rather than its total
    // duration. For any run under 24 hours (every real one) that's an exact reconstruction of
    // the total elapsed seconds; the bug only bites for a run lasting a full day or more,
    // which silently drops the whole-day count. `total_secs % 86_400` reproduces that wrap.
    let total_secs = (*switch_times.last().unwrap() - switch_times[0]).num_seconds();
    let secs_for_period = if compat.replicate_period_truncation_bug {
        total_secs % 86_400
    } else {
        total_secs
    };
    let period_minutes = (secs_for_period as f32 / 60.0) / num_cycles_count as f32;
    let frequency = 2.0 * std::f32::consts::PI / period_minutes;

    let pv_amp_raw = (peaks_sum - troughs_sum) / (2.0 * num_cycles_count as f32);
    let pv_amp_percent = pv_amp_raw / (pv_range.high - pv_range.low) * 100.0;

    Oscillation {
        period_minutes,
        frequency,
        pv_amp_raw,
        pv_amp_percent,
    }
}

/// Applies one [`ResponseLevel`]'s tuning constants to an [`Oscillation`], producing Kp/Ti/Td.
/// Pure port of the per-response-level portion of `TuningConstantsCalc`.
pub fn calculate_tuning_result(
    oscillation: Oscillation,
    config: LoopConfig,
    response_level: ResponseLevel,
) -> TuningResult {
    let tc = lookup(config.process_type, config.controller_type, response_level);

    // Matches `Convert.ToSingle(Math.PI * PvAmpPercent)`: the product is computed in `double`
    // (like `Math.PI`) and only truncated to `f32` afterward, so this widens explicitly rather
    // than multiplying two `f32`s directly, to keep rounding identical to the legacy app.
    let kp_denom = (std::f64::consts::PI * oscillation.pv_amp_percent as f64) as f32;
    let kp = tc.c1 * 4.0 * config.relay_amp_percent / kp_denom;

    // Unlike the Kp denominator above, `Convert.ToSingle(Math.PI)` here converts only the
    // constant itself (not a product) before it's used in `f32` arithmetic — equivalent to
    // using `f32::consts::PI` directly, with no `f64` intermediate needed.
    let ti_minutes = tc.c2 * 2.0 * std::f32::consts::PI / oscillation.frequency;
    let td_minutes = tc.c3 * 2.0 * std::f32::consts::PI / oscillation.frequency;

    TuningResult {
        response_level,
        kp,
        ti_minutes,
        td_minutes,
    }
}

/// Converts a [`TuningResult`] into the PID parameters a specific DCS/PLC template expects.
/// Pure port of `CalculatePIDparameters`.
///
/// Order matters and matches the legacy app exactly: the integral/derivative unit conversion
/// (minutes -> seconds) happens *before* the reset-rate/reset-gain/derivative-gain type
/// conversion, so e.g. `Ki` ends up computed from `Ti` already expressed in seconds whenever
/// the template's `integral_unit` is [`TimeUnit::Seconds`] — not from `Ti` in minutes.
pub fn calculate_pid_parameters(result: TuningResult, template: &DcsTemplate) -> PidParameters {
    let proportional = match template.proportional_type {
        ProportionalType::Gain => result.kp,
        ProportionalType::Band => 100.0 / result.kp,
    };

    let integral_in_template_unit = match template.integral_unit {
        TimeUnit::Seconds => result.ti_minutes * 60.0,
        TimeUnit::Minutes => result.ti_minutes,
    };
    let integral = match template.integral_type {
        IntegralType::ResetTime => integral_in_template_unit,
        IntegralType::ResetRate => 1.0 / integral_in_template_unit,
        IntegralType::ResetGain => result.kp / integral_in_template_unit,
    };

    let derivative_in_template_unit = match template.derivative_unit {
        TimeUnit::Seconds => result.td_minutes * 60.0,
        TimeUnit::Minutes => result.td_minutes,
    };
    let derivative = match template.derivative_type {
        DerivativeType::DerivativeTime => derivative_in_template_unit,
        DerivativeType::DerivativeGain => result.kp * derivative_in_template_unit,
    };

    PidParameters {
        response_level: result.response_level,
        proportional,
        integral,
        derivative,
    }
}

/// The literal integral/derivative values to write back to the DCS/PLC for one
/// [`PidParameters`], given the controller type the run was configured for. Pure port of the
/// controller-type-conditional part of `WritePIDparametersToOPCtags` — proportional is always
/// `pid.proportional`, so this only exists to decide integral/derivative, which
/// [`ControllerType::P`]/[`ControllerType::Pi`] algorithms don't use:
///
/// - Integral: a [`ControllerType::P`]-only run never writes `pid.integral` (it was computed
///   but is meaningless for an algorithm with no integral term). Instead it writes a sentinel
///   that disables integral action in whatever representation the template uses — `9999` for
///   [`IntegralType::ResetTime`] (an effectively-infinite reset time), or `0` for
///   [`IntegralType::ResetRate`]/[`IntegralType::ResetGain`] (a zero rate/gain has the same
///   disabling effect). [`ControllerType::Pi`]/[`ControllerType::Pid`] always write the real
///   calculated value.
/// - Derivative: only a [`ControllerType::Pid`] run writes `pid.derivative`; `P`/`Pi` always
///   write `0`.
pub fn opc_write_values(
    pid: PidParameters,
    controller_type: ControllerType,
    integral_type: IntegralType,
) -> OpcWriteValues {
    let integral = match controller_type {
        ControllerType::P => match integral_type {
            IntegralType::ResetTime => 9999.0,
            IntegralType::ResetRate | IntegralType::ResetGain => 0.0,
        },
        ControllerType::Pi | ControllerType::Pid => pid.integral,
    };
    let derivative = match controller_type {
        ControllerType::Pid => pid.derivative,
        ControllerType::P | ControllerType::Pi => 0.0,
    };

    OpcWriteValues {
        response_level: pid.response_level,
        proportional: pid.proportional,
        integral,
        derivative,
    }
}

/// The top-level entry point: computes the PID parameters for all three response levels from
/// a completed MRFT run, in one call. Composes [`measure_oscillation`] (once) with
/// [`calculate_tuning_result`] and [`calculate_pid_parameters`] (once per [`ResponseLevel`]).
/// Pure port of `MRFTcompletionActions`'s call into `TuningConstantsCalc` +
/// `CalculatePIDparameters`.
///
/// Returns both the intermediate [`TuningResult`] (Kp/Ti/Td, DCS-unit-independent) and the
/// final [`PidParameters`] (in the template's own units) for each response level, since
/// callers that persist a run (e.g. `bhtune-cli`'s `tune` command, via
/// `bhtune_db::TuneResultRow::from_calculated`) need both — the schema records the
/// control-theory result alongside the exact values it derived for the connected DCS.
#[allow(clippy::too_many_arguments)]
pub fn calculate_all(
    peaks: &[f32],
    troughs: &[f32],
    switch_times: &[DateTime<Utc>],
    mv_sign_init: i8,
    direction: ControllerDirection,
    config: LoopConfig,
    pv_range: PvRange,
    template: &DcsTemplate,
    compat: TuningMathCompat,
) -> [(TuningResult, PidParameters); 3] {
    let osc = measure_oscillation(
        peaks,
        troughs,
        switch_times,
        mv_sign_init,
        direction,
        config,
        pv_range,
        compat,
    );
    ResponseLevel::ALL.map(|level| {
        let result = calculate_tuning_result(osc, config, level);
        let pid = calculate_pid_parameters(result, template);
        (result, pid)
    })
}

#[cfg(test)]
mod tests {
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
        assert!(
            (actual - expected).abs() <= epsilon,
            "expected {expected}, got {actual} (diff {})",
            (actual - expected).abs()
        );
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
            TuningMathCompat::default(),
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
            TuningMathCompat::default(),
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
            TuningMathCompat::default(),
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
            TuningMathCompat::default(),
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
            TuningMathCompat::default(),
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
            TuningMathCompat::default(),
        );
    }

    /// A run "lasting" >= 24 hours: the fixed (default) period calculation uses the true
    /// total elapsed time, while the compat flag reproduces the legacy bug that silently
    /// drops whole days (`TimeSpan.Hours` wraps at 24). Verified against the independent
    /// Python oracle: 90000s total / 2 cycles => 750.0 minutes fixed, vs (90000 % 86400) =
    /// 3600s => 30.0 minutes with the bug.
    #[test]
    fn measure_oscillation_period_truncation_bug_vs_fixed() {
        let switch_times = [t(0), t(20_000), t(40_000), t(60_000), t(90_000)];

        let fixed = measure_oscillation(
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
            TuningMathCompat::default(),
        );
        assert_approx(fixed.period_minutes, 750.0, 1e-3);

        let buggy = measure_oscillation(
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
            TuningMathCompat {
                replicate_period_truncation_bug: true,
            },
        );
        assert_approx(buggy.period_minutes, 30.0, 1e-3);
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
            TuningMathCompat::default(),
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

    /// Fields unpacked from a real engine's `Action::Complete`, named here purely to keep
    /// clippy's `type_complexity` lint happy for the one test that needs them.
    type CompletionFields = (Vec<f32>, Vec<f32>, Vec<DateTime<Utc>>, i8);

    /// End-to-end: run a real `MrftEngine` to completion and feed its `Action::Complete`
    /// straight into `calculate_all`, proving the two modules compose without needing any
    /// glue beyond what `Action::Complete` already carries.
    #[test]
    fn calculate_all_consumes_a_real_mrft_engine_completion() {
        use crate::mrft::{Action, InitialReadings, MrftCompat, MrftEngine, Tick};
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
        let mut engine = MrftEngine::new(
            config,
            ControllerDirection::Reverse,
            tc.beta,
            initial,
            t(0),
            MrftCompat::default(),
        );

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
        let mut mv_history: VecDeque<f32> =
            std::iter::repeat_n(initial.mv_ini, DELAY_TICKS).collect();
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
            TuningMathCompat::default(),
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
}
