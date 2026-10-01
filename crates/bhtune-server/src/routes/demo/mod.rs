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
use bhtune_core::{ControllerDirection, template::built_in_templates};
use bhtune_db::models::{
    DemoSessionRow, Pagination, TemplateOrigin, TuneDriver, TuneMvActuationRow, TuneOutcome,
    TuneResultRow, TuneRunRow, TuneSampleRow, TuneWriteRow,
};
use bhtune_runtime::config::{
    DEMO_COOKIE_NAME, DEMO_CYCLES_COUNT_DEFAULT, DEMO_CYCLES_COUNT_MAX, DEMO_CYCLES_COUNT_MIN,
    DEMO_CYCLES_SKIP_DEFAULT, DEMO_CYCLES_SKIP_MAX, DEMO_CYCLES_SKIP_MIN,
    DEMO_NOISE_PROTECTION_SECS_DEFAULT, DEMO_NOISE_PROTECTION_SECS_MAX,
    DEMO_NOISE_PROTECTION_SECS_MIN, DEMO_RANGE_ENDPOINT_MAX, DEMO_RANGE_ENDPOINT_MIN,
    DEMO_RANGE_HIGH, DEMO_RANGE_LOW, DEMO_RANGE_SPAN_MAX, DEMO_RANGE_SPAN_MIN,
    DEMO_RELAY_AMP_DEFAULT, DEMO_RELAY_AMP_MAX, DEMO_RELAY_AMP_MIN, DEMO_SIM_DEAD_TIME_DEFAULT,
    DEMO_SIM_DEAD_TIME_MAX, DEMO_SIM_DEAD_TIME_MIN, DEMO_SIM_GAIN_DEFAULT, DEMO_SIM_GAIN_MAX,
    DEMO_SIM_GAIN_MIN, DEMO_SIM_INITIAL_VALUE_DEFAULT, DEMO_SIM_NOISE_DEFAULT,
    DEMO_SIM_NOISE_MAX_PV_SPAN_FRACTION, DEMO_SIM_SEED_DEFAULT, DEMO_SIM_SEED_MAX,
    DEMO_SIM_TAU_DEFAULT, DEMO_SIM_TAU_MAX, DEMO_SIM_TAU_MIN, DEMO_TAG_NAME, DemoPolicy,
    ServerMode,
};
use bhtune_runtime::tune::{drive, prepare_owned};
use chrono::{Duration, Utc};
use rand::random;
use sha2::{Digest, Sha256};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

use crate::AppState;
use crate::error::{ApiError, require_present};
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

mod dto;
mod handlers;
mod helpers;
mod validation;

pub(crate) use handlers::{
    cancel_run, delete_run, export_run, get_run, get_template, last_request, list_runs,
    list_templates, start_run, stream_run,
};
use helpers::api_not_found;
pub(crate) use helpers::{PeerAddress, ordinary_request_permit, session_cookie_header};

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
