use super::helpers::bad_request;
use super::*;

pub(super) fn validate_demo_request(request: &StartRunRequest) -> Result<(), ApiError> {
    validate_demo_identity(request)?;
    validate_demo_scalar_ranges(request)?;
    validate_demo_gain(request)?;
    validate_demo_discrete_fields(request)?;
    validate_demo_process_ranges(request)?;
    Ok(())
}

pub(super) fn validate_demo_identity(request: &StartRunRequest) -> Result<(), ApiError> {
    if request.driver != TuneDriver::Simulator {
        return Err(bad_request("demo mode only supports the simulator driver"));
    }
    if !built_in_templates()
        .iter()
        .any(|template| template.name == request.template)
    {
        return Err(bad_request(
            "demo mode only supports built-in DCS/PLC templates",
        ));
    }
    if request.tagname != DEMO_TAG_NAME {
        return Err(bad_request(format!(
            "demo requests must use the fixed tag label '{DEMO_TAG_NAME}'"
        )));
    }
    if !request.controller_type.is_allowed_for(request.process_type) {
        return Err(bad_request(
            "the selected controller type is not valid for the selected process type",
        ));
    }
    Ok(())
}

pub(super) fn validate_demo_scalar_ranges(request: &StartRunRequest) -> Result<(), ApiError> {
    let ranges = [
        (
            "relay_amp",
            request.relay_amp,
            DEMO_RELAY_AMP_MIN,
            DEMO_RELAY_AMP_MAX,
        ),
        (
            "sim_tau",
            request.sim_tau,
            DEMO_SIM_TAU_MIN,
            DEMO_SIM_TAU_MAX,
        ),
        (
            "sim_dead_time",
            request.sim_dead_time,
            DEMO_SIM_DEAD_TIME_MIN,
            DEMO_SIM_DEAD_TIME_MAX,
        ),
    ];
    for (field, value, min, max) in ranges {
        if !value.is_finite() || !(min..=max).contains(&value) {
            return Err(bad_request(format!(
                "demo field '{field}' must be finite and between {min} and {max}"
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_demo_gain(request: &StartRunRequest) -> Result<(), ApiError> {
    if !request.sim_gain.is_finite()
        || !(DEMO_SIM_GAIN_MIN..=DEMO_SIM_GAIN_MAX).contains(&request.sim_gain)
    {
        return Err(bad_request(format!(
            "demo field 'sim_gain' must be finite and between {DEMO_SIM_GAIN_MIN} and \
             {DEMO_SIM_GAIN_MAX}"
        )));
    }
    Ok(())
}

pub(super) fn validate_demo_discrete_fields(request: &StartRunRequest) -> Result<(), ApiError> {
    if request.sim_seed > DEMO_SIM_SEED_MAX {
        return Err(bad_request(
            "demo field 'sim_seed' exceeds the supported maximum",
        ));
    }
    if !request
        .cycles_skip
        .is_some_and(|value| (DEMO_CYCLES_SKIP_MIN..=DEMO_CYCLES_SKIP_MAX).contains(&value))
    {
        return Err(bad_request(format!(
            "demo field 'cycles_skip' must be between {DEMO_CYCLES_SKIP_MIN} and {DEMO_CYCLES_SKIP_MAX}"
        )));
    }
    if !request
        .cycles_count
        .is_some_and(|value| (DEMO_CYCLES_COUNT_MIN..=DEMO_CYCLES_COUNT_MAX).contains(&value))
    {
        return Err(bad_request(format!(
            "demo field 'cycles_count' must be between {DEMO_CYCLES_COUNT_MIN} and {DEMO_CYCLES_COUNT_MAX}"
        )));
    }
    if !request.noise_protection_secs.is_some_and(|value| {
        (DEMO_NOISE_PROTECTION_SECS_MIN..=DEMO_NOISE_PROTECTION_SECS_MAX).contains(&value)
    }) {
        return Err(bad_request(format!(
            "demo field 'noise_protection_secs' must be between {DEMO_NOISE_PROTECTION_SECS_MIN} and {DEMO_NOISE_PROTECTION_SECS_MAX}"
        )));
    }
    Ok(())
}

pub(super) fn validate_demo_process_ranges(request: &StartRunRequest) -> Result<(), ApiError> {
    let pv_low = validate_range_endpoint("pv_range_low", request.pv_range_low)?;
    let pv_high = validate_range_endpoint("pv_range_high", request.pv_range_high)?;
    let mv_low = validate_range_endpoint("mv_range_low", request.mv_range_low)?;
    let mv_high = validate_range_endpoint("mv_range_high", request.mv_range_high)?;
    let pv_span = validate_range_span("PV", pv_low, pv_high)?;
    validate_range_span("MV", mv_low, mv_high)?;
    validate_initial_value("sim_initial_pv", request.sim_initial_pv, pv_low, pv_high)?;
    validate_initial_value("sim_initial_mv", request.sim_initial_mv, mv_low, mv_high)?;
    let max_noise = pv_span * DEMO_SIM_NOISE_MAX_PV_SPAN_FRACTION;
    if !request.sim_noise.is_finite() || !(0.0..=max_noise).contains(&request.sim_noise) {
        return Err(bad_request(format!(
            "demo field 'sim_noise' must be finite and between 0 and {max_noise} (5% of the PV span)"
        )));
    }
    Ok(())
}

pub(super) fn validate_range_endpoint(field: &str, value: Option<f32>) -> Result<f32, ApiError> {
    let value = value.ok_or_else(|| bad_request(format!("demo field '{field}' is required")))?;
    if value.is_finite() && (DEMO_RANGE_ENDPOINT_MIN..=DEMO_RANGE_ENDPOINT_MAX).contains(&value) {
        Ok(value)
    } else {
        Err(bad_request(format!(
            "demo field '{field}' must be finite and between {DEMO_RANGE_ENDPOINT_MIN} and {DEMO_RANGE_ENDPOINT_MAX}"
        )))
    }
}

pub(super) fn validate_range_span(label: &str, low: f32, high: f32) -> Result<f32, ApiError> {
    let span = high - low;
    if (DEMO_RANGE_SPAN_MIN..=DEMO_RANGE_SPAN_MAX).contains(&span) {
        Ok(span)
    } else {
        Err(bad_request(format!(
            "demo {label} range must have an ordered span between {DEMO_RANGE_SPAN_MIN} and {DEMO_RANGE_SPAN_MAX}"
        )))
    }
}

pub(super) fn validate_initial_value(
    field: &str,
    value: f32,
    low: f32,
    high: f32,
) -> Result<(), ApiError> {
    if value.is_finite() && (low..=high).contains(&value) {
        Ok(())
    } else {
        Err(bad_request(format!(
            "demo field '{field}' must be finite and within its configured range"
        )))
    }
}

pub(super) fn apply_demo_defaults(object: &mut serde_json::Map<String, serde_json::Value>) {
    let defaults = [
        ("relay_amp", serde_json::json!(DEMO_RELAY_AMP_DEFAULT)),
        ("cycles_skip", serde_json::json!(DEMO_CYCLES_SKIP_DEFAULT)),
        ("cycles_count", serde_json::json!(DEMO_CYCLES_COUNT_DEFAULT)),
        (
            "noise_protection_secs",
            serde_json::json!(DEMO_NOISE_PROTECTION_SECS_DEFAULT),
        ),
        ("sim_gain", serde_json::json!(DEMO_SIM_GAIN_DEFAULT)),
        ("sim_tau", serde_json::json!(DEMO_SIM_TAU_DEFAULT)),
        (
            "sim_dead_time",
            serde_json::json!(DEMO_SIM_DEAD_TIME_DEFAULT),
        ),
        ("sim_noise", serde_json::json!(DEMO_SIM_NOISE_DEFAULT)),
        ("sim_seed", serde_json::json!(DEMO_SIM_SEED_DEFAULT)),
        (
            "sim_initial_pv",
            serde_json::json!(DEMO_SIM_INITIAL_VALUE_DEFAULT),
        ),
        (
            "sim_initial_mv",
            serde_json::json!(DEMO_SIM_INITIAL_VALUE_DEFAULT),
        ),
        ("pv_range_high", serde_json::json!(DEMO_RANGE_HIGH)),
        ("pv_range_low", serde_json::json!(DEMO_RANGE_LOW)),
        ("mv_range_high", serde_json::json!(DEMO_RANGE_HIGH)),
        ("mv_range_low", serde_json::json!(DEMO_RANGE_LOW)),
        ("direction", serde_json::json!(ControllerDirection::Reverse)),
    ];
    for (field, value) in defaults {
        object.entry(field.to_owned()).or_insert(value);
    }
}

pub(super) fn parse_demo_request(
    mut value: serde_json::Value,
) -> Result<StartRunRequest, ApiError> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| bad_request("demo run request body must be a JSON object"))?;
    const ALLOWED_FIELDS: &[&str] = &[
        "tagname",
        "template",
        "process_type",
        "controller_type",
        "relay_amp",
        "cycles_skip",
        "cycles_count",
        "noise_protection_secs",
        "driver",
        "sim_gain",
        "sim_tau",
        "sim_dead_time",
        "sim_noise",
        "sim_seed",
        "sim_initial_pv",
        "sim_initial_mv",
        "pv_range_high",
        "pv_range_low",
        "mv_range_high",
        "mv_range_low",
        "direction",
    ];
    if let Some(field) = object
        .keys()
        .find(|field| !ALLOWED_FIELDS.contains(&field.as_str()))
    {
        return Err(bad_request(format!("demo requests may not set '{field}'")));
    }
    apply_demo_defaults(object);
    let request: StartRunRequest = serde_json::from_value(value)
        .map_err(|error| bad_request(format!("invalid demo run request: {error}")))?;
    validate_demo_request(&request)?;
    Ok(request)
}
