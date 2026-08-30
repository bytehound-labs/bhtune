//! `POST /api/runs` (start a new tune run) and `POST /api/runs/{id}/cancel` (request its
//! cancellation) -- the write side of the run-history API `routes::history` reads from.
//!
//! Reuses `bhtune-cli`'s own [`bhtune_cli::commands::tune::prepare`]/[`bhtune_cli::commands::tune::drive`]
//! split unchanged, so a run started over HTTP goes through exactly the same template
//! lookup, tag derivation, driver connection, quality checks, restore-on-abort, and
//! write-back rollback as a run started by the CLI -- only the setup/reporting differs (see
//! those functions' own doc comments for the full rationale). `crate::active_run` tracks
//! every in-flight run so each can be cancelled independently.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{post, put};
use axum::{Json, Router};
use bhtune_cli::args::{DriverKindArg, TuneArgs};
use bhtune_cli::cancel::CtrlC;
use bhtune_cli::commands::tune::{
    PidWriteOutcome, drive, prepare, validate_restore_timeout_secs, write_pid_values,
};
use bhtune_cli::output::OutputFormat;
use bhtune_core::{
    ControllerDirection, ControllerType, PidParameters, ProcessType, ResponseLevel, TagOverrides,
    opc_write_values,
};
use bhtune_db::models::{
    TuneDriver, TuneOutcome, TuneResultRow, TuneRunRow, TuneWriteRow, WriteKind, WriteReadback,
};
use bhtune_driver::OpcDaDriver;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::active_run::RunAlreadyActive;
use crate::error::{ApiError, ErrorBody};
use crate::routes::history::{RunDetailResponse, build_run_detail};
use crate::state::AppState;

fn default_sim_gain() -> f32 {
    1.0
}
fn default_sim_tau() -> f32 {
    2.0
}
fn default_sim_dead_time() -> f32 {
    5.0
}
fn default_sim_initial_value() -> f32 {
    50.0
}
fn default_poll_interval_ms() -> u64 {
    800
}
fn default_timeout_secs() -> u64 {
    3600
}
fn default_op_or_restore_timeout_secs() -> u64 {
    30
}

/// The body of `POST /api/runs` -- full field parity with [`TuneArgs`], since starting a run
/// over HTTP must be able to express everything `bhtune tune` can. Every field that has a
/// CLI default (`--sim-gain`, `--poll-interval-ms`, etc.) repeats that exact default here via
/// `#[serde(default = "...")]`, so an HTTP caller that omits a field gets identical behavior
/// to a CLI invocation that omits the matching flag. `Option<T>` fields need no
/// `#[serde(default)]` of their own -- serde already treats a missing key as `None` for an
/// `Option` field.
///
/// Also derives `Serialize` so the exact same type can serve as `GET /api/runs/last-request`'s
/// response (`ui-prefill-last-run`, in `routes::history::last_request`): that endpoint parses
/// a run's stored `request_json` straight into a `StartRunRequest` rather than duplicating
/// its ~30 fields into a second struct, giving a "what you `GET` is exactly what you'd `POST`
/// to repeat it" symmetry in both the Rust types and the generated OpenAPI schema. This is
/// safe precisely because `request_json` is *already* built to this exact shape --
/// `bhtune-cli`'s `RequestSnapshot` doc comment describes the two as kept in sync by
/// convention.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StartRunRequest {
    /// PV tag prefix; ignored for `driver: "simulator"`. See [`TuneArgs::tagname`].
    pub tagname: String,
    /// DCS/PLC template name (see `GET /api/templates`).
    pub template: String,
    pub process_type: ProcessType,
    pub controller_type: ControllerType,
    /// Relay amplitude, as a percentage of the MV range.
    pub relay_amp: f32,
    /// Relay cycles to skip before counting begins (default: looked up per `process_type`).
    pub cycles_skip: Option<u32>,
    /// Relay cycles to count once the skip period ends (default: looked up per
    /// `process_type`).
    pub cycles_count: Option<u32>,
    /// Seconds a switch must persist before it's accepted (default: looked up per
    /// `process_type`).
    pub noise_protection_secs: Option<u32>,
    /// Pre/post-test recording padding, in seconds.
    #[serde(default)]
    pub mrft_delay: u32,
    /// Which driver drives this tune. `"replay"` is rejected -- that driver exists only
    /// for offline golden-trace validation, not for starting a live/simulated run.
    pub driver: TuneDriver,
    /// opcda-bridge gateway address. Only meaningful with `driver: "opcda"` (default:
    /// resolved the same way the CLI resolves `--bridge-host`, via this process's own
    /// config/env).
    pub bridge_host: Option<String>,
    /// OPC DA server ProgID. Required with `driver: "opcda"`.
    pub server: Option<String>,
    /// Simulator process gain (`driver: "simulator"` only).
    #[serde(default = "default_sim_gain")]
    pub sim_gain: f32,
    /// Simulator process time constant, in seconds (`driver: "simulator"` only).
    #[serde(default = "default_sim_tau")]
    pub sim_tau: f32,
    /// Simulator dead time, in seconds (`driver: "simulator"` only).
    #[serde(default = "default_sim_dead_time")]
    pub sim_dead_time: f32,
    /// Simulator measurement noise amplitude (`driver: "simulator"` only).
    #[serde(default)]
    pub sim_noise: f32,
    /// Simulator RNG seed, for reproducible noise (`driver: "simulator"` only).
    #[serde(default)]
    pub sim_seed: u64,
    /// Simulator initial PV (`driver: "simulator"` only).
    #[serde(default = "default_sim_initial_value")]
    pub sim_initial_pv: f32,
    /// Simulator initial MV (`driver: "simulator"` only).
    #[serde(default = "default_sim_initial_value")]
    pub sim_initial_mv: f32,
    /// Fixed PV range high, overriding a live tag read. Required for `driver: "simulator"`,
    /// which has no range tags at all.
    pub pv_range_high: Option<f32>,
    /// Fixed PV range low, overriding a live tag read.
    pub pv_range_low: Option<f32>,
    /// Fixed MV range high, overriding a live tag read.
    pub mv_range_high: Option<f32>,
    /// Fixed MV range low, overriding a live tag read.
    pub mv_range_low: Option<f32>,
    /// Fixed controller direction, overriding a live tag read.
    pub direction: Option<ControllerDirection>,
    /// Per-tune replacements for template-derived OPC tag names. Blank or missing fields use
    /// the template-derived tag.
    pub tag_overrides: Option<TagOverrides>,
    /// How often to poll the driver, in milliseconds.
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
    /// Hard wall-clock cap on this run's total duration, in seconds. See
    /// [`TuneArgs::timeout_secs`] -- always enforced, exactly as for a CLI-driven run.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Operator notes to attach to this run. Notes can be edited or cleared later through
    /// the run-history endpoints.
    #[serde(default)]
    pub notes: Option<String>,
    /// Confirm an unattended PID write-back. Required alongside `write_pid` -- the request
    /// is rejected otherwise, identically to `--write-pid` without `--yes` on the CLI.
    #[serde(default)]
    pub yes: bool,
    /// Non-interactively write this response level's calculated PID parameters back to the
    /// DCS. Requires `yes: true`.
    pub write_pid: Option<ResponseLevel>,
    /// Cap on any single driver read/write during the run, in seconds.
    #[serde(default = "default_op_or_restore_timeout_secs")]
    pub op_timeout_secs: u64,
    /// Cap on restoring the loop to its pre-test state after the run ends, in seconds.
    /// OPC DA runs require at least 4 seconds so the internal MV actuation confirmation
    /// window can complete; simulator runs only require a positive value.
    #[serde(default = "default_op_or_restore_timeout_secs")]
    #[schema(minimum = 4, example = 30)]
    pub restore_timeout_secs: u64,
}

/// `value.is_finite()`, as an [`ApiError::BadRequest`] on failure -- the HTTP-path
/// equivalent of `bhtune-cli`'s `finite_f32` clap `value_parser`, which never runs for a
/// [`TuneArgs`] built directly in Rust code rather than parsed from `std::env::args()`. Well-
/// formed JSON can still produce a non-finite `f32` here: a numeric literal within JSON's own
/// unbounded range (e.g. `1e40`) silently saturates to `f32::INFINITY` on conversion, with no
/// parse error from `serde_json` -- so this check is a real gap this DTO must close, not
/// belt-and-suspenders.
fn require_finite(field: &str, value: f32) -> Result<(), ApiError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!(
            "'{field}' must be a finite number (not NaN or infinite), got {value}"
        )))
    }
}

fn require_finite_if_some(field: &str, value: Option<f32>) -> Result<(), ApiError> {
    match value {
        Some(v) => require_finite(field, v),
        None => Ok(()),
    }
}

/// `value >= 1`, as an [`ApiError::BadRequest`] on failure -- the HTTP-path equivalent of
/// `bhtune-cli`'s `positive_u64` clap `value_parser`, for the same reason [`require_finite`]
/// exists: a [`TuneArgs`] built directly in Rust code bypasses clap's parsers entirely.
fn require_positive(field: &str, value: u64) -> Result<(), ApiError> {
    if value >= 1 {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!(
            "'{field}' must be at least 1, got {value}"
        )))
    }
}

impl StartRunRequest {
    /// Validates and converts this request into a [`TuneArgs`], ready for
    /// [`bhtune_cli::commands::tune::prepare`].
    ///
    /// Only validates the specific fields that have **no** downstream safety net regardless
    /// of transport: `relay_amp`, `cycles_count` (after `process_type`-defaulting), and
    /// `mrft_delay` are already checked by [`bhtune_core::LoopConfig::validate`], called
    /// from inside `prepare()` itself, so re-checking them here would be redundant. Every
    /// other numeric field bypasses clap's `value_parser`s entirely when constructed this
    /// way (see AGENTS.md's `server-start-tune-api` notes) and has no other guard, so this
    /// function is where that gap is closed.
    pub(crate) fn into_tune_args(self) -> Result<TuneArgs, ApiError> {
        require_finite("relay_amp", self.relay_amp)?;
        require_finite("sim_gain", self.sim_gain)?;
        require_finite("sim_tau", self.sim_tau)?;
        require_finite("sim_dead_time", self.sim_dead_time)?;
        require_finite("sim_noise", self.sim_noise)?;
        require_finite("sim_initial_pv", self.sim_initial_pv)?;
        require_finite("sim_initial_mv", self.sim_initial_mv)?;
        require_finite_if_some("pv_range_high", self.pv_range_high)?;
        require_finite_if_some("pv_range_low", self.pv_range_low)?;
        require_finite_if_some("mv_range_high", self.mv_range_high)?;
        require_finite_if_some("mv_range_low", self.mv_range_low)?;
        require_positive("poll_interval_ms", self.poll_interval_ms)?;
        require_positive("timeout_secs", self.timeout_secs)?;
        require_positive("op_timeout_secs", self.op_timeout_secs)?;
        if let Some(tag_overrides) = &self.tag_overrides {
            tag_overrides
                .validate()
                .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        }

        let driver = DriverKindArg::try_from(self.driver)
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        validate_restore_timeout_secs(driver, self.restore_timeout_secs)
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;

        Ok(TuneArgs {
            tagname: self.tagname,
            template: self.template,
            process_type: self.process_type.into(),
            controller_type: self.controller_type.into(),
            relay_amp: self.relay_amp,
            cycles_skip: self.cycles_skip,
            cycles_count: self.cycles_count,
            noise_protection_secs: self.noise_protection_secs,
            mrft_delay: self.mrft_delay,
            driver,
            bridge_host: self.bridge_host,
            server: self.server,
            sim_gain: self.sim_gain,
            sim_tau: self.sim_tau,
            sim_dead_time: self.sim_dead_time,
            sim_noise: self.sim_noise,
            sim_seed: self.sim_seed,
            sim_initial_pv: self.sim_initial_pv,
            sim_initial_mv: self.sim_initial_mv,
            pv_range_high: self.pv_range_high,
            pv_range_low: self.pv_range_low,
            mv_range_high: self.mv_range_high,
            mv_range_low: self.mv_range_low,
            direction: self.direction.map(Into::into),
            tag_overrides: self.tag_overrides,
            poll_interval_ms: self.poll_interval_ms,
            timeout_secs: self.timeout_secs,
            notes: self.notes,
            yes: self.yes,
            write_pid: self.write_pid.map(Into::into),
            op_timeout_secs: self.op_timeout_secs,
            restore_timeout_secs: self.restore_timeout_secs,
            // `drive()`'s doc comment requires `Json` for every HTTP-started run: `execute`'s
            // interactive write-back prompt (`maybe_write_back`) only skips reading stdin
            // when `output == OutputFormat::Json`, and this background task has no stdin to
            // read from at all.
            output: OutputFormat::Json,
        })
    }
}

/// Start a new tune run.
///
/// `POST /api/runs` -- runs `prepare()` (template lookup, tag derivation, driver connect,
/// and the `tune_runs` insert) inline and returns as soon as that succeeds, having already
/// `tokio::spawn`ed the actual polling/tuning phase in the background. `201 Created` carries
/// the same [`RunDetailResponse`] `GET /api/runs/{id}` would show for this run at this
/// instant (almost certainly still `outcome: "running"`) -- poll that endpoint, or use
/// `POST /api/runs/{id}/cancel`, to follow the run to completion.
///
/// `409 Conflict` if an exclusive post-hoc PID write/revert is active; independent tune runs
/// may execute concurrently.
#[utoipa::path(
    post,
    path = "/api/runs",
    tag = "runs",
    request_body = StartRunRequest,
    responses(
        (status = 201, description = "The run was started; detail reflects its state right now.", body = RunDetailResponse),
        (status = 400, description = "The request failed validation, or `prepare()` itself failed (unknown template, invalid flag combination, unreachable driver).", body = ErrorBody),
        (status = 409, description = "An exclusive PID write/revert is already active.", body = ErrorBody),
    ),
)]
pub(crate) async fn start_run(
    State(state): State<AppState>,
    Json(request): Json<StartRunRequest>,
) -> Result<(StatusCode, Json<RunDetailResponse>), ApiError> {
    start_run_with_hook(state, request, |_| async {}).await
}

async fn start_run_with_hook<F, Fut>(
    state: AppState,
    request: StartRunRequest,
    after_prepare: F,
) -> Result<(StatusCode, Json<RunDetailResponse>), ApiError>
where
    F: FnOnce(&AppState) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    // Optimistic pre-check: avoids a wasted `prepare()` call while a post-hoc PID write/revert
    // is holding the exclusive live-loop reservation. It deliberately does not reject an
    // already-running tune: independent tunes are allowed to execute concurrently.
    if let Some(active_id) = state.active_run.exclusive_id().await {
        return Err(ApiError::Conflict(format!(
            "run {active_id} has an exclusive PID write/revert in progress; wait for it to finish before starting another tune"
        )));
    }

    let args = request.into_tune_args()?;

    // `prepare()`'s own doc comment: its failures (bad template name, `--write-pid` without
    // `--yes`, an unreachable driver) are "exactly the kind of problem an HTTP client
    // expects a synchronous error response for" -- so they map to `400`, not the generic
    // `500` a bare `?`/`Internal` conversion would give.
    let app_config = state.config_snapshot()?;
    let prepared = prepare(&state.pool, args, &app_config)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let run_id = prepared.run_id();
    after_prepare(&state).await;

    let (ctrl_c, cancel_handle) = CtrlC::manual();
    let pool_for_task = state.pool.clone();
    let active_run_for_task = state.active_run.clone();
    let task = async move {
        let mut ctrl_c = ctrl_c;
        // `drive()` already records every outcome (completion, abort, failure) to the
        // `tune_runs` row itself; this task has no caller left to report a `Result` to, so
        // its own `Err` is intentionally discarded here.
        let _ = drive(&pool_for_task, prepared, &mut ctrl_c).await;
        active_run_for_task.release(run_id).await;
    };

    // The authoritative check: if this loses the race (a post-hoc write/revert reserved the
    // live-loop operation between the pre-check above and here), the just-inserted row is
    // marked `failed` rather than left forever showing an outcome it never actually reached.
    if let Err(RunAlreadyActive { run_id: existing }) =
        state.active_run.start(run_id, cancel_handle, task).await
    {
        let failure_reason = format!(
            "run {existing} has an exclusive PID write/revert in progress; no tune task was started"
        );
        TuneRunRow::fail(&state.pool, run_id, Utc::now(), &failure_reason).await?;
        return Err(ApiError::Conflict(failure_reason));
    }

    let detail = build_run_detail(&state.pool, run_id).await?.expect(
        "the tune_runs row this handler just inserted via prepare() must exist immediately \
         afterward",
    );
    Ok((StatusCode::CREATED, Json(detail)))
}

/// Request cancellation of a run, exactly as if Ctrl+C had been pressed against an
/// equivalent CLI-driven run.
///
/// `POST /api/runs/{id}/cancel` -- `404` if no run has that id; otherwise always `204`,
/// whether or not the run was actually active at the moment this was called (a run that
/// already finished simply has nothing left to cancel). Cancellation is asynchronous: the
/// run's background task still has to observe it, stop polling, and run its restore --
/// `GET /api/runs/{id}` shows the eventual outcome.
#[utoipa::path(
    post,
    path = "/api/runs/{id}/cancel",
    tag = "runs",
    params(
        ("id" = i64, Path, description = "Run id"),
    ),
    responses(
        (status = 204, description = "Cancellation requested (or the run was already inactive)."),
        (status = 404, description = "No run with that id.", body = ErrorBody),
    ),
)]
pub(crate) async fn cancel_run(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if TuneRunRow::get(&state.pool, run_id).await?.is_none() {
        return Err(ApiError::NotFound(format!("no run with id {run_id}")));
    }
    state.active_run.cancel(run_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// The body of `PUT /api/runs/{id}/notes`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateNotesRequest {
    /// Replacement note text. Blank or whitespace-only text clears the note.
    pub notes: String,
}

fn normalized_notes(notes: String) -> Option<String> {
    let trimmed = notes.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Replace the operator notes attached to a run.
///
/// `PUT /api/runs/{id}/notes` deliberately works for both running and terminal runs. Notes
/// are metadata, not a plant mutation, so they do not take the active-run registry reservation.
#[utoipa::path(
    put,
    path = "/api/runs/{id}/notes",
    tag = "runs",
    params(
        ("id" = i64, Path, description = "Run id"),
    ),
    request_body = UpdateNotesRequest,
    responses(
        (status = 200, description = "The run with its updated notes.", body = RunDetailResponse),
        (status = 404, description = "No run with that id.", body = ErrorBody),
    ),
)]
pub(crate) async fn update_notes(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
    Json(request): Json<UpdateNotesRequest>,
) -> Result<Json<RunDetailResponse>, ApiError> {
    update_notes_with_hook(state, run_id, request, |_| async {}).await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NotesHookStage {
    AfterLookup,
    AfterUpdate,
}

async fn update_notes_with_hook<F, Fut>(
    state: AppState,
    run_id: i64,
    request: UpdateNotesRequest,
    mut hook: F,
) -> Result<Json<RunDetailResponse>, ApiError>
where
    F: FnMut(NotesHookStage) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    TuneRunRow::get(&state.pool, run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no run with id {run_id}")))?;
    hook(NotesHookStage::AfterLookup).await;
    TuneRunRow::update_notes(
        &state.pool,
        run_id,
        normalized_notes(request.notes).as_deref(),
    )
    .await?;
    hook(NotesHookStage::AfterUpdate).await;
    build_run_detail(&state.pool, run_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("no run with id {run_id}")))
}

/// Clear a run's operator notes.
///
/// `DELETE /api/runs/{id}/notes` is idempotent and works while a run is active or after it
/// finishes.
#[utoipa::path(
    delete,
    path = "/api/runs/{id}/notes",
    tag = "runs",
    params(
        ("id" = i64, Path, description = "Run id"),
    ),
    responses(
        (status = 200, description = "The run with its notes cleared.", body = RunDetailResponse),
        (status = 404, description = "No run with that id.", body = ErrorBody),
    ),
)]
pub(crate) async fn delete_notes(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
) -> Result<Json<RunDetailResponse>, ApiError> {
    delete_notes_with_hook(state, run_id, |_| async {}).await
}

async fn delete_notes_with_hook<F, Fut>(
    state: AppState,
    run_id: i64,
    after_update: F,
) -> Result<Json<RunDetailResponse>, ApiError>
where
    F: FnOnce(&AppState) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    TuneRunRow::get(&state.pool, run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no run with id {run_id}")))?;
    TuneRunRow::update_notes(&state.pool, run_id, None).await?;
    after_update(&state).await;
    build_run_detail(&state.pool, run_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("no run with id {run_id}")))
}

/// The body of `POST /api/runs/{id}/write`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WriteRunRequest {
    /// Which of the run's three calculated candidate result sets to write.
    pub response_level: ResponseLevel,
}

/// Checks that `run` is eligible for a post-hoc PID write or revert (`api-post-run-write`):
/// finished (not still running its own test), used the `opcda` driver, has PID constant
/// tags in its snapshotted [`bhtune_core::LoopTags`], and recorded the OPC server/bridge
/// host it actually connected through. Shared by [`write_run`] and [`revert_run`] -- both
/// need exactly the same eligibility, only the *target* values to write differ.
fn require_writable_run(run: &TuneRunRow) -> Result<(), ApiError> {
    if run.outcome == TuneOutcome::Running {
        return Err(ApiError::BadRequest(format!(
            "run {} is still running; wait for it to finish before writing or reverting PID \
             constants",
            run.id
        )));
    }
    if run.driver != TuneDriver::Opcda {
        return Err(ApiError::BadRequest(format!(
            "run {} used the {:?} driver, which has no live loop to write PID constants to",
            run.id, run.driver
        )));
    }
    if run.tags.proportional_constant.is_none()
        || run.tags.integral_constant.is_none()
        || run.tags.derivative_constant.is_none()
    {
        return Err(ApiError::BadRequest(format!(
            "run {}'s snapshotted tags have no PID constant tags configured",
            run.id
        )));
    }
    if run.opc_server.is_none() || run.bridge_host.is_none() {
        return Err(ApiError::BadRequest(format!(
            "run {} has no recorded OPC server/bridge host; refusing to guess which \
             connection to use",
            run.id
        )));
    }
    Ok(())
}

/// Connects an [`OpcDaDriver`] using `run`'s own recorded `opc_server`/`bridge_host` --
/// never re-resolved from this process's own config/flags, for exactly the reason
/// `bhtune-cli`'s `commands::history::resolve_revert_connection` documents: a value
/// re-resolved at write/revert time could silently point at a different gateway than the
/// run itself actually used. [`require_writable_run`] must already have confirmed both
/// fields are present.
async fn connect_to_runs_recorded_driver(run: &TuneRunRow) -> Result<OpcDaDriver, ApiError> {
    let opc_server = run
        .opc_server
        .as_deref()
        .expect("require_writable_run already checked opc_server is Some");
    let bridge_host = run
        .bridge_host
        .as_deref()
        .expect("require_writable_run already checked bridge_host is Some");
    OpcDaDriver::connect(bridge_host, opc_server)
        .await
        .map_err(|e| {
            ApiError::BadRequest(format!(
                "failed to connect to OPC server '{opc_server}' via bridge '{bridge_host}': {e}"
            ))
        })
}

/// Reserves the [`crate::active_run::ActiveRun`] exclusive write/revert reservation for
/// `run.id`, connects, and calls [`write_pid_values`] -- releasing the reservation on every
/// exit path (this project's established "no `Drop`-based cleanup, `Drop` cannot await" rule;
/// see
/// `crate::active_run::ActiveRun::reserve`'s own doc comment) -- then rebuilds and returns
/// the run's fresh [`RunDetailResponse`] regardless of whether the write/revert itself
/// succeeded. A [`PidWriteOutcome::Failed`] is not an HTTP error: the request was processed
/// successfully and its result -- including the failure -- is recorded in the returned
/// `writes[]` array's `success`/`error_message` fields, exactly how a client already reads a
/// write-back outcome from `GET /api/runs/{id}`. Only [`ApiError::Conflict`] (a tune or another
/// write/revert operation holds the exclusive reservation), [`ApiError::BadRequest`] (the driver connection itself
/// failed), or [`ApiError::Internal`] (an unexpected database failure inside
/// [`write_pid_values`]) short-circuit this into an actual error response.
#[allow(clippy::too_many_arguments)]
async fn reserve_connect_and_write(
    state: &AppState,
    run_id: i64,
    run: &TuneRunRow,
    p_tag: &str,
    i_tag: &str,
    d_tag: &str,
    response_level: ResponseLevel,
    target: WriteReadback,
    kind: WriteKind,
    allow_uncertain_quality: bool,
) -> Result<RunDetailResponse, ApiError> {
    reserve_connect_and_write_with_hook(
        state,
        run_id,
        run,
        p_tag,
        i_tag,
        d_tag,
        response_level,
        target,
        kind,
        allow_uncertain_quality,
        |_| async {},
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn reserve_connect_and_write_with_hook<F, Fut>(
    state: &AppState,
    run_id: i64,
    run: &TuneRunRow,
    p_tag: &str,
    i_tag: &str,
    d_tag: &str,
    response_level: ResponseLevel,
    target: WriteReadback,
    kind: WriteKind,
    allow_uncertain_quality: bool,
    after_release: F,
) -> Result<RunDetailResponse, ApiError>
where
    F: FnOnce(&AppState) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    reserve_connect_and_write_with_hooks(
        state,
        run_id,
        run,
        p_tag,
        i_tag,
        d_tag,
        response_level,
        target,
        kind,
        allow_uncertain_quality,
        |_| async {},
        after_release,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn reserve_connect_and_write_with_hooks<F, Fut, G, Gut>(
    state: &AppState,
    run_id: i64,
    run: &TuneRunRow,
    p_tag: &str,
    i_tag: &str,
    d_tag: &str,
    response_level: ResponseLevel,
    target: WriteReadback,
    kind: WriteKind,
    allow_uncertain_quality: bool,
    before_write: F,
    after_release: G,
) -> Result<RunDetailResponse, ApiError>
where
    F: FnOnce(&AppState) -> Fut,
    Fut: std::future::Future<Output = ()>,
    G: FnOnce(&AppState) -> Gut,
    Gut: std::future::Future<Output = ()>,
{
    state
        .active_run
        .reserve(run_id)
        .await
        .map_err(|RunAlreadyActive { run_id: existing }| {
            ApiError::Conflict(format!(
                "run {existing} or another PID write/revert is active; try again once it finishes"
            ))
        })?;

    let result: Result<PidWriteOutcome, ApiError> = async {
        let driver = connect_to_runs_recorded_driver(run).await?;
        before_write(state).await;
        let outcome = write_pid_values(
            &state.pool,
            run_id,
            &driver,
            p_tag,
            i_tag,
            d_tag,
            response_level,
            target,
            kind,
            allow_uncertain_quality,
        )
        .await?;
        Ok(outcome)
    }
    .await;

    state.active_run.release(run_id).await;
    after_release(state).await;
    result?;

    build_run_detail(&state.pool, run_id).await?.ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!(
            "run {run_id} vanished while its write/revert was being processed"
        ))
    })
}

/// Write one of a run's calculated candidate PID parameter sets back to the live loop.
///
/// `POST /api/runs/{id}/write` -- unlike the CLI's `--write-pid`, which can only fire once
/// at the end of the run it belongs to, this can be called at any time after the run has
/// finished, letting an engineer compare Sluggish/Moderate/Aggressive on screen before
/// picking one. Pre-reads the selected tag's current P/I/D, writes and verifies each constant in
/// turn, and rolls back to the pre-read values if a later constant is rejected
/// (`safety-writeback-rollback`) -- recorded as a new write-back audit row exactly like an
/// in-run write.
///
/// Always `200` once the request itself is valid and no conflicting operation is active,
/// whether or not the write actually succeeded -- see [`reserve_connect_and_write`]'s doc
/// comment for why a physical write failure is not a `4xx`/`5xx`.
#[utoipa::path(
    post,
    path = "/api/runs/{id}/write",
    tag = "runs",
    params(
        ("id" = i64, Path, description = "Run id"),
    ),
    request_body = WriteRunRequest,
    responses(
        (status = 200, description = "The write was attempted; see `writes[]` in the body for its outcome.", body = RunDetailResponse),
        (status = 400, description = "The run isn't eligible for a post-hoc write (still running, wrong driver, no PID tags/connection recorded, or no calculated result for the requested response level), or the driver connection itself failed.", body = ErrorBody),
        (status = 404, description = "No run with that id.", body = ErrorBody),
        (status = 409, description = "A tune or another PID write/revert is already active.", body = ErrorBody),
    ),
)]
pub(crate) async fn write_run(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
    Json(request): Json<WriteRunRequest>,
) -> Result<Json<RunDetailResponse>, ApiError> {
    let run = TuneRunRow::get(&state.pool, run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no run with id {run_id}")))?;
    require_writable_run(&run)?;
    let allow_uncertain_quality = state.config_snapshot()?.allow_uncertain_quality;

    let results = TuneResultRow::list_for_run(&state.pool, run_id).await?;
    let selected = results
        .iter()
        .find(|r| r.response_level == request.response_level)
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "run {run_id} has no calculated {:?} result to write",
                request.response_level
            ))
        })?;

    let pid = PidParameters {
        response_level: selected.response_level,
        proportional: selected.proportional,
        integral: selected.integral,
        derivative: selected.derivative,
    };
    let written = opc_write_values(pid, run.config.controller_type, run.template.integral_type);
    let target = WriteReadback {
        proportional: written.proportional,
        integral: written.integral,
        derivative: written.derivative,
    };

    // `require_writable_run` already confirmed all three tags are `Some`.
    let p_tag = run.tags.proportional_constant.clone().unwrap();
    let i_tag = run.tags.integral_constant.clone().unwrap();
    let d_tag = run.tags.derivative_constant.clone().unwrap();

    let detail = reserve_connect_and_write(
        &state,
        run_id,
        &run,
        &p_tag,
        &i_tag,
        &d_tag,
        request.response_level,
        target,
        WriteKind::Write,
        allow_uncertain_quality,
    )
    .await?;
    Ok(Json(detail))
}

/// Revert a run's most recent PID write-back, restoring the pre-write values it recorded.
///
/// `POST /api/runs/{id}/revert` -- no request body: like `POST /api/runs/{id}/cancel`, the
/// GUI's own confirmation dialog (naming the tag, the tags, and the exact values from
/// `writes[]`) is the human confirmation step, not a body field. Finds the run's last
/// [`WriteKind::Write`] row regardless of whether it succeeded (matching
/// `bhtune history revert`'s own semantics exactly), requiring it to have recorded pre-write
/// values to revert to. A revert never attempts a nested rollback of itself if it fails
/// partway through -- see [`WriteKind`]'s doc comment.
///
/// Always `200` once the request itself is valid and no conflicting operation is active; see
/// [`reserve_connect_and_write`]'s doc comment for why a physical revert failure is not a
/// `4xx`/`5xx`.
#[utoipa::path(
    post,
    path = "/api/runs/{id}/revert",
    tag = "runs",
    params(
        ("id" = i64, Path, description = "Run id"),
    ),
    responses(
        (status = 200, description = "The revert was attempted; see `writes[]` in the body for its outcome.", body = RunDetailResponse),
        (status = 400, description = "The run isn't eligible for a post-hoc revert (still running, wrong driver, no PID tags/connection recorded, no recorded write-back to revert, or its pre-write values were never recorded), or the driver connection itself failed.", body = ErrorBody),
        (status = 404, description = "No run with that id.", body = ErrorBody),
        (status = 409, description = "A tune or another PID write/revert is already active.", body = ErrorBody),
    ),
)]
pub(crate) async fn revert_run(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
) -> Result<Json<RunDetailResponse>, ApiError> {
    let run = TuneRunRow::get(&state.pool, run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no run with id {run_id}")))?;
    require_writable_run(&run)?;
    let allow_uncertain_quality = state.config_snapshot()?.allow_uncertain_quality;

    let writes = TuneWriteRow::list_for_run(&state.pool, run_id).await?;
    let last_write = writes
        .iter()
        .rev()
        .find(|w| w.kind == WriteKind::Write)
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "run {run_id} has no recorded PID write-back to revert"
            ))
        })?;
    let response_level = last_write.response_level;
    let target = last_write.previous.ok_or_else(|| {
        ApiError::BadRequest(format!(
            "run {run_id}'s {response_level:?} PID write-back never recorded pre-write values; \
             nothing to revert to"
        ))
    })?;

    // `require_writable_run` already confirmed all three tags are `Some`.
    let p_tag = run.tags.proportional_constant.clone().unwrap();
    let i_tag = run.tags.integral_constant.clone().unwrap();
    let d_tag = run.tags.derivative_constant.clone().unwrap();

    let detail = reserve_connect_and_write(
        &state,
        run_id,
        &run,
        &p_tag,
        &i_tag,
        &d_tag,
        response_level,
        target,
        WriteKind::Revert,
        allow_uncertain_quality,
    )
    .await?;
    Ok(Json(detail))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/runs", post(start_run))
        .route("/api/runs/{id}/cancel", post(cancel_run))
        .route(
            "/api/runs/{id}/notes",
            put(update_notes).delete(delete_notes),
        )
        .route("/api/runs/{id}/write", post(write_run))
        .route("/api/runs/{id}/revert", post(revert_run))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use bhtune_core::LoopConfig;
    use bhtune_db::models::{Pagination, TuneRunFilter};
    use tower::ServiceExt;

    /// A fast-converging simulator-backed request body, mirroring `bhtune-cli`'s own
    /// `fast_simulator_args()` test fixture (`commands::tune`'s test module) field-for-field
    /// -- that fixture's doc comment explains why these exact values converge in well under
    /// a second of real wall-clock time.
    fn fast_simulator_request_json() -> serde_json::Value {
        serde_json::json!({
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
            "notes": "http test note",
        })
    }

    async fn post_json(
        app: axum::Router,
        path: &str,
        body: serde_json::Value,
    ) -> axum::http::Response<Body> {
        app.oneshot(
            Request::post(path)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    async fn body_json(response: axum::http::Response<Body>) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Polls `GET /api/runs/{id}` (via the merged `history` router) until `outcome` is no
    /// longer `"running"`, bounded so a real bug can't hang the test suite forever.
    async fn wait_for_outcome(state: &AppState, run_id: i64) -> serde_json::Value {
        wait_for_outcome_with_timeout(state, run_id, std::time::Duration::from_secs(10)).await
    }

    async fn wait_for_outcome_with_timeout(
        state: &AppState,
        run_id: i64,
        timeout: std::time::Duration,
    ) -> serde_json::Value {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let app = crate::build_router(state.clone());
            let response = app
                .oneshot(
                    Request::get(format!("/api/runs/{run_id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let detail = body_json(response).await;
            if detail["outcome"] != "running" {
                return detail;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("run {run_id} did not leave 'running' within {timeout:?}: {detail:?}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn wait_for_outcome_panics_after_an_injected_deadline() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = start_opcda_run(&state).await;
        let state_for_task = state.clone();
        let join = tokio::spawn(async move {
            wait_for_outcome_with_timeout(&state_for_task, run_id, std::time::Duration::ZERO).await
        });

        let panic = join.await.expect_err("a zero deadline should panic");
        assert!(panic.is_panic());
    }

    #[tokio::test]
    async fn starting_a_simulator_run_returns_201_and_it_eventually_completes() {
        let state = crate::test_support::in_memory_state().await;
        let app = crate::build_router(state.clone());

        let response = post_json(app, "/api/runs", fast_simulator_request_json()).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let detail = body_json(response).await;
        let run_id = detail["id"].as_i64().expect("response must carry an id");
        assert_eq!(detail["tag_name"], "ignored-for-simulator");

        let final_detail = wait_for_outcome(&state, run_id).await;
        assert_eq!(final_detail["outcome"], "completed");
        assert_eq!(final_detail["results"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn notes_can_be_edited_while_running_and_after_completion_then_deleted() {
        let state = crate::test_support::in_memory_state().await;
        let mut request = fast_simulator_request_json();
        request["poll_interval_ms"] = serde_json::json!(1000);
        request["cycles_count"] = serde_json::json!(50);

        let response = post_json(crate::build_router(state.clone()), "/api/runs", request).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let run_id = body_json(response).await["id"].as_i64().unwrap();

        let updated = crate::build_router(state.clone())
            .oneshot(
                Request::put(format!("/api/runs/{run_id}/notes"))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "notes": "edited while running"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        assert_eq!(body_json(updated).await["notes"], "edited while running");

        state.active_run.cancel(run_id).await;
        let final_detail = wait_for_outcome(&state, run_id).await;
        assert_eq!(final_detail["outcome"], "aborted");

        let replaced = crate::build_router(state.clone())
            .oneshot(
                Request::put(format!("/api/runs/{run_id}/notes"))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "notes": "edited after completion"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replaced.status(), StatusCode::OK);
        assert_eq!(
            body_json(replaced).await["notes"],
            "edited after completion"
        );

        let cleared = crate::build_router(state.clone())
            .oneshot(
                Request::delete(format!("/api/runs/{run_id}/notes"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cleared.status(), StatusCode::OK);
        assert!(body_json(cleared).await["notes"].is_null());
    }

    #[tokio::test]
    async fn run_route_lookups_propagate_database_failures_as_500() {
        let state = crate::test_support::in_memory_state().await;
        let app = crate::build_router(state.clone());
        state.pool.close().await;

        let update = app
            .clone()
            .oneshot(
                Request::put("/api/runs/1/notes")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"notes":"updated"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(update.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let delete = app
            .clone()
            .oneshot(
                Request::delete("/api/runs/1/notes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let write = post_json(
            app.clone(),
            "/api/runs/1/write",
            serde_json::json!({ "response_level": "moderate" }),
        )
        .await;
        assert_eq!(write.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let revert = post_empty(app, "/api/runs/1/revert").await;
        assert_eq!(revert.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn note_routes_propagate_failures_after_each_successful_database_step() {
        let update_state = crate::test_support::in_memory_state().await;
        let update_run_id = start_opcda_run(&update_state).await;
        let update_pool = update_state.pool.clone();
        let update_result = update_notes_with_hook(
            update_state.clone(),
            update_run_id,
            UpdateNotesRequest {
                notes: "updated".to_string(),
            },
            move |_| {
                let pool = update_pool.clone();
                async move { pool.close().await }
            },
        )
        .await;
        assert!(matches!(update_result, Err(ApiError::Internal(_))));

        let detail_state = crate::test_support::in_memory_state().await;
        let detail_run_id = start_opcda_run(&detail_state).await;
        let detail_pool = detail_state.pool.clone();
        let detail_result = update_notes_with_hook(
            detail_state.clone(),
            detail_run_id,
            UpdateNotesRequest {
                notes: "updated".to_string(),
            },
            move |stage| {
                let pool = detail_pool.clone();
                async move {
                    if stage == NotesHookStage::AfterUpdate {
                        pool.close().await;
                    }
                }
            },
        )
        .await;
        assert!(matches!(detail_result, Err(ApiError::Internal(_))));

        let delete_state = crate::test_support::in_memory_state().await;
        let delete_run_id = start_opcda_run(&delete_state).await;
        let delete_result = delete_notes_with_hook(delete_state.clone(), delete_run_id, |state| {
            let pool = state.pool.clone();
            async move { pool.close().await }
        })
        .await;
        assert!(matches!(delete_result, Err(ApiError::Internal(_))));
    }

    #[tokio::test]
    async fn starting_a_second_run_while_one_is_active_succeeds() {
        let state = crate::test_support::in_memory_state().await;

        // Slow enough (1s/tick, many cycles) that it is still active by the time the second
        // request below is issued, mirroring `bhtune-cli`'s own
        // `ctrl_c_aborts_a_running_tune_and_restores_the_loop` timing rationale.
        let mut slow_request = fast_simulator_request_json();
        slow_request["poll_interval_ms"] = serde_json::json!(1000);
        slow_request["cycles_count"] = serde_json::json!(50);

        let first = post_json(
            crate::build_router(state.clone()),
            "/api/runs",
            slow_request,
        )
        .await;
        assert_eq!(first.status(), StatusCode::CREATED);
        let first_id = body_json(first).await["id"].as_i64().unwrap();
        assert_eq!(state.active_run.active_run_ids().await, vec![first_id]);

        let second = post_json(
            crate::build_router(state.clone()),
            "/api/runs",
            fast_simulator_request_json(),
        )
        .await;
        assert_eq!(second.status(), StatusCode::CREATED);
        let second_id = body_json(second).await["id"].as_i64().unwrap();
        assert_ne!(second_id, first_id);

        // Clean up rather than leaving the slow run to finish on its own 50-cycle schedule.
        state.active_run.cancel(first_id).await;
        wait_for_outcome(&state, first_id).await;
        wait_for_outcome(&state, second_id).await;
    }

    /// Calling the handler directly (bypassing the router/tower stack) and racing two
    /// invocations with `tokio::join!` proves concurrent starts both survive their overlapping
    /// `prepare()` calls and register independent background tasks.
    #[tokio::test]
    async fn a_genuine_race_between_two_starts_creates_two_runs() {
        let state = crate::test_support::in_memory_state().await;

        let mut request_a = fast_simulator_request_json();
        request_a["notes"] = serde_json::json!("racer-a");
        let mut request_b = fast_simulator_request_json();
        request_b["notes"] = serde_json::json!("racer-b");

        let (result_a, result_b) = tokio::join!(
            start_run(
                State(state.clone()),
                Json(serde_json::from_value(request_a).unwrap())
            ),
            start_run(
                State(state.clone()),
                Json(serde_json::from_value(request_b).unwrap())
            ),
        );

        let outcomes = [result_a, result_b];
        assert!(
            outcomes.iter().all(Result::is_ok),
            "both concurrent starts should succeed: {outcomes:?}"
        );

        // Clean up both runs, whether either one finished before the other handler returned.
        for outcome in outcomes {
            let (_, Json(detail)) = outcome.unwrap();
            state.active_run.cancel(detail.id).await;
            wait_for_outcome(&state, detail.id).await;
        }
    }

    #[tokio::test]
    async fn a_reservation_starting_after_prepare_marks_the_new_run_failed() {
        let state = crate::test_support::in_memory_state().await;
        let reservation_id = 9_999;
        let result = start_run_with_hook(
            state.clone(),
            serde_json::from_value(fast_simulator_request_json()).unwrap(),
            |state| {
                let active_run = state.active_run.clone();
                async move {
                    active_run.reserve(reservation_id).await.unwrap();
                }
            },
        )
        .await;

        let error = result.unwrap_err();
        assert!(matches!(error, ApiError::Conflict(_)));
        let filter = TuneRunFilter::default();
        let runs = TuneRunRow::list(&state.pool, &filter, Pagination::default())
            .await
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, TuneOutcome::Failed);
        assert!(
            runs[0]
                .failure_reason
                .as_deref()
                .unwrap()
                .contains("no tune task was started")
        );
        state.active_run.release(reservation_id).await;
    }

    #[tokio::test]
    async fn starting_a_run_while_a_write_reservation_is_active_returns_409() {
        let state = crate::test_support::in_memory_state().await;
        let reservation_id = 9_998;
        state.active_run.reserve(reservation_id).await.unwrap();

        let response = post_json(
            crate::build_router(state.clone()),
            "/api/runs",
            fast_simulator_request_json(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(
            body_json(response).await["error"]
                .as_str()
                .unwrap()
                .contains("exclusive PID write/revert")
        );
        state.active_run.release(reservation_id).await;
    }

    #[tokio::test]
    async fn unknown_template_name_returns_400() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let mut request = fast_simulator_request_json();
        request["template"] = serde_json::json!("Not A Real Template");

        let response = post_json(app, "/api/runs", request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("Not A Real Template")
        );
    }

    #[tokio::test]
    async fn write_pid_without_yes_returns_400() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let mut request = fast_simulator_request_json();
        request["write_pid"] = serde_json::json!("aggressive");
        // `yes` omitted -- defaults to `false`.

        let response = post_json(app, "/api/runs", request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(error["error"].as_str().unwrap().contains("--yes"));
    }

    #[test]
    fn opcda_restore_timeout_requires_the_four_second_confirmation_window() {
        for timeout in [1, 3] {
            let mut request = fast_simulator_request_json();
            request["driver"] = serde_json::json!("opcda");
            request["restore_timeout_secs"] = serde_json::json!(timeout);
            let parsed: StartRunRequest = serde_json::from_value(request).unwrap();
            let error = parsed.into_tune_args().unwrap_err();
            assert!(matches!(
                error,
                ApiError::BadRequest(message) if message.contains("at least 4 seconds")
            ));
        }

        let mut request = fast_simulator_request_json();
        request["driver"] = serde_json::json!("opcda");
        request["restore_timeout_secs"] = serde_json::json!(4);
        let parsed: StartRunRequest = serde_json::from_value(request).unwrap();
        let args = parsed.into_tune_args().expect("4 seconds must be accepted");
        assert_eq!(args.restore_timeout_secs, 4);
        assert_eq!(args.driver, DriverKindArg::Opcda);
    }

    #[test]
    fn simulator_restore_timeout_remains_positive_only() {
        let mut request = fast_simulator_request_json();
        request["restore_timeout_secs"] = serde_json::json!(0);
        let parsed: StartRunRequest = serde_json::from_value(request).unwrap();
        let error = parsed.into_tune_args().unwrap_err();
        assert!(matches!(
            error,
            ApiError::BadRequest(message) if message.contains("greater than zero")
        ));

        let mut request = fast_simulator_request_json();
        request["restore_timeout_secs"] = serde_json::json!(1);
        let parsed: StartRunRequest = serde_json::from_value(request).unwrap();
        let args = parsed
            .into_tune_args()
            .expect("one second remains valid for the simulator");
        assert_eq!(args.restore_timeout_secs, 1);
        assert_eq!(args.driver, DriverKindArg::Simulator);
    }

    #[tokio::test]
    async fn opcda_restore_timeout_below_four_returns_400_before_prepare() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let mut request = fast_simulator_request_json();
        request["driver"] = serde_json::json!("opcda");
        request["restore_timeout_secs"] = serde_json::json!(3);

        let response = post_json(app, "/api/runs", request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("at least 4 seconds")
        );
    }

    #[tokio::test]
    async fn invalid_tag_override_returns_400_before_starting_a_run() {
        let state = crate::test_support::in_memory_state().await;
        let app = crate::build_router(state.clone());
        let mut request = fast_simulator_request_json();
        request["tag_overrides"] = serde_json::json!({
            "process_variable": "Loop\u{0000}PV"
        });

        let response = post_json(app, "/api/runs", request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            body_json(response).await["error"]
                .as_str()
                .unwrap()
                .contains("process_variable")
        );
        assert!(
            TuneRunRow::list(
                &state.pool,
                &TuneRunFilter::default(),
                Pagination::default(),
            )
            .await
            .unwrap()
            .is_empty()
        );
    }

    #[tokio::test]
    async fn a_json_number_that_overflows_f32_to_infinity_is_rejected() {
        // `1e40` is well-formed JSON (an ordinary, if large, decimal literal) but silently
        // saturates to `f32::INFINITY` on conversion -- serde_json never errors on this, so
        // this proves `into_tune_args`'s manual finiteness check is a real gap being closed,
        // not redundant with what axum's `Json` extractor already rejects.
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let mut request = fast_simulator_request_json();
        request["sim_gain"] = serde_json::json!(1e40);

        let response = post_json(app, "/api/runs", request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(error["error"].as_str().unwrap().contains("sim_gain"));
    }

    /// Covers `require_finite_if_some`'s `Some` arm specifically -- the sibling test above
    /// only exercises `require_finite` directly (via `sim_gain`, a plain non-optional
    /// `f32`), never a genuinely optional range field.
    #[tokio::test]
    async fn a_non_finite_optional_range_field_is_also_rejected() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let mut request = fast_simulator_request_json();
        request["pv_range_high"] = serde_json::json!(1e40);

        let response = post_json(app, "/api/runs", request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(error["error"].as_str().unwrap().contains("pv_range_high"));
    }

    /// Covers `require_finite_if_some`'s `None` arm -- every other test in this module sets
    /// `pv_range_high` explicitly, so omitting it entirely (deserializing to `None`, since
    /// `Option<f32>` fields default to `None` when missing with no `#[serde(default)]`
    /// needed) is the only way to reach it. `prepare()` still rejects the request -- a fixed
    /// PV range is mandatory for `driver: "simulator"` -- so this asserts `400`, just from a
    /// different validator further down the same handler.
    #[tokio::test]
    async fn an_omitted_optional_range_field_passes_validation_but_prepare_still_requires_it() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let mut request = fast_simulator_request_json();
        request.as_object_mut().unwrap().remove("pv_range_high");

        let response = post_json(app, "/api/runs", request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("--pv-range-high is required")
        );
    }

    /// Omits every field with a `#[serde(default = "...")]` custom default function. Every
    /// other test in this module sets these explicitly (for fast, deterministic convergence),
    /// which left the default-value functions themselves untested. Cancels immediately after
    /// starting rather than waiting for completion, since `poll_interval_ms` here really is
    /// the slow, real CLI default (800ms/tick).
    #[tokio::test]
    async fn omitted_fields_with_custom_defaults_fall_back_to_the_cli_defaults() {
        let state = crate::test_support::in_memory_state().await;
        let mut request = fast_simulator_request_json();
        let object = request.as_object_mut().unwrap();
        object.remove("sim_gain");
        object.remove("sim_tau");
        object.remove("sim_dead_time");
        object.remove("sim_initial_pv");
        object.remove("sim_initial_mv");
        object.remove("poll_interval_ms");
        object.remove("timeout_secs");
        object.remove("op_timeout_secs");
        object.remove("restore_timeout_secs");

        let parsed: StartRunRequest = serde_json::from_value(request.clone()).unwrap();
        assert_eq!(parsed.sim_gain, 1.0);
        assert_eq!(parsed.sim_tau, 2.0);
        assert_eq!(parsed.sim_dead_time, 5.0);
        assert_eq!(parsed.sim_initial_pv, 50.0);
        assert_eq!(parsed.sim_initial_mv, 50.0);
        assert_eq!(parsed.poll_interval_ms, 800);
        assert_eq!(parsed.timeout_secs, 3600);
        assert_eq!(parsed.op_timeout_secs, 30);
        assert_eq!(parsed.restore_timeout_secs, 30);

        let response = post_json(crate::build_router(state.clone()), "/api/runs", request).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let run_id = body_json(response).await["id"].as_i64().unwrap();

        state.active_run.cancel(run_id).await;
        wait_for_outcome(&state, run_id).await;
    }

    #[tokio::test]
    async fn zero_poll_interval_ms_is_rejected() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let mut request = fast_simulator_request_json();
        request["poll_interval_ms"] = serde_json::json!(0);

        let response = post_json(app, "/api/runs", request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("poll_interval_ms")
        );
    }

    #[tokio::test]
    async fn the_replay_driver_is_rejected() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let mut request = fast_simulator_request_json();
        request["driver"] = serde_json::json!("replay");

        let response = post_json(app, "/api/runs", request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(error["error"].as_str().unwrap().contains("replay"));
    }

    #[tokio::test]
    async fn cancelling_an_unknown_run_returns_404() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(
                Request::post("/api/runs/999999/cancel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cancelling_an_active_run_aborts_it_and_returns_204() {
        let state = crate::test_support::in_memory_state().await;

        let mut slow_request = fast_simulator_request_json();
        slow_request["poll_interval_ms"] = serde_json::json!(1000);
        slow_request["cycles_count"] = serde_json::json!(50);

        let start_response = post_json(
            crate::build_router(state.clone()),
            "/api/runs",
            slow_request,
        )
        .await;
        assert_eq!(start_response.status(), StatusCode::CREATED);
        let run_id = body_json(start_response).await["id"].as_i64().unwrap();

        let cancel_response = crate::build_router(state.clone())
            .oneshot(
                Request::post(format!("/api/runs/{run_id}/cancel"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cancel_response.status(), StatusCode::NO_CONTENT);

        let final_detail = wait_for_outcome(&state, run_id).await;
        assert_eq!(final_detail["outcome"], "aborted");
    }

    #[tokio::test]
    async fn cancelling_a_run_that_already_finished_still_returns_204() {
        let state = crate::test_support::in_memory_state().await;
        let start_response = post_json(
            crate::build_router(state.clone()),
            "/api/runs",
            fast_simulator_request_json(),
        )
        .await;
        let run_id = body_json(start_response).await["id"].as_i64().unwrap();
        wait_for_outcome(&state, run_id).await;

        let cancel_response = crate::build_router(state.clone())
            .oneshot(
                Request::post(format!("/api/runs/{run_id}/cancel"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cancel_response.status(), StatusCode::NO_CONTENT);
    }

    /// Starts (but does not complete, record a connection for, or attach any result/write
    /// to) an `opcda`-driven run with real PID constant tags derived from the Yokogawa
    /// CentumVP template -- the common setup shared by every `write_run`/`revert_run`
    /// eligibility fixture below. Uses `ControllerType::Pid` and
    /// `ProcessType::TemperatureHeatExchange` (the only process types PID is offered for,
    /// matching the legacy app's own rule -- see `core-model`) purely so `opc_write_values`
    /// never zeroes the derivative constant, keeping every written PID value in these tests
    /// exactly `10.0` regardless of which constant is inspected.
    async fn start_opcda_run(state: &AppState) -> i64 {
        let template_row =
            bhtune_db::models::DcsTemplateRow::get_by_name(&state.pool, "Yokogawa CentumVP")
                .await
                .unwrap()
                .unwrap();
        let template = template_row.template;
        let config = LoopConfig {
            process_type: ProcessType::TemperatureHeatExchange,
            controller_type: ControllerType::Pid,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 3,
            noise_protection_secs: 0,
            mrft_delay_secs: 0,
        };
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Loop3.PV", &template);
        let run = TuneRunRow::start(
            &state.pool,
            None,
            "Loop3",
            TuneDriver::Opcda,
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

    /// Same as `start_opcda_run`, but with all three PID constant tags stripped after
    /// derivation -- exercises `require_writable_run`'s "no PID constant tags configured"
    /// branch, which a normal built-in template's fixture can never reach (every built-in
    /// template always defines these suffixes; see `core-model`).
    async fn start_opcda_run_without_pid_tags(state: &AppState) -> i64 {
        let template_row =
            bhtune_db::models::DcsTemplateRow::get_by_name(&state.pool, "Yokogawa CentumVP")
                .await
                .unwrap()
                .unwrap();
        let template = template_row.template;
        let config = LoopConfig {
            process_type: ProcessType::TemperatureHeatExchange,
            controller_type: ControllerType::Pid,
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
            TuneDriver::Opcda,
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

    /// Marks `run_id` completed and attaches a single calculated Moderate result
    /// (P = I = D = `10.0`) -- the minimum `write_run` needs to have something to write.
    async fn add_moderate_result(state: &AppState, run_id: i64) {
        TuneRunRow::complete(&state.pool, run_id, Utc::now())
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
                td_minutes: 1.0,
                proportional: 10.0,
                integral: 10.0,
                derivative: 10.0,
            },
        )
        .await
        .unwrap();
    }

    /// The full happy-path fixture: a completed `opcda` run with a recorded connection to
    /// `bridge_host`/`opc_server` and a calculated Moderate result ready to write.
    async fn seed_writable_opcda_run(state: &AppState, bridge_host: &str, opc_server: &str) -> i64 {
        let run_id = start_opcda_run(state).await;
        TuneRunRow::record_connection(
            &state.pool,
            run_id,
            Some(opc_server),
            Some(bridge_host),
            "{}",
        )
        .await
        .unwrap();
        add_moderate_result(state, run_id).await;
        run_id
    }

    async fn post_empty(app: axum::Router, path: &str) -> axum::http::Response<Body> {
        app.oneshot(Request::post(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn write_run_succeeds_and_records_a_write_kind_row() {
        use crate::test_support::mock_bridge::{
            MockBridgeService, good_reading, start_mock_server,
        };

        let host = start_mock_server(MockBridgeService {
            read_response: good_reading("10.0"),
            write_response: opcda_bridge_proto::bridge::WriteResponse {
                tag_id: "ignored".to_string(),
                success: true,
                error: None,
            },
            ..Default::default()
        })
        .await;

        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, &host, "Sim.Server").await;

        let response = post_json(
            crate::build_router(state.clone()),
            &format!("/api/runs/{run_id}/write"),
            serde_json::json!({ "response_level": "moderate" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let detail = body_json(response).await;

        let writes = detail["writes"].as_array().unwrap();
        let write_row = writes.iter().find(|w| w["kind"] == "write").unwrap();
        assert_eq!(write_row["response_level"], "moderate");
        assert_eq!(write_row["success"], true);
        assert_eq!(write_row["proportional_written"], 10.0);
        assert_eq!(write_row["integral_written"], 10.0);
        assert_eq!(write_row["derivative_written"], 10.0);
        assert_eq!(write_row["proportional_readback"], 10.0);
        assert!(write_row["rollback_state"].is_null());

        // The exclusive reservation must be free again for a later request, not left held by this one.
        assert!(state.active_run.reserve(999).await.is_ok());
        state.active_run.release(999).await;
    }

    #[tokio::test]
    async fn write_reports_an_internal_error_when_the_run_vanishes_after_the_write() {
        use crate::test_support::mock_bridge::{
            MockBridgeService, good_reading, start_mock_server,
        };

        let host = start_mock_server(MockBridgeService {
            read_response: good_reading("10.0"),
            write_response: opcda_bridge_proto::bridge::WriteResponse {
                tag_id: "ignored".to_string(),
                success: true,
                error: None,
            },
            ..Default::default()
        })
        .await;
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, &host, "Sim.Server").await;
        let run = TuneRunRow::get(&state.pool, run_id).await.unwrap().unwrap();
        let p_tag = run.tags.proportional_constant.clone().unwrap();
        let i_tag = run.tags.integral_constant.clone().unwrap();
        let d_tag = run.tags.derivative_constant.clone().unwrap();
        let pool = state.pool.clone();

        let error = reserve_connect_and_write_with_hook(
            &state,
            run_id,
            &run,
            &p_tag,
            &i_tag,
            &d_tag,
            ResponseLevel::Moderate,
            WriteReadback {
                proportional: 10.0,
                integral: 10.0,
                derivative: 10.0,
            },
            WriteKind::Write,
            true,
            move |_| async move {
                assert!(TuneRunRow::delete(&pool, run_id).await.unwrap());
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ApiError::Internal(_)));
    }

    #[tokio::test]
    async fn write_propagates_an_unexpected_database_failure_and_releases_its_reservation() {
        use crate::test_support::mock_bridge::{
            MockBridgeService, good_reading, start_mock_server,
        };

        let host = start_mock_server(MockBridgeService {
            read_response: good_reading("10.0"),
            write_response: opcda_bridge_proto::bridge::WriteResponse {
                tag_id: "ignored".to_string(),
                success: true,
                error: None,
            },
            ..Default::default()
        })
        .await;
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, &host, "Sim.Server").await;
        let run = TuneRunRow::get(&state.pool, run_id).await.unwrap().unwrap();
        let p_tag = run.tags.proportional_constant.clone().unwrap();
        let i_tag = run.tags.integral_constant.clone().unwrap();
        let d_tag = run.tags.derivative_constant.clone().unwrap();

        let error = reserve_connect_and_write_with_hooks(
            &state,
            run_id,
            &run,
            &p_tag,
            &i_tag,
            &d_tag,
            ResponseLevel::Moderate,
            WriteReadback {
                proportional: 10.0,
                integral: 10.0,
                derivative: 10.0,
            },
            WriteKind::Write,
            true,
            |state| {
                let pool = state.pool.clone();
                async move { pool.close().await }
            },
            |_| async {},
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ApiError::Internal(_)));
        assert!(state.active_run.reserve(999).await.is_ok());
        state.active_run.release(999).await;
    }

    #[tokio::test]
    async fn write_run_reports_a_failed_write_as_200_not_an_http_error() {
        use crate::test_support::mock_bridge::{
            MockBridgeService, good_reading, start_mock_server,
        };

        let host = start_mock_server(MockBridgeService {
            // The pre-read (all three constants) still succeeds; every subsequent *write*
            // is rejected at the transport level, so the very first write attempted
            // (Proportional) fails before anything is confirmed -- no rollback is even
            // attempted, matching `write_pid_values`'s documented "nothing yet to roll
            // back" short-circuit.
            read_response: good_reading("10.0"),
            write_error: Some(tonic::Status::invalid_argument("nope")),
            ..Default::default()
        })
        .await;

        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, &host, "Sim.Server").await;

        let response = post_json(
            crate::build_router(state.clone()),
            &format!("/api/runs/{run_id}/write"),
            serde_json::json!({ "response_level": "moderate" }),
        )
        .await;
        // A physical write failure is not an HTTP error -- see `reserve_connect_and_write`'s
        // doc comment.
        assert_eq!(response.status(), StatusCode::OK);
        let detail = body_json(response).await;

        let writes = detail["writes"].as_array().unwrap();
        let write_row = writes.iter().find(|w| w["kind"] == "write").unwrap();
        assert_eq!(write_row["success"], false);
        // `DriverError::Operation`'s `Display` is the fixed message "driver operation
        // failed" (thiserror doesn't interpolate the boxed source's own text unless the
        // format string names it), so this asserts on that fixed wording rather than the
        // mock's "nope" status message, which never surfaces here.
        assert!(
            write_row["error_message"]
                .as_str()
                .unwrap()
                .contains("driver operation failed")
        );
        assert!(write_row["rollback_state"].is_null());
    }

    #[tokio::test]
    async fn write_run_reports_a_failed_pre_read_as_200_not_an_http_error() {
        use crate::test_support::mock_bridge::{MockBridgeService, start_mock_server};

        let host = start_mock_server(MockBridgeService {
            // Every `read` (including the Proportional pre-read, the very first driver call
            // `write_pid_values` makes) is rejected at the transport level -- no `write` is
            // ever attempted, and the resulting row's `previous` stays entirely unset.
            read_error: Some(tonic::Status::unavailable("gateway unreachable")),
            ..Default::default()
        })
        .await;

        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, &host, "Sim.Server").await;

        let response = post_json(
            crate::build_router(state),
            &format!("/api/runs/{run_id}/write"),
            serde_json::json!({ "response_level": "moderate" }),
        )
        .await;
        // A failed pre-read is not an HTTP error either -- same rationale as a failed write.
        assert_eq!(response.status(), StatusCode::OK);
        let detail = body_json(response).await;

        let writes = detail["writes"].as_array().unwrap();
        let write_row = writes.iter().find(|w| w["kind"] == "write").unwrap();
        assert_eq!(write_row["success"], false);
        assert!(write_row["proportional_previous"].is_null());
        assert!(write_row["proportional_written"].is_null());
        assert!(write_row["rollback_state"].is_null());
        assert!(
            write_row["error_message"]
                .as_str()
                .unwrap()
                .contains("pre-read")
        );
    }

    #[tokio::test]
    async fn write_run_returns_404_for_unknown_run() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let response = post_json(
            app,
            "/api/runs/999999/write",
            serde_json::json!({ "response_level": "moderate" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn write_run_returns_400_when_run_is_still_running() {
        let state = crate::test_support::in_memory_state().await;
        // Never completed -- `require_writable_run` checks this before the driver, tags, or
        // connection, so no result/connection needs to be attached for this fixture.
        let run_id = start_opcda_run(&state).await;

        let response = post_json(
            crate::build_router(state),
            &format!("/api/runs/{run_id}/write"),
            serde_json::json!({ "response_level": "moderate" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(error["error"].as_str().unwrap().contains("still running"));
    }

    #[tokio::test]
    async fn write_run_returns_400_for_simulator_driver() {
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
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Sim.PV", &template);
        let run = TuneRunRow::start(
            &state.pool,
            None,
            "SimLoop",
            TuneDriver::Simulator,
            config,
            template_row.origin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        add_moderate_result(&state, run.id).await;

        let response = post_json(
            crate::build_router(state),
            &format!("/api/runs/{}/write", run.id),
            serde_json::json!({ "response_level": "moderate" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(error["error"].as_str().unwrap().contains("Simulator"));
    }

    #[tokio::test]
    async fn write_run_returns_400_when_run_has_no_pid_constant_tags() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = start_opcda_run_without_pid_tags(&state).await;
        TuneRunRow::complete(&state.pool, run_id, Utc::now())
            .await
            .unwrap();

        let response = post_json(
            crate::build_router(state),
            &format!("/api/runs/{run_id}/write"),
            serde_json::json!({ "response_level": "moderate" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("no PID constant tags configured")
        );
    }

    #[tokio::test]
    async fn each_missing_pid_constant_tag_is_rejected_individually() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, "127.0.0.1:1", "Sim.Server").await;
        let base = TuneRunRow::get(&state.pool, run_id).await.unwrap().unwrap();

        let mut missing_proportional = base.clone();
        missing_proportional.tags.proportional_constant = None;
        let mut missing_integral = base.clone();
        missing_integral.tags.integral_constant = None;
        let mut missing_derivative = base;
        missing_derivative.tags.derivative_constant = None;

        for (missing_tag, run) in [
            ("proportional", missing_proportional),
            ("integral", missing_integral),
            ("derivative", missing_derivative),
        ] {
            let error = require_writable_run(&run).unwrap_err();
            assert!(
                matches!(
                    error,
                    ApiError::BadRequest(ref message)
                        if message.contains("no PID constant tags configured")
                ),
                "missing {missing_tag} tag should be rejected: {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn each_missing_recorded_connection_field_is_rejected_individually() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, "127.0.0.1:1", "Sim.Server").await;
        let base = TuneRunRow::get(&state.pool, run_id).await.unwrap().unwrap();

        let mut missing_server = base.clone();
        missing_server.opc_server = None;
        let mut missing_bridge = base;
        missing_bridge.bridge_host = None;

        for (missing_field, run) in [
            ("opc_server", missing_server),
            ("bridge_host", missing_bridge),
        ] {
            let error = require_writable_run(&run).unwrap_err();
            assert!(
                matches!(
                    error,
                    ApiError::BadRequest(ref message)
                        if message.contains("no recorded OPC server")
                ),
                "missing {missing_field} should be rejected: {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn write_run_returns_400_when_no_connection_was_recorded() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = start_opcda_run(&state).await;
        add_moderate_result(&state, run_id).await;
        // `record_connection` deliberately never called -- mirrors a run that somehow
        // never recorded its connection (should not happen in practice, since `start_run`
        // always records it for an `opcda` run, but `require_writable_run` must still
        // refuse to guess).

        let response = post_json(
            crate::build_router(state),
            &format!("/api/runs/{run_id}/write"),
            serde_json::json!({ "response_level": "moderate" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("no recorded OPC server")
        );
    }

    #[tokio::test]
    async fn write_run_returns_400_when_no_result_for_the_requested_level() {
        let state = crate::test_support::in_memory_state().await;
        // Only a Moderate result is attached -- requesting Sluggish must fail cleanly.
        let run_id = seed_writable_opcda_run(&state, "127.0.0.1:1", "Sim.Server").await;

        let response = post_json(
            crate::build_router(state),
            &format!("/api/runs/{run_id}/write"),
            serde_json::json!({ "response_level": "sluggish" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(error["error"].as_str().unwrap().contains("Sluggish"));
    }

    #[tokio::test]
    async fn write_run_returns_400_when_the_driver_connection_fails() {
        let state = crate::test_support::in_memory_state().await;
        // Nothing is listening on this port, so `OpcDaDriver::connect` fails at the
        // transport level -- mirrors `bhtune-driver`'s own
        // `connect_failure_maps_to_driver_error_connect` test.
        let run_id = seed_writable_opcda_run(&state, "127.0.0.1:1", "Sim.Server").await;

        let response = post_json(
            crate::build_router(state),
            &format!("/api/runs/{run_id}/write"),
            serde_json::json!({ "response_level": "moderate" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("failed to connect")
        );
    }

    #[tokio::test]
    async fn write_run_returns_409_when_another_operation_is_active() {
        use crate::test_support::mock_bridge::{
            MockBridgeService, good_reading, start_mock_server,
        };

        let host = start_mock_server(MockBridgeService {
            read_response: good_reading("10.0"),
            write_response: opcda_bridge_proto::bridge::WriteResponse {
                tag_id: "ignored".to_string(),
                success: true,
                error: None,
            },
            ..Default::default()
        })
        .await;

        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, &host, "Sim.Server").await;
        state.active_run.reserve(424242).await.unwrap();

        let response = post_json(
            crate::build_router(state.clone()),
            &format!("/api/runs/{run_id}/write"),
            serde_json::json!({ "response_level": "moderate" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let error = body_json(response).await;
        assert!(error["error"].as_str().unwrap().contains("424242"));

        state.active_run.release(424242).await;
    }

    #[tokio::test]
    async fn revert_run_succeeds_and_records_a_revert_kind_row() {
        use crate::test_support::mock_bridge::{
            MockBridgeService, good_reading, start_mock_server,
        };

        let host = start_mock_server(MockBridgeService {
            read_response: good_reading("10.0"),
            write_response: opcda_bridge_proto::bridge::WriteResponse {
                tag_id: "ignored".to_string(),
                success: true,
                error: None,
            },
            ..Default::default()
        })
        .await;

        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, &host, "Sim.Server").await;

        let mut previous_write =
            bhtune_db::models::NewTuneWrite::new(ResponseLevel::Moderate, Utc::now());
        previous_write.previous = Some(WriteReadback {
            proportional: 10.0,
            integral: 10.0,
            derivative: 10.0,
        });
        previous_write.proportional_written = Some(66.7);
        previous_write.integral_written = Some(2.0);
        previous_write.derivative_written = Some(0.5);
        previous_write.proportional_readback = Some(66.7);
        previous_write.integral_readback = Some(2.0);
        previous_write.derivative_readback = Some(0.5);
        previous_write.success = true;
        TuneWriteRow::insert(&state.pool, run_id, previous_write)
            .await
            .unwrap();

        let response = post_empty(
            crate::build_router(state.clone()),
            &format!("/api/runs/{run_id}/revert"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let detail = body_json(response).await;

        let writes = detail["writes"].as_array().unwrap();
        assert_eq!(writes.len(), 2);
        let revert_row = writes.iter().find(|w| w["kind"] == "revert").unwrap();
        assert_eq!(revert_row["response_level"], "moderate");
        assert_eq!(revert_row["success"], true);
        assert_eq!(revert_row["proportional_written"], 10.0);
        assert_eq!(revert_row["integral_written"], 10.0);
        assert_eq!(revert_row["derivative_written"], 10.0);
        // Reverts never chain a nested rollback of themselves.
        assert!(revert_row["rollback_state"].is_null());
    }

    #[tokio::test]
    async fn revert_run_returns_400_when_there_is_no_write_to_revert() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, "127.0.0.1:1", "Sim.Server").await;
        // No `TuneWriteRow` attached at all.

        let response = post_empty(
            crate::build_router(state),
            &format!("/api/runs/{run_id}/revert"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("no recorded PID write-back to revert")
        );
    }

    #[tokio::test]
    async fn revert_run_returns_400_when_the_last_write_has_no_previous_values() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, "127.0.0.1:1", "Sim.Server").await;

        // A `Write`-kind row whose pre-read itself failed -- `previous` stays `None` and
        // nothing else on the row was ever attempted (mirrors `write_pid_values`'s
        // pre-read-failure short-circuit). No driver connection is needed to prove this:
        // `revert_run` must refuse before ever trying to connect.
        let mut failed_write =
            bhtune_db::models::NewTuneWrite::new(ResponseLevel::Moderate, Utc::now());
        failed_write.success = false;
        failed_write.error_message =
            Some("pre-read of Proportional tag failed: unavailable".to_string());
        TuneWriteRow::insert(&state.pool, run_id, failed_write)
            .await
            .unwrap();

        let response = post_empty(
            crate::build_router(state),
            &format!("/api/runs/{run_id}/revert"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await;
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("never recorded pre-write values")
        );
    }

    #[tokio::test]
    async fn revert_run_returns_400_when_the_driver_connection_fails() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_writable_opcda_run(&state, "127.0.0.1:1", "Sim.Server").await;
        let mut previous_write =
            bhtune_db::models::NewTuneWrite::new(ResponseLevel::Moderate, Utc::now());
        previous_write.previous = Some(WriteReadback {
            proportional: 10.0,
            integral: 20.0,
            derivative: 30.0,
        });
        previous_write.success = true;
        TuneWriteRow::insert(&state.pool, run_id, previous_write)
            .await
            .unwrap();

        let response = post_empty(
            crate::build_router(state),
            &format!("/api/runs/{run_id}/revert"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            body_json(response).await["error"]
                .as_str()
                .unwrap()
                .contains("failed to connect")
        );
    }

    #[tokio::test]
    async fn revert_run_returns_404_for_unknown_run() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let response = post_empty(app, "/api/runs/999999/revert").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
