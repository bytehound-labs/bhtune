//! `POST /api/runs` (start a new tune run) and `POST /api/runs/{id}/cancel` (request its
//! cancellation) -- the write side of the run-history API `routes::history` reads from.
//!
//! Reuses `bhtune-cli`'s own [`bhtune_cli::commands::tune::prepare`]/[`bhtune_cli::commands::tune::drive`]
//! split unchanged, so a run started over HTTP goes through exactly the same template
//! lookup, tag derivation, backend connection, quality checks, restore-on-abort, and
//! write-back rollback as a run started by the CLI -- only the setup/reporting differs (see
//! those functions' own doc comments for the full rationale). `crate::active_run` enforces
//! the v1 constraint that only one run may be active at a time.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use bhtune_cli::args::{BackendKindArg, TuneArgs};
use bhtune_cli::cancel::CtrlC;
use bhtune_cli::commands::tune::{drive, prepare};
use bhtune_cli::output::OutputFormat;
use bhtune_core::{ControllerDirection, ControllerType, ProcessType, ResponseLevel};
use bhtune_db::models::{TuneBackend, TuneRunRow};
use chrono::Utc;
use serde::Deserialize;
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
#[derive(Debug, Deserialize, ToSchema)]
pub struct StartRunRequest {
    /// PV tag prefix; ignored for `backend: "simulator"`. See [`TuneArgs::tagname`].
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
    /// Which backend drives this tune. `"replay"` is rejected -- that backend exists only
    /// for offline golden-trace validation, not for starting a live/simulated run.
    pub backend: TuneBackend,
    /// opcda-bridge gateway address. Only meaningful with `backend: "opcda"` (default:
    /// resolved the same way the CLI resolves `--bridge-host`, via this process's own
    /// config/env).
    pub bridge_host: Option<String>,
    /// OPC DA server ProgID. Required with `backend: "opcda"`.
    pub server: Option<String>,
    /// Simulator process gain (`backend: "simulator"` only).
    #[serde(default = "default_sim_gain")]
    pub sim_gain: f32,
    /// Simulator process time constant, in seconds (`backend: "simulator"` only).
    #[serde(default = "default_sim_tau")]
    pub sim_tau: f32,
    /// Simulator dead time, in seconds (`backend: "simulator"` only).
    #[serde(default = "default_sim_dead_time")]
    pub sim_dead_time: f32,
    /// Simulator measurement noise amplitude (`backend: "simulator"` only).
    #[serde(default)]
    pub sim_noise: f32,
    /// Simulator RNG seed, for reproducible noise (`backend: "simulator"` only).
    #[serde(default)]
    pub sim_seed: u64,
    /// Simulator initial PV (`backend: "simulator"` only).
    #[serde(default = "default_sim_initial_value")]
    pub sim_initial_pv: f32,
    /// Simulator initial MV (`backend: "simulator"` only).
    #[serde(default = "default_sim_initial_value")]
    pub sim_initial_mv: f32,
    /// Fixed PV range high, overriding a live tag read. Required for `backend: "simulator"`,
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
    /// How often to poll the backend, in milliseconds.
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
    /// Hard wall-clock cap on this run's total duration, in seconds. See
    /// [`TuneArgs::timeout_secs`] -- always enforced, exactly as for a CLI-driven run.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// A friendly name for this run, recorded as `loop_name` (default: the PV tag name).
    pub name: Option<String>,
    /// Confirm an unattended PID write-back. Required alongside `write_pid` -- the request
    /// is rejected otherwise, identically to `--write-pid` without `--yes` on the CLI.
    #[serde(default)]
    pub yes: bool,
    /// Non-interactively write this response level's calculated PID parameters back to the
    /// DCS. Requires `yes: true`.
    pub write_pid: Option<ResponseLevel>,
    /// Accept `Quality::Uncertain` OPC readings instead of hard-failing on them. See
    /// [`TuneArgs::allow_uncertain_quality`].
    #[serde(default)]
    pub allow_uncertain_quality: bool,
    /// Cap on any single backend read/write during the run, in seconds.
    #[serde(default = "default_op_or_restore_timeout_secs")]
    pub op_timeout_secs: u64,
    /// Cap on restoring the loop to its pre-test state after the run ends, in seconds.
    #[serde(default = "default_op_or_restore_timeout_secs")]
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
        require_positive("restore_timeout_secs", self.restore_timeout_secs)?;

        let backend = BackendKindArg::try_from(self.backend)
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;

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
            backend,
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
            poll_interval_ms: self.poll_interval_ms,
            timeout_secs: self.timeout_secs,
            name: self.name,
            yes: self.yes,
            write_pid: self.write_pid.map(Into::into),
            allow_uncertain_quality: self.allow_uncertain_quality,
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
/// `POST /api/runs` -- runs `prepare()` (template lookup, tag derivation, backend connect,
/// and the `tune_runs` insert) inline and returns as soon as that succeeds, having already
/// `tokio::spawn`ed the actual polling/tuning phase in the background. `201 Created` carries
/// the same [`RunDetailResponse`] `GET /api/runs/{id}` would show for this run at this
/// instant (almost certainly still `outcome: "running"`) -- poll that endpoint, or use
/// `POST /api/runs/{id}/cancel`, to follow the run to completion.
///
/// `409 Conflict` if another run is already active: v1 allows only one at a time (see
/// `crate::active_run`).
#[utoipa::path(
    post,
    path = "/api/runs",
    tag = "runs",
    request_body = StartRunRequest,
    responses(
        (status = 201, description = "The run was started; detail reflects its state right now.", body = RunDetailResponse),
        (status = 400, description = "The request failed validation, or `prepare()` itself failed (unknown template, invalid flag combination, unreachable backend).", body = ErrorBody),
        (status = 409, description = "Another tune run is already active.", body = ErrorBody),
    ),
)]
pub(crate) async fn start_run(
    State(state): State<AppState>,
    Json(request): Json<StartRunRequest>,
) -> Result<(StatusCode, Json<RunDetailResponse>), ApiError> {
    // Optimistic pre-check: avoids a wasted `prepare()` call (template lookup, a real
    // backend connection attempt) in the common case where it's already obvious a run is
    // active. Not authoritative -- `state.active_run.start()` below is what actually decides,
    // since a run can start or finish between this check and that call.
    if let Some(active_id) = state.active_run.active_run_id().await {
        return Err(ApiError::Conflict(format!(
            "run {active_id} is already active; cancel it first via `POST /api/runs/{active_id}/cancel` or wait for it to finish"
        )));
    }

    let args = request.into_tune_args()?;

    // `prepare()`'s own doc comment: its failures (bad template name, `--write-pid` without
    // `--yes`, an unreachable backend) are "exactly the kind of problem an HTTP client
    // expects a synchronous error response for" -- so they map to `400`, not the generic
    // `500` a bare `?`/`Internal` conversion would give.
    let prepared = prepare(&state.pool, args, &state.app_config)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let run_id = prepared.run_id();

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

    // The authoritative check: if this loses the race (another `POST /api/runs` reserved the
    // slot between the pre-check above and here), nothing has mutated the live loop yet --
    // `prepare()` only connects and inserts a row -- so the just-inserted row is marked
    // `failed` rather than left forever showing an outcome it never actually reached.
    if let Err(RunAlreadyActive { run_id: existing }) =
        state.active_run.start(run_id, cancel_handle, task).await
    {
        let failure_reason = format!(
            "run {existing} was already active when this run tried to start; no backend I/O was performed"
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/runs", post(start_run))
        .route("/api/runs/{id}/cancel", post(cancel_run))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
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
            "backend": "simulator",
            "sim_gain": 1.0,
            "sim_tau": 0.01,
            "sim_dead_time": 0.025,
            "pv_range_high": 100.0,
            "pv_range_low": 0.0,
            "mv_range_high": 100.0,
            "mv_range_low": 0.0,
            "direction": "reverse",
            "poll_interval_ms": 5,
            "name": "http-test-loop",
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
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
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
                panic!("run {run_id} did not leave 'running' within 10s: {detail:?}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn starting_a_simulator_run_returns_201_and_it_eventually_completes() {
        let state = crate::test_support::in_memory_state().await;
        let app = crate::build_router(state.clone());

        let response = post_json(app, "/api/runs", fast_simulator_request_json()).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let detail = body_json(response).await;
        let run_id = detail["id"].as_i64().expect("response must carry an id");
        assert_eq!(detail["loop_name"], "http-test-loop");

        let final_detail = wait_for_outcome(&state, run_id).await;
        assert_eq!(final_detail["outcome"], "completed");
        assert_eq!(final_detail["results"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn starting_a_second_run_while_one_is_active_returns_409() {
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

        let second = post_json(
            crate::build_router(state.clone()),
            "/api/runs",
            fast_simulator_request_json(),
        )
        .await;
        assert_eq!(second.status(), StatusCode::CONFLICT);
        let error = body_json(second).await;
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains(&first_id.to_string()),
            "409 body should name the already-active run id, got: {error:?}"
        );

        // Clean up rather than leaving the slow run to finish on its own 50-cycle schedule.
        state.active_run.cancel(first_id).await;
        wait_for_outcome(&state, first_id).await;
    }

    /// Exercises the *authoritative* conflict check inside `start_run` itself -- as opposed
    /// to `starting_a_second_run_while_one_is_active_returns_409`'s optimistic pre-check,
    /// which always wins the race in practice because the first request's `active_run.start()`
    /// call has already completed by the time a sequential second HTTP request's handler even
    /// begins. Calling the handler directly (bypassing the router/tower stack) and racing two
    /// invocations with `tokio::join!` gives both a real chance to pass the optimistic
    /// pre-check before either reaches its own authoritative `active_run.start()` -- `prepare()`
    /// awaits real (if in-memory) database I/O in between, which is what creates the window.
    #[tokio::test]
    async fn a_genuine_race_between_two_starts_marks_the_losing_row_failed() {
        let state = crate::test_support::in_memory_state().await;

        let mut request_a = fast_simulator_request_json();
        request_a["name"] = serde_json::json!("racer-a");
        let mut request_b = fast_simulator_request_json();
        request_b["name"] = serde_json::json!("racer-b");

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

        // Exactly one of the two must win -- the other must lose, specifically via the
        // authoritative check (identifiable by its distinct wording, "no backend I/O was
        // performed", versus the optimistic pre-check's "cancel it first via ..." message).
        let outcomes = [result_a, result_b];
        let winners = outcomes.iter().filter(|r| r.is_ok()).count();
        let losers: Vec<_> = outcomes.iter().filter_map(|r| r.as_ref().err()).collect();
        assert_eq!(winners, 1, "exactly one racer must win: {outcomes:?}");
        assert_eq!(losers.len(), 1);
        let ApiError::Conflict(message) = losers[0] else {
            panic!("loser must be a 409 Conflict, got {:?}", losers[0]);
        };
        assert!(
            message.contains("no backend I/O was performed"),
            "loser should be rejected by the authoritative check specifically, got: {message}"
        );

        // Clean up whichever run actually won the race.
        for (_, Json(detail)) in outcomes.iter().flatten() {
            state.active_run.cancel(detail.id).await;
            wait_for_outcome(&state, detail.id).await;
        }
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
    /// PV range is mandatory for `backend: "simulator"` -- so this asserts `400`, just from a
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

    /// Omits every field with a `#[serde(default = "...")]` custom default function
    /// (`sim_gain`, `sim_tau`, `sim_dead_time`, `poll_interval_ms`) -- every other test in
    /// this module always sets these explicitly (for fast, deterministic convergence), which
    /// left the default-value functions themselves untested. Cancels immediately after
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
        object.remove("poll_interval_ms");

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
    async fn the_replay_backend_is_rejected() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let mut request = fast_simulator_request_json();
        request["backend"] = serde_json::json!("replay");

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
}
