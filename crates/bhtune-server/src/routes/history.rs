//! Run-history routes: `GET /api/runs` (filtered, paginated list), `GET /api/runs/{id}`
//! (full run detail: config, initial readings, samples, results, writes), `GET
//! /api/runs/{id}/export` (CSV/JSON sample export), and `DELETE /api/runs/{id}`.
//!
//! DTO shapes deliberately mirror `bhtune-cli`'s `commands::history` `--output json` JSON
//! (`RunSummaryJson`/`RunDetailJson`/etc.) field-for-field, so the CLI and the HTTP API
//! describe the same run the same way -- one shape for the product's two faces, per this
//! workspace's DTO-decoupling convention (every JSON-facing consumer builds its own
//! projection of the non-`Serialize` `bhtune-db` row types, rather than the row types
//! themselves growing a `Serialize` impl). The one deliberate addition over the CLI's own
//! `RunDetailJson` is a full `samples` array (not just a `samples_recorded` count) -- the
//! trend chart (`history-explorer-ui`) needs the raw per-tick data, and the data-volume math
//! in AGENTS.md's "History explorer" section (thousands of rows per run, not millions) says
//! inlining it is cheap enough not to need its own paginated route.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bhtune_core::{
    ControllerDirection, ControllerType, LoopConfig, ProcessType, ResponseLevel, Tick,
};
use bhtune_db::models::{
    Pagination, RestoreStatus, RollbackState, SampleQuality, TemplateOrigin, TuneDriver,
    TuneOutcome, TuneResultRow, TuneRunFilter, TuneRunRow, TuneSampleRow, TuneWriteRow, WriteKind,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::error::{ApiError, ErrorBody};
use crate::routes::runs::StartRunRequest;
use crate::state::AppState;

/// Query parameters for `GET /api/runs`, mirroring [`TuneRunFilter`]'s fields one-to-one
/// plus [`Pagination`]. Every field is optional; an absent `limit`/`offset` falls back to
/// [`Pagination::default`] (50 rows, offset 0), matching the CLI's own default page size.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RunListQuery {
    pub loop_id: Option<i64>,
    pub process_type: Option<ProcessType>,
    pub controller_type: Option<ControllerType>,
    pub outcome: Option<TuneOutcome>,
    pub driver: Option<TuneDriver>,
    pub started_after: Option<DateTime<Utc>>,
    pub started_before: Option<DateTime<Utc>>,
    pub template_name: Option<String>,
    pub template_origin: Option<TemplateOrigin>,
    /// Filters on the run's recorded, *resolved* OPC server (`db-run-request-snapshot`) --
    /// always absent for a simulator/replay run, so this filter alone never matches one.
    pub opc_server: Option<String>,
    /// Filters on the run's recorded, resolved bridge host, matching `opc_server` above.
    pub bridge_host: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn filter_from_query(query: &RunListQuery) -> TuneRunFilter {
    let mut filter = TuneRunFilter::default();
    if let Some(v) = query.loop_id {
        filter = filter.with_loop_id(v);
    }
    if let Some(v) = query.process_type {
        filter = filter.with_process_type(v);
    }
    if let Some(v) = query.controller_type {
        filter = filter.with_controller_type(v);
    }
    if let Some(v) = query.outcome {
        filter = filter.with_outcome(v);
    }
    if let Some(v) = query.driver {
        filter = filter.with_driver(v);
    }
    if let Some(v) = query.started_after {
        filter = filter.with_started_after(v);
    }
    if let Some(v) = query.started_before {
        filter = filter.with_started_before(v);
    }
    if let Some(v) = &query.template_name {
        filter = filter.with_template_name(v.clone());
    }
    if let Some(v) = query.template_origin {
        filter = filter.with_template_origin(v);
    }
    if let Some(v) = &query.opc_server {
        filter = filter.with_opc_server(v.clone());
    }
    if let Some(v) = &query.bridge_host {
        filter = filter.with_bridge_host(v.clone());
    }
    filter
}

/// One run in `GET /api/runs`'s `runs` array -- deliberately a subset matching the CLI's own
/// `history list` table columns, not the full detail (that's [`RunDetailResponse`], for
/// `GET /api/runs/{id}`).
#[derive(Debug, Serialize, ToSchema)]
pub struct RunSummaryResponse {
    pub id: i64,
    pub tag_name: String,
    pub notes: Option<String>,
    pub driver: TuneDriver,
    pub outcome: TuneOutcome,
    pub process_type: ProcessType,
    pub started_at: DateTime<Utc>,
}

impl From<&TuneRunRow> for RunSummaryResponse {
    fn from(run: &TuneRunRow) -> Self {
        RunSummaryResponse {
            id: run.id,
            tag_name: run.loop_name.clone(),
            notes: run.notes.clone(),
            driver: run.driver,
            outcome: run.outcome,
            process_type: run.config.process_type,
            started_at: run.started_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RunListResponse {
    pub runs: Vec<RunSummaryResponse>,
    /// How many rows are in `runs` (this page) -- distinct from `total`, the count of every
    /// run matching the filter across all pages.
    pub returned: usize,
    pub total: i64,
}

/// List tune runs, filtered and paginated.
///
/// `GET /api/runs` -- newest-started-first, filtered by every present [`RunListQuery`] field,
/// one [`Pagination`] page at a time.
#[utoipa::path(
    get,
    path = "/api/runs",
    tag = "runs",
    params(RunListQuery),
    responses(
        (status = 200, description = "A page of runs matching the filter.", body = RunListResponse),
    ),
)]
pub(crate) async fn list_runs(
    State(state): State<AppState>,
    Query(query): Query<RunListQuery>,
) -> Result<Json<RunListResponse>, ApiError> {
    let filter = filter_from_query(&query);
    let pagination = Pagination::new(
        query.limit.unwrap_or_else(|| Pagination::default().limit),
        query.offset.unwrap_or(0),
    );
    let runs = TuneRunRow::list(&state.pool, &filter, pagination).await?;
    let total = TuneRunRow::count(&state.pool, &filter).await?;
    Ok(Json(RunListResponse {
        returned: runs.len(),
        runs: runs.iter().map(RunSummaryResponse::from).collect(),
        total,
    }))
}

/// The most recently started run's original request, if any.
///
/// `GET /api/runs/last-request` -- returns the newest run's `request_json`
/// (`db-run-request-snapshot`), parsed back into a [`StartRunRequest`], or `null` on a fresh
/// install with no runs yet, or if the newest run's stored request isn't usable (see
/// [`parse_stored_request`]) (`ui-prefill-last-run`). The New tune form seeds itself from this
/// response on load, so connection details, tag names, ranges, and every other field an
/// engineer typed follow them across browsers and machines instead of resetting to hardcoded
/// defaults on every visit -- deliberately server-side rather than `localStorage` for that
/// reason. "Newest" means newest by `started_at`, matching `GET /api/runs`'s own ordering,
/// regardless of that run's `outcome` -- a still-`running` run is a perfectly good source of
/// "what was just submitted" to prefill from.
#[utoipa::path(
    get,
    path = "/api/runs/last-request",
    tag = "runs",
    responses(
        (status = 200, description = "The newest run's original request, or `null` if no runs exist yet or its request isn't usable.", body = Option<StartRunRequest>),
    ),
)]
pub(crate) async fn last_request(
    State(state): State<AppState>,
) -> Result<Json<Option<StartRunRequest>>, ApiError> {
    let newest =
        TuneRunRow::list(&state.pool, &TuneRunFilter::default(), Pagination::first(1)).await?;
    let Some(run) = newest.into_iter().next() else {
        return Ok(Json(None));
    };
    Ok(Json(parse_stored_request(run.id, &run.request_json)))
}

/// Parses a run's stored `request_json` back into a [`StartRunRequest`], or `None` if it
/// isn't usable -- shared by [`last_request`] and [`build_run_detail`]'s `original_request`
/// field, both of which exist to *prefill a form*, not to guarantee every historical row is
/// well-formed. A row created by `prepare()` (every real CLI- or HTTP-started run) always
/// parses; this only fails for a row that predates `db-run-request-snapshot`, or one poked
/// at directly through the SQLite file itself -- a supported way to interact with bhtune's
/// data, per this project's "just an open SQLite db, nothing hidden" design goal. Either way,
/// the honest response is "nothing to prefill from", logged at `warn` so the data quality
/// issue is visible without failing the request a real user is waiting on.
fn parse_stored_request(run_id: i64, request_json: &str) -> Option<StartRunRequest> {
    match serde_json::from_str(request_json) {
        Ok(request) => Some(request),
        Err(e) => {
            tracing::warn!(
                run_id,
                error = %e,
                "run's stored request_json did not parse as StartRunRequest; treating as unavailable"
            );
            None
        }
    }
}

/// Local projection of [`bhtune_db::models::TuneRunInitialReadings`] -- see this module's
/// doc comment for why every JSON-facing type here is its own projection rather than a
/// `Serialize` impl on the `bhtune-db` row type.
#[derive(Debug, Serialize, ToSchema)]
pub struct InitialReadingsResponse {
    pub pv_ini: f32,
    pub mv_ini: f32,
    pub mv_range_low: f32,
    pub mv_range_high: f32,
    pub pv_range_high: f32,
    pub pv_range_low: f32,
    pub controller_direction: ControllerDirection,
    pub mode_raw: Option<String>,
    pub mode_attribute_raw: Option<String>,
    pub setpoint_ini: Option<f32>,
}

impl From<bhtune_db::models::TuneRunInitialReadings> for InitialReadingsResponse {
    fn from(r: bhtune_db::models::TuneRunInitialReadings) -> Self {
        InitialReadingsResponse {
            pv_ini: r.pv_ini,
            mv_ini: r.mv_ini,
            mv_range_low: r.mv_range_low,
            mv_range_high: r.mv_range_high,
            pv_range_high: r.pv_range_high,
            pv_range_low: r.pv_range_low,
            controller_direction: r.controller_direction,
            mode_raw: r.mode_raw,
            mode_attribute_raw: r.mode_attribute_raw,
            setpoint_ini: r.setpoint_ini,
        }
    }
}

/// One recorded tick: the [`Tick`] input and resulting engine state, plus the driver-
/// reported PV quality at read time. `Tick`/`MrftState` already derive `Serialize` in
/// `bhtune-core` (they round-trip through golden-trace fixtures too), so they're embedded
/// directly rather than re-projected field-by-field like the other DTOs here.
#[derive(Debug, Serialize, ToSchema)]
pub struct SampleResponse {
    pub tick_index: i64,
    pub sample: Tick,
    pub state: bhtune_core::MrftState,
    pub pv_quality: SampleQuality,
}

impl From<&TuneSampleRow> for SampleResponse {
    fn from(row: &TuneSampleRow) -> Self {
        SampleResponse {
            tick_index: row.tick_index,
            sample: row.sample,
            state: row.state,
            pv_quality: row.pv_quality,
        }
    }
}

/// Local projection of [`TuneResultRow`].
#[derive(Debug, Serialize, ToSchema)]
pub struct ResultResponse {
    pub response_level: ResponseLevel,
    pub kp: f32,
    pub ti_minutes: f32,
    pub td_minutes: f32,
    pub proportional: f32,
    pub integral: f32,
    pub derivative: f32,
}

impl From<&TuneResultRow> for ResultResponse {
    fn from(r: &TuneResultRow) -> Self {
        ResultResponse {
            response_level: r.response_level,
            kp: r.kp,
            ti_minutes: r.ti_minutes,
            td_minutes: r.td_minutes,
            proportional: r.proportional,
            integral: r.integral,
            derivative: r.derivative,
        }
    }
}

/// Local projection of [`TuneWriteRow`].
#[derive(Debug, Serialize, ToSchema)]
pub struct WriteResponse {
    pub kind: WriteKind,
    pub response_level: ResponseLevel,
    pub written_at: DateTime<Utc>,
    pub proportional_previous: Option<f32>,
    pub integral_previous: Option<f32>,
    pub derivative_previous: Option<f32>,
    pub proportional_written: Option<f32>,
    pub integral_written: Option<f32>,
    pub derivative_written: Option<f32>,
    pub proportional_readback: Option<f32>,
    pub integral_readback: Option<f32>,
    pub derivative_readback: Option<f32>,
    pub success: bool,
    pub error_message: Option<String>,
    pub rollback_state: Option<RollbackState>,
    pub rollback_error: Option<String>,
}

impl From<&TuneWriteRow> for WriteResponse {
    fn from(w: &TuneWriteRow) -> Self {
        WriteResponse {
            kind: w.kind,
            response_level: w.response_level,
            written_at: w.written_at,
            proportional_previous: w.previous.map(|p| p.proportional),
            integral_previous: w.previous.map(|p| p.integral),
            derivative_previous: w.previous.map(|p| p.derivative),
            proportional_written: w.proportional_written,
            integral_written: w.integral_written,
            derivative_written: w.derivative_written,
            proportional_readback: w.proportional_readback,
            integral_readback: w.integral_readback,
            derivative_readback: w.derivative_readback,
            success: w.success,
            error_message: w.error_message.clone(),
            rollback_state: w.rollback_state,
            rollback_error: w.rollback_error.clone(),
        }
    }
}

/// A run's snapshotted PID constant tag names, present only when all three were configured.
/// Nested under `RunDetailResponse::pid_constant_tags` following the same
/// "`Option<...>` presence itself is the signal" convention `initial_readings` already uses,
/// rather than a separate boolean plus three more nullable top-level fields.
#[derive(Debug, Serialize, ToSchema)]
pub struct PidConstantTagsResponse {
    pub proportional: String,
    pub integral: String,
    pub derivative: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RunDetailResponse {
    pub id: i64,
    pub tag_name: String,
    pub notes: Option<String>,
    pub driver: TuneDriver,
    pub outcome: TuneOutcome,
    pub failure_reason: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Name of the template snapshotted onto this run at start time -- not necessarily what
    /// `template_name` currently resolves to in the catalog (`safety-run-snapshot`).
    pub template_name: String,
    pub template_origin: TemplateOrigin,
    pub config: LoopConfig,
    /// The resolved OPC DA server ProgID this run actually used, or `None` for a
    /// simulator/replay run (`db-run-request-snapshot`). This is what `history revert`
    /// trusts over any `--server` flag -- see `bhtune-cli::commands::history`.
    pub opc_server: Option<String>,
    /// The resolved bridge host this run actually used, matching `opc_server` above.
    pub bridge_host: Option<String>,
    /// `Some` exactly when `routes::runs::require_writable_run`'s tag-presence check would
    /// pass -- i.e. when all three PID constant tags were configured on this run. The
    /// frontend uses this (together with `driver`/`outcome`/`opc_server`/`bridge_host`) to
    /// decide whether the post-hoc write/revert buttons are enabled and, when they are not,
    /// to explain why -- without duplicating `require_writable_run`'s logic client-side or
    /// discovering ineligibility only after a failed request (`api-post-run-write`).
    pub pid_constant_tags: Option<PidConstantTagsResponse>,
    pub initial_readings: Option<InitialReadingsResponse>,
    pub samples: Vec<SampleResponse>,
    pub results: Vec<ResultResponse>,
    pub writes: Vec<WriteResponse>,
    pub restore_status: Option<RestoreStatus>,
    pub restore_detail: Option<String>,
    /// This run's own `request_json` (`db-run-request-snapshot`), parsed back into a
    /// [`StartRunRequest`], or `None` if it isn't usable -- see [`parse_stored_request`].
    /// Powers the run detail page's "Duplicate this run" action (`ui-prefill-last-run`):
    /// unlike `GET /api/runs/last-request`, which only ever answers for the single newest
    /// run, this lets the New tune form seed itself from *this specific* historical run
    /// regardless of how many later runs exist.
    pub original_request: Option<StartRunRequest>,
}

/// Builds the full `RunDetailResponse` for one run, or `Ok(None)` if no run has that id --
/// shared by `show_run` (`GET /api/runs/{id}`, which maps `None` to a 404) and
/// `routes::runs::start_run` (`POST /api/runs`'s `201` body is the very same detail view of
/// the run it just created, so both routes describe a run identically rather than the HTTP
/// API growing two different shapes for "what a run looks like").
pub(crate) async fn build_run_detail(
    pool: &bhtune_db::SqlitePool,
    run_id: i64,
) -> Result<Option<RunDetailResponse>, ApiError> {
    let Some(run) = TuneRunRow::get(pool, run_id).await? else {
        return Ok(None);
    };
    let samples = TuneSampleRow::list_for_run(pool, run_id).await?;
    let results = TuneResultRow::list_for_run(pool, run_id).await?;
    let writes = TuneWriteRow::list_for_run(pool, run_id).await?;
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
        config: run.config,
        opc_server: run.opc_server,
        bridge_host: run.bridge_host,
        pid_constant_tags,
        initial_readings: run.initial_readings.map(InitialReadingsResponse::from),
        samples: samples.iter().map(SampleResponse::from).collect(),
        results: results.iter().map(ResultResponse::from).collect(),
        writes: writes.iter().map(WriteResponse::from).collect(),
        restore_status: run.restore_status,
        restore_detail: run.restore_detail,
        original_request: parse_stored_request(run.id, &run.request_json),
    }))
}

/// Fetch one run's full detail.
///
/// `GET /api/runs/{id}` -- 404 if no run has that id.
#[utoipa::path(
    get,
    path = "/api/runs/{id}",
    tag = "runs",
    params(
        ("id" = i64, Path, description = "Run id"),
    ),
    responses(
        (status = 200, description = "The full recorded detail for one run.", body = RunDetailResponse),
        (status = 404, description = "No run with that id.", body = ErrorBody),
    ),
)]
pub(crate) async fn show_run(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
) -> Result<Json<RunDetailResponse>, ApiError> {
    build_run_detail(&state.pool, run_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("no run with id {run_id}")))
}

/// Format for `GET /api/runs/{id}/export` -- deliberately a local, HTTP-facing enum rather
/// than reusing `bhtune_cli::args::ExportFormat` directly: that type is `clap`-oriented
/// (`ValueEnum`) and has no `Deserialize`/`ToSchema`, matching this module's own
/// DTO-decoupling convention (see the module doc comment). Converted to
/// `bhtune_cli::args::ExportFormat` at the one call site that needs it ([`export_run`]), so
/// the actual CSV/JSON serialization (`bhtune_cli::commands::export::samples_to_bytes`) is
/// implemented exactly once and the CLI's `bhtune export` and this route can never disagree
/// about what a run's export looks like.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunExportFormat {
    Csv,
    Json,
}

impl From<RunExportFormat> for bhtune_cli::args::ExportFormat {
    fn from(format: RunExportFormat) -> Self {
        match format {
            RunExportFormat::Csv => bhtune_cli::args::ExportFormat::Csv,
            RunExportFormat::Json => bhtune_cli::args::ExportFormat::Json,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RunExportQuery {
    /// Defaults to `csv` when omitted, matching `bhtune export`'s own CLI default.
    pub format: Option<RunExportFormat>,
}

/// Export one run's recorded samples as CSV or JSON.
///
/// `GET /api/runs/{id}/export?format=csv|json` -- 404 if no run has that id or it has no
/// recorded samples yet. Defaults to CSV. Sets `Content-Disposition: attachment` so a
/// browser downloads the response as a file rather than rendering it.
#[utoipa::path(
    get,
    path = "/api/runs/{id}/export",
    tag = "runs",
    params(
        ("id" = i64, Path, description = "Run id"),
        RunExportQuery,
    ),
    responses(
        (status = 200, description = "The run's recorded samples, as CSV (default) or JSON.", content_type = "text/csv"),
        (status = 404, description = "No run with that id, or it has no recorded samples.", body = ErrorBody),
    ),
)]
pub(crate) async fn export_run(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
    Query(query): Query<RunExportQuery>,
) -> Result<Response, ApiError> {
    let format = query.format.unwrap_or(RunExportFormat::Csv);
    let samples = TuneSampleRow::list_for_run(&state.pool, run_id).await?;
    if samples.is_empty() {
        return Err(ApiError::NotFound(format!(
            "run {run_id} has no recorded samples (unknown run id, or it never started)"
        )));
    }
    let bytes = bhtune_cli::commands::export::samples_to_bytes(&samples, format.into())?;
    let (content_type, extension) = match format {
        RunExportFormat::Csv => ("text/csv", "csv"),
        RunExportFormat::Json => ("application/json", "json"),
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"run-{run_id}.{extension}\""
        ))
        .map_err(|e| ApiError::Internal(e.into()))?,
    );
    Ok(response)
}

/// Delete one run and its recorded samples/results/write-back audit rows.
///
/// `DELETE /api/runs/{id}` -- 404 if no run has that id, 409 if the run's own recorded
/// `outcome` is still [`TuneOutcome::Running`] (deleting the row out from under an in-flight
/// task would corrupt whatever it tries to write next; cancel it first). Deliberately checks
/// the run row's own `outcome` rather than [`crate::active_run::ActiveRun`]'s in-memory
/// active-run slot: `drive()` persists every terminal outcome (`persist_results` then
/// `TuneRunRow::complete`/`fail`/`abort`) *before* returning, and `ActiveRun::release` only
/// runs strictly after `drive()` returns (see `routes::runs::start_run`'s spawned task), so
/// there is a real -- if brief -- window where a run's outcome is already durably
/// `completed`/`failed`/`aborted` but `ActiveRun` hasn't been told the slot is free yet.
/// Checking the DB's own authoritative, durable state instead of the best-effort in-memory
/// tracker closes that race outright, rather than requiring the caller to retry (as
/// `frontend/e2e/tune.spec.ts`'s `startTune()` already has to for the equivalent gap on the
/// *start* side).
#[utoipa::path(
    delete,
    path = "/api/runs/{id}",
    tag = "runs",
    params(
        ("id" = i64, Path, description = "Run id"),
    ),
    responses(
        (status = 204, description = "Run deleted."),
        (status = 404, description = "No run with that id.", body = ErrorBody),
        (status = 409, description = "The run has not finished yet.", body = ErrorBody),
    ),
)]
pub(crate) async fn delete_run(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let run = TuneRunRow::get(&state.pool, run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no run with id {run_id}")))?;
    if run.outcome == TuneOutcome::Running {
        return Err(ApiError::Conflict(format!(
            "run {run_id} has not finished yet; cancel it before deleting"
        )));
    }
    if TuneRunRow::delete(&state.pool, run_id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        // Only reachable if the row was deleted by a concurrent request between the
        // `get` above and this call -- still a well-defined 404 ("no run with that id"
        // is simply true again by the time this responds), not a real error.
        Err(ApiError::NotFound(format!("no run with id {run_id}")))
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/runs", get(list_runs))
        .route("/api/runs/last-request", get(last_request))
        .route("/api/runs/{id}", get(show_run).delete(delete_run))
        .route("/api/runs/{id}/export", get(export_run))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn seed_one_run(state: &AppState) -> i64 {
        let template_row =
            bhtune_db::models::DcsTemplateRow::get_by_name(&state.pool, "Yokogawa CentumVP")
                .await
                .unwrap()
                .unwrap();
        let template = template_row.template;
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 3,
            noise_protection_secs: 0,
            mrft_delay_secs: 0,
        };
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Loop1.PV", &template);
        let run = TuneRunRow::start(
            &state.pool,
            None,
            "Loop1",
            TuneDriver::Simulator,
            config,
            template_row.origin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        run.id
    }

    /// Seeds a run carrying real data in every optional slot `seed_one_run` leaves empty --
    /// initial readings, a sample, a result, and both a successful and a rolled-back write --
    /// so every `From` impl in this module and every `filter_from_query` branch actually runs
    /// at least once. Returns `(run_id, loop_id)`: unlike `seed_one_run`'s ad hoc
    /// `loop_id = None` run, this one is attached to a real `loops` row so the `loop_id`
    /// filter has something to match against.
    async fn seed_full_run(state: &AppState) -> (i64, i64) {
        let template_row =
            bhtune_db::models::DcsTemplateRow::get_by_name(&state.pool, "Yokogawa CentumVP")
                .await
                .unwrap()
                .unwrap();
        let template = template_row.template;
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 3,
            noise_protection_secs: 0,
            mrft_delay_secs: 0,
        };
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Loop2.PV", &template);
        let now = Utc::now();

        let loop_id = sqlx::query(
            r#"
            INSERT INTO loops (
                name, dcs_template_id, tags_json, process_type, controller_type,
                relay_amp_percent, num_cycles_skip, num_cycles_count, noise_protection_secs,
                mrft_delay_secs, created_at, updated_at
            ) VALUES ('Loop2', ?, '{}', 'flow', 'pi', 5.0, 1, 3, 0, 0, ?, ?)
            "#,
        )
        .bind(template_row.id)
        .bind(now)
        .bind(now)
        .execute(&state.pool)
        .await
        .unwrap()
        .last_insert_rowid();

        let run = TuneRunRow::start(
            &state.pool,
            Some(loop_id),
            "Loop2",
            TuneDriver::Simulator,
            config,
            template_row.origin,
            &template,
            &tags,
            now,
        )
        .await
        .unwrap();
        let run_id = run.id;

        TuneRunRow::record_initial_readings(
            &state.pool,
            run_id,
            bhtune_db::models::TuneRunInitialReadings {
                pv_ini: 50.0,
                mv_ini: 45.0,
                mv_range_low: 0.0,
                mv_range_high: 100.0,
                pv_range_high: 100.0,
                pv_range_low: 0.0,
                controller_direction: ControllerDirection::Direct,
                mode_raw: Some("1".to_string()),
                mode_attribute_raw: Some("2".to_string()),
                setpoint_ini: Some(50.0),
            },
        )
        .await
        .unwrap();

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
                cycles_remaining: 3,
            },
            SampleQuality::Good,
        )
        .await
        .unwrap();

        TuneResultRow::insert(
            &state.pool,
            &TuneResultRow {
                id: 0,
                run_id,
                response_level: ResponseLevel::Moderate,
                kp: 1.5,
                ti_minutes: 2.0,
                td_minutes: 0.0,
                proportional: 66.7,
                integral: 2.0,
                derivative: 0.0,
            },
        )
        .await
        .unwrap();

        // A confirmed write: previous/written/readback all populated, no rollback needed.
        let mut successful_write =
            bhtune_db::models::NewTuneWrite::new(ResponseLevel::Moderate, now);
        successful_write.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 60.0,
            integral: 3.0,
            derivative: 0.0,
        });
        successful_write.proportional_written = Some(66.7);
        successful_write.integral_written = Some(2.0);
        successful_write.derivative_written = Some(0.0);
        successful_write.proportional_readback = Some(66.7);
        successful_write.integral_readback = Some(2.0);
        successful_write.derivative_readback = Some(0.0);
        successful_write.success = true;
        TuneWriteRow::insert(&state.pool, run_id, successful_write)
            .await
            .unwrap();

        // A rejected write that was rolled back: readback stays unset (never confirmed) and
        // `rollback_state` is populated -- exercises the `Option`/`success = false` branches
        // `WriteResponse::from` otherwise never reaches.
        let mut failed_write = bhtune_db::models::NewTuneWrite::new(ResponseLevel::Aggressive, now);
        failed_write.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 60.0,
            integral: 3.0,
            derivative: 0.0,
        });
        failed_write.proportional_written = Some(100.0);
        failed_write.success = false;
        failed_write.error_message = Some("write rejected: value out of range".to_string());
        failed_write.rollback_state = Some(RollbackState::Succeeded);
        TuneWriteRow::insert(&state.pool, run_id, failed_write)
            .await
            .unwrap();

        TuneRunRow::complete(&state.pool, run_id, now)
            .await
            .unwrap();

        (run_id, loop_id)
    }

    #[tokio::test]
    async fn list_runs_returns_empty_when_no_runs_exist() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(Request::get("/api/runs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["total"], 0);
        assert_eq!(body["returned"], 0);
        assert!(body["runs"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_runs_returns_a_seeded_run() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_one_run(&state).await;
        let app = router().with_state(state);
        let response = app
            .oneshot(Request::get("/api/runs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["total"], 1);
        assert_eq!(body["runs"][0]["id"], run_id);
        assert_eq!(body["runs"][0]["process_type"], "flow");
    }

    #[tokio::test]
    async fn list_runs_filters_by_outcome() {
        let state = crate::test_support::in_memory_state().await;
        seed_one_run(&state).await;
        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::get("/api/runs?outcome=completed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        // The seeded run is still `running` (never completed), so filtering for `completed`
        // must exclude it.
        assert_eq!(body["total"], 0);
    }

    #[tokio::test]
    async fn show_run_returns_full_detail() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_one_run(&state).await;
        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["id"], run_id);
        assert_eq!(body["template_name"], "Yokogawa CentumVP");
        assert!(body["samples"].as_array().unwrap().is_empty());
        assert!(body["results"].as_array().unwrap().is_empty());
        assert!(body["writes"].as_array().unwrap().is_empty());
        // Yokogawa CentumVP always defines P/I/D suffixes, so a run derived from it always
        // has all three PID constant tags -- see `pid_constant_tags`'s doc comment.
        assert_eq!(body["pid_constant_tags"]["proportional"], "Loop1.P");
        assert_eq!(body["pid_constant_tags"]["integral"], "Loop1.I");
        assert_eq!(body["pid_constant_tags"]["derivative"], "Loop1.D");
        // `seed_one_run` never calls `record_connection`, so `request_json` is left at the
        // column default `"{}"` -- not a valid `StartRunRequest`, so `original_request` must
        // gracefully read `null` rather than the request failing (see
        // `parse_stored_request`'s doc comment).
        assert!(body["original_request"].is_null());
    }

    /// `pid_constant_tags` must be `null`, not merely three `null` fields, when the run's
    /// snapshotted tags lack any of the three PID constants -- exactly the case
    /// `routes::runs::require_writable_run` refuses a post-hoc write/revert for (see that
    /// module's own `write_run_returns_400_when_run_has_no_pid_constant_tags` test).
    #[tokio::test]
    async fn show_run_reports_no_pid_constant_tags_as_a_null_pid_constant_tags_field() {
        let state = crate::test_support::in_memory_state().await;
        let template_row =
            bhtune_db::models::DcsTemplateRow::get_by_name(&state.pool, "Yokogawa CentumVP")
                .await
                .unwrap()
                .unwrap();
        let template = template_row.template;
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 3,
            noise_protection_secs: 0,
            mrft_delay_secs: 0,
        };
        let mut tags = bhtune_core::LoopTags::derive_from_pv_tag("Loop3.PV", &template);
        tags.proportional_constant = None;
        tags.integral_constant = None;
        tags.derivative_constant = None;
        let run = TuneRunRow::start(
            &state.pool,
            None,
            "Loop3",
            TuneDriver::Simulator,
            config,
            template_row.origin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{}", run.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert!(body["pid_constant_tags"].is_null());
    }

    #[tokio::test]
    async fn show_run_returns_full_detail_with_samples_results_and_writes() {
        let state = crate::test_support::in_memory_state().await;
        let (run_id, _loop_id) = seed_full_run(&state).await;
        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;

        assert_eq!(body["id"], run_id);
        assert_eq!(body["outcome"], "completed");
        assert!(body["completed_at"].is_string());
        // seed_full_run uses the simulator driver and never calls `record_connection`, so
        // both connection fields are null -- see `list_runs_filters_by_opc_server_and_bridge_host`
        // below for the opcda-with-a-recorded-connection case.
        assert!(body["opc_server"].is_null());
        assert!(body["bridge_host"].is_null());
        assert_eq!(body["pid_constant_tags"]["proportional"], "Loop2.P");
        assert_eq!(body["pid_constant_tags"]["integral"], "Loop2.I");
        assert_eq!(body["pid_constant_tags"]["derivative"], "Loop2.D");

        let initial_readings = &body["initial_readings"];
        assert_eq!(initial_readings["pv_ini"], 50.0);
        assert_eq!(initial_readings["mv_ini"], 45.0);
        assert_eq!(initial_readings["controller_direction"], "direct");
        assert_eq!(initial_readings["mode_raw"], "1");
        assert_eq!(initial_readings["mode_attribute_raw"], "2");
        assert_eq!(initial_readings["setpoint_ini"], 50.0);

        let samples = body["samples"].as_array().unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0]["tick_index"], 0);
        assert_eq!(samples[0]["sample"]["pv"], 50.0);
        assert_eq!(samples[0]["state"]["mv_value_current"], 45.0);
        assert_eq!(samples[0]["pv_quality"], "good");

        let results = body["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["response_level"], "moderate");
        assert_eq!(results[0]["kp"], 1.5);
        assert_eq!(results[0]["proportional"], 66.7);

        let writes = body["writes"].as_array().unwrap();
        assert_eq!(writes.len(), 2);
        let successful = writes
            .iter()
            .find(|w| w["response_level"] == "moderate")
            .unwrap();
        assert_eq!(successful["success"], true);
        assert_eq!(successful["proportional_previous"], 60.0);
        assert_eq!(successful["proportional_readback"], 66.7);
        assert!(successful["rollback_state"].is_null());
        assert!(successful["error_message"].is_null());

        let failed = writes
            .iter()
            .find(|w| w["response_level"] == "aggressive")
            .unwrap();
        assert_eq!(failed["success"], false);
        assert_eq!(failed["proportional_written"], 100.0);
        assert!(failed["proportional_readback"].is_null());
        assert_eq!(failed["rollback_state"], "succeeded");
        assert_eq!(
            failed["error_message"],
            "write rejected: value out of range"
        );
    }

    #[tokio::test]
    async fn list_runs_filters_by_every_supported_query_parameter_simultaneously() {
        let state = crate::test_support::in_memory_state().await;
        let (run_id, loop_id) = seed_full_run(&state).await;
        // A second, unrelated run proves the filters actually narrow the result set rather
        // than just happening to match everything in an otherwise-empty database.
        seed_one_run(&state).await;
        let app = router().with_state(state);

        let started_after = (Utc::now() - chrono::Duration::hours(1))
            .to_rfc3339()
            .replace('+', "%2B");
        let started_before = (Utc::now() + chrono::Duration::hours(1))
            .to_rfc3339()
            .replace('+', "%2B");
        let uri = format!(
            "/api/runs?loop_id={loop_id}&process_type=flow&controller_type=pi\
             &outcome=completed&driver=simulator&started_after={started_after}\
             &started_before={started_before}&template_name=Yokogawa+CentumVP\
             &template_origin=builtin"
        );

        let response = app
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["total"], 1);
        assert_eq!(body["runs"][0]["id"], run_id);
    }

    #[tokio::test]
    async fn list_runs_filters_by_opc_server_and_bridge_host() {
        let state = crate::test_support::in_memory_state().await;
        // A run whose recorded connection this test filters for.
        let (run_id, _loop_id) = seed_full_run(&state).await;
        TuneRunRow::record_connection(
            &state.pool,
            run_id,
            Some("Kepware.KEPServerEX.V6"),
            Some("gateway-a:7600"),
            "{}",
        )
        .await
        .unwrap();
        // A second run with no recorded connection at all (mirrors a real simulator run),
        // proving the filter actually narrows rather than matching everything.
        seed_one_run(&state).await;
        let app = router().with_state(state);

        let response = app
            .clone()
            .oneshot(
                Request::get(
                    "/api/runs?opc_server=Kepware.KEPServerEX.V6&bridge_host=gateway-a:7600",
                )
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["total"], 1);
        assert_eq!(body["runs"][0]["id"], run_id);

        // The full detail view surfaces both fields too.
        let detail_response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detail_response.status(), StatusCode::OK);
        let detail_body = body_json(detail_response).await;
        assert_eq!(detail_body["opc_server"], "Kepware.KEPServerEX.V6");
        assert_eq!(detail_body["bridge_host"], "gateway-a:7600");
    }

    /// A simulator-driven [`StartRunRequest`] JSON body, built the same way
    /// `runs::tests::fast_simulator_request_json` is but parsed through the real
    /// `StartRunRequest` `Deserialize` impl and re-serialized -- so the resulting string is
    /// guaranteed well-formed per that type's actual schema (defaults filled in for every
    /// field this literal omits) rather than a hand-typed guess that could silently drift
    /// from it.
    fn fast_simulator_request_json_string() -> String {
        let value = serde_json::json!({
            "tagname": "ignored-for-simulator",
            "template": "Yokogawa CentumVP",
            "process_type": "flow",
            "controller_type": "pi",
            "relay_amp": 10.0,
            "cycles_skip": 1,
            "cycles_count": 2,
            "noise_protection_secs": 0,
            "driver": "simulator",
            "sim_gain": 1.0,
            "sim_tau": 0.01,
            "sim_dead_time": 0.025,
            "pv_range_high": 100.0,
            "pv_range_low": 0.0,
            "mv_range_high": 100.0,
            "mv_range_low": 0.0,
            "direction": "reverse",
            "poll_interval_ms": 5,
            "notes": "http-test-loop",
        });
        let request: StartRunRequest = serde_json::from_value(value).unwrap();
        serde_json::to_string(&request).unwrap()
    }

    #[tokio::test]
    async fn last_request_returns_null_when_the_newest_runs_request_json_does_not_parse() {
        // The column default `"{}"` (what `seed_one_run` leaves behind, since it never
        // calls `record_connection`) isn't a valid `StartRunRequest` -- proving this
        // gracefully reads `null` rather than 500ing is what actually justifies
        // `parse_stored_request` existing instead of just propagating `?`.
        let state = crate::test_support::in_memory_state().await;
        seed_one_run(&state).await;
        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::get("/api/runs/last-request")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_json(response).await.is_null());
    }

    /// `show_run`'s `original_request` field is the mechanism `ui-prefill-last-run`'s
    /// "Duplicate this run" action relies on: unlike `last_request`, which only ever answers
    /// for the single newest run, this must round-trip a *specific*, non-newest run's own
    /// stored request correctly.
    #[tokio::test]
    async fn show_run_returns_the_runs_own_original_request() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_one_run(&state).await;
        let request_json = fast_simulator_request_json_string();
        TuneRunRow::record_connection(&state.pool, run_id, None, None, &request_json)
            .await
            .unwrap();
        // A newer run exists too, proving `show_run` returns *this* run's request rather
        // than always answering with the newest one the way `last_request` does.
        seed_one_run(&state).await;

        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["original_request"]["tagname"], "ignored-for-simulator");
        assert_eq!(body["original_request"]["driver"], "simulator");
        assert_eq!(body["original_request"]["notes"], "http-test-loop");
    }

    #[tokio::test]
    async fn last_request_returns_null_when_no_runs_exist() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(
                Request::get("/api/runs/last-request")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_json(response).await.is_null());
    }

    #[tokio::test]
    async fn last_request_returns_the_newest_runs_request_not_the_first() {
        let state = crate::test_support::in_memory_state().await;
        let older_run_id = seed_one_run(&state).await;
        TuneRunRow::record_connection(&state.pool, older_run_id, None, None, "{}")
            .await
            .unwrap();

        // A second, newer run carrying a real request body -- `seed_one_run` itself only
        // ever inserts `request_json = "{}"` (the column default), so this is also what
        // proves the endpoint parses a *real* snapshot correctly, not just an empty one.
        let template_row =
            bhtune_db::models::DcsTemplateRow::get_by_name(&state.pool, "Yokogawa CentumVP")
                .await
                .unwrap()
                .unwrap();
        let template = template_row.template;
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 10.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 0,
            mrft_delay_secs: 0,
        };
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Sim.PV", &template);
        let newer_run = TuneRunRow::start(
            &state.pool,
            None,
            "http-test-loop",
            TuneDriver::Simulator,
            config,
            template_row.origin,
            &template,
            &tags,
            Utc::now() + chrono::Duration::seconds(1),
        )
        .await
        .unwrap();
        let request_json = fast_simulator_request_json_string();
        TuneRunRow::record_connection(&state.pool, newer_run.id, None, None, &request_json)
            .await
            .unwrap();

        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::get("/api/runs/last-request")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["tagname"], "ignored-for-simulator");
        assert_eq!(body["template"], "Yokogawa CentumVP");
        assert_eq!(body["process_type"], "flow");
        assert_eq!(body["controller_type"], "pi");
        assert_eq!(body["driver"], "simulator");
        assert_eq!(body["notes"], "http-test-loop");
        assert_eq!(body["relay_amp"], 10.0);
    }

    #[tokio::test]
    async fn show_run_404s_for_an_unknown_id() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(
                Request::get("/api/runs/999999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn export_run_defaults_to_csv_with_the_expected_headers_and_body() {
        let state = crate::test_support::in_memory_state().await;
        let (run_id, _loop_id) = seed_full_run(&state).await;
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}/export"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/csv"
        );
        assert_eq!(
            response.headers().get(header::CONTENT_DISPOSITION).unwrap(),
            &format!("attachment; filename=\"run-{run_id}.csv\"")
        );
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        let mut lines = text.lines();
        assert_eq!(
            lines.next().unwrap(),
            "tick,time,pv,pv_quality,hysteresis,mv_value_current,mv_sign_next_step,counter_all_switches,cycles_completed,cycles_remaining"
        );
        assert!(lines.next().unwrap().starts_with("0,"));
    }

    #[tokio::test]
    async fn export_run_supports_the_json_format() {
        let state = crate::test_support::in_memory_state().await;
        let (run_id, _loop_id) = seed_full_run(&state).await;
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}/export?format=json"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            response.headers().get(header::CONTENT_DISPOSITION).unwrap(),
            &format!("attachment; filename=\"run-{run_id}.json\"")
        );
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed[0]["tick"], 0);
        assert_eq!(parsed[0]["pv"], 50.0);
    }

    #[tokio::test]
    async fn export_run_404s_for_an_unknown_id() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(
                Request::get("/api/runs/999999/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn export_run_404s_for_a_run_with_no_recorded_samples() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_one_run(&state).await;
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}/export"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_run_removes_the_run_and_returns_204() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_one_run(&state).await;
        // `seed_one_run` leaves `outcome = running` (it never calls `complete`/`fail`/
        // `abort`); a real deletable run must have finished first.
        TuneRunRow::complete(&state.pool, run_id, Utc::now())
            .await
            .unwrap();
        let pool = state.pool.clone();
        let app = router().with_state(state);

        let response = app
            .clone()
            .oneshot(
                Request::delete(format!("/api/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(TuneRunRow::get(&pool, run_id).await.unwrap().is_none());

        // A follow-up GET for the same id now 404s -- proves the row is really gone, not
        // just hidden.
        let follow_up = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(follow_up.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_run_404s_for_an_unknown_id() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(
                Request::delete("/api/runs/999999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_run_409s_when_the_run_has_not_finished_yet() {
        let state = crate::test_support::in_memory_state().await;
        // `seed_one_run` leaves `outcome = running` -- proves `delete_run` rejects a run
        // based on its own durable DB outcome, with no `ActiveRun` bookkeeping involved at
        // all (nothing here ever reserves a slot).
        let run_id = seed_one_run(&state).await;
        let pool = state.pool.clone();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::delete(format!("/api/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        // Still present -- the conflict must short-circuit before any delete is attempted.
        assert!(TuneRunRow::get(&pool, run_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn delete_run_succeeds_for_a_completed_run_even_if_active_run_has_not_released_it_yet() {
        // Regression test for the race this guard was rewritten to close: `drive()` persists
        // a run's terminal outcome to the DB *before* returning, and `ActiveRun::release` only
        // runs strictly after `drive()` returns (see `routes::runs::start_run`), so there is a
        // real window where a run is already durably `completed` but `ActiveRun` still reports
        // it as the active run. `delete_run` must succeed here regardless, since it checks the
        // run's own DB outcome rather than `ActiveRun`.
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_one_run(&state).await;
        TuneRunRow::complete(&state.pool, run_id, Utc::now())
            .await
            .unwrap();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        state
            .active_run
            .start(run_id, handle, std::future::pending())
            .await
            .unwrap();
        let pool = state.pool.clone();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::delete(format!("/api/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(TuneRunRow::get(&pool, run_id).await.unwrap().is_none());
    }
}
