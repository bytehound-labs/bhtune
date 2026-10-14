//! Tuning-constant math: turns a completed MRFT run's peaks/troughs/switch-times into Kp/Ti/Td
//! for all three [`ResponseLevel`]s, then into the PID parameters in whatever
//! representation/units a DCS/PLC template expects.
//!
//! Pure port of `TuningConstantsCalc` ([`measure_oscillation`] + [`calculate_tuning_result`])
//! and `CalculatePIDparameters` ([`calculate_pid_parameters`]), split the same way the legacy
//! app split them. Like the MRFT engine, this module does no I/O and reads no clock — every
//! timestamp it reasons about is already inside `switch_times`, taken from a completed
//! [`crate::mrft::Action::Complete`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    constants::{ResponseLevel, lookup},
    controller_type::ControllerType,
    direction::ControllerDirection,
    loop_config::LoopConfig,
    pid_config::{
        DerivativeType, IntegralType, PidRounding, PidRoundingError, ProportionalType,
        RoundedPidValue, TimeUnit,
    },
    range::PvRange,
    template::DcsTemplate,
};

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

/// Whether a calculated response-level result contains values that are safe to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TuningResultStatus {
    Valid,
    Invalid,
}

/// Why a calculated response-level result was marked invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TuningResultInvalidReason {
    NonFinitePvAmplitude,
    NonPositivePvAmplitude,
    NonFinitePeriod,
    NonPositivePeriod,
    NonFiniteFrequency,
    NonPositiveFrequency,
    NonFiniteKp,
    NonFiniteTiMinutes,
    NonFiniteTdMinutes,
    NonFiniteProportional,
    NonFiniteIntegral,
    NonFiniteDerivative,
}

impl std::fmt::Display for TuningResultInvalidReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::NonFinitePvAmplitude => "PV amplitude is not finite",
            Self::NonPositivePvAmplitude => "PV amplitude is not positive",
            Self::NonFinitePeriod => "oscillation period is not finite",
            Self::NonPositivePeriod => "oscillation period is not positive",
            Self::NonFiniteFrequency => "oscillation frequency is not finite",
            Self::NonPositiveFrequency => "oscillation frequency is not positive",
            Self::NonFiniteKp => "Kp is not finite",
            Self::NonFiniteTiMinutes => "Ti is not finite",
            Self::NonFiniteTdMinutes => "Td is not finite",
            Self::NonFiniteProportional => "proportional value is not finite",
            Self::NonFiniteIntegral => "integral value is not finite",
            Self::NonFiniteDerivative => "derivative value is not finite",
        };
        f.write_str(message)
    }
}

/// A checked result for one response level.
///
/// Invalid results retain their response level and an explicit diagnostic reason, but never
/// expose numeric tuning/PID values for callers to accidentally write or display as usable
/// constants.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CheckedTuningResult {
    pub response_level: ResponseLevel,
    pub status: TuningResultStatus,
    pub invalid_reason: Option<TuningResultInvalidReason>,
    pub tuning: Option<TuningResult>,
    pub pid: Option<PidParameters>,
}

impl CheckedTuningResult {
    fn invalid(response_level: ResponseLevel, reason: TuningResultInvalidReason) -> Self {
        Self {
            response_level,
            status: TuningResultStatus::Invalid,
            invalid_reason: Some(reason),
            tuning: None,
            pid: None,
        }
    }

    fn valid(tuning: TuningResult, pid: PidParameters) -> Self {
        Self {
            response_level: tuning.response_level,
            status: TuningResultStatus::Valid,
            invalid_reason: None,
            tuning: Some(tuning),
            pid: Some(pid),
        }
    }

    /// Returns the calculated values only when this result is safe to use.
    pub fn usable_values(&self) -> Option<(TuningResult, PidParameters)> {
        match (self.status, self.invalid_reason, self.tuning, self.pid) {
            (TuningResultStatus::Valid, None, Some(tuning), Some(pid)) => Some((tuning, pid)),
            _ => None,
        }
    }
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
pub fn measure_oscillation(
    peaks: &[f32],
    troughs: &[f32],
    switch_times: &[DateTime<Utc>],
    mv_sign_init: i8,
    direction: ControllerDirection,
    config: LoopConfig,
    pv_range: PvRange,
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

    // Keep millisecond precision and total elapsed time: sub-second periods must stay nonzero,
    // and long runs must not wrap after 24 hours.
    let elapsed_ms = (switch_times[switch_times.len() - 1] - switch_times[0]).num_milliseconds();
    let secs_for_period = elapsed_ms as f32 / 1000.0;
    let period_minutes = (secs_for_period / 60.0) / num_cycles_count as f32;
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

/// Controller-ready targets, separate from the full-precision calculated PID parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ControllerPidValues {
    pub response_level: ResponseLevel,
    pub proportional: RoundedPidValue,
    pub integral: RoundedPidValue,
    pub derivative: RoundedPidValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerPidError {
    pub term: &'static str,
    pub reason: PidRoundingError,
}

impl std::fmt::Display for ControllerPidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.term, self.reason)
    }
}

impl std::error::Error for ControllerPidError {}

fn rounded_active_term(
    value: f32,
    term: &'static str,
    policy: PidRounding,
) -> Result<RoundedPidValue, ControllerPidError> {
    let rounded = policy
        .round_value(value)
        .map_err(|reason| ControllerPidError { term, reason })?;
    let reason = if value <= 0.0 {
        Some(PidRoundingError::NonPositive)
    } else if rounded.value == 0.0 {
        Some(PidRoundingError::RoundedToZero)
    } else {
        None
    };
    match reason {
        Some(reason) => Err(ControllerPidError { term, reason }),
        None => Ok(rounded),
    }
}

/// Applies template precision only to active, already-converted PID terms. Disabled
/// integral/derivative targets retain the exact values chosen by [`opc_write_values`].
pub fn controller_pid_values(
    pid: PidParameters,
    controller_type: ControllerType,
    template: &DcsTemplate,
) -> Result<ControllerPidValues, ControllerPidError> {
    let raw = opc_write_values(pid, controller_type, template.integral_type);
    let policy = template.pid_rounding;
    let proportional = rounded_active_term(raw.proportional, "proportional", policy)?;
    let integral = match controller_type {
        ControllerType::P => policy.disabled_value(raw.integral),
        ControllerType::Pi | ControllerType::Pid => {
            rounded_active_term(raw.integral, "integral", policy)?
        }
    };
    let derivative = match controller_type {
        ControllerType::Pid => rounded_active_term(raw.derivative, "derivative", policy)?,
        ControllerType::P | ControllerType::Pi => policy.disabled_value(raw.derivative),
    };
    Ok(ControllerPidValues {
        response_level: raw.response_level,
        proportional,
        integral,
        derivative,
    })
}

/// The top-level entry point: computes the PID parameters for all three response levels from
/// a completed MRFT run, in one call. Composes [`measure_oscillation`] (once) with
/// [`calculate_tuning_result`] and [`calculate_pid_parameters`] (once per [`ResponseLevel`]).
/// Pure port of `MRFTcompletionActions`'s call into `TuningConstantsCalc` +
/// `CalculatePIDparameters`.
///
/// Returns both the intermediate [`TuningResult`] (Kp/Ti/Td, DCS-unit-independent) and the
/// final [`PidParameters`] (in the template's own units) for each response level, since
/// callers that persist a run (e.g. the `bhtune` CLI's `tune` command, via
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
) -> [(TuningResult, PidParameters); 3] {
    let osc = measure_oscillation(
        peaks,
        troughs,
        switch_times,
        mv_sign_init,
        direction,
        config,
        pv_range,
    );
    ResponseLevel::ALL.map(|level| {
        let result = calculate_tuning_result(osc, config, level);
        let pid = calculate_pid_parameters(result, template);
        (result, pid)
    })
}

/// Checked counterpart to [`calculate_all`].
///
/// The legacy numeric functions remain available for parity/replay callers. Production
/// persistence should use this entry point so a degenerate PV amplitude or any non-finite
/// intermediate/final value becomes an explicit invalid result instead of reaching SQLite or
/// PID write-back as `NaN`/infinity.
#[allow(clippy::too_many_arguments)]
pub fn calculate_all_checked(
    peaks: &[f32],
    troughs: &[f32],
    switch_times: &[DateTime<Utc>],
    mv_sign_init: i8,
    direction: ControllerDirection,
    config: LoopConfig,
    pv_range: PvRange,
    template: &DcsTemplate,
) -> [CheckedTuningResult; 3] {
    let oscillation = measure_oscillation(
        peaks,
        troughs,
        switch_times,
        mv_sign_init,
        direction,
        config,
        pv_range,
    );

    let oscillation_reason = invalid_oscillation_reason(oscillation);

    ResponseLevel::ALL.map(|level| {
        if let Some(reason) = oscillation_reason {
            return CheckedTuningResult::invalid(level, reason);
        }

        let tuning = calculate_tuning_result(oscillation, config, level);
        checked_result_from_tuning(tuning, template)
    })
}

fn invalid_oscillation_reason(oscillation: Oscillation) -> Option<TuningResultInvalidReason> {
    if !oscillation.pv_amp_raw.is_finite() || !oscillation.pv_amp_percent.is_finite() {
        Some(TuningResultInvalidReason::NonFinitePvAmplitude)
    } else if oscillation.pv_amp_raw <= 0.0 || oscillation.pv_amp_percent <= 0.0 {
        Some(TuningResultInvalidReason::NonPositivePvAmplitude)
    } else if !oscillation.period_minutes.is_finite() {
        Some(TuningResultInvalidReason::NonFinitePeriod)
    } else if oscillation.period_minutes <= 0.0 {
        Some(TuningResultInvalidReason::NonPositivePeriod)
    } else if !oscillation.frequency.is_finite() {
        Some(TuningResultInvalidReason::NonFiniteFrequency)
    } else if oscillation.frequency <= 0.0 {
        Some(TuningResultInvalidReason::NonPositiveFrequency)
    } else {
        None
    }
}

fn checked_result_from_tuning(tuning: TuningResult, template: &DcsTemplate) -> CheckedTuningResult {
    if !tuning.kp.is_finite() {
        return CheckedTuningResult::invalid(
            tuning.response_level,
            TuningResultInvalidReason::NonFiniteKp,
        );
    }
    if !tuning.ti_minutes.is_finite() {
        return CheckedTuningResult::invalid(
            tuning.response_level,
            TuningResultInvalidReason::NonFiniteTiMinutes,
        );
    }
    if !tuning.td_minutes.is_finite() {
        return CheckedTuningResult::invalid(
            tuning.response_level,
            TuningResultInvalidReason::NonFiniteTdMinutes,
        );
    }

    let pid = calculate_pid_parameters(tuning, template);
    if !pid.proportional.is_finite() {
        return CheckedTuningResult::invalid(
            tuning.response_level,
            TuningResultInvalidReason::NonFiniteProportional,
        );
    }
    if !pid.integral.is_finite() {
        return CheckedTuningResult::invalid(
            tuning.response_level,
            TuningResultInvalidReason::NonFiniteIntegral,
        );
    }
    if !pid.derivative.is_finite() {
        return CheckedTuningResult::invalid(
            tuning.response_level,
            TuningResultInvalidReason::NonFiniteDerivative,
        );
    }

    CheckedTuningResult::valid(tuning, pid)
}

#[cfg(test)]
mod tests;
