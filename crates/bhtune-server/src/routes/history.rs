//! Run-history routes: `GET /api/runs` (filtered, paginated list), `GET /api/runs/{id}`
//! (full run detail: config, initial readings, samples, results, writes), `GET
//! /api/runs/{id}/export` (CSV/JSON sample export), and `DELETE /api/runs/{id}`.
//!
//! DTO shapes deliberately mirror the `bhtune` CLI's `commands::history` `--output json` JSON
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
    ControllerDirection, ControllerPidValues, ControllerType, DcsTemplate, LoopConfig, PidRounding,
    ProcessType, ResponseLevel, Tick, TuningResultInvalidReason, TuningResultStatus,
};
use bhtune_db::models::{
    MvActuationKind, MvActuationStatus, Pagination, RestoreStatus, RollbackState, SampleQuality,
    TemplateOrigin, TimingMetrics, TuneDriver, TuneMvActuationRow, TuneOutcome, TuneRecoveryState,
    TuneResultRow, TuneRunFilter, TuneRunRow, TuneSampleRow, TuneWriteRow, WriteKind,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::future::Future;
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

pub(crate) fn filter_from_query(query: &RunListQuery) -> TuneRunFilter {
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
/// [`parse_stored_request`]) (`ui-prefill-last-run`). The New tune form uses this only as a
/// one-time fallback when no mutable `/api/runs/draft` exists, which lets older installations
/// upgrade without losing their previous run settings while keeping future edits independent of
/// immutable history. "Newest" means newest by `started_at`, matching `GET /api/runs`'s own
/// ordering, regardless of that run's `outcome`.
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
pub(crate) fn parse_stored_request(run_id: i64, request_json: &str) -> Option<StartRunRequest> {
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
    pub kp: Option<f32>,
    pub ti_minutes: Option<f32>,
    pub td_minutes: Option<f32>,
    pub proportional: Option<f32>,
    pub integral: Option<f32>,
    pub derivative: Option<f32>,
    pub status: TuningResultStatus,
    pub invalid_reason: Option<TuningResultInvalidReason>,
    /// Controller-ready final-unit values and canonical text; raw fields remain unchanged.
    pub controller_values: Option<ControllerPidValues>,
    /// Why no safe controller target exists, including an active term rounded to zero.
    pub controller_target_error: Option<String>,
}

impl ResultResponse {
    pub(super) fn from_recorded(r: &TuneResultRow, run: &TuneRunRow) -> Self {
        let preview =
            bhtune_runtime::tune::pid_write_preview(r, run.config.controller_type, &run.template);
        ResultResponse {
            response_level: r.response_level,
            kp: r.kp,
            ti_minutes: r.ti_minutes,
            td_minutes: r.td_minutes,
            proportional: r.proportional,
            integral: r.integral,
            derivative: r.derivative,
            status: r.status,
            invalid_reason: r.invalid_reason,
            controller_values: preview.values,
            controller_target_error: preview.unavailable_reason,
        }
    }
}

/// Local projection of [`TuneWriteRow`].
#[derive(Debug, Serialize, ToSchema)]
pub struct WriteResponse {
    pub kind: WriteKind,
    pub response_level: ResponseLevel,
    pub allow_uncertain_quality: bool,
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
            allow_uncertain_quality: w.allow_uncertain_quality,
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

/// Local projection of one accepted OPC DA manipulated-variable command and its independent
/// live readback evidence. Commanded MV samples remain in [`SampleResponse`]; this audit trail
/// is the only response surface that reports measured MV values.
#[derive(Debug, Serialize, ToSchema)]
pub struct MvActuationResponse {
    pub id: i64,
    pub sequence: i64,
    pub kind: MvActuationKind,
    pub commanded_at: DateTime<Utc>,
    pub target_mv: f32,
    pub previous_commanded_mv: Option<f32>,
    pub tolerance: f32,
    pub confirmation_due_at: DateTime<Utc>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub readback_mv: Option<f32>,
    pub readback_quality: Option<SampleQuality>,
    pub attempt_count: i64,
    pub status: MvActuationStatus,
    pub detail: Option<String>,
}

impl From<&TuneMvActuationRow> for MvActuationResponse {
    fn from(row: &TuneMvActuationRow) -> Self {
        Self {
            id: row.id,
            sequence: row.sequence,
            kind: row.kind,
            commanded_at: row.commanded_at,
            target_mv: row.target_mv,
            previous_commanded_mv: row.previous_commanded_mv,
            tolerance: row.tolerance,
            confirmation_due_at: row.confirmation_due_at,
            last_checked_at: row.last_checked_at,
            readback_mv: row.readback_mv,
            readback_quality: row.readback_quality,
            attempt_count: row.attempt_count,
            status: row.status,
            detail: row.detail.clone(),
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

/// The operator-facing names for the three calculated PID constants, derived from the
/// template snapshot stored on the run rather than the mutable template catalog.
#[derive(Debug, Serialize, ToSchema)]
pub struct PidParameterLabelsResponse {
    pub proportional: String,
    pub integral: String,
    pub derivative: String,
}

impl From<&DcsTemplate> for PidParameterLabelsResponse {
    fn from(template: &DcsTemplate) -> Self {
        Self {
            proportional: pid_parameter_label(&template.proportional_constant_suffix, "P"),
            integral: pid_parameter_label(&template.integral_constant_suffix, "I"),
            derivative: pid_parameter_label(&template.derivative_constant_suffix, "D"),
        }
    }
}

fn pid_parameter_label(suffix: &str, fallback: &str) -> String {
    if suffix.is_empty() {
        fallback.to_owned()
    } else {
        suffix.to_owned()
    }
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
    /// Whether this run accepted `Uncertain` OPC quality, captured when the run started.
    pub allow_uncertain_quality: bool,
    pub config: LoopConfig,
    /// Precision policy from the run's immutable template snapshot.
    pub pid_rounding: PidRounding,
    /// Concrete global timing and safety values frozen when this run was prepared. `None`
    /// identifies a run created before effective-tuning snapshots were stored.
    pub effective_tuning: Option<bhtune_db::models::EffectiveTuning>,
    /// The resolved OPC DA server ProgID this run actually used, or `None` for a
    /// simulator/replay run (`db-run-request-snapshot`). This is what `history revert`
    /// trusts over any `--server` flag -- see `bhtune_runtime::history::revert_run`.
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
    /// Operator-facing calculated-result column labels from the run's historical template
    /// snapshot. Empty user-template suffixes use the conventional P/I/D labels.
    pub pid_parameter_labels: PidParameterLabelsResponse,
    pub initial_readings: Option<InitialReadingsResponse>,
    pub timing_metrics: Option<TimingMetrics>,
    pub samples: Vec<SampleResponse>,
    pub results: Vec<ResultResponse>,
    pub writes: Vec<WriteResponse>,
    pub mv_actuations: Vec<MvActuationResponse>,
    pub restore_status: Option<RestoreStatus>,
    pub restore_detail: Option<String>,
    /// This run's own `request_json` (`db-run-request-snapshot`), parsed back into a
    /// [`StartRunRequest`], or `None` if it isn't usable -- see `parse_stored_request`.
    /// Powers the run detail page's "Duplicate this run" action (`ui-prefill-last-run`):
    /// unlike `GET /api/runs/last-request`, which only ever answers for the single newest
    /// run, this lets the New tune form seed itself from *this specific* historical run
    /// regardless of how many later runs exist.
    pub original_request: Option<StartRunRequest>,
    /// Gateway protocol snapshot recorded before this run's live mutation, when the run used
    /// OPC DA. `None` for simulator/replay runs and for OPC runs that predate the snapshot.
    pub gateway_compatibility: Option<crate::routes::opc::GatewayCompatibilityResponse>,
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
    let mv_actuations = TuneMvActuationRow::list_for_run(pool, run_id).await?;
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
    let pid_parameter_labels = PidParameterLabelsResponse::from(&run.template);
    let results = results
        .iter()
        .map(|result| ResultResponse::from_recorded(result, &run))
        .collect();

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
        pid_rounding: run.template.pid_rounding,
        effective_tuning: run.effective_tuning,
        opc_server: run.opc_server,
        bridge_host: run.bridge_host,
        pid_constant_tags,
        pid_parameter_labels,
        initial_readings: run.initial_readings.map(InitialReadingsResponse::from),
        timing_metrics: run.timing_metrics,
        samples: samples.iter().map(SampleResponse::from).collect(),
        results,
        writes: writes.iter().map(WriteResponse::from).collect(),
        mv_actuations: mv_actuations
            .iter()
            .map(MvActuationResponse::from)
            .collect(),
        restore_status: run.restore_status,
        restore_detail: run.restore_detail,
        original_request: parse_stored_request(run.id, &run.request_json),
        gateway_compatibility: run
            .gateway_compatibility_json
            .as_deref()
            .map(bhtune_runtime::gateway::parse_stored_compatibility)
            .transpose()?
            .map(crate::routes::opc::GatewayCompatibilityResponse::from),
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
/// than reusing the runtime's internal
/// [`bhtune_runtime::export::SampleExportFormat`] directly, matching this module's
/// DTO-decoupling convention (see the module doc comment). Converted at the call site so
/// the shared runtime owns CSV/JSON serialization while this type remains specific to the
/// HTTP query and its OpenAPI schema.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunExportFormat {
    Csv,
    Json,
}

impl From<RunExportFormat> for bhtune_runtime::export::SampleExportFormat {
    fn from(format: RunExportFormat) -> Self {
        match format {
            RunExportFormat::Csv => bhtune_runtime::export::SampleExportFormat::Csv,
            RunExportFormat::Json => bhtune_runtime::export::SampleExportFormat::Json,
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
    let bytes = bhtune_runtime::export::samples_to_bytes(&samples, format.into())?;
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
/// active-run registry entry: `drive()` persists every terminal outcome (`persist_results` then
/// `TuneRunRow::complete`/`fail`/`abort`) *before* returning, and `ActiveRun::release` only
/// runs strictly after `drive()` returns (see `routes::runs::start_run`'s spawned task), so
/// there is a real -- if brief -- window where a run's outcome is already durably
/// `completed`/`failed`/`aborted` but `ActiveRun` hasn't been told the registry entry is free yet.
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
        (status = 409, description = "The run is active or has unresolved live recovery.", body = ErrorBody),
    ),
)]
pub(crate) async fn delete_run(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    delete_run_with_hook(state, run_id, |_| async {}).await
}

async fn delete_run_with_hook<F, Fut>(
    state: AppState,
    run_id: i64,
    after_lookup: F,
) -> Result<StatusCode, ApiError>
where
    F: FnOnce(&AppState) -> Fut,
    Fut: Future<Output = ()>,
{
    let run = TuneRunRow::get(&state.pool, run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no run with id {run_id}")))?;
    if run.outcome == TuneOutcome::Running {
        return Err(ApiError::Conflict(format!(
            "run {run_id} has not finished yet; cancel it before deleting"
        )));
    }
    if recovery_blocks_deletion(run.recovery_state) {
        return Err(recovery_delete_conflict(run_id));
    }
    after_lookup(&state).await;
    if TuneRunRow::delete(&state.pool, run_id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        if TuneRunRow::get(&state.pool, run_id)
            .await?
            .is_some_and(|run| recovery_blocks_deletion(run.recovery_state))
        {
            return Err(recovery_delete_conflict(run_id));
        }
        // Only reachable if the row was deleted by a concurrent request between the
        // `get` above and this call -- still a well-defined 404 ("no run with that id"
        // is simply true again by the time this responds), not a real error.
        Err(ApiError::NotFound(format!("no run with id {run_id}")))
    }
}

fn recovery_blocks_deletion(recovery_state: Option<TuneRecoveryState>) -> bool {
    matches!(
        recovery_state,
        Some(
            TuneRecoveryState::Eligible
                | TuneRecoveryState::Running
                | TuneRecoveryState::Incomplete
        )
    )
}

fn recovery_delete_conflict(run_id: i64) -> ApiError {
    ApiError::Conflict(format!(
        "run {run_id} has an unresolved live-loop recovery; use `bhtune restore-loop {run_id} --yes` or follow the manual recovery procedure before deleting it"
    ))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/runs", get(list_runs))
        .route("/api/runs/last-request", get(last_request))
        .route("/api/runs/{id}", get(show_run).delete(delete_run))
        .route("/api/runs/{id}/export", get(export_run))
}

#[cfg(test)]
mod tests;
