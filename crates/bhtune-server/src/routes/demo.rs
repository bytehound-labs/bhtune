//! Restricted simulator-only public Demo API.

use std::convert::Infallible;
use std::future::Future;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::time::Duration as StdDuration;

use async_stream::stream;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use bhtune_cli::commands::tune::{drive, prepare_owned};
use bhtune_cli::config::{
    DEMO_COOKIE_NAME, DEMO_CYCLES_COUNT_DEFAULT, DEMO_CYCLES_COUNT_MAX, DEMO_CYCLES_COUNT_MIN,
    DEMO_CYCLES_SKIP_DEFAULT, DEMO_CYCLES_SKIP_MAX, DEMO_CYCLES_SKIP_MIN,
    DEMO_NOISE_PROTECTION_SECS_DEFAULT, DEMO_NOISE_PROTECTION_SECS_MAX,
    DEMO_NOISE_PROTECTION_SECS_MIN, DEMO_RANGE_ENDPOINT_MAX, DEMO_RANGE_ENDPOINT_MIN,
    DEMO_RANGE_HIGH, DEMO_RANGE_LOW, DEMO_RANGE_SPAN_MAX, DEMO_RANGE_SPAN_MIN,
    DEMO_RELAY_AMP_DEFAULT, DEMO_RELAY_AMP_MAX, DEMO_RELAY_AMP_MIN, DEMO_SIM_DEAD_TIME_DEFAULT,
    DEMO_SIM_DEAD_TIME_MAX, DEMO_SIM_DEAD_TIME_MIN, DEMO_SIM_GAIN_ABS_MIN, DEMO_SIM_GAIN_DEFAULT,
    DEMO_SIM_GAIN_MAX, DEMO_SIM_INITIAL_VALUE_DEFAULT, DEMO_SIM_NOISE_DEFAULT,
    DEMO_SIM_NOISE_MAX_PV_SPAN_FRACTION, DEMO_SIM_SEED_DEFAULT, DEMO_SIM_SEED_MAX,
    DEMO_SIM_TAU_DEFAULT, DEMO_SIM_TAU_MAX, DEMO_SIM_TAU_MIN, DEMO_TAG_NAME, DemoPolicy,
    ServerMode,
};
use bhtune_core::{ControllerDirection, template::built_in_templates};
use bhtune_db::models::{
    DemoSessionRow, Pagination, TemplateOrigin, TuneDriver, TuneMvActuationRow, TuneOutcome,
    TuneResultRow, TuneRunRow, TuneSampleRow, TuneWriteRow,
};
use chrono::{Duration, Utc};
use rand::random;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

use crate::AppState;
use crate::error::ApiError;
use crate::routes::history::{
    InitialReadingsResponse, MvActuationResponse, PidConstantTagsResponse,
    PidParameterLabelsResponse, ResultResponse, RunDetailResponse, RunExportFormat, RunExportQuery,
    RunListQuery, RunListResponse, RunSummaryResponse, SampleResponse, WriteResponse,
    filter_from_query, parse_stored_request,
};
use crate::routes::runs::StartRunRequest;
use crate::routes::templates::TemplateResponse;
use crate::state::DemoQuotaExceeded;

const SSE_POLL_INTERVAL: StdDuration = StdDuration::from_millis(300);
const FORWARDED_CLIENT_IP_HEADER: &str = "X-BHTune-Client-IP";

pub(crate) struct PeerAddress(SocketAddr);

impl<S> FromRequestParts<S> for PeerAddress
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<SocketAddr>>()
                .map(|value| Self(value.0))
                .ok_or_else(|| {
                    ApiError::Internal(anyhow::anyhow!("Demo request is missing peer ConnectInfo"))
                }),
        )
    }
}

fn ensure_demo(state: &AppState) -> Result<(), ApiError> {
    (state.mode == ServerMode::Demo)
        .then_some(())
        .ok_or_else(|| ApiError::NotFound("demo mode is not enabled".into()))
}

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError::BadRequest(message.into())
}

fn too_many(message: impl Into<String>, retry_after_secs: u64) -> ApiError {
    ApiError::TooManyRequests {
        message: message.into(),
        retry_after_secs,
    }
}

fn global_capacity(message: impl Into<String>, retry_after_secs: u64) -> ApiError {
    ApiError::GlobalCapacity {
        message: message.into(),
        retry_after_secs,
    }
}

pub(crate) fn ordinary_request_permit(
    state: &AppState,
) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    state
        .demo_runtime
        .try_acquire_ordinary_request()
        .map_err(|_| {
            global_capacity(
                "demo request capacity is temporarily exhausted",
                state.demo_policy.ordinary_request_timeout_secs,
            )
        })
}

fn quota_error(error: DemoQuotaExceeded) -> ApiError {
    too_many(
        "demo accepted-start quota exceeded; wait before starting another tune",
        error.retry_after_secs,
    )
}

fn validate_demo_request(request: &StartRunRequest) -> Result<(), ApiError> {
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
    if !request.sim_gain.is_finite()
        || !(DEMO_SIM_GAIN_ABS_MIN..=DEMO_SIM_GAIN_MAX).contains(&request.sim_gain.abs())
    {
        return Err(bad_request(format!(
            "demo field 'sim_gain' must be finite and between -{DEMO_SIM_GAIN_MAX} and \
             -{DEMO_SIM_GAIN_ABS_MIN} or between {DEMO_SIM_GAIN_ABS_MIN} and {DEMO_SIM_GAIN_MAX}"
        )));
    }
    let expected_direction = if request.sim_gain.is_sign_positive() {
        ControllerDirection::Reverse
    } else {
        ControllerDirection::Direct
    };
    if request.direction != Some(expected_direction) {
        return Err(bad_request(format!(
            "demo direction must be '{}' for a {} process gain so the simulated loop uses negative feedback",
            match expected_direction {
                ControllerDirection::Direct => "direct",
                ControllerDirection::Reverse => "reverse",
            },
            if request.sim_gain.is_sign_positive() {
                "positive"
            } else {
                "negative"
            }
        )));
    }
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

fn validate_range_endpoint(field: &str, value: Option<f32>) -> Result<f32, ApiError> {
    let value = value.ok_or_else(|| bad_request(format!("demo field '{field}' is required")))?;
    if value.is_finite() && (DEMO_RANGE_ENDPOINT_MIN..=DEMO_RANGE_ENDPOINT_MAX).contains(&value) {
        Ok(value)
    } else {
        Err(bad_request(format!(
            "demo field '{field}' must be finite and between {DEMO_RANGE_ENDPOINT_MIN} and {DEMO_RANGE_ENDPOINT_MAX}"
        )))
    }
}

fn validate_range_span(label: &str, low: f32, high: f32) -> Result<f32, ApiError> {
    let span = high - low;
    if (DEMO_RANGE_SPAN_MIN..=DEMO_RANGE_SPAN_MAX).contains(&span) {
        Ok(span)
    } else {
        Err(bad_request(format!(
            "demo {label} range must have an ordered span between {DEMO_RANGE_SPAN_MIN} and {DEMO_RANGE_SPAN_MAX}"
        )))
    }
}

fn validate_initial_value(field: &str, value: f32, low: f32, high: f32) -> Result<(), ApiError> {
    if value.is_finite() && (low..=high).contains(&value) {
        Ok(())
    } else {
        Err(bad_request(format!(
            "demo field '{field}' must be finite and within its configured range"
        )))
    }
}

fn apply_demo_defaults(object: &mut serde_json::Map<String, serde_json::Value>) {
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

fn parse_demo_request(mut value: serde_json::Value) -> Result<StartRunRequest, ApiError> {
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

fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn trusted_proxy(peer: SocketAddr, configured: Option<&str>) -> bool {
    let Some(configured) = configured else {
        return false;
    };
    let configured = configured.trim();
    if let Ok(address) = configured.parse::<IpAddr>() {
        return address == peer.ip();
    }
    let Some((network, bits)) = configured.split_once('/') else {
        return false;
    };
    let (Ok(network), Ok(bits)) = (network.parse::<IpAddr>(), bits.parse::<u32>()) else {
        return false;
    };
    match (peer.ip(), network) {
        (IpAddr::V4(peer), IpAddr::V4(network)) if bits <= 32 => {
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(peer) & mask == u32::from(network) & mask
        }
        (IpAddr::V6(peer), IpAddr::V6(network)) if bits <= 128 => {
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            u128::from(peer) & mask == u128::from(network) & mask
        }
        _ => false,
    }
}

fn quota_ip(address: IpAddr) -> String {
    match address {
        IpAddr::V4(address) => address.to_string(),
        IpAddr::V6(address) => {
            let network = Ipv6Addr::from(u128::from(address) & (u128::MAX << 64));
            format!("{network}/64")
        }
    }
}

fn client_ip(headers: &HeaderMap, peer: PeerAddress, configured_proxy: Option<&str>) -> String {
    if trusted_proxy(peer.0, configured_proxy) {
        let mut values = headers.get_all(FORWARDED_CLIENT_IP_HEADER).iter();
        if let (Some(value), None) = (values.next(), values.next())
            && let Ok(value) = value.to_str()
            && value.trim() == value
            && !value.contains(',')
            && let Ok(address) = value.parse::<IpAddr>()
        {
            return quota_ip(address);
        }
    }
    quota_ip(peer.0.ip())
}

fn valid_token(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn cookie_token(headers: &HeaderMap) -> Result<Option<String>, ()> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        let value = value.to_str().map_err(|_| ())?;
        for part in value.split(';') {
            let Some((name, value)) = part.trim().split_once('=') else {
                continue;
            };
            if name != DEMO_COOKIE_NAME {
                continue;
            }
            if found.is_some() || !valid_token(value) {
                return Err(());
            }
            found = Some(value.to_owned());
        }
    }
    Ok(found)
}

fn parsed_token_hash(headers: &HeaderMap) -> Result<String, ApiError> {
    let token = cookie_token(headers)
        .map_err(|()| ApiError::Unauthorized("invalid demo session cookie".into()))?
        .ok_or_else(|| ApiError::Unauthorized("a demo session cookie is required".into()))?;
    Ok(token_hash(&token))
}

pub(crate) fn session_cookie_header(
    headers: &HeaderMap,
    policy: DemoPolicy,
) -> Result<Option<HeaderValue>, ApiError> {
    if matches!(cookie_token(headers), Ok(Some(_))) {
        return Ok(None);
    }
    let token: [u8; 32] = random();
    let cookie = format!(
        "{DEMO_COOKIE_NAME}={}; Path=/; Max-Age={}; HttpOnly; SameSite=Strict; Secure",
        hex_encode(&token),
        policy.session_ttl_secs
    );
    HeaderValue::from_str(&cookie)
        .map(Some)
        .map_err(|error| ApiError::Internal(error.into()))
}

struct DemoIdentity {
    token_hash: String,
    persisted: Option<DemoSessionRow>,
}

async fn identify(state: &AppState, headers: &HeaderMap) -> Result<DemoIdentity, ApiError> {
    let token_hash = parsed_token_hash(headers)?;
    let persisted = DemoSessionRow::get_by_token_hash(&state.pool, &token_hash, Utc::now()).await?;
    Ok(DemoIdentity {
        token_hash,
        persisted,
    })
}

fn identify_without_lookup(headers: &HeaderMap) -> Result<DemoIdentity, ApiError> {
    Ok(DemoIdentity {
        token_hash: parsed_token_hash(headers)?,
        persisted: None,
    })
}

fn owner_id(identity: &DemoIdentity, run_id: i64) -> Result<i64, ApiError> {
    identity
        .persisted
        .as_ref()
        .map(|session| session.id)
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))
}

async fn build_owned_run_detail(
    state: &AppState,
    run_id: i64,
    owner_id: i64,
) -> Result<Option<RunDetailResponse>, ApiError> {
    let Some(run) = TuneRunRow::get_for_demo_session(&state.pool, run_id, owner_id).await? else {
        return Ok(None);
    };
    let samples = TuneSampleRow::list_for_run(&state.pool, run_id).await?;
    let results = TuneResultRow::list_for_run(&state.pool, run_id).await?;
    let writes = TuneWriteRow::list_for_run(&state.pool, run_id).await?;
    let mv_actuations = TuneMvActuationRow::list_for_run(&state.pool, run_id).await?;
    let pid_constant_tags = match (
        &run.tags.proportional_constant,
        &run.tags.integral_constant,
        &run.tags.derivative_constant,
    ) {
        (Some(proportional), Some(integral), Some(derivative)) => Some(PidConstantTagsResponse {
            proportional: proportional.clone(),
            integral: integral.clone(),
            derivative: derivative.clone(),
        }),
        _ => None,
    };
    let original_request = serde_json::from_str(&run.request_json).ok();
    let pid_parameter_labels = PidParameterLabelsResponse::from(&run.template);
    Ok(Some(RunDetailResponse {
        id: run.id,
        tag_name: run.loop_name,
        notes: run.notes,
        driver: run.driver,
        outcome: run.outcome,
        failure_reason: run.failure_reason,
        started_at: run.started_at,
        completed_at: run.completed_at,
        template_name: run.template.name,
        template_origin: run.template_origin,
        allow_uncertain_quality: run.allow_uncertain_quality,
        config: run.config,
        effective_tuning: run.effective_tuning,
        opc_server: run.opc_server,
        bridge_host: run.bridge_host,
        pid_constant_tags,
        pid_parameter_labels,
        initial_readings: run.initial_readings.map(InitialReadingsResponse::from),
        timing_metrics: run.timing_metrics,
        samples: samples.iter().map(SampleResponse::from).collect(),
        results: results.iter().map(ResultResponse::from).collect(),
        writes: writes.iter().map(WriteResponse::from).collect(),
        mv_actuations: mv_actuations
            .iter()
            .map(MvActuationResponse::from)
            .collect(),
        restore_status: run.restore_status,
        restore_detail: run.restore_detail,
        original_request,
    }))
}

pub(crate) async fn list_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RunListQuery>,
) -> Result<Json<RunListResponse>, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let Some(owner_id) = identity.persisted.map(|session| session.id) else {
        return Ok(Json(RunListResponse {
            runs: Vec::new(),
            returned: 0,
            total: 0,
        }));
    };
    if query.offset.is_some_and(|offset| offset < 0) || query.limit.is_some_and(|limit| limit < 1) {
        return Err(bad_request(
            "demo run pagination requires limit >= 1 and offset >= 0",
        ));
    }
    let max_page = i64::from(
        state.demo_policy.retained_runs_per_visitor + state.demo_policy.max_active_runs_per_visitor,
    );
    let pagination = Pagination::new(
        query.limit.unwrap_or(max_page).min(max_page),
        query.offset.unwrap_or(0),
    );
    let filter = filter_from_query(&query).with_demo_session_id(owner_id);
    let runs = TuneRunRow::list(&state.pool, &filter, pagination).await?;
    let total = TuneRunRow::count(&state.pool, &filter).await?;
    Ok(Json(RunListResponse {
        returned: runs.len(),
        runs: runs.iter().map(RunSummaryResponse::from).collect(),
        total,
    }))
}

pub(crate) async fn last_request(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Option<StartRunRequest>>, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let Some(owner_id) = identity.persisted.map(|session| session.id) else {
        return Ok(Json(None));
    };
    let request = TuneRunRow::newest_for_demo_session(&state.pool, owner_id)
        .await?
        .and_then(|run| parse_stored_request(run.id, &run.request_json));
    Ok(Json(request))
}

pub(crate) async fn get_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
) -> Result<Json<RunDetailResponse>, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let owner_id = owner_id(&identity, run_id)?;
    build_owned_run_detail(&state, run_id, owner_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))
}

fn ok_event(event: Event) -> Result<Event, Infallible> {
    Ok(event)
}

#[derive(Serialize)]
struct DemoStreamDone {
    outcome: TuneOutcome,
}

pub(crate) async fn stream_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_demo(&state)?;
    let identity = identify(&state, &headers).await?;
    let owner_id = owner_id(&identity, run_id)?;
    TuneRunRow::get_for_demo_session(&state.pool, run_id, owner_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))?;

    let global_permit = state.demo_runtime.try_acquire_global_sse().map_err(|_| {
        global_capacity(
            "demo SSE capacity is temporarily exhausted",
            state.demo_policy.sse_lifetime_secs,
        )
    })?;
    let visitor_permit = state
        .demo_runtime
        .try_acquire_visitor_sse(&identity.token_hash, state.demo_policy.max_sse_per_visitor)
        .await
        .map_err(|_| {
            too_many(
                "demo SSE connection limit exceeded for this visitor",
                state.demo_policy.sse_lifetime_secs,
            )
        })?;

    let pool = state.pool.clone();
    let lifetime = StdDuration::from_secs(state.demo_policy.sse_lifetime_secs);
    let events = stream! {
        let _global_permit = global_permit;
        let _visitor_permit = visitor_permit;
        if lifetime.is_zero() {
            yield ok_event(Event::default().event("error").data("demo stream lifetime exceeded"));
        } else {
            let deadline = tokio::time::Instant::now() + lifetime;
            let mut last_tick = -1;
            let mut sent_initial = false;
            loop {
            let run = match tokio::time::timeout_at(
                deadline,
                TuneRunRow::get_for_demo_session(&pool, run_id, owner_id),
            ).await {
                Ok(Ok(Some(run))) => run,
                Ok(Ok(None)) => {
                    yield ok_event(Event::default().event("error").data("demo run is no longer available"));
                    break;
                }
                Ok(Err(error)) => {
                    tracing::error!(run_id, owner_id, %error, "failed to poll owned demo run");
                    yield ok_event(Event::default().event("error").data("demo stream unavailable"));
                    break;
                }
                Err(_) => {
                    yield ok_event(Event::default().event("error").data("demo stream lifetime exceeded"));
                    break;
                }
            };
            if !sent_initial
                && let Some(initial) = run.initial_readings.clone()
            {
                match Event::default()
                    .event("initial")
                    .json_data(InitialReadingsResponse::from(initial))
                {
                    Ok(event) => {
                        sent_initial = true;
                        yield ok_event(event);
                    }
                    Err(error) => {
                        tracing::error!(run_id, owner_id, %error, "failed to encode owned demo initial readings");
                        yield ok_event(Event::default().event("error").data("demo stream unavailable"));
                        break;
                    }
                }
            }
            match tokio::time::timeout_at(
                deadline,
                TuneSampleRow::list_for_run_since(&pool, run_id, last_tick),
            ).await {
                Ok(Ok(samples)) => {
                    let mut encoding_failed = false;
                    for sample in &samples {
                        last_tick = sample.tick_index;
                        match Event::default()
                            .event("sample")
                            .json_data(SampleResponse::from(sample))
                        {
                            Ok(event) => yield ok_event(event),
                            Err(error) => {
                                tracing::error!(run_id, owner_id, %error, "failed to encode owned demo sample");
                                yield ok_event(Event::default().event("error").data("demo stream unavailable"));
                                encoding_failed = true;
                                break;
                            }
                        }
                    }
                    if encoding_failed {
                        break;
                    }
                }
                Ok(Err(error)) => {
                    tracing::error!(run_id, owner_id, %error, "failed to poll owned demo samples");
                    yield ok_event(Event::default().event("error").data("demo stream unavailable"));
                    break;
                }
                Err(_) => {
                    yield ok_event(Event::default().event("error").data("demo stream lifetime exceeded"));
                    break;
                }
            }
            if run.outcome != TuneOutcome::Running {
                match Event::default()
                    .event("done")
                    .json_data(DemoStreamDone { outcome: run.outcome })
                {
                    Ok(event) => yield ok_event(event),
                    Err(error) => {
                        tracing::error!(run_id, owner_id, %error, "failed to encode owned demo terminal event");
                        yield ok_event(Event::default().event("error").data("demo stream unavailable"));
                    }
                }
                break;
            }
            if tokio::time::timeout_at(deadline, tokio::time::sleep(SSE_POLL_INTERVAL))
                .await
                .is_err()
            {
                yield ok_event(Event::default().event("error").data("demo stream lifetime exceeded"));
                break;
            }
        }
        }
    };
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

pub(crate) async fn cancel_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let owner_id = owner_id(&identity, run_id)?;
    let run = TuneRunRow::get_for_demo_session(&state.pool, run_id, owner_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))?;
    let _cancelled_or_already_terminal =
        run.outcome != TuneOutcome::Running || state.active_run.cancel(run_id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let owner_id = owner_id(&identity, run_id)?;
    let run = TuneRunRow::get_for_demo_session(&state.pool, run_id, owner_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))?;
    if run.outcome == TuneOutcome::Running {
        return Err(ApiError::Conflict(
            "cancel the demo run before deleting it".into(),
        ));
    }
    delete_existing_demo_run(&state, run_id, owner_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_existing_demo_run(
    state: &AppState,
    run_id: i64,
    owner_id: i64,
) -> Result<(), ApiError> {
    if !TuneRunRow::delete_for_demo_session(&state.pool, run_id, owner_id).await? {
        return Err(ApiError::NotFound(format!("no demo run with id {run_id}")));
    }
    Ok(())
}

pub(crate) async fn export_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
    Query(query): Query<RunExportQuery>,
) -> Result<Response, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let owner_id = owner_id(&identity, run_id)?;
    TuneRunRow::get_for_demo_session(&state.pool, run_id, owner_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))?;
    let samples = TuneSampleRow::list_for_run(&state.pool, run_id).await?;
    if samples.is_empty() {
        return Err(ApiError::NotFound(format!(
            "demo run {run_id} has no samples"
        )));
    }
    let format = query.format.unwrap_or(RunExportFormat::Csv);
    let bytes = bhtune_cli::commands::export::samples_to_bytes(&samples, format.into())?;
    let (content_type, extension) = match format {
        RunExportFormat::Csv => ("text/csv", "csv"),
        RunExportFormat::Json => ("application/json", "json"),
    };
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"demo-run-{run_id}.{extension}\""
        ))
        .map_err(|error| ApiError::Internal(error.into()))?,
    );
    Ok(response)
}

pub(crate) async fn list_templates(
    State(state): State<AppState>,
) -> Result<Json<Vec<TemplateResponse>>, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let rows = bhtune_db::models::DcsTemplateRow::list(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .filter(|row| row.origin == TemplateOrigin::Builtin)
            .map(TemplateResponse::from)
            .collect(),
    ))
}

pub(crate) async fn get_template(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<TemplateResponse>, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let row = bhtune_db::models::DcsTemplateRow::get_by_name(&state.pool, &name)
        .await?
        .filter(|row| row.origin == TemplateOrigin::Builtin)
        .ok_or_else(|| ApiError::NotFound(format!("no built-in template named '{name}'")))?;
    Ok(Json(row.into()))
}

async fn trim_owned_history(state: &AppState, owner_id: i64) {
    if let Err(error) = TuneRunRow::prune_terminal_for_demo_session(
        &state.pool,
        owner_id,
        state.demo_policy.retained_runs_per_visitor,
    )
    .await
    {
        tracing::warn!(owner_id, %error, "failed to prune excess demo history");
    }
}

async fn ensure_global_run_capacity(state: &AppState) -> Result<(), ApiError> {
    let current = TuneRunRow::count_demo_owned(&state.pool).await?;
    if current >= i64::from(state.demo_policy.max_tune_run_rows_global) {
        return Err(ApiError::GlobalCapacity {
            message: "demo history capacity is temporarily exhausted".into(),
            retry_after_secs: state.demo_policy.cleanup_interval_secs,
        });
    }
    Ok(())
}

fn prepare_error(error: anyhow::Error, state: &AppState) -> ApiError {
    if error.chain().any(|cause| {
        cause
            .to_string()
            .contains("demo tune run row limit reached")
    }) {
        ApiError::GlobalCapacity {
            message: "demo history capacity is temporarily exhausted".into(),
            retry_after_secs: state.demo_policy.cleanup_interval_secs,
        }
    } else {
        ApiError::Internal(error)
    }
}

pub(crate) async fn start_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddress,
    Json(value): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<RunDetailResponse>), ApiError> {
    ensure_demo(&state)?;
    // The raw object allow-list is intentionally checked before acquiring a permit, touching
    // session/history storage, reading configuration, or constructing a driver. Deserializing
    // directly to `StartRunRequest` would lose the distinction between an omitted forbidden
    // field and an explicitly supplied `null`/`false` field.
    let request = parse_demo_request(value)?;
    let _request_permit = ordinary_request_permit(&state)?;

    let identity = identify_without_lookup(&headers)?;
    let client_ip = client_ip(&headers, peer, state.trusted_proxy.as_deref());
    let _start_admission = state.demo_runtime.lock_start_admission().await;
    let global_permit = state.demo_runtime.try_acquire_global_run().map_err(|_| {
        global_capacity(
            "demo run capacity is temporarily exhausted",
            state.demo_policy.run_timeout_secs,
        )
    })?;
    let visitor_permit = state
        .demo_runtime
        .try_acquire_visitor_run(
            &identity.token_hash,
            state.demo_policy.max_active_runs_per_visitor,
        )
        .await
        .map_err(|_| {
            too_many(
                "a demo tune is already active for this visitor",
                state.demo_policy.run_timeout_secs,
            )
        })?;

    let now = Utc::now();
    let accepted_start = state
        .demo_runtime
        .reserve_accepted_start(&identity.token_hash, &client_ip, now, state.demo_policy)
        .await
        .map_err(quota_error)?;
    let preparation =
        async {
            // On-demand cleanup prevents an expired visitor or over-retained history from causing a
            // friendly capacity rejection until the next periodic sweep.
            DemoSessionRow::cleanup_expired(&state.pool, now).await?;
            bhtune_db::models::TuneRunRow::prune_terminal_demo_owned(
                &state.pool,
                state.demo_policy.retained_runs_per_visitor,
            )
            .await?;
            ensure_global_run_capacity(&state).await?;
            let session =
                match DemoSessionRow::get_by_token_hash(&state.pool, &identity.token_hash, now)
                    .await?
                {
                    Some(session) => session,
                    None => {
                        DemoSessionRow::get_or_create(
                            &state.pool,
                            &identity.token_hash,
                            now,
                            now + Duration::seconds(state.demo_policy.session_ttl_secs as i64),
                        )
                        .await?
                    }
                };
            trim_owned_history(&state, session.id).await;
            let accepted_runs = TuneRunRow::count_for_demo_session(&state.pool, session.id).await?;
            if accepted_runs >= i64::from(state.demo_policy.max_runs_per_session) {
                return Err(too_many(
                    "maximum demo runs for this session has been reached",
                    state.demo_policy.accepted_start_window_secs,
                ));
            }

            let args = request.into_tune_args()?;
            let mut config = state.config_snapshot()?;
            config.tuning.mrft_delay_secs = Some(0);
            config.tuning.poll_interval_ms = Some(state.demo_policy.poll_interval_ms);
            config.tuning.timeout_secs = Some(state.demo_policy.run_timeout_secs);
            let prepared = prepare_owned(&state.pool, args, &config, session.id)
                .await
                .map_err(|error| prepare_error(error, &state))?;
            Ok::<_, ApiError>((session.id, prepared))
        }
        .await;
    let (session_id, prepared) = match preparation {
        Ok(prepared) => prepared,
        Err(error) => {
            state
                .demo_runtime
                .release_accepted_start(accepted_start)
                .await;
            return Err(error);
        }
    };
    let run_id = prepared.run_id();

    let (mut ctrl_c, cancel_handle) = bhtune_cli::cancel::CtrlC::manual();
    let pool = state.pool.clone();
    let state_for_task = state.clone();
    let task = async move {
        let _global_permit = global_permit;
        let _visitor_permit = visitor_permit;
        let _ = drive(&pool, prepared, &mut ctrl_c).await;
        trim_owned_history(&state_for_task, session_id).await;
    };
    if state
        .active_run
        .start(run_id, cancel_handle, task)
        .await
        .is_err()
    {
        state
            .demo_runtime
            .release_accepted_start(accepted_start)
            .await;
        discard_unscheduled_demo_run(&state, run_id, session_id).await;
        return Err(global_capacity(
            "demo run could not be scheduled; retry shortly",
            state.demo_policy.run_timeout_secs,
        ));
    }

    let detail = build_owned_run_detail(&state, run_id, session_id)
        .await?
        .expect("prepare_owned inserted this owner-scoped demo run");
    Ok((StatusCode::CREATED, Json(detail)))
}

async fn discard_unscheduled_demo_run(state: &AppState, run_id: i64, session_id: i64) {
    if let Err(error) = TuneRunRow::delete_for_demo_session(&state.pool, run_id, session_id).await {
        tracing::error!(run_id, session_id, %error, "failed to discard unscheduled demo run");
    }
}

async fn api_not_found() -> ApiError {
    ApiError::NotFound("API route is not available in Demo mode".into())
}

pub fn router(policy: DemoPolicy) -> Router<AppState> {
    let ordinary = Router::new()
        .route("/api/templates", get(list_templates))
        .route("/api/templates/{name}", get(get_template))
        .route("/api/runs", get(list_runs).post(start_run))
        .route("/api/runs/last-request", get(last_request))
        .route("/api/runs/{id}", get(get_run).delete(delete_run))
        .route("/api/runs/{id}/cancel", axum::routing::post(cancel_run))
        .route("/api/runs/{id}/export", get(export_run))
        .route("/api", any(api_not_found))
        .route("/api/{*path}", any(api_not_found))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            StdDuration::from_secs(policy.ordinary_request_timeout_secs),
        ));
    let streaming = Router::new().route("/api/runs/{id}/stream", get(stream_run));
    ordinary.merge(streaming).layer(RequestBodyLimitLayer::new(
        policy.max_json_body_bytes as usize,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use bhtune_cli::config::DEMO_TEMPLATE_NAME;
    use bhtune_core::{ControllerType, LoopConfig, LoopTags, ProcessType, Tick};
    use bhtune_db::models::{DcsTemplateRow, SampleQuality, TemplateOrigin};
    use tower::ServiceExt;

    const DEMO_ORIGIN: &str = "https://demo.test";

    fn raw_token(seed: &str) -> String {
        seed.repeat(64 / seed.len())
    }

    fn cookie(seed: &str) -> String {
        format!("{DEMO_COOKIE_NAME}={}", raw_token(seed))
    }

    fn with_peer(mut request: Request<Body>, peer: &str) -> Request<Body> {
        request
            .extensions_mut()
            .insert(ConnectInfo::<SocketAddr>(peer.parse().unwrap()));
        request
    }

    async fn body_text(response: axum::response::Response) -> String {
        String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        serde_json::from_str(&body_text(response).await).unwrap()
    }

    fn valid_request(
        process_type: ProcessType,
        controller_type: ControllerType,
    ) -> serde_json::Value {
        serde_json::json!({
            "tagname": DEMO_TAG_NAME,
            "template": DEMO_TEMPLATE_NAME,
            "process_type": process_type,
            "controller_type": controller_type,
            "relay_amp": DEMO_RELAY_AMP_DEFAULT,
            "cycles_skip": DEMO_CYCLES_SKIP_DEFAULT,
            "cycles_count": DEMO_CYCLES_COUNT_DEFAULT,
            "noise_protection_secs": DEMO_NOISE_PROTECTION_SECS_DEFAULT,
            "driver": "simulator",
            "sim_gain": DEMO_SIM_GAIN_DEFAULT,
            "sim_tau": DEMO_SIM_TAU_DEFAULT,
            "sim_dead_time": DEMO_SIM_DEAD_TIME_DEFAULT,
            "sim_noise": DEMO_SIM_NOISE_DEFAULT,
            "sim_seed": DEMO_SIM_SEED_DEFAULT,
            "sim_initial_pv": DEMO_SIM_INITIAL_VALUE_DEFAULT,
            "sim_initial_mv": DEMO_SIM_INITIAL_VALUE_DEFAULT,
            "pv_range_high": DEMO_RANGE_HIGH,
            "pv_range_low": DEMO_RANGE_LOW,
            "mv_range_high": DEMO_RANGE_HIGH,
            "mv_range_low": DEMO_RANGE_LOW,
            "direction": ControllerDirection::Reverse,
        })
    }

    async fn seed_owned_run(
        state: &AppState,
        raw_token: &str,
        now: chrono::DateTime<Utc>,
    ) -> (i64, i64) {
        let session = DemoSessionRow::get_or_create(
            &state.pool,
            &token_hash(raw_token),
            now,
            now + Duration::hours(1),
        )
        .await
        .unwrap();
        let row = DcsTemplateRow::get_by_name(&state.pool, DEMO_TEMPLATE_NAME)
            .await
            .unwrap()
            .unwrap();
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 0,
            mrft_delay_secs: 0,
        };
        let tags = LoopTags::derive_from_pv_tag(DEMO_TAG_NAME, &row.template);
        let run = TuneRunRow::start_owned(
            &state.pool,
            session.id,
            DEMO_TAG_NAME,
            config,
            row.origin,
            &row.template,
            &tags,
            now,
        )
        .await
        .unwrap();
        (session.id, run.id)
    }

    async fn insert_one_sample(state: &AppState, run_id: i64, now: chrono::DateTime<Utc>) {
        TuneSampleRow::insert(
            &state.pool,
            run_id,
            0,
            Tick {
                time: now,
                pv: 50.0,
            },
            bhtune_core::MrftState {
                hysteresis: 1.0,
                mv_value_current: 45.0,
                mv_sign_next_step: 1,
                counter_all_switches: 0,
                cycles_completed: 0,
                cycles_remaining: 1,
            },
            SampleQuality::Good,
        )
        .await
        .unwrap();
    }

    async fn wait_until_inactive(state: &AppState, run_id: i64) {
        tokio::time::timeout(StdDuration::from_secs(2), async {
            while state.active_run.active_run_ids().await.contains(&run_id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("demo tune task should finish or honour cancellation");
    }

    #[test]
    fn trusted_proxy_and_dedicated_client_ip_rules_fail_closed_and_bucket_ipv6() {
        let peer = "10.0.0.4:443".parse().unwrap();
        assert!(trusted_proxy(peer, Some("10.0.0.4")));
        assert!(!trusted_proxy(peer, Some("10.0.0.5")));
        assert!(trusted_proxy(peer, Some("10.0.0.0/24")));
        assert!(trusted_proxy(peer, Some("0.0.0.0/0")));
        assert!(!trusted_proxy(peer, Some("10.0.1.0/24")));
        assert!(!trusted_proxy(peer, Some("not-a-proxy")));
        assert!(!trusted_proxy(peer, Some("10.0.0.0/not-bits")));
        assert!(!trusted_proxy(peer, Some("2001:db8::/32")));
        assert!(trusted_proxy(
            "[2001:db8::4]:443".parse().unwrap(),
            Some("2001:db8::/32")
        ));
        assert!(trusted_proxy(
            "[2001:db8::4]:443".parse().unwrap(),
            Some("::/0")
        ));
        let mut headers = HeaderMap::new();
        headers.insert(FORWARDED_CLIENT_IP_HEADER, "198.51.100.7".parse().unwrap());
        assert_eq!(
            client_ip(&headers, PeerAddress(peer), Some("10.0.0.0/24")),
            "198.51.100.7"
        );
        assert_eq!(client_ip(&headers, PeerAddress(peer), None), "10.0.0.4");
        headers.insert(
            FORWARDED_CLIENT_IP_HEADER,
            "198.51.100.7, 203.0.113.9".parse().unwrap(),
        );
        assert_eq!(
            client_ip(&headers, PeerAddress(peer), Some("10.0.0.0/24")),
            "10.0.0.4"
        );
        headers.insert(FORWARDED_CLIENT_IP_HEADER, "not-an-ip".parse().unwrap());
        assert_eq!(
            client_ip(&headers, PeerAddress(peer), Some("10.0.0.0/24")),
            "10.0.0.4"
        );
        headers.clear();
        headers.append(FORWARDED_CLIENT_IP_HEADER, "198.51.100.7".parse().unwrap());
        headers.append(FORWARDED_CLIENT_IP_HEADER, "203.0.113.9".parse().unwrap());
        assert_eq!(
            client_ip(&headers, PeerAddress(peer), Some("10.0.0.0/24")),
            "10.0.0.4"
        );
        headers.clear();
        headers.insert("x-forwarded-for", "203.0.113.9".parse().unwrap());
        assert_eq!(
            client_ip(&headers, PeerAddress(peer), Some("10.0.0.0/24")),
            "10.0.0.4"
        );

        let ipv6_peer = PeerAddress("[2001:db8:1:2:1234::1]:443".parse().unwrap());
        assert_eq!(
            client_ip(&HeaderMap::new(), ipv6_peer, None),
            "2001:db8:1:2::/64"
        );
        let mut forwarded_ipv6 = HeaderMap::new();
        forwarded_ipv6.insert(
            FORWARDED_CLIENT_IP_HEADER,
            "2001:db8:abcd:12:ffff::1".parse().unwrap(),
        );
        assert_eq!(
            client_ip(&forwarded_ipv6, PeerAddress(peer), Some("10.0.0.0/24")),
            "2001:db8:abcd:12::/64"
        );
    }

    #[test]
    fn cookie_parser_rejects_ambiguous_or_malformed_values() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            format!("ignored; {DEMO_COOKIE_NAME}={}", raw_token("ab"))
                .parse()
                .unwrap(),
        );
        assert_eq!(cookie_token(&headers).unwrap().unwrap(), raw_token("ab"));
        headers.append(
            header::COOKIE,
            format!("{DEMO_COOKIE_NAME}={}", raw_token("cd"))
                .parse()
                .unwrap(),
        );
        assert!(cookie_token(&headers).is_err());

        let mut malformed = HeaderMap::new();
        malformed.insert(
            header::COOKIE,
            format!("{DEMO_COOKIE_NAME}=UPPERCASE").parse().unwrap(),
        );
        assert!(cookie_token(&malformed).is_err());
        malformed.insert(header::COOKIE, HeaderValue::from_bytes(&[0xff]).unwrap());
        assert!(cookie_token(&malformed).is_err());
    }

    #[test]
    fn session_cookie_header_reuses_valid_cookies_and_replaces_missing_or_invalid_ones() {
        let policy = DemoPolicy::default();
        let mut headers = HeaderMap::new();
        let generated = session_cookie_header(&headers, policy).unwrap().unwrap();
        let generated = generated.to_str().unwrap();
        assert!(generated.starts_with(&format!("{DEMO_COOKIE_NAME}=")));
        assert!(generated.contains("HttpOnly"));
        assert!(generated.contains("SameSite=Strict"));

        headers.insert(header::COOKIE, cookie("ab").parse().unwrap());
        assert!(session_cookie_header(&headers, policy).unwrap().is_none());

        headers.insert(
            header::COOKIE,
            format!("{DEMO_COOKIE_NAME}=bad").parse().unwrap(),
        );
        assert!(session_cookie_header(&headers, policy).unwrap().is_some());
    }

    #[tokio::test]
    async fn demo_helpers_map_full_mode_and_capacity_errors() {
        let state = crate::test_support::in_memory_state().await;
        assert!(matches!(ensure_demo(&state), Err(ApiError::NotFound(_))));

        let base = crate::test_support::in_memory_state().await;
        let state = AppState::for_mode(
            base.pool,
            base.config_store,
            ServerMode::Demo,
            DemoPolicy {
                ordinary_request_concurrency: 1,
                ..DemoPolicy::default()
            },
        );
        let ordinary = ordinary_request_permit(&state).unwrap();
        let second = ordinary_request_permit(&state);
        assert!(matches!(second, Err(ApiError::GlobalCapacity { .. })));
        drop(ordinary);

        assert!(matches!(
            prepare_error(
                anyhow::anyhow!("outer").context("demo tune run row limit reached"),
                &state
            ),
            ApiError::GlobalCapacity { .. }
        ));
        assert!(matches!(
            prepare_error(anyhow::anyhow!("ordinary failure"), &state),
            ApiError::Internal(_)
        ));

        assert!(matches!(
            delete_existing_demo_run(&state, 12345, 67890).await,
            Err(ApiError::NotFound(_))
        ));

        let closed = crate::test_support::in_memory_state().await;
        closed.pool.close().await;
        discard_unscheduled_demo_run(&closed, 1, 1).await;
    }

    #[tokio::test]
    async fn shared_routes_require_a_cookie() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        let app = crate::build_router(state.clone());
        let shared = app
            .oneshot(Request::get("/api/runs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(shared.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn demo_mode_replaces_unavailable_full_api_routes_with_404() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        let response = crate::build_router(state)
            .oneshot(
                Request::get("/api/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(body_text(response).await.contains("Demo mode"));
    }

    #[tokio::test]
    async fn start_requires_peer_connect_info() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.allowed_origin = Some(DEMO_ORIGIN.into());
        let response = crate::build_router(state)
            .oneshot(
                Request::post("/api/runs")
                    .header(header::ORIGIN, DEMO_ORIGIN)
                    .header(header::COOKIE, cookie("ab"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        valid_request(ProcessType::Flow, ControllerType::Pi).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(body_text(response).await.contains("internal server error"));
    }

    #[tokio::test]
    async fn forbidden_fields_are_rejected_by_presence_before_database_io() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.allowed_origin = Some(DEMO_ORIGIN.into());
        state.pool.close().await;
        let mut body = valid_request(ProcessType::Flow, ControllerType::Pi);
        body["notes"] = serde_json::Value::Null;
        let request = with_peer(
            Request::post("/api/runs")
                .header(header::ORIGIN, DEMO_ORIGIN)
                .header(header::COOKIE, cookie("ab"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            "127.0.0.1:12345",
        );
        let response = crate::build_router(state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("'notes'"));
    }

    #[test]
    fn every_process_type_uses_authoritative_controller_compatibility() {
        for process_type in ProcessType::ALL {
            for controller_type in ControllerType::ALL {
                assert_eq!(
                    parse_demo_request(valid_request(process_type, controller_type)).is_ok(),
                    controller_type.is_allowed_for(process_type),
                    "{process_type:?}/{controller_type:?}"
                );
            }
        }
    }

    #[test]
    fn malformed_and_non_demo_start_requests_are_rejected_before_defaults() {
        assert!(parse_demo_request(serde_json::Value::Null).is_err());

        let mut request = valid_request(ProcessType::Flow, ControllerType::Pi);
        request.as_object_mut().unwrap().remove("tagname");
        assert!(parse_demo_request(request).is_err());

        let mut request = valid_request(ProcessType::Flow, ControllerType::Pi);
        request["relay_amp"] = serde_json::json!("large");
        assert!(parse_demo_request(request).is_err());

        for (field, value) in [
            ("driver", serde_json::json!("opcda")),
            ("template", serde_json::json!("Private custom template")),
            ("tagname", serde_json::json!("Other tag")),
            ("sim_seed", serde_json::json!(DEMO_SIM_SEED_MAX + 1)),
            ("cycles_skip", serde_json::Value::Null),
            ("cycles_count", serde_json::Value::Null),
            ("noise_protection_secs", serde_json::Value::Null),
            ("pv_range_high", serde_json::Value::Null),
            ("mv_range_low", serde_json::Value::Null),
        ] {
            let mut request = valid_request(ProcessType::Flow, ControllerType::Pi);
            request[field] = value;
            assert!(parse_demo_request(request).is_err(), "{field}");
        }

        for field in ["sim_tau", "sim_dead_time", "sim_initial_mv"] {
            let mut request = valid_request(ProcessType::Flow, ControllerType::Pi);
            request[field] = serde_json::json!(f32::NAN);
            assert!(parse_demo_request(request).is_err(), "{field}");
        }

        let mut request = valid_request(ProcessType::Flow, ControllerType::Pi);
        request["mv_range_low"] = serde_json::json!(10.0);
        request["mv_range_high"] = serde_json::json!(9.0);
        assert!(parse_demo_request(request).is_err());
    }

    #[test]
    fn omitted_demo_values_use_the_approved_defaults() {
        let mut request = valid_request(ProcessType::Flow, ControllerType::Pi);
        for field in [
            "relay_amp",
            "cycles_skip",
            "cycles_count",
            "noise_protection_secs",
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
        ] {
            request.as_object_mut().unwrap().remove(field);
        }

        let parsed = parse_demo_request(request).unwrap();
        assert_eq!(parsed.relay_amp, 10.0);
        assert_eq!(parsed.cycles_skip, Some(1));
        assert_eq!(parsed.cycles_count, Some(2));
        assert_eq!(parsed.noise_protection_secs, Some(0));
        assert_eq!(parsed.sim_gain, 1.0);
        assert_eq!(parsed.sim_tau, 0.1);
        assert_eq!(parsed.sim_dead_time, 0.25);
        assert_eq!(parsed.sim_noise, 0.0);
        assert_eq!(parsed.sim_seed, 0);
        assert_eq!(parsed.sim_initial_pv, 50.0);
        assert_eq!(parsed.sim_initial_mv, 50.0);
        assert_eq!(parsed.pv_range_low, Some(0.0));
        assert_eq!(parsed.pv_range_high, Some(100.0));
        assert_eq!(parsed.mv_range_low, Some(0.0));
        assert_eq!(parsed.mv_range_high, Some(100.0));
        assert_eq!(parsed.direction, Some(ControllerDirection::Reverse));
    }

    #[test]
    fn demo_bounds_cover_test_parameters_ranges_initials_and_noise() {
        let mut lower_bounds = valid_request(ProcessType::Flow, ControllerType::Pi);
        lower_bounds["relay_amp"] = serde_json::json!(DEMO_RELAY_AMP_MIN);
        lower_bounds["cycles_skip"] = serde_json::json!(DEMO_CYCLES_SKIP_MIN);
        lower_bounds["cycles_count"] = serde_json::json!(DEMO_CYCLES_COUNT_MIN);
        lower_bounds["noise_protection_secs"] = serde_json::json!(DEMO_NOISE_PROTECTION_SECS_MIN);
        lower_bounds["pv_range_low"] = serde_json::json!(-1_000.0);
        lower_bounds["pv_range_high"] = serde_json::json!(-999.0);
        lower_bounds["mv_range_low"] = serde_json::json!(999.0);
        lower_bounds["mv_range_high"] = serde_json::json!(1_000.0);
        lower_bounds["sim_initial_pv"] = serde_json::json!(-999.5);
        lower_bounds["sim_initial_mv"] = serde_json::json!(999.5);
        lower_bounds["sim_noise"] = serde_json::json!(0.05);
        assert!(parse_demo_request(lower_bounds).is_ok());

        let mut upper_bounds = valid_request(ProcessType::Flow, ControllerType::Pi);
        upper_bounds["relay_amp"] = serde_json::json!(DEMO_RELAY_AMP_MAX);
        upper_bounds["cycles_skip"] = serde_json::json!(DEMO_CYCLES_SKIP_MAX);
        upper_bounds["cycles_count"] = serde_json::json!(DEMO_CYCLES_COUNT_MAX);
        upper_bounds["noise_protection_secs"] = serde_json::json!(DEMO_NOISE_PROTECTION_SECS_MAX);
        upper_bounds["pv_range_low"] = serde_json::json!(-500.0);
        upper_bounds["pv_range_high"] = serde_json::json!(500.0);
        upper_bounds["sim_initial_pv"] = serde_json::json!(500.0);
        upper_bounds["sim_noise"] = serde_json::json!(50.0);
        assert!(parse_demo_request(upper_bounds).is_ok());

        for (field, value) in [
            ("relay_amp", serde_json::json!(20.1)),
            ("cycles_skip", serde_json::json!(3)),
            ("cycles_count", serde_json::json!(4)),
            ("noise_protection_secs", serde_json::json!(4)),
            ("pv_range_low", serde_json::json!(-1000.1)),
            ("sim_initial_pv", serde_json::json!(100.1)),
            ("sim_noise", serde_json::json!(5.1)),
        ] {
            let mut request = valid_request(ProcessType::Flow, ControllerType::Pi);
            request[field] = value;
            assert!(parse_demo_request(request).is_err(), "{field}");
        }

        for (low, high) in [(0.0, 0.5), (0.0, 1_000.1), (10.0, 9.0)] {
            let mut request = valid_request(ProcessType::Flow, ControllerType::Pi);
            request["pv_range_low"] = serde_json::json!(low);
            request["pv_range_high"] = serde_json::json!(high);
            request["sim_initial_pv"] = serde_json::json!(low);
            assert!(parse_demo_request(request).is_err(), "{low}..{high}");
        }
    }

    #[test]
    fn gain_magnitude_and_direction_must_form_negative_feedback() {
        for (gain, direction) in [
            (1.0, ControllerDirection::Reverse),
            (-1.0, ControllerDirection::Direct),
            (DEMO_SIM_GAIN_MAX, ControllerDirection::Reverse),
            (-DEMO_SIM_GAIN_MAX, ControllerDirection::Direct),
        ] {
            let mut request = valid_request(ProcessType::Flow, ControllerType::Pi);
            request["sim_gain"] = serde_json::json!(gain);
            request["direction"] = serde_json::json!(direction);
            assert!(parse_demo_request(request).is_ok());
        }
        for (gain, direction) in [
            (0.0, ControllerDirection::Reverse),
            (DEMO_SIM_GAIN_ABS_MIN / 2.0, ControllerDirection::Reverse),
            (1.0, ControllerDirection::Direct),
            (-1.0, ControllerDirection::Reverse),
        ] {
            let mut request = valid_request(ProcessType::Flow, ControllerType::Pi);
            request["sim_gain"] = serde_json::json!(gain);
            request["direction"] = serde_json::json!(direction);
            assert!(parse_demo_request(request).is_err());
        }
    }

    #[tokio::test]
    async fn built_in_templates_are_read_only_and_user_templates_are_hidden() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.allowed_origin = Some(DEMO_ORIGIN.into());
        let mut custom = DcsTemplateRow::get_by_name(&state.pool, DEMO_TEMPLATE_NAME)
            .await
            .unwrap()
            .unwrap()
            .template;
        custom.name = "Private custom template".into();
        DcsTemplateRow::insert(&state.pool, &custom, TemplateOrigin::User, Utc::now())
            .await
            .unwrap();
        let app = crate::build_router(state.clone());
        let list = app
            .clone()
            .oneshot(Request::get("/api/templates").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let templates: serde_json::Value =
            serde_json::from_slice(&to_bytes(list.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert!(
            templates
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["origin"] == "builtin")
        );
        assert!(
            !templates
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["name"] == custom.name)
        );

        let hidden = app
            .clone()
            .oneshot(
                Request::get("/api/templates/Private%20custom%20template")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
        let mutation = app
            .clone()
            .oneshot(
                Request::post("/api/templates")
                    .header(header::ORIGIN, DEMO_ORIGIN)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mutation.status(), StatusCode::METHOD_NOT_ALLOWED);

        let shown = app
            .oneshot(
                Request::get("/api/templates/Yokogawa%20CentumVP")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(shown.status(), StatusCode::OK);
        assert_eq!(body_json(shown).await["origin"], "builtin");
    }

    #[tokio::test]
    async fn shared_history_is_owner_scoped_and_uses_the_normal_list_shape() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        let token_a = raw_token("ab");
        let token_b = raw_token("cd");
        let now = Utc::now();
        let (_, run_a) = seed_owned_run(&state, &token_a, now).await;
        let (_, run_b) = seed_owned_run(&state, &token_b, now + Duration::seconds(1)).await;
        let app = crate::build_router(state);

        let list = app
            .clone()
            .oneshot(
                Request::get("/api/runs?limit=50")
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token_a}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(list.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(body["returned"], 1);
        assert_eq!(body["total"], 1);
        assert_eq!(body["runs"][0]["id"], run_a);

        let foreign = app
            .oneshot(
                Request::get(format!("/api/runs/{run_b}"))
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token_a}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unpersisted_history_is_empty_and_pagination_is_validated_and_capped() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.demo_policy.retained_runs_per_visitor = 1;
        state.demo_policy.max_active_runs_per_visitor = 1;
        let token = raw_token("ab");
        let now = Utc::now();
        let (_, first) = seed_owned_run(&state, &token, now).await;
        let (_, second) = seed_owned_run(&state, &token, now + Duration::seconds(1)).await;
        let app = crate::build_router(state);

        let empty = app
            .clone()
            .oneshot(
                Request::get("/api/runs")
                    .header(header::COOKIE, cookie("cd"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(empty.status(), StatusCode::OK);
        assert_eq!(body_json(empty).await["returned"], 0);

        for uri in ["/api/runs?limit=0", "/api/runs?offset=-1"] {
            let response = app
                .clone()
                .oneshot(
                    Request::get(uri)
                        .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }

        let capped = app
            .oneshot(
                Request::get("/api/runs?limit=50")
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let capped = body_json(capped).await;
        assert_eq!(capped["returned"], 2);
        assert_eq!(capped["total"], 2);
        assert_eq!(capped["runs"][0]["id"], second);
        assert_eq!(capped["runs"][1]["id"], first);
    }

    #[tokio::test]
    async fn owned_run_detail_and_last_request_include_the_stored_request_when_valid() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        let token = raw_token("ab");
        let now = Utc::now();
        let (_, run_id) = seed_owned_run(&state, &token, now).await;
        let request =
            parse_demo_request(valid_request(ProcessType::Flow, ControllerType::Pi)).unwrap();
        TuneRunRow::record_connection(
            &state.pool,
            run_id,
            None,
            None,
            &serde_json::to_string(&request).unwrap(),
        )
        .await
        .unwrap();
        let app = crate::build_router(state);

        let detail = app
            .clone()
            .oneshot(
                Request::get(format!("/api/runs/{run_id}"))
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK);
        let detail = body_json(detail).await;
        assert_eq!(detail["id"], run_id);
        assert_eq!(detail["pid_constant_tags"]["proportional"], "P");
        assert_eq!(detail["original_request"]["tagname"], DEMO_TAG_NAME);

        let last = app
            .oneshot(
                Request::get("/api/runs/last-request")
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(last.status(), StatusCode::OK);
        assert_eq!(body_json(last).await["tagname"], DEMO_TAG_NAME);
    }

    #[tokio::test]
    async fn every_run_resource_returns_404_for_a_different_owner() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.allowed_origin = Some(DEMO_ORIGIN.into());
        let owner_token = raw_token("ab");
        let other_token = raw_token("cd");
        let now = Utc::now();
        let (_, run_id) = seed_owned_run(&state, &owner_token, now).await;
        TuneRunRow::complete(&state.pool, run_id, now)
            .await
            .unwrap();
        let app = crate::build_router(state);
        let requests = [
            Request::get(format!("/api/runs/{run_id}"))
                .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={other_token}"))
                .body(Body::empty())
                .unwrap(),
            Request::get(format!("/api/runs/{run_id}/stream"))
                .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={other_token}"))
                .body(Body::empty())
                .unwrap(),
            Request::get(format!("/api/runs/{run_id}/export"))
                .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={other_token}"))
                .body(Body::empty())
                .unwrap(),
            Request::post(format!("/api/runs/{run_id}/cancel"))
                .header(header::ORIGIN, DEMO_ORIGIN)
                .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={other_token}"))
                .body(Body::empty())
                .unwrap(),
            Request::delete(format!("/api/runs/{run_id}"))
                .header(header::ORIGIN, DEMO_ORIGIN)
                .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={other_token}"))
                .body(Body::empty())
                .unwrap(),
        ];
        for request in requests {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn last_request_is_null_until_this_owner_has_started_a_run() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        let response = crate::build_router(state)
            .oneshot(
                Request::get("/api/runs/last-request")
                    .header(header::COOKIE, cookie("ab"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            b"null"
        );
    }

    #[tokio::test]
    async fn valid_shared_start_is_lazily_persisted_and_can_be_cancelled() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.allowed_origin = Some(DEMO_ORIGIN.into());
        let app = crate::build_router(state.clone());
        let start = app
            .clone()
            .oneshot(with_peer(
                Request::post("/api/runs")
                    .header(header::ORIGIN, DEMO_ORIGIN)
                    .header(header::COOKIE, cookie("ab"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        valid_request(ProcessType::Flow, ControllerType::Pi).to_string(),
                    ))
                    .unwrap(),
                "127.0.0.1:12345",
            ))
            .await
            .unwrap();
        assert_eq!(start.status(), StatusCode::CREATED);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(start.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let run_id = body["id"].as_i64().unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM demo_sessions")
                .fetch_one(&state.pool)
                .await
                .unwrap(),
            1
        );
        let cancel = app
            .clone()
            .oneshot(
                Request::post(format!("/api/runs/{run_id}/cancel"))
                    .header(header::ORIGIN, DEMO_ORIGIN)
                    .header(header::COOKIE, cookie("ab"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cancel.status(), StatusCode::NO_CONTENT);
        wait_until_inactive(&state, run_id).await;

        let second_start = app
            .clone()
            .oneshot(with_peer(
                Request::post("/api/runs")
                    .header(header::ORIGIN, DEMO_ORIGIN)
                    .header(header::COOKIE, cookie("ab"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        valid_request(ProcessType::Flow, ControllerType::Pi).to_string(),
                    ))
                    .unwrap(),
                "127.0.0.1:12345",
            ))
            .await
            .unwrap();
        assert_eq!(second_start.status(), StatusCode::CREATED);
        let second_run_id = body_json(second_start).await["id"].as_i64().unwrap();
        state.active_run.cancel(second_run_id).await;
        wait_until_inactive(&state, second_run_id).await;
    }

    #[tokio::test]
    async fn start_rejects_global_visitor_and_accepted_start_quota_exhaustion() {
        for (policy, expected) in [
            (
                DemoPolicy {
                    max_active_runs_global: 0,
                    ..DemoPolicy::default()
                },
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                DemoPolicy {
                    max_active_runs_per_visitor: 0,
                    ..DemoPolicy::default()
                },
                StatusCode::TOO_MANY_REQUESTS,
            ),
        ] {
            let base = crate::test_support::in_memory_state().await;
            let mut state =
                AppState::for_mode(base.pool, base.config_store, ServerMode::Demo, policy);
            state.allowed_origin = Some(DEMO_ORIGIN.into());
            let response = crate::build_router(state)
                .oneshot(with_peer(
                    Request::post("/api/runs")
                        .header(header::ORIGIN, DEMO_ORIGIN)
                        .header(header::COOKIE, cookie("ab"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            valid_request(ProcessType::Flow, ControllerType::Pi).to_string(),
                        ))
                        .unwrap(),
                    "127.0.0.1:12345",
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }

        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.allowed_origin = Some(DEMO_ORIGIN.into());
        state.demo_policy.accepted_starts_per_token = 1;
        state
            .demo_runtime
            .reserve_accepted_start(
                &token_hash(&raw_token("ab")),
                "127.0.0.1",
                Utc::now(),
                state.demo_policy,
            )
            .await
            .unwrap();
        let response = crate::build_router(state)
            .oneshot(with_peer(
                Request::post("/api/runs")
                    .header(header::ORIGIN, DEMO_ORIGIN)
                    .header(header::COOKIE, cookie("ab"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        valid_request(ProcessType::Flow, ControllerType::Pi).to_string(),
                    ))
                    .unwrap(),
                "127.0.0.1:12345",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn preparation_failure_releases_accepted_start_quota() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.allowed_origin = Some(DEMO_ORIGIN.into());
        state.pool.close().await;
        let response = crate::build_router(state)
            .oneshot(with_peer(
                Request::post("/api/runs")
                    .header(header::ORIGIN, DEMO_ORIGIN)
                    .header(header::COOKIE, cookie("ab"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        valid_request(ProcessType::Flow, ControllerType::Pi).to_string(),
                    ))
                    .unwrap(),
                "127.0.0.1:12345",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn scheduling_conflict_discards_the_prepared_run() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.allowed_origin = Some(DEMO_ORIGIN.into());
        state.active_run.reserve(999).await.unwrap();
        let response = crate::build_router(state.clone())
            .oneshot(with_peer(
                Request::post("/api/runs")
                    .header(header::ORIGIN, DEMO_ORIGIN)
                    .header(header::COOKIE, cookie("ab"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        valid_request(ProcessType::Flow, ControllerType::Pi).to_string(),
                    ))
                    .unwrap(),
                "127.0.0.1:12345",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(TuneRunRow::count_demo_owned(&state.pool).await.unwrap(), 0);
        state.active_run.release(999).await;
    }

    #[tokio::test]
    async fn owned_completed_stream_ends_with_exactly_one_done_event() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        let token = raw_token("ab");
        let now = Utc::now();
        let (_, run_id) = seed_owned_run(&state, &token, now).await;
        TuneRunRow::complete(&state.pool, run_id, now)
            .await
            .unwrap();
        let response = crate::build_router(state)
            .oneshot(
                Request::get(format!("/api/runs/{run_id}/stream"))
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(body.matches("event: done").count(), 1);
        assert!(body.contains("\"outcome\":\"completed\""));
        assert!(!body.contains("event: error"));
    }

    #[tokio::test]
    async fn streams_report_missing_deleted_or_unavailable_runs_and_permit_limits() {
        let base = crate::test_support::in_memory_state().await;
        let policy = DemoPolicy {
            max_sse_global: 1,
            max_sse_per_visitor: 1,
            sse_lifetime_secs: 5,
            ..DemoPolicy::default()
        };
        let state = AppState::for_mode(base.pool, base.config_store, ServerMode::Demo, policy);
        let token = raw_token("ab");
        let other = raw_token("cd");
        let now = Utc::now();
        let (_, run_id) = seed_owned_run(&state, &token, now).await;
        let (_, other_run_id) = seed_owned_run(&state, &other, now).await;
        let app = crate::build_router(state.clone());

        let missing = app
            .clone()
            .oneshot(
                Request::get("/api/runs/999999/stream")
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let held = app
            .clone()
            .oneshot(
                Request::get(format!("/api/runs/{run_id}/stream"))
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(held.status(), StatusCode::OK);

        let global_limited = app
            .clone()
            .oneshot(
                Request::get(format!("/api/runs/{other_run_id}/stream"))
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={other}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(global_limited.status(), StatusCode::SERVICE_UNAVAILABLE);
        drop(held);

        let base = crate::test_support::in_memory_state().await;
        let visitor_policy = DemoPolicy {
            max_sse_global: 2,
            max_sse_per_visitor: 1,
            sse_lifetime_secs: 5,
            ..DemoPolicy::default()
        };
        let visitor_state = AppState::for_mode(
            base.pool,
            base.config_store,
            ServerMode::Demo,
            visitor_policy,
        );
        let (_, visitor_run_id) = seed_owned_run(&visitor_state, &token, now).await;
        let visitor_app = crate::build_router(visitor_state);
        let held = visitor_app
            .clone()
            .oneshot(
                Request::get(format!("/api/runs/{visitor_run_id}/stream"))
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let visitor_limited = visitor_app
            .clone()
            .oneshot(
                Request::get(format!("/api/runs/{visitor_run_id}/stream"))
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(visitor_limited.status(), StatusCode::TOO_MANY_REQUESTS);
        drop(held);

        let deleted_before_body = app
            .clone()
            .oneshot(
                Request::get(format!("/api/runs/{run_id}/stream"))
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        TuneRunRow::delete_for_demo_session(&state.pool, run_id, 1)
            .await
            .unwrap();
        let body = body_text(deleted_before_body).await;
        assert!(body.contains("demo run is no longer available"));

        let unavailable = app
            .oneshot(
                Request::get(format!("/api/runs/{other_run_id}/stream"))
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={other}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        state.pool.close().await;
        let body = body_text(unavailable).await;
        assert!(body.contains("demo stream unavailable"));
    }

    #[tokio::test]
    async fn stream_lifetime_emits_a_generic_error_and_releases_its_permits() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.demo_policy.sse_lifetime_secs = 0;
        let token = raw_token("ab");
        let (_, run_id) = seed_owned_run(&state, &token, Utc::now()).await;
        let app = crate::build_router(state.clone());
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    Request::get(format!("/api/runs/{run_id}/stream"))
                        .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = String::from_utf8(
                to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert_eq!(body.matches("event: error").count(), 1);
            assert!(body.contains("demo stream lifetime exceeded"));
            assert!(!body.contains("event: done"));
        }
    }

    #[tokio::test]
    async fn global_demo_run_capacity_uses_the_global_policy_limit() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.demo_policy.max_tune_run_rows_global = 1;
        assert!(ensure_global_run_capacity(&state).await.is_ok());
        seed_owned_run(&state, &raw_token("ab"), Utc::now()).await;
        assert!(matches!(
            ensure_global_run_capacity(&state).await.unwrap_err(),
            ApiError::GlobalCapacity { .. }
        ));
    }

    #[tokio::test]
    async fn owned_history_retention_keeps_running_rows_and_newest_terminal_rows() {
        let mut state = crate::test_support::in_memory_state().await;
        state.demo_policy.retained_runs_per_visitor = 1;
        let token = raw_token("ab");
        let now = Utc::now();
        let (owner_id, first) = seed_owned_run(&state, &token, now).await;
        TuneRunRow::fail(&state.pool, first, now, "finished")
            .await
            .unwrap();
        let (_, second) = seed_owned_run(&state, &token, now + Duration::seconds(1)).await;
        TuneRunRow::fail(&state.pool, second, now + Duration::seconds(1), "finished")
            .await
            .unwrap();
        let (_, running) = seed_owned_run(&state, &token, now + Duration::seconds(2)).await;
        trim_owned_history(&state, owner_id).await;
        assert!(TuneRunRow::get(&state.pool, first).await.unwrap().is_none());
        assert!(
            TuneRunRow::get(&state.pool, second)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            TuneRunRow::get(&state.pool, running)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn capacity_and_retention_database_failures_are_safe() {
        let state = crate::test_support::in_memory_state().await;
        state.pool.close().await;
        assert!(matches!(
            ensure_global_run_capacity(&state).await.unwrap_err(),
            ApiError::Internal(_)
        ));
        trim_owned_history(&state, 1).await;
    }

    #[tokio::test]
    async fn cancel_delete_and_export_cover_owned_terminal_and_running_variants() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.allowed_origin = Some(DEMO_ORIGIN.into());
        let token = raw_token("ab");
        let now = Utc::now();
        let (_, running) = seed_owned_run(&state, &token, now).await;
        let (_ctrl_c, cancel_handle) = bhtune_cli::cancel::CtrlC::manual();
        let (finish, finished) = tokio::sync::oneshot::channel::<()>();
        state
            .active_run
            .start(running, cancel_handle, async move {
                let _ = finished.await;
            })
            .await
            .unwrap();
        let (_, empty_terminal) = seed_owned_run(&state, &token, now + Duration::seconds(1)).await;
        TuneRunRow::complete(&state.pool, empty_terminal, now)
            .await
            .unwrap();
        let (_, sampled_terminal) =
            seed_owned_run(&state, &token, now + Duration::seconds(2)).await;
        insert_one_sample(&state, sampled_terminal, now).await;
        TuneRunRow::complete(&state.pool, sampled_terminal, now)
            .await
            .unwrap();
        let app = crate::build_router(state.clone());

        let cancel = app
            .clone()
            .oneshot(
                Request::post(format!("/api/runs/{running}/cancel"))
                    .header(header::ORIGIN, DEMO_ORIGIN)
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cancel.status(), StatusCode::NO_CONTENT);
        finish.send(()).unwrap();
        wait_until_inactive(&state, running).await;

        let conflict = app
            .clone()
            .oneshot(
                Request::delete(format!("/api/runs/{running}"))
                    .header(header::ORIGIN, DEMO_ORIGIN)
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);

        let empty_export = app
            .clone()
            .oneshot(
                Request::get(format!("/api/runs/{empty_terminal}/export"))
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(empty_export.status(), StatusCode::NOT_FOUND);

        for (format, content_type, extension) in [
            ("csv", "text/csv", "csv"),
            ("json", "application/json", "json"),
        ] {
            let export = app
                .clone()
                .oneshot(
                    Request::get(format!(
                        "/api/runs/{sampled_terminal}/export?format={format}"
                    ))
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(export.status(), StatusCode::OK);
            assert_eq!(
                export.headers().get(header::CONTENT_TYPE).unwrap(),
                content_type
            );
            assert_eq!(
                export.headers().get(header::CONTENT_DISPOSITION).unwrap(),
                &format!("attachment; filename=\"demo-run-{sampled_terminal}.{extension}\"")
            );
            assert!(!body_text(export).await.is_empty());
        }

        let deleted = app
            .oneshot(
                Request::delete(format!("/api/runs/{sampled_terminal}"))
                    .header(header::ORIGIN, DEMO_ORIGIN)
                    .header(header::COOKIE, format!("{DEMO_COOKIE_NAME}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    }
}
