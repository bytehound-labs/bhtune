use super::*;
use bhtune_core::{ControllerType, DcsTemplate, LoopConfig, LoopTags, ProcessType};
use bhtune_db::models::{NewTuneMvActuation, TemplateOrigin, TuneDriver, TuneRunInitialReadings};

fn sample_config() -> LoopConfig {
    LoopConfig {
        process_type: ProcessType::Flow,
        controller_type: ControllerType::Pi,
        relay_amp_percent: 10.0,
        num_cycles_skip: 1,
        num_cycles_count: 2,
        noise_protection_secs: 3,
        mrft_delay_secs: 0,
    }
}

fn sample_template() -> DcsTemplate {
    bhtune_core::built_in_templates().remove(0)
}

fn sample_tags() -> LoopTags {
    LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &sample_template())
}

#[test]
fn row_and_connection_display_decisions_match_their_data() {
    assert!(!has_rows::<u8>(&[]));
    assert!(has_rows(&[1_u8]));
    assert!(!has_recorded_connection(None, None));
    assert!(has_recorded_connection(Some("Sim.Server"), None));
    assert!(has_recorded_connection(None, Some("127.0.0.1:7600")));
    assert!(has_recorded_connection(
        Some("Sim.Server"),
        Some("127.0.0.1:7600")
    ));
}

#[test]
fn pid_audit_formatting_preserves_original_precision() {
    assert_eq!(fmt_pid_recorded(Some(1.234567)), "1.234567");
    assert_eq!(fmt_pid_recorded(Some(9999.0)), "9999");
    assert_eq!(fmt_pid_recorded(None), "-");
}

#[test]
fn table_output_decision_matches_the_requested_format() {
    assert!(is_table_output(OutputFormat::Table));
    assert!(!is_table_output(OutputFormat::Json));
}

#[test]
fn sampling_adequacy_labels_cover_all_states() {
    assert_eq!(
        sampling_adequacy_label(SamplingAdequacy::Adequate),
        "adequate"
    );
    assert_eq!(
        sampling_adequacy_label(SamplingAdequacy::Marginal),
        "marginal"
    );
    assert_eq!(
        sampling_adequacy_label(SamplingAdequacy::NotAssessed),
        "not assessed"
    );
}

#[test]
fn fmt_opt_f32_formats_values_and_missing_fields() {
    assert_eq!(fmt_opt_f32(Some(1.23456)), "1.2346");
    assert_eq!(fmt_opt_f32(Some(-0.5)), "-0.5000");
    assert_eq!(fmt_opt_f32(None), "-");
}

#[test]
fn fmt_opt_f64_formats_values_and_missing_fields() {
    assert_eq!(fmt_opt_f64(Some(1.23456)), "1.235");
    assert_eq!(fmt_opt_f64(Some(-0.5)), "-0.500");
    assert_eq!(fmt_opt_f64(None), "-");
}

#[tokio::test]
async fn list_handles_an_empty_database() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    list(&pool, None, 50, 0, OutputFormat::Table).await.unwrap();
    list(&pool, None, 50, 0, OutputFormat::Json).await.unwrap();
}

#[tokio::test]
async fn list_propagates_database_errors() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    pool.close().await;

    assert!(list(&pool, None, 50, 0, OutputFormat::Table).await.is_err());
}

#[tokio::test]
async fn list_and_show_reflect_a_real_run() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let now = chrono::Utc::now();
    let run = TuneRunRow::start(
        &pool,
        None,
        "Unit1.LIC101.PV",
        TuneDriver::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();

    TuneRunRow::record_initial_readings(
        &pool,
        run.id,
        TuneRunInitialReadings {
            pv_ini: 50.0,
            mv_ini: 50.0,
            mv_range_low: 0.0,
            mv_range_high: 100.0,
            pv_range_high: 100.0,
            pv_range_low: 0.0,
            controller_direction: bhtune_core::ControllerDirection::Reverse,
            mode_raw: Some("1".to_string()),
            mode_attribute_raw: None,
            setpoint_ini: Some(50.0),
        },
    )
    .await
    .unwrap();

    TuneRunRow::record_timing_metrics(
        &pool,
        run.id,
        bhtune_db::models::TimingMetrics {
            basis: bhtune_db::models::TimingBasis::SimulatedFixedStep,
            requested_interval_ms: 5,
            sample_gap_count: 9,
            mean_sample_gap_ms: Some(5.0),
            max_sample_gap_ms: Some(5.0),
            missed_poll_opportunity_count: 0,
            measured_oscillation_period_ms: Some(50.0),
            approximate_samples_per_period: Some(10.0),
            sampling_adequacy: bhtune_db::models::SamplingAdequacy::Adequate,
            poll_latency: None,
        },
    )
    .await
    .unwrap();

    TuneRunRow::complete(&pool, run.id, now).await.unwrap();

    list(&pool, None, 50, 0, OutputFormat::Table).await.unwrap();
    list(
        &pool,
        Some(bhtune_db::models::TuneOutcome::Completed),
        50,
        0,
        OutputFormat::Table,
    )
    .await
    .unwrap();
    show(&pool, run.id, OutputFormat::Table).await.unwrap();
    // This run has recorded initial readings (unlike the other JSON-path fixtures in
    // this file), so this is the one call site that exercises `InitialReadingsJson`'s
    // conversion.
    show(&pool, run.id, OutputFormat::Json).await.unwrap();
}

#[tokio::test]
async fn show_errors_for_an_unknown_run() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let err = show(&pool, 999, OutputFormat::Table).await.unwrap_err();
    assert!(err.to_string().contains("999"));
}

#[tokio::test]
async fn show_handles_a_failed_run_with_no_initial_readings() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let now = chrono::Utc::now();
    let run = TuneRunRow::start(
        &pool,
        None,
        "Unit1.LIC101.PV",
        TuneDriver::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();
    TuneRunRow::fail(&pool, run.id, now, "connection refused")
        .await
        .unwrap();
    show(&pool, run.id, OutputFormat::Table).await.unwrap();
    show(&pool, run.id, OutputFormat::Json).await.unwrap();
}

#[tokio::test]
async fn show_prints_incomplete_restore_status() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let now = chrono::Utc::now();
    let run = TuneRunRow::start(
        &pool,
        None,
        "Unit1.LIC101.PV",
        TuneDriver::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();
    TuneRunRow::complete(&pool, run.id, now).await.unwrap();
    TuneRunRow::record_restore_status(
        &pool,
        run.id,
        bhtune_db::models::RestoreStatus::Incomplete,
        Some("MV: write failed"),
    )
    .await
    .unwrap();

    show(&pool, run.id, OutputFormat::Table).await.unwrap();
}

/// A run carrying at least one `TuneResultRow` and one `TuneWriteRow`, so `show`'s
/// "Calculated results" and "PID write-back audit" print blocks (otherwise never
/// exercised, since every other fixture in this file completes with no results/writes
/// recorded) both execute.
async fn run_with_results_and_writes() -> (SqlitePool, i64) {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let now = chrono::Utc::now();
    let run = TuneRunRow::start(
        &pool,
        None,
        "Unit1.LIC101.PV",
        TuneDriver::Opcda,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();
    // Records a real connection so `show`'s "Connection:" table line and its JSON
    // `opc_server`/`bridge_host` fields both get exercised here, rather than only ever
    // seeing the `None`/`None` simulator case elsewhere in this file.
    TuneRunRow::record_connection(
        &pool,
        run.id,
        Some("Kepware.KEPServerEX.V6"),
        Some("127.0.0.1:7600"),
        "{}",
    )
    .await
    .unwrap();
    TuneRunRow::complete(&pool, run.id, now).await.unwrap();

    TuneResultRow::insert(
        &pool,
        &TuneResultRow {
            id: 0,
            run_id: run.id,
            response_level: bhtune_core::ResponseLevel::Moderate,
            kp: Some(1.5),
            ti_minutes: Some(0.7),
            td_minutes: Some(0.15),
            proportional: Some(12.0),
            integral: Some(2.5),
            derivative: Some(0.6),
            status: bhtune_core::TuningResultStatus::Valid,
            invalid_reason: None,
        },
    )
    .await
    .unwrap();

    let mut successful =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    successful.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 2.0,
        derivative: 0.5,
    });
    successful.proportional_written = Some(12.0);
    successful.integral_written = Some(2.5);
    successful.derivative_written = Some(0.6);
    successful.proportional_readback = Some(12.0);
    successful.integral_readback = Some(2.5);
    successful.derivative_readback = Some(0.6);
    successful.success = true;
    TuneWriteRow::insert(&pool, run.id, successful)
        .await
        .unwrap();

    let mut failed =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    failed.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 2.0,
        derivative: 0.5,
    });
    failed.proportional_written = Some(12.0);
    failed.error_message = Some("mock failure".to_string());
    failed.rollback_state = Some(bhtune_db::models::RollbackState::Failed);
    failed.rollback_error = Some("mock rollback failure".to_string());
    TuneWriteRow::insert(&pool, run.id, failed).await.unwrap();

    let confirmed_actuation = TuneMvActuationRow::insert_pending(
        &pool,
        run.id,
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
        &pool,
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
        &pool,
        run.id,
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
        &pool,
        superseded_actuation.id,
        MvActuationStatus::Superseded,
        Some("restore took over confirmation"),
    )
    .await
    .unwrap();

    (pool, run.id)
}

#[tokio::test]
async fn show_prints_calculated_results_and_write_back_audit_rows() {
    let (pool, run_id) = run_with_results_and_writes().await;
    show(&pool, run_id, OutputFormat::Table).await.unwrap();
}

#[tokio::test]
async fn show_prints_timing_metrics_when_present() {
    let (pool, run_id) = run_with_results_and_writes().await;
    TuneRunRow::record_timing_metrics(
        &pool,
        run_id,
        bhtune_db::models::TimingMetrics {
            basis: bhtune_db::models::TimingBasis::LiveMonotonic,
            requested_interval_ms: 800,
            sample_gap_count: 2,
            mean_sample_gap_ms: Some(900.0),
            max_sample_gap_ms: Some(1_200.0),
            missed_poll_opportunity_count: 1,
            measured_oscillation_period_ms: Some(4_800.0),
            approximate_samples_per_period: Some(5.33),
            sampling_adequacy: bhtune_db::models::SamplingAdequacy::Marginal,
            poll_latency: Some(bhtune_db::models::PollLatencyMetrics {
                pv_read: bhtune_db::models::TimingSummary {
                    count: 2,
                    mean_ms: Some(11.0),
                    max_ms: Some(15.0),
                },
                mv_write: bhtune_db::models::TimingSummary {
                    count: 1,
                    mean_ms: Some(20.0),
                    max_ms: Some(20.0),
                },
                mv_verification: bhtune_db::models::TimingSummary {
                    count: 1,
                    mean_ms: Some(30.0),
                    max_ms: Some(30.0),
                },
                sample_persist: bhtune_db::models::TimingSummary {
                    count: 2,
                    mean_ms: Some(4.0),
                    max_ms: Some(5.0),
                },
                tick_work: bhtune_db::models::TimingSummary {
                    count: 2,
                    mean_ms: Some(70.0),
                    max_ms: Some(75.0),
                },
            }),
        },
    )
    .await
    .unwrap();
    show(&pool, run_id, OutputFormat::Table).await.unwrap();
}

#[tokio::test]
async fn list_output_json_is_valid_json_with_the_expected_shape() {
    let (pool, _run_id) = run_with_results_and_writes().await;
    // Can't easily capture stdout here, so this test's main job is proving the JSON
    // path doesn't panic/error across every branch -- the DTOs' field shapes are a
    // straightforward 1:1 projection of already-tested `bhtune-db` row structs.
    list(&pool, None, 50, 0, OutputFormat::Json).await.unwrap();
    list(
        &pool,
        Some(bhtune_db::models::TuneOutcome::Completed),
        50,
        0,
        OutputFormat::Json,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn show_output_json_covers_readings_results_and_writes() {
    let (pool, run_id) = run_with_results_and_writes().await;
    show(&pool, run_id, OutputFormat::Json).await.unwrap();
}

#[tokio::test]
async fn show_prints_a_stored_gateway_compatibility_snapshot() {
    let (pool, run_id) = run_with_results_and_writes().await;
    let snapshot = serde_json::json!({
        "client_version": "0.1.0",
        "gateway_version": "0.5.9",
        "source": "gateway_info",
        "status": "partial",
        "features": []
    });
    TuneRunRow::record_gateway_compatibility(&pool, run_id, &snapshot.to_string())
        .await
        .unwrap();

    show(&pool, run_id, OutputFormat::Table).await.unwrap();
    show(&pool, run_id, OutputFormat::Json).await.unwrap();

    let stored = TuneRunRow::get(&pool, run_id).await.unwrap().unwrap();
    let parsed = crate::gateway::parse_stored_compatibility(
        stored.gateway_compatibility_json.as_deref().unwrap(),
    )
    .expect("stored snapshot should parse");
    assert_eq!(
        parsed.status,
        bhtune_driver::OpcDaCompatibilityStatus::Partial
    );
}

#[tokio::test]
async fn run_dispatches_list_and_show_in_both_output_formats() {
    let (pool, run_id) = run_with_results_and_writes().await;
    let config = crate::config::BhtuneConfig::default();
    run(
        &pool,
        HistoryCommand::List {
            outcome: None,
            limit: 50,
            offset: 0,
            output: OutputFormat::Table,
        },
        &config,
    )
    .await
    .unwrap();
    run(
        &pool,
        HistoryCommand::List {
            outcome: None,
            limit: 50,
            offset: 0,
            output: OutputFormat::Json,
        },
        &config,
    )
    .await
    .unwrap();
    run(
        &pool,
        HistoryCommand::Show {
            run_id,
            output: OutputFormat::Table,
        },
        &config,
    )
    .await
    .unwrap();
    run(
        &pool,
        HistoryCommand::Show {
            run_id,
            output: OutputFormat::Json,
        },
        &config,
    )
    .await
    .unwrap();
    run(
        &pool,
        HistoryCommand::Prune {
            older_than_days: Some(1),
            dry_run: true,
            output: OutputFormat::Table,
        },
        &crate::config::BhtuneConfig {
            retention_days: Some(1),
            ..crate::config::BhtuneConfig::default()
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn run_dispatches_revert_and_surfaces_its_error() {
    // No live gateway is running, so `revert` fails while connecting -- but this still
    // proves `run` actually dispatches `HistoryCommand::Revert` to the `revert` function
    // rather than, say, silently no-op'ing. `revert`'s own success/failure/validation
    // behavior is covered directly by the `revert_*` tests below. Flags are omitted
    // (`None`/`None`) so the connection resolves from the run's own recorded values
    // (`run_with_results_and_writes` now records one) rather than being rejected earlier
    // by `resolve_revert_connection`'s contradiction check.
    let (pool, run_id) = run_with_results_and_writes().await;
    let config = crate::config::BhtuneConfig::default();
    let err = run(
        &pool,
        HistoryCommand::Revert {
            run_id,
            bridge_host: None,
            server: None,
            yes: true,
            output: OutputFormat::Table,
        },
        &config,
    )
    .await
    .unwrap_err();
    assert!(!err.to_string().is_empty());
}

struct FileBackedTestPool {
    pool: SqlitePool,
    _directory: tempfile::TempDir,
}

impl std::ops::Deref for FileBackedTestPool {
    type Target = SqlitePool;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

/// Starts an `Opcda`-driver run (using the sample template/tags, which have PID
/// constant tags configured) with `record_connection` already called for it, so
/// `revert`'s own connection-resolution logic has a stored value to resolve against.
/// Returns it without recording any write-back yet -- each
/// `revert_*` test below inserts whatever `TuneWriteRow` fixture its scenario needs.
async fn opcda_run_with_no_writes(bridge_host: &str, server: &str) -> (FileBackedTestPool, i64) {
    let directory = tempfile::tempdir().unwrap();
    let pool = bhtune_db::connect(&directory.path().join("revert-test.db"))
        .await
        .unwrap();
    let now = chrono::Utc::now();
    let run = TuneRunRow::start(
        &pool,
        None,
        "Unit1.LIC101.PV",
        TuneDriver::Opcda,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();
    TuneRunRow::record_connection(&pool, run.id, Some(server), Some(bridge_host), "{}")
        .await
        .unwrap();
    (
        FileBackedTestPool {
            pool,
            _directory: directory,
        },
        run.id,
    )
}

#[tokio::test]
async fn revert_errors_when_no_such_run_exists() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let err = revert(
        &pool,
        999,
        None,
        None,
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no run with id 999"));
}

#[tokio::test]
async fn revert_errors_when_the_run_did_not_use_the_opcda_driver() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let now = chrono::Utc::now();
    let run = TuneRunRow::start(
        &pool,
        None,
        "Unit1.LIC101.PV",
        TuneDriver::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();
    let err = revert(
        &pool,
        run.id,
        None,
        None,
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Simulator"));
}

#[tokio::test]
async fn revert_errors_when_no_write_back_is_recorded() {
    let (pool, run_id) = opcda_run_with_no_writes("bridge:1", "Sim.Server").await;
    let err = revert(
        &pool,
        run_id,
        None,
        None,
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no recorded PID write-back"));
}

#[tokio::test]
async fn revert_errors_when_the_original_writes_pre_read_failed() {
    let (pool, run_id) = opcda_run_with_no_writes("bridge:1", "Sim.Server").await;
    let now = chrono::Utc::now();
    let mut failed_pre_read =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    failed_pre_read.error_message = Some("pre-read of Proportional tag 'X' failed".to_string());
    TuneWriteRow::insert(&pool, run_id, failed_pre_read)
        .await
        .unwrap();

    let err = revert(
        &pool,
        run_id,
        None,
        None,
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("nothing to revert to"));
}

#[tokio::test]
async fn revert_errors_when_yes_is_not_set() {
    let (pool, run_id) = opcda_run_with_no_writes("bridge:1", "Sim.Server").await;
    let now = chrono::Utc::now();
    let mut successful =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    successful.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 2.0,
        derivative: 0.5,
    });
    successful.success = true;
    TuneWriteRow::insert(&pool, run_id, successful)
        .await
        .unwrap();

    let err = revert(
        &pool,
        run_id,
        None,
        None,
        false,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("--yes"));
}

#[tokio::test]
async fn revert_errors_when_the_snapshotted_tags_have_no_pid_constant_tags() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let now = chrono::Utc::now();
    let mut tags = sample_tags();
    tags.proportional_constant = None;
    let run = TuneRunRow::start(
        &pool,
        None,
        "Unit1.LIC101.PV",
        TuneDriver::Opcda,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &tags,
        now,
    )
    .await
    .unwrap();
    TuneRunRow::record_connection(&pool, run.id, Some("Sim.Server"), Some("bridge:1"), "{}")
        .await
        .unwrap();
    let mut successful =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    successful.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 2.0,
        derivative: 0.5,
    });
    successful.success = true;
    TuneWriteRow::insert(&pool, run.id, successful)
        .await
        .unwrap();

    let err = revert(
        &pool,
        run.id,
        None,
        None,
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no PID constant tags"));
}

#[tokio::test]
async fn revert_errors_when_the_driver_connection_fails() {
    // Port 1 is a privileged/unlikely-bound port; connecting should fail promptly,
    // proving every validation step above passed and `revert` genuinely reached the
    // connect step, resolving the connection from the run's own recorded values (no
    // explicit `--bridge-host`/`--server` flags passed at all here).
    let (pool, run_id) = opcda_run_with_no_writes("127.0.0.1:1", "Sim.Server").await;
    let now = chrono::Utc::now();
    let mut successful =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    successful.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 2.0,
        derivative: 0.5,
    });
    successful.success = true;
    TuneWriteRow::insert(&pool, run_id, successful)
        .await
        .unwrap();

    let err = revert(
        &pool,
        run_id,
        None,
        None,
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn revert_errors_when_the_run_has_no_recorded_connection() {
    // A run created directly via `TuneRunRow::start` without ever calling
    // `record_connection` -- shouldn't happen for a real run created through
    // `prepare`/the HTTP API, but `revert` must still refuse loudly rather than guess
    // which OPC server/gateway to target.
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let now = chrono::Utc::now();
    let run = TuneRunRow::start(
        &pool,
        None,
        "Unit1.LIC101.PV",
        TuneDriver::Opcda,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();
    let mut successful =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    successful.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 2.0,
        derivative: 0.5,
    });
    successful.success = true;
    TuneWriteRow::insert(&pool, run.id, successful)
        .await
        .unwrap();

    let err = revert(
        &pool,
        run.id,
        None,
        None,
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("recorded OPC server is missing"));
}

#[tokio::test]
async fn resolve_revert_connection_rejects_a_missing_recorded_bridge_host() {
    let (pool, run_id) = opcda_run_with_no_writes("bridge:1", "Sim.Server").await;
    let mut run = TuneRunRow::get(&pool, run_id).await.unwrap().unwrap();
    run.bridge_host = None;

    let err = resolve_revert_connection(&run, None, None).unwrap_err();
    assert!(err.to_string().contains("recorded bridge host is missing"));
}

#[tokio::test]
async fn revert_errors_when_an_explicit_server_flag_contradicts_the_recorded_one() {
    let (pool, run_id) = opcda_run_with_no_writes("bridge:1", "Sim.Server").await;
    let now = chrono::Utc::now();
    let mut successful =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    successful.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 2.0,
        derivative: 0.5,
    });
    successful.success = true;
    TuneWriteRow::insert(&pool, run_id, successful)
        .await
        .unwrap();

    let err = revert(
        &pool,
        run_id,
        None,
        Some("Different.Server".to_string()),
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("contradicts"));
    assert!(message.contains("Sim.Server"));
}

#[tokio::test]
async fn revert_errors_when_an_explicit_bridge_host_flag_contradicts_the_recorded_one() {
    let (pool, run_id) = opcda_run_with_no_writes("bridge:1", "Sim.Server").await;
    let now = chrono::Utc::now();
    let mut successful =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    successful.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 2.0,
        derivative: 0.5,
    });
    successful.success = true;
    TuneWriteRow::insert(&pool, run_id, successful)
        .await
        .unwrap();

    let err = revert(
        &pool,
        run_id,
        Some("different-bridge:2".to_string()),
        None,
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("contradicts"));
    assert!(message.contains("bridge:1"));
}

#[tokio::test]
async fn revert_succeeds_and_records_a_revert_kind_write() {
    use bhtune_test_support::{MockBridgeService, start_mock_server};
    use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue, WriteResponse};

    // Every read (both the live pre-read and every write's confirmation readback)
    // returns this same fixed "10.0"/Good response regardless of which tag was
    // requested -- so the fixture's recorded `previous` values are all set to 10.0 too,
    // ensuring `write_and_verify_pid_value`'s tolerance check always sees a matching
    // readback no matter which constant is being reverted.
    let (host, server) = start_mock_server(MockBridgeService {
        read_response: ReadResponse {
            values: vec![ProtoTagValue {
                tag_id: "ignored".to_string(),
                value: "10.0".to_string(),
                quality: "Good".to_string(),
                timestamp: "2024-01-15 10:23:45".to_string(),
            }],
        },
        write_response: WriteResponse {
            tag_id: "ignored".to_string(),
            success: true,
            error: None,
        },
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");

    let (pool, run_id) = opcda_run_with_no_writes(&host, "Sim.Server").await;
    let now = chrono::Utc::now();
    let mut successful =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    successful.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 10.0,
        derivative: 10.0,
    });
    successful.success = true;
    TuneWriteRow::insert(&pool, run_id, successful)
        .await
        .unwrap();

    // Explicit `--bridge-host`/`--server` flags are passed here too (matching the
    // recorded connection exactly), deliberately exercising the cross-check-passes path
    // rather than the "omit both, use the recorded value" path already covered by
    // `revert_errors_when_the_driver_connection_fails` above.
    revert(
        &pool,
        run_id,
        Some(host),
        Some("Sim.Server".to_string()),
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap();

    let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
    let revert_row = writes.iter().find(|w| w.kind == WriteKind::Revert).unwrap();
    assert!(revert_row.success);
    assert_eq!(revert_row.proportional_written, Some(10.0));
    assert_eq!(revert_row.integral_written, Some(10.0));
    assert_eq!(revert_row.derivative_written, Some(10.0));
    assert_eq!(revert_row.proportional_readback, Some(10.0));
    assert_eq!(revert_row.integral_readback, Some(10.0));
    assert_eq!(revert_row.derivative_readback, Some(10.0));
    // Reverts never chain a nested rollback-of-a-revert.
    assert_eq!(revert_row.rollback_state, None);
    // The pre-read of the *live* current value (also fed by the fixed mock response)
    // becomes this revert row's own `previous`, so a second revert could undo it too.
    assert_eq!(
        revert_row.previous,
        Some(bhtune_db::models::WriteReadback {
            proportional: 10.0,
            integral: 10.0,
            derivative: 10.0,
        })
    );

    server.shutdown().await;
}

#[tokio::test]
async fn revert_refuses_an_incompatible_gateway_before_writing() {
    use bhtune_test_support::{MockBridgeService, start_mock_server};
    use opcda_bridge_proto::bridge::{
        GetGatewayInfoResponse, ProtocolFeature, ProtocolFeatureKind,
    };

    let (host, server) = start_mock_server(MockBridgeService {
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
    .await
    .expect("bind mock bridge");

    let (pool, run_id) = opcda_run_with_no_writes(&host, "Sim.Server").await;
    let now = chrono::Utc::now();
    let mut successful =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    successful.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 10.0,
        derivative: 10.0,
    });
    successful.success = true;
    TuneWriteRow::insert(&pool, run_id, successful)
        .await
        .unwrap();

    let error = revert(
        &pool,
        run_id,
        None,
        None,
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .expect_err("an incompatible core must refuse revert");
    assert!(error.to_string().contains("incompatible"));
    let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
    assert!(writes.iter().all(|write| write.kind != WriteKind::Revert));
    server.shutdown().await;
}

#[tokio::test]
async fn revert_records_a_failed_audit_row_when_a_later_constant_fails_verification() {
    use bhtune_test_support::{MockBridgeService, start_mock_server};
    use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue, WriteResponse};

    // Calls 1-3: the live pre-read of P/I/D (all succeed). Call 4: Proportional's
    // post-write verification readback (succeeds). Call 5: Integral's post-write
    // verification readback -- fails, per `failing_read_from_call(5)`. Derivative is
    // never attempted, since the loop breaks on the first failure.
    let (host, server) = start_mock_server(
        MockBridgeService {
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "ignored".to_string(),
                    value: "10.0".to_string(),
                    quality: "Good".to_string(),
                    timestamp: "2024-01-15 10:23:45".to_string(),
                }],
            },
            write_response: WriteResponse {
                tag_id: "ignored".to_string(),
                success: true,
                error: None,
            },
            ..Default::default()
        }
        .failing_read_from_call(5),
    )
    .await
    .expect("bind mock bridge");

    let (pool, run_id) = opcda_run_with_no_writes(&host, "Sim.Server").await;
    let now = chrono::Utc::now();
    let mut successful =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    successful.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 10.0,
        derivative: 10.0,
    });
    successful.success = true;
    TuneWriteRow::insert(&pool, run_id, successful)
        .await
        .unwrap();

    let err = revert(
        &pool,
        run_id,
        None,
        None,
        true,
        OutputFormat::Table,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("revert failed partway through"));

    let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
    let revert_row = writes.iter().find(|w| w.kind == WriteKind::Revert).unwrap();
    assert!(!revert_row.success);
    assert!(
        revert_row
            .error_message
            .as_ref()
            .unwrap()
            .contains("Integral")
    );
    assert_eq!(revert_row.proportional_written, Some(10.0));
    assert_eq!(revert_row.proportional_readback, Some(10.0));
    assert_eq!(revert_row.integral_written, Some(10.0));
    assert_eq!(revert_row.integral_readback, None);
    assert_eq!(revert_row.derivative_written, None);
    assert_eq!(revert_row.derivative_readback, None);
    assert_eq!(revert_row.rollback_state, None);

    server.shutdown().await;
}

#[tokio::test]
async fn revert_succeeds_with_json_output_format() {
    use bhtune_test_support::{MockBridgeService, start_mock_server};
    use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue, WriteResponse};

    // This isn't a substitute for a real stdout-capture test proving JSON mode never
    // interleaves prose ahead of the final object (that end-to-end contract belongs to
    // subprocess contract test in `tests/json_output_contract.rs`, across every
    // subcommand at once) -- it
    // only proves `revert`'s `OutputFormat::Json` branch itself runs to completion
    // without erroring, i.e. that constructing and serializing `RevertJson` from a real
    // successful revert actually works, which the Table-mode test above never exercises.
    let (host, server) = start_mock_server(MockBridgeService {
        read_response: ReadResponse {
            values: vec![ProtoTagValue {
                tag_id: "ignored".to_string(),
                value: "10.0".to_string(),
                quality: "Good".to_string(),
                timestamp: "2024-01-15 10:23:45".to_string(),
            }],
        },
        write_response: WriteResponse {
            tag_id: "ignored".to_string(),
            success: true,
            error: None,
        },
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");

    let (pool, run_id) = opcda_run_with_no_writes(&host, "Sim.Server").await;
    let now = chrono::Utc::now();
    let mut successful =
        bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
    successful.previous = Some(bhtune_db::models::WriteReadback {
        proportional: 10.0,
        integral: 10.0,
        derivative: 10.0,
    });
    successful.success = true;
    TuneWriteRow::insert(&pool, run_id, successful)
        .await
        .unwrap();

    revert(
        &pool,
        run_id,
        None,
        None,
        true,
        OutputFormat::Json,
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap();

    server.shutdown().await;
}

/// Starts a run with `started_at` set explicitly (unlike every other fixture in this
/// file, which always uses `chrono::Utc::now()`) so `prune`'s tests can put a run
/// unambiguously on either side of a retention cutoff.
async fn start_run_at(pool: &SqlitePool, started_at: chrono::DateTime<chrono::Utc>) -> i64 {
    TuneRunRow::start(
        pool,
        None,
        "Unit1.LIC101.PV",
        TuneDriver::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        started_at,
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn prune_errors_when_no_retention_policy_is_configured() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let config = crate::config::BhtuneConfig::default();
    let err = prune(&pool, &config, None, false, OutputFormat::Table)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no retention policy configured"));
}

#[tokio::test]
async fn prune_dry_run_reports_the_count_without_deleting_anything() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let now = chrono::Utc::now();
    let old_id = start_run_at(&pool, now - chrono::Duration::days(45)).await;
    let recent_id = start_run_at(&pool, now - chrono::Duration::days(1)).await;
    let config = crate::config::BhtuneConfig {
        retention_days: Some(30),
        ..Default::default()
    };

    prune(&pool, &config, None, true, OutputFormat::Table)
        .await
        .unwrap();
    prune(&pool, &config, None, true, OutputFormat::Json)
        .await
        .unwrap();

    // Nothing was actually deleted by either dry-run call.
    assert!(TuneRunRow::get(&pool, old_id).await.unwrap().is_some());
    assert!(TuneRunRow::get(&pool, recent_id).await.unwrap().is_some());
}

#[tokio::test]
async fn prune_deletes_matching_runs_when_not_a_dry_run() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let now = chrono::Utc::now();
    let old_id = start_run_at(&pool, now - chrono::Duration::days(45)).await;
    let recent_id = start_run_at(&pool, now - chrono::Duration::days(1)).await;
    let config = crate::config::BhtuneConfig {
        retention_days: Some(30),
        ..Default::default()
    };

    prune(&pool, &config, None, false, OutputFormat::Table)
        .await
        .unwrap();

    assert!(TuneRunRow::get(&pool, old_id).await.unwrap().is_none());
    assert!(TuneRunRow::get(&pool, recent_id).await.unwrap().is_some());
}

#[tokio::test]
async fn prune_older_than_days_overrides_the_configured_policy() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let now = chrono::Utc::now();
    // 45 days old: survives the configured 90-day policy, but not an ad-hoc
    // `--older-than-days 30` override.
    let run_id = start_run_at(&pool, now - chrono::Duration::days(45)).await;
    let config = crate::config::BhtuneConfig {
        retention_days: Some(90),
        ..Default::default()
    };

    prune(&pool, &config, Some(30), false, OutputFormat::Table)
        .await
        .unwrap();

    assert!(TuneRunRow::get(&pool, run_id).await.unwrap().is_none());
}

#[tokio::test]
async fn prune_json_output_is_a_single_parseable_object() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let config = crate::config::BhtuneConfig {
        retention_days: Some(30),
        ..Default::default()
    };
    // Only proves the `Json` branch runs to completion and produces a well-formed
    // `PruneJson` in both the dry-run and real-deletion cases -- the full
    // one-JSON-value-on-stdout contract across every subcommand belongs to
    // dedicated subprocess contract test in `tests/json_output_contract.rs`, not to this
    // unit test.
    prune(&pool, &config, None, true, OutputFormat::Json)
        .await
        .unwrap();
    prune(&pool, &config, None, false, OutputFormat::Json)
        .await
        .unwrap();
}
