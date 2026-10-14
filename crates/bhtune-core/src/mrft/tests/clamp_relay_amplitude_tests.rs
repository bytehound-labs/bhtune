use super::*;

#[test]
fn no_clamp_needed_when_within_range() {
    let amp = clamp_relay_amplitude(10.0, 50.0, 0.0, 100.0);
    assert_eq!(amp, 10.0); // 10% of (100-0)
}

#[test]
fn unclamped_amplitude_uses_the_mv_span() {
    let amp = clamp_relay_amplitude(10.0, 50.0, 20.0, 100.0);
    assert_eq!(amp, 8.0); // 10% of (100-20)
}

#[test]
fn upper_clamp_engages_near_ceiling() {
    let amp = clamp_relay_amplitude(10.0, 95.0, 0.0, 100.0);
    assert_eq!(amp, 5.0); // mv_range_high - mv_ini
}

#[test]
fn lower_clamp_uses_dimensionally_correct_formula() {
    // mv_ini=10, mv_range_low=5: naive 10% of (100-5) = 9.5 would drive MV to 0.5,
    // below the floor of 5, so it must clamp to mv_ini - mv_range_low = 5.
    let amp = clamp_relay_amplitude(10.0, 10.0, 5.0, 100.0);
    assert_eq!(amp, 5.0);
}

#[test]
fn cascade_case_with_nonzero_floor() {
    // mv_ini=10, mv_msl=5, mv_msh=100: naive 10% of (100-5)=9.5 drives MV to 0.5,
    // below the floor, so the fixed clamp gives mv_ini - mv_range_low = 5.
    let amp = clamp_relay_amplitude(10.0, 10.0, 5.0, 100.0);
    assert_eq!(amp, 5.0);
}

#[test]
fn lower_clamp_boundary_uses_subtraction() {
    let relay_amp_percent = 100.0 * 1.1 / 91.0;
    let amp = clamp_relay_amplitude(relay_amp_percent, 10.0, 9.0, 100.0);
    assert_eq!(amp, 1.0);
}
