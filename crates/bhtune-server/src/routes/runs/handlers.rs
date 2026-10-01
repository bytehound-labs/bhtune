use super::helpers::{
    D_TAG_PRESENT, I_TAG_PRESENT, INSERTED_RUN_PRESENT, P_TAG_PRESENT, normalized_notes,
    reserve_and_revert, reserve_connect_and_write,
};
use super::validation::require_writable_run;
use super::{
    ApiError, AppState, CtrlC, ErrorBody, Json, Path, RunAlreadyActive, RunDetailResponse,
    StartRunRequest, State, StatusCode, TuneResultRow, TuneRunRow, UpdateNotesRequest, Utc,
    WriteKind, WriteReadback, WriteRunRequest, build_run_detail, drive, opc_write_values,
    pid_parameters_for_result, prepare, require_present,
};

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

    let request = request.into_tune_request()?;

    // `prepare()`'s own doc comment: its failures (bad template name, `write_pid` without
    // `yes`, an unreachable driver) are "exactly the kind of problem an HTTP client
    // expects a synchronous error response for" -- so they map to `400`, not the generic
    // `500` a bare `?`/`Internal` conversion would give.
    let app_config = state.config_snapshot()?;
    let prepared = prepare(&state.pool, request, &app_config)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let run_id = prepared.run_id();
    after_prepare(&state).await;

    let (ctrl_c, cancel_handle) = CtrlC::manual();
    let pool_for_task = state.pool.clone();
    let task = async move {
        let mut ctrl_c = ctrl_c;
        // `drive()` already records every outcome (completion, abort, failure) to the
        // `tune_runs` row itself; this task has no caller left to report a `Result` to, so
        // its own `Err` is intentionally discarded here.
        let _ = drive(&pool_for_task, prepared, &mut ctrl_c).await;
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

    let built = build_run_detail(&state.pool, run_id).await?;
    let detail = require_present(built, INSERTED_RUN_PRESENT)?;
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

    let pid = pid_parameters_for_result(selected)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let written = opc_write_values(pid, run.config.controller_type, run.template.integral_type);
    let target = WriteReadback {
        proportional: written.proportional,
        integral: written.integral,
        derivative: written.derivative,
    };

    // `require_writable_run` already confirmed all three tags are `Some`.
    let p_tag = require_present(run.tags.proportional_constant.clone(), P_TAG_PRESENT)?;
    let i_tag = require_present(run.tags.integral_constant.clone(), I_TAG_PRESENT)?;
    let d_tag = require_present(run.tags.derivative_constant.clone(), D_TAG_PRESENT)?;

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
    TuneRunRow::get(&state.pool, run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no run with id {run_id}")))?;
    let allow_uncertain_quality = state.config_snapshot()?.allow_uncertain_quality;
    let detail = reserve_and_revert(&state, run_id, allow_uncertain_quality).await?;
    Ok(Json(detail))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
