use super::super::helpers::{
    map_revert_error, refreshed_run_detail, reserve_connect_and_write_with_hook,
    reserve_connect_and_write_with_hooks,
};
use super::*;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use bhtune_cli::args::{Cli as CliArgs, Command as CliCommand};
use bhtune_core::{ControllerType, LoopConfig, ProcessType, ResponseLevel};
use bhtune_db::models::{Pagination, TuneDriver, TuneOutcome, TuneRunFilter, TuneWriteRow};
use bhtune_runtime::tune::ValidatedTuneRequest;
use clap::Parser;
use tokio::sync::oneshot;
use tower::ServiceExt;

/// A fast-converging simulator-backed request body, mirroring `bhtune-cli`'s own
/// `fast_simulator_args()` test fixture for the per-run inputs. Global timing is supplied
/// by `in_memory_state()`'s fast test configuration.
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
        "notes": "http test note",
    })
}

fn minimal_http_start_request() -> serde_json::Value {
    serde_json::json!({
        "tagname": "Unit1.LIC101.PV",
        "template": "Yokogawa CentumVP",
        "process_type": "flow",
        "controller_type": "pi",
        "relay_amp": 10.0,
        "driver": "simulator",
    })
}

fn cli_validated_request(
    driver: &str,
    extra_args: &[&str],
) -> Result<ValidatedTuneRequest, String> {
    let mut args = vec![
        "bhtune",
        "tune",
        "--tagname",
        "Unit1.LIC101.PV",
        "--template",
        "Yokogawa CentumVP",
        "--process-type",
        "flow",
        "--controller-type",
        "pi",
        "--relay-amp",
        "10",
        "--driver",
        driver,
    ];
    args.extend_from_slice(extra_args);
    let cli = CliArgs::try_parse_from(args).map_err(|error| error.to_string())?;
    let CliCommand::Tune(args) = cli.command else {
        return Err("expected the tune command".to_owned());
    };
    ValidatedTuneRequest::try_from(args).map_err(|error| error.to_string())
}

fn http_validated_request(value: serde_json::Value) -> Result<ValidatedTuneRequest, String> {
    let request: StartRunRequest =
        serde_json::from_value(value).map_err(|error| error.to_string())?;
    request
        .into_validated_tune_request()
        .map_err(|error| match error {
            ApiError::BadRequest(message) => message,
            other => format!("{other:?}"),
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

#[test]
fn cli_and_http_start_requests_share_simulator_defaults() {
    let cli = cli_validated_request("simulator", &[]).unwrap();
    let http = http_validated_request(minimal_http_start_request()).unwrap();
    assert_eq!(cli.as_request(), http.as_request());

    let request = cli.as_request();
    assert_eq!(request.sim_gain, 1.0);
    assert_eq!(request.sim_tau, 2.0);
    assert_eq!(request.sim_dead_time, 5.0);
    assert_eq!(request.sim_noise, 0.0);
    assert_eq!(request.sim_seed, 0);
    assert_eq!(request.sim_initial_pv, 50.0);
    assert_eq!(request.sim_initial_mv, 50.0);
    assert_eq!(request.cycles_skip, None);
    assert_eq!(request.cycles_count, None);
    assert_eq!(request.noise_protection_secs, None);
}

#[test]
fn equivalent_cli_and_http_inputs_produce_the_same_validated_request() {
    let cli = cli_validated_request(
        "simulator",
        &[
            "--cycles-skip",
            "1",
            "--cycles-count",
            "3",
            "--noise-protection-secs",
            "2",
            "--bridge-host",
            "127.0.0.1:7600",
            "--server",
            "MockServer",
            "--sim-gain",
            "0.75",
            "--sim-tau",
            "1.5",
            "--sim-dead-time",
            "0.25",
            "--sim-noise",
            "0.05",
            "--sim-seed",
            "42",
            "--sim-initial-pv",
            "45",
            "--sim-initial-mv",
            "35",
            "--pv-range-high",
            "100",
            "--pv-range-low",
            "0",
            "--mv-range-high",
            "100",
            "--mv-range-low",
            "0",
            "--direction",
            "reverse",
            "--notes",
            "scheduled",
            "--yes",
            "--write-pid",
            "moderate",
        ],
    )
    .unwrap();
    let mut http_request = minimal_http_start_request();
    http_request["cycles_skip"] = serde_json::json!(1);
    http_request["cycles_count"] = serde_json::json!(3);
    http_request["noise_protection_secs"] = serde_json::json!(2);
    http_request["bridge_host"] = serde_json::json!("127.0.0.1:7600");
    http_request["server"] = serde_json::json!("MockServer");
    http_request["sim_gain"] = serde_json::json!(0.75);
    http_request["sim_tau"] = serde_json::json!(1.5);
    http_request["sim_dead_time"] = serde_json::json!(0.25);
    http_request["sim_noise"] = serde_json::json!(0.05);
    http_request["sim_seed"] = serde_json::json!(42);
    http_request["sim_initial_pv"] = serde_json::json!(45.0);
    http_request["sim_initial_mv"] = serde_json::json!(35.0);
    http_request["pv_range_high"] = serde_json::json!(100.0);
    http_request["pv_range_low"] = serde_json::json!(0.0);
    http_request["mv_range_high"] = serde_json::json!(100.0);
    http_request["mv_range_low"] = serde_json::json!(0.0);
    http_request["direction"] = serde_json::json!("reverse");
    http_request["notes"] = serde_json::json!("scheduled");
    http_request["yes"] = serde_json::json!(true);
    http_request["write_pid"] = serde_json::json!("moderate");
    let http = http_validated_request(http_request).unwrap();

    assert_eq!(cli.as_request(), http.as_request());
}

#[test]
fn cli_and_http_reject_the_same_invalid_values() {
    let cli_error = cli_validated_request("simulator", &["--sim-gain", "1e40"]).unwrap_err();
    let mut http_request = minimal_http_start_request();
    http_request["sim_gain"] = serde_json::json!(1e40);
    let http_error = http_validated_request(http_request).unwrap_err();
    assert!(cli_error.contains("finite"), "{cli_error}");
    assert!(http_error.contains("finite"), "{http_error}");

    let cli_error = cli_validated_request("simulator", &["--cycles-count", "0"]).unwrap_err();
    let mut http_request = minimal_http_start_request();
    http_request["cycles_count"] = serde_json::json!(0);
    let http_error = http_validated_request(http_request).unwrap_err();
    assert!(cli_error.contains("at least 1"), "{cli_error}");
    assert!(http_error.contains("at least 1"), "{http_error}");

    let cli_error = cli_validated_request("simulator", &["--pv-range-high", "1e40"]).unwrap_err();
    let mut http_request = minimal_http_start_request();
    http_request["pv_range_high"] = serde_json::json!(1e40);
    let http_error = http_validated_request(http_request).unwrap_err();
    assert!(cli_error.contains("finite"), "{cli_error}");
    assert!(http_error.contains("pv_range_high"), "{http_error}");
}

#[test]
fn cli_and_http_reject_replay_as_a_tune_driver() {
    let cli_error = cli_validated_request("replay", &[]).unwrap_err();
    let mut http_request = minimal_http_start_request();
    http_request["driver"] = serde_json::json!("replay");
    let http_error = http_validated_request(http_request).unwrap_err();
    assert!(cli_error.contains("replay"), "{cli_error}");
    assert!(http_error.contains("replay"), "{http_error}");
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

async fn wait_until_inactive(state: &AppState, run_id: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while state.active_run.active_run_ids().await.contains(&run_id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a terminal run should release its active-run registration");
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
async fn wait_until_inactive_observes_an_active_registration_before_release() {
    let state = crate::test_support::in_memory_state().await;
    let run_id = 42;
    let (_ctrl_c, cancel) = bhtune_runtime::cancel::CtrlC::manual();
    let (finish_tx, finish_rx) = oneshot::channel();
    state
        .active_run
        .start(run_id, cancel, async move {
            finish_rx.await.unwrap();
        })
        .await
        .unwrap();

    let release = tokio::spawn(async move {
        tokio::task::yield_now().await;
        finish_tx.send(()).unwrap();
    });
    wait_until_inactive(&state, run_id).await;
    release.await.unwrap();
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
    wait_until_inactive(&state, run_id).await;
    assert!(state.active_run.reserve(999).await.is_ok());
    state.active_run.release(999).await;
}

#[tokio::test]
async fn notes_can_be_edited_while_running_and_after_completion_then_deleted() {
    let state = crate::test_support::in_memory_state().await;
    let mut request = fast_simulator_request_json();
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

    // Many cycles keep the first run active by the time the second request below is issued.
    let mut slow_request = fast_simulator_request_json();
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
fn legacy_http_timing_fields_are_ignored() {
    let mut request = fast_simulator_request_json();
    request["mrft_delay"] = serde_json::json!(123);
    request["poll_interval_ms"] = serde_json::json!(1);
    request["timeout_secs"] = serde_json::json!(1);
    request["op_timeout_secs"] = serde_json::json!(1);
    request["restore_timeout_secs"] = serde_json::json!(1);

    let parsed: StartRunRequest = serde_json::from_value(request).unwrap();
    let args = parsed
        .into_validated_tune_request()
        .expect("legacy fields must not affect request parsing");
    assert_eq!(args.as_request().template, "Yokogawa CentumVP");
    assert_eq!(args.as_request().relay_amp, 10.0);
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
async fn a_zero_cycles_count_is_rejected_before_creating_a_run() {
    let state = crate::test_support::in_memory_state().await;
    let mut request = fast_simulator_request_json();
    request["cycles_count"] = serde_json::json!(0);

    let response = post_json(crate::build_router(state.clone()), "/api/runs", request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        body_json(response).await["error"]
            .as_str()
            .unwrap()
            .contains("at least 1")
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
    // this proves runtime request validation is not redundant with what axum's `Json`
    // extractor already rejects.
    let app = crate::build_router(crate::test_support::in_memory_state().await);
    let mut request = fast_simulator_request_json();
    request["sim_gain"] = serde_json::json!(1e40);

    let response = post_json(app, "/api/runs", request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error = body_json(response).await;
    assert!(error["error"].as_str().unwrap().contains("sim_gain"));
}

/// Covers common validation of a genuinely optional range field.
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

/// Missing optional ranges pass common validation; `prepare()` then enforces the simulator's
/// requirement that fixed PV and MV ranges be supplied.
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

/// Omits all simulator fields with request defaults. The timing values are global
/// configuration, not request defaults.
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

    let parsed: StartRunRequest = serde_json::from_value(request.clone()).unwrap();
    assert_eq!(parsed.sim_gain, 1.0);
    assert_eq!(parsed.sim_tau, 2.0);
    assert_eq!(parsed.sim_dead_time, 5.0);
    assert_eq!(parsed.sim_noise, 0.0);
    assert_eq!(parsed.sim_seed, 0);
    assert_eq!(parsed.sim_initial_pv, 50.0);
    assert_eq!(parsed.sim_initial_mv, 50.0);

    let response = post_json(crate::build_router(state.clone()), "/api/runs", request).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let run_id = body_json(response).await["id"].as_i64().unwrap();

    state.active_run.cancel(run_id).await;
    wait_for_outcome(&state, run_id).await;
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
    wait_until_inactive(&state, run_id).await;
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
            kp: Some(1.5),
            ti_minutes: Some(2.0),
            td_minutes: Some(1.0),
            proportional: Some(10.0),
            integral: Some(10.0),
            derivative: Some(10.0),
            status: bhtune_core::TuningResultStatus::Valid,
            invalid_reason: None,
        },
    )
    .await
    .unwrap();
}

async fn add_invalid_moderate_result(state: &AppState, run_id: i64) {
    TuneRunRow::complete(&state.pool, run_id, Utc::now())
        .await
        .unwrap();
    TuneResultRow::insert(
        &state.pool,
        &TuneResultRow {
            id: 0,
            run_id,
            response_level: ResponseLevel::Moderate,
            kp: None,
            ti_minutes: None,
            td_minutes: None,
            proportional: None,
            integral: None,
            derivative: None,
            status: bhtune_core::TuningResultStatus::Invalid,
            invalid_reason: Some(bhtune_core::TuningResultInvalidReason::NonPositivePvAmplitude),
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
    use crate::test_support::mock_bridge::{MockBridgeService, good_reading, start_mock_server};

    let (host, _host_server) = start_mock_server(MockBridgeService {
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

/// A later write must not replace the snapshot recorded when the run started.
#[tokio::test]
async fn write_run_keeps_an_existing_gateway_compatibility_snapshot() {
    use crate::test_support::mock_bridge::{MockBridgeService, good_reading, start_mock_server};

    let (host, _host_server) = start_mock_server(MockBridgeService {
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
    let existing = r#"{"status":"full","source":"prior"}"#;
    TuneRunRow::record_gateway_compatibility(&state.pool, run_id, existing)
        .await
        .unwrap();

    let response = post_json(
        crate::build_router(state.clone()),
        &format!("/api/runs/{run_id}/write"),
        serde_json::json!({ "response_level": "moderate" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let run = TuneRunRow::get(&state.pool, run_id).await.unwrap().unwrap();
    assert_eq!(run.gateway_compatibility_json.as_deref(), Some(existing));
}

#[tokio::test]
async fn write_reports_an_internal_error_when_the_run_vanishes_after_the_write() {
    use crate::test_support::mock_bridge::{MockBridgeService, good_reading, start_mock_server};

    let (host, _host_server) = start_mock_server(MockBridgeService {
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
    use crate::test_support::mock_bridge::{MockBridgeService, good_reading, start_mock_server};

    let (host, _host_server) = start_mock_server(MockBridgeService {
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
    use crate::test_support::mock_bridge::{MockBridgeService, good_reading, start_mock_server};

    let (host, _host_server) = start_mock_server(MockBridgeService {
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

    let (host, _host_server) = start_mock_server(MockBridgeService {
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
async fn write_run_rejects_an_invalid_result_before_connecting_to_the_driver() {
    let state = crate::test_support::in_memory_state().await;
    let run_id = start_opcda_run(&state).await;
    TuneRunRow::record_connection(
        &state.pool,
        run_id,
        Some("Sim.Server"),
        Some("127.0.0.1:1"),
        "{}",
    )
    .await
    .unwrap();
    add_invalid_moderate_result(&state, run_id).await;

    let response = post_json(
        crate::build_router(state),
        &format!("/api/runs/{run_id}/write"),
        serde_json::json!({ "response_level": "moderate" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error = body_json(response).await;
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("Moderate"));
    assert!(message.contains("invalid"));
    assert!(message.contains("PV amplitude is not positive"));
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
async fn write_run_returns_400_when_the_gateway_core_is_incompatible() {
    use crate::test_support::mock_bridge::{MockBridgeService, start_mock_server};
    use opcda_bridge_proto::bridge::{
        GetGatewayInfoResponse, ProtocolFeature, ProtocolFeatureKind,
    };

    let (host, _host_server) = start_mock_server(MockBridgeService {
        gateway_info_response: GetGatewayInfoResponse {
            application_version: "0.5.9".to_string(),
            compatibility_schema_version: 1,
            features: vec![ProtocolFeature {
                kind: ProtocolFeatureKind::Core as i32,
                min_version: 9,
                max_version: 9,
            }],
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
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error = body_json(response).await;
    assert!(error["error"].as_str().unwrap().contains("incompatible"));
    assert!(
        TuneWriteRow::list_for_run(&state.pool, run_id)
            .await
            .unwrap()
            .is_empty()
    );
    let run = TuneRunRow::get(&state.pool, run_id).await.unwrap().unwrap();
    assert!(run.gateway_compatibility_json.is_none());
}

#[tokio::test]
async fn write_run_returns_409_when_another_operation_is_active() {
    use crate::test_support::mock_bridge::{MockBridgeService, good_reading, start_mock_server};

    let (host, _host_server) = start_mock_server(MockBridgeService {
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
    use crate::test_support::mock_bridge::{MockBridgeService, good_reading, start_mock_server};

    let (host, _host_server) = start_mock_server(MockBridgeService {
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
async fn revert_run_can_restore_recorded_values_when_calculated_result_is_invalid() {
    use crate::test_support::mock_bridge::{MockBridgeService, good_reading, start_mock_server};

    let (host, _host_server) = start_mock_server(MockBridgeService {
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
    let run_id = start_opcda_run(&state).await;
    TuneRunRow::record_connection(&state.pool, run_id, Some("Sim.Server"), Some(&host), "{}")
        .await
        .unwrap();
    add_invalid_moderate_result(&state, run_id).await;

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
        crate::build_router(state),
        &format!("/api/runs/{run_id}/revert"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let detail = body_json(response).await;
    assert_eq!(detail["results"][0]["status"], "invalid");
    let revert_row = detail["writes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|write| write["kind"] == "revert")
        .unwrap();
    assert_eq!(revert_row["success"], true);
    assert_eq!(revert_row["derivative_written"], 10.0);
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

#[tokio::test]
async fn revert_run_reservation_conflict_is_reported_before_runtime_work() {
    let state = crate::test_support::in_memory_state().await;
    state.active_run.reserve(424242).await.unwrap();

    let result = reserve_and_revert(&state, 7, true).await;
    assert!(matches!(
        result,
        Err(ApiError::Conflict(message)) if message.contains("424242")
    ));

    state.active_run.release(424242).await;
}

#[tokio::test]
async fn revert_error_mapping_preserves_http_error_categories() {
    use bhtune_runtime::history::RevertRunError;

    assert!(matches!(
        map_revert_error(RevertRunError::Invalid(anyhow::anyhow!("invalid"))),
        ApiError::BadRequest(_)
    ));
    assert!(matches!(
        map_revert_error(RevertRunError::Connection(anyhow::anyhow!("connection"))),
        ApiError::BadRequest(_)
    ));
    assert!(matches!(
        map_revert_error(RevertRunError::Gateway(anyhow::anyhow!("gateway"))),
        ApiError::BadRequest(_)
    ));
    assert!(matches!(
        map_revert_error(RevertRunError::Persistence(anyhow::anyhow!("database"))),
        ApiError::Internal(_)
    ));
}

#[tokio::test]
async fn refreshed_run_detail_reports_a_run_removed_after_a_live_operation() {
    let state = crate::test_support::in_memory_state().await;
    assert!(matches!(
        refreshed_run_detail(&state.pool, 999999).await,
        Err(ApiError::Internal(_))
    ));
}
