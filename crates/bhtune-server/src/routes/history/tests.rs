use super::*;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use bhtune_db::models::NewTuneMvActuation;
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
        r"
            INSERT INTO loops (
                name, dcs_template_id, tags_json, process_type, controller_type,
                relay_amp_percent, num_cycles_skip, num_cycles_count, noise_protection_secs,
                mrft_delay_secs, created_at, updated_at
            ) VALUES ('Loop2', ?, '{}', 'flow', 'pi', 5.0, 1, 3, 0, 0, ?, ?)
            ",
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

    TuneRunRow::record_timing_metrics(
        &state.pool,
        run_id,
        TimingMetrics {
            basis: bhtune_db::models::TimingBasis::SimulatedFixedStep,
            requested_interval_ms: 5,
            sample_gap_count: 9,
            mean_sample_gap_ms: Some(5.0),
            max_sample_gap_ms: Some(5.0),
            missed_poll_opportunity_count: 0,
            measured_oscillation_period_ms: Some(50.0),
            approximate_samples_per_period: Some(10.0),
            sampling_adequacy: bhtune_db::models::SamplingAdequacy::Adequate,
            poll_latency: Some(bhtune_db::models::PollLatencyMetrics {
                pv_read: bhtune_db::models::TimingSummary {
                    count: 10,
                    mean_ms: Some(1.25),
                    max_ms: Some(2.5),
                },
                mv_write: bhtune_db::models::TimingSummary {
                    count: 3,
                    mean_ms: Some(3.5),
                    max_ms: Some(4.0),
                },
                mv_verification: bhtune_db::models::TimingSummary {
                    count: 3,
                    mean_ms: Some(2.0),
                    max_ms: Some(2.5),
                },
                sample_persist: bhtune_db::models::TimingSummary {
                    count: 10,
                    mean_ms: Some(0.75),
                    max_ms: Some(1.0),
                },
                tick_work: bhtune_db::models::TimingSummary {
                    count: 10,
                    mean_ms: Some(5.5),
                    max_ms: Some(7.0),
                },
            }),
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
            kp: Some(1.5),
            ti_minutes: Some(2.0),
            td_minutes: Some(0.0),
            proportional: Some(66.7),
            integral: Some(2.0),
            derivative: Some(0.0),
            status: TuningResultStatus::Valid,
            invalid_reason: None,
        },
    )
    .await
    .unwrap();

    // A confirmed write: previous/written/readback all populated, no rollback needed.
    let mut successful_write = bhtune_db::models::NewTuneWrite::new(ResponseLevel::Moderate, now);
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

    let confirmed_actuation = TuneMvActuationRow::insert_pending(
        &state.pool,
        run_id,
        NewTuneMvActuation {
            sequence: 0,
            kind: MvActuationKind::Relay,
            commanded_at: now,
            target_mv: 55.0,
            previous_commanded_mv: Some(50.0),
            tolerance: 0.5,
            confirmation_due_at: now + chrono::Duration::seconds(4),
        },
    )
    .await
    .unwrap();
    TuneMvActuationRow::record_final_observation(
        &state.pool,
        confirmed_actuation.id,
        now + chrono::Duration::seconds(1),
        Some(55.0),
        Some(SampleQuality::Good),
        MvActuationStatus::Confirmed,
        None,
    )
    .await
    .unwrap();

    let superseded_actuation = TuneMvActuationRow::insert_pending(
        &state.pool,
        run_id,
        NewTuneMvActuation {
            sequence: 1,
            kind: MvActuationKind::Restore,
            commanded_at: now + chrono::Duration::seconds(5),
            target_mv: 50.0,
            previous_commanded_mv: Some(55.0),
            tolerance: 0.5,
            confirmation_due_at: now + chrono::Duration::seconds(9),
        },
    )
    .await
    .unwrap();
    TuneMvActuationRow::finalize(
        &state.pool,
        superseded_actuation.id,
        MvActuationStatus::Superseded,
        Some("restore took over confirmation"),
    )
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
    assert!(body["timing_metrics"].is_null());
    // Yokogawa CentumVP always defines P/I/D suffixes, so a run derived from it always
    // has all three PID constant tags -- see `pid_constant_tags`'s doc comment.
    assert_eq!(body["pid_constant_tags"]["proportional"], "Loop1.P");
    assert_eq!(body["pid_constant_tags"]["integral"], "Loop1.I");
    assert_eq!(body["pid_constant_tags"]["derivative"], "Loop1.D");
    assert_eq!(body["pid_parameter_labels"]["proportional"], "P");
    assert_eq!(body["pid_parameter_labels"]["integral"], "I");
    assert_eq!(body["pid_parameter_labels"]["derivative"], "D");
    // `seed_one_run` never calls `record_connection`, so `request_json` is left at the
    // column default `"{}"` -- not a valid `StartRunRequest`, so `original_request` must
    // gracefully read `null` rather than the request failing (see
    // `parse_stored_request`'s doc comment).
    assert!(body["original_request"].is_null());
}

#[tokio::test]
async fn show_run_includes_a_stored_gateway_compatibility_snapshot() {
    let state = crate::test_support::in_memory_state().await;
    let run_id = seed_one_run(&state).await;
    let snapshot = serde_json::json!({
        "client_version": "0.1.0",
        "gateway_version": "0.5.9",
        "source": "gateway_info",
        "status": "partial",
        "features": []
    });
    bhtune_db::models::TuneRunRow::record_gateway_compatibility(
        &state.pool,
        run_id,
        &snapshot.to_string(),
    )
    .await
    .unwrap();

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
    assert_eq!(body["gateway_compatibility"]["status"], "partial");
    assert_eq!(body["gateway_compatibility"]["gateway_version"], "0.5.9");
    assert_eq!(body["gateway_compatibility"]["source"], "gateway_info");
}

#[test]
fn pid_parameter_labels_fall_back_for_blank_template_suffixes() {
    let mut template = bhtune_core::built_in_templates().remove(0);
    template.proportional_constant_suffix.clear();
    template.integral_constant_suffix.clear();
    template.derivative_constant_suffix.clear();

    let labels = PidParameterLabelsResponse::from(&template);

    assert_eq!(labels.proportional, "P");
    assert_eq!(labels.integral, "I");
    assert_eq!(labels.derivative, "D");
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

    let timing = &body["timing_metrics"];
    assert_eq!(timing["basis"], "simulated_fixed_step");
    assert_eq!(timing["requested_interval_ms"], 5);
    assert_eq!(timing["sample_gap_count"], 9);
    assert_eq!(timing["mean_sample_gap_ms"], 5.0);
    assert_eq!(timing["max_sample_gap_ms"], 5.0);
    assert_eq!(timing["missed_poll_opportunity_count"], 0);
    assert_eq!(timing["measured_oscillation_period_ms"], 50.0);
    assert_eq!(timing["approximate_samples_per_period"], 10.0);
    assert_eq!(timing["sampling_adequacy"], "adequate");
    assert_eq!(timing["poll_latency"]["pv_read"]["count"], 10);
    assert_eq!(timing["poll_latency"]["pv_read"]["mean_ms"], 1.25);
    assert_eq!(timing["poll_latency"]["pv_read"]["max_ms"], 2.5);
    assert_eq!(timing["poll_latency"]["mv_write"]["count"], 3);
    assert_eq!(timing["poll_latency"]["mv_verification"]["count"], 3);
    assert_eq!(timing["poll_latency"]["sample_persist"]["count"], 10);
    assert_eq!(timing["poll_latency"]["tick_work"]["count"], 10);

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

    let actuations = body["mv_actuations"].as_array().unwrap();
    assert_eq!(actuations.len(), 2);
    let confirmed = actuations
        .iter()
        .find(|actuation| actuation["kind"] == "relay")
        .unwrap();
    assert_eq!(confirmed["sequence"], 0);
    assert_eq!(confirmed["target_mv"], 55.0);
    assert_eq!(confirmed["previous_commanded_mv"], 50.0);
    assert_eq!(confirmed["tolerance"], 0.5);
    assert!(confirmed["commanded_at"].is_string());
    assert!(confirmed["confirmation_due_at"].is_string());
    assert!(confirmed["last_checked_at"].is_string());
    assert_eq!(confirmed["readback_mv"], 55.0);
    assert_eq!(confirmed["readback_quality"], "good");
    assert_eq!(confirmed["attempt_count"], 1);
    assert_eq!(confirmed["status"], "confirmed");
    assert!(confirmed["detail"].is_null());

    let superseded = actuations
        .iter()
        .find(|actuation| actuation["kind"] == "restore")
        .unwrap();
    assert_eq!(superseded["sequence"], 1);
    assert_eq!(superseded["target_mv"], 50.0);
    assert_eq!(superseded["previous_commanded_mv"], 55.0);
    assert!(superseded["last_checked_at"].is_null());
    assert!(superseded["readback_mv"].is_null());
    assert!(superseded["readback_quality"].is_null());
    assert_eq!(superseded["attempt_count"], 0);
    assert_eq!(superseded["status"], "superseded");
    assert_eq!(superseded["detail"], "restore took over confirmation");
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
            Request::get("/api/runs?opc_server=Kepware.KEPServerEX.V6&bridge_host=gateway-a:7600")
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
async fn show_and_delete_propagate_database_failures_as_500() {
    let state = crate::test_support::in_memory_state().await;
    let app = router().with_state(state.clone());
    state.pool.close().await;

    let show = app
        .clone()
        .oneshot(Request::get("/api/runs/1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(show.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let delete = app
        .oneshot(Request::delete("/api/runs/1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::INTERNAL_SERVER_ERROR);
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
async fn delete_run_409s_while_live_recovery_is_pending() {
    let state = crate::test_support::in_memory_state().await;
    let run_id = seed_one_run(&state).await;
    TuneRunRow::fail(&state.pool, run_id, Utc::now(), "orphaned live tune")
        .await
        .unwrap();
    sqlx::query("UPDATE tune_runs SET recovery_state = 'eligible' WHERE id = ?")
        .bind(run_id)
        .execute(&state.pool)
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
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let retained = TuneRunRow::get(&pool, run_id).await.unwrap().unwrap();
    assert_eq!(retained.recovery_state, Some(TuneRecoveryState::Eligible));
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
    let (_ctrl_c, handle) = bhtune_runtime::cancel::CtrlC::manual();
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

#[tokio::test]
async fn delete_run_returns_404_when_the_row_vanishes_after_lookup() {
    let state = crate::test_support::in_memory_state().await;
    let run_id = seed_one_run(&state).await;
    TuneRunRow::complete(&state.pool, run_id, Utc::now())
        .await
        .unwrap();

    let result = delete_run_with_hook(state.clone(), run_id, |state| {
        let pool = state.pool.clone();
        async move {
            assert!(TuneRunRow::delete(&pool, run_id).await.unwrap());
        }
    })
    .await;

    assert!(
        matches!(result, Err(ApiError::NotFound(message)) if message.contains(&run_id.to_string()))
    );
}

#[tokio::test]
async fn delete_run_conflicts_if_recovery_becomes_pending_after_lookup() {
    let state = crate::test_support::in_memory_state().await;
    let run_id = seed_one_run(&state).await;
    TuneRunRow::complete(&state.pool, run_id, Utc::now())
        .await
        .unwrap();

    let result = delete_run_with_hook(state.clone(), run_id, |state| {
        let pool = state.pool.clone();
        async move {
            sqlx::query("UPDATE tune_runs SET recovery_state = 'eligible' WHERE id = ?")
                .bind(run_id)
                .execute(&pool)
                .await
                .unwrap();
        }
    })
    .await;

    assert!(
        matches!(result, Err(ApiError::Conflict(message)) if message.contains("unresolved live-loop recovery"))
    );
    assert!(
        TuneRunRow::get(&state.pool, run_id)
            .await
            .unwrap()
            .is_some()
    );
}
