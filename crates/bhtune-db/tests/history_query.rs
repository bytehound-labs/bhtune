//! Integration tests for `history-query-api`: the typed repository/query layer models.rs
//! builds on top of the raw schema `tests/schema.rs` already proves. These tests cover run
//! lifecycle transitions, filter/pagination correctness, and the samples/results/writes
//! `list_for_run` helpers — the actual query patterns the CLI and GUI history explorer will
//! depend on.

use bhtune_core::{
    ControllerDirection, ControllerType, DcsTemplate, LoopConfig, LoopTags, MrftState, ProcessType,
    ResponseLevel, Tick, built_in_templates,
    tuning_math::{PidParameters, TuningResult},
};
use bhtune_db::{
    connect_in_memory,
    models::{
        DcsTemplateRow, NewTuneWrite, Pagination, RollbackState, SampleQuality, TemplateOrigin,
        TuneBackend, TuneOutcome, TuneResultRow, TuneRunFilter, TuneRunInitialReadings, TuneRunRow,
        TuneSampleRow, TuneWriteRow, WriteReadback,
    },
};
use chrono::{DateTime, Duration, Utc};
use sqlx::Row;

// Fixture helpers {{{1

/// Inserts one built-in template and returns its id. Mirrors `tests/schema.rs`'s helper of
/// the same name; duplicated rather than shared, matching that file's existing lack of a
/// `tests/common` module.
async fn seed_template(pool: &sqlx::SqlitePool) -> i64 {
    let template = built_in_templates().remove(0);
    DcsTemplateRow::insert(pool, &template, TemplateOrigin::Builtin, Utc::now())
        .await
        .unwrap()
        .id
}

/// Inserts a loop named `name`, owned by `template_id`, and returns its id. Takes an
/// already-seeded `template_id` (rather than seeding its own, like `tests/schema.rs`'s
/// `seed_loop`) so a single test can create more than one loop without colliding on
/// `dcs_templates.name`'s uniqueness constraint.
async fn seed_loop_named(pool: &sqlx::SqlitePool, template_id: i64, name: &str) -> i64 {
    let now = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO loops (
            name, dcs_template_id, tags_json, process_type, controller_type,
            relay_amp_percent, num_cycles_skip, num_cycles_count, noise_protection_secs,
            mrft_delay_secs, created_at, updated_at
        ) VALUES (?, ?, '{}', 'flow', 'pi', 5.0, 1, 2, 3, 0, ?, ?)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(template_id)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await
    .unwrap()
    .try_get("id")
    .unwrap()
}

fn sample_config() -> LoopConfig {
    LoopConfig {
        process_type: ProcessType::Flow,
        controller_type: ControllerType::Pi,
        relay_amp_percent: 5.0,
        num_cycles_skip: 1,
        num_cycles_count: 2,
        noise_protection_secs: 3,
        mrft_delay_secs: 0,
    }
}

/// The template every [`seed_run`]/direct `TuneRunRow::start` call in this file snapshots --
/// none of these tests care *which* template was used, only that the run/filter/pagination
/// machinery around it behaves correctly, so one fixed built-in is enough.
fn sample_template() -> DcsTemplate {
    built_in_templates().remove(0)
}

fn sample_tags() -> LoopTags {
    LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &sample_template())
}

fn sample_initial_readings() -> TuneRunInitialReadings {
    TuneRunInitialReadings {
        pv_ini: 50.0,
        mv_ini: 45.0,
        mv_range_low: 0.0,
        mv_range_high: 100.0,
        pv_range_high: 100.0,
        pv_range_low: 0.0,
        controller_direction: ControllerDirection::Direct,
        mode_raw: Some("1".to_string()),
        mode_attribute_raw: None,
        setpoint_ini: Some(50.0),
    }
}

/// Starts a run and immediately drives it to `outcome`, for filter/pagination tests that
/// only care about the resulting row shape, not the transition itself (covered separately by
/// the lifecycle tests below).
async fn seed_run(
    pool: &sqlx::SqlitePool,
    loop_id: Option<i64>,
    backend: TuneBackend,
    config: LoopConfig,
    outcome: TuneOutcome,
    started_at: DateTime<Utc>,
) -> TuneRunRow {
    let started = TuneRunRow::start(
        pool,
        loop_id,
        "LIC-X",
        backend,
        config,
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        started_at,
    )
    .await
    .unwrap();
    match outcome {
        TuneOutcome::Running => started,
        TuneOutcome::Completed => TuneRunRow::complete(pool, started.id, started_at)
            .await
            .unwrap(),
        TuneOutcome::Failed => TuneRunRow::fail(pool, started.id, started_at, "seeded failure")
            .await
            .unwrap(),
        TuneOutcome::Aborted => TuneRunRow::abort(pool, started.id, started_at)
            .await
            .unwrap(),
    }
}
// }}}1

// Lifecycle transitions {{{1

#[tokio::test]
async fn run_lifecycle_start_then_record_initial_readings_then_complete() {
    let pool = connect_in_memory().await.unwrap();
    let template_id = seed_template(&pool).await;
    let loop_id = seed_loop_named(&pool, template_id, "LIC101").await;
    let now = Utc::now();

    let started = TuneRunRow::start(
        &pool,
        Some(loop_id),
        "LIC101",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();
    assert!(started.id > 0);
    assert_eq!(started.outcome, TuneOutcome::Running);
    assert_eq!(started.loop_id, Some(loop_id));
    assert_eq!(started.loop_name, "LIC101");
    assert_eq!(started.config, sample_config());
    assert_eq!(started.template_origin, TemplateOrigin::Builtin);
    assert_eq!(started.template, sample_template());
    assert_eq!(started.tags, sample_tags());
    assert!(started.initial_readings.is_none());
    assert!(started.completed_at.is_none());
    assert_eq!(started.started_at, now);
    assert_eq!(started.created_at, now);

    let readings = sample_initial_readings();
    let with_readings = TuneRunRow::record_initial_readings(&pool, started.id, readings.clone())
        .await
        .unwrap();
    assert_eq!(with_readings.initial_readings, Some(readings.clone()));
    assert_eq!(
        with_readings.outcome,
        TuneOutcome::Running,
        "recording readings must not itself change the outcome"
    );

    let completed_at = now + Duration::minutes(5);
    let completed = TuneRunRow::complete(&pool, started.id, completed_at)
        .await
        .unwrap();
    assert_eq!(completed.outcome, TuneOutcome::Completed);
    assert_eq!(completed.completed_at, Some(completed_at));
    assert_eq!(
        completed.initial_readings,
        Some(readings),
        "completing a run must not clear readings recorded earlier"
    );

    let fetched = TuneRunRow::get(&pool, started.id).await.unwrap().unwrap();
    assert_eq!(fetched, completed);
}

#[tokio::test]
async fn run_can_fail_before_initial_readings_are_ever_recorded() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();

    let started = TuneRunRow::start(
        &pool,
        None,
        "LIC102",
        TuneBackend::Opcda,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();
    assert_eq!(
        started.loop_id, None,
        "an ad-hoc run with no saved loop stays loop_id = NULL"
    );

    let failed = TuneRunRow::fail(
        &pool,
        started.id,
        now + Duration::seconds(2),
        "InvalidCastException reading initial values",
    )
    .await
    .unwrap();
    assert_eq!(failed.outcome, TuneOutcome::Failed);
    assert_eq!(
        failed.failure_reason.as_deref(),
        Some("InvalidCastException reading initial values")
    );
    assert!(failed.initial_readings.is_none());
}

#[tokio::test]
async fn run_can_fail_after_initial_readings_were_recorded() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    let started = TuneRunRow::start(
        &pool,
        None,
        "LIC103",
        TuneBackend::Opcda,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();
    TuneRunRow::record_initial_readings(&pool, started.id, sample_initial_readings())
        .await
        .unwrap();

    let failed = TuneRunRow::fail(&pool, started.id, now, "backend disconnected mid-test")
        .await
        .unwrap();
    assert_eq!(failed.outcome, TuneOutcome::Failed);
    assert!(
        failed.initial_readings.is_some(),
        "readings recorded before the failure must survive it"
    );
}

#[tokio::test]
async fn run_can_be_aborted() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    let started = TuneRunRow::start(
        &pool,
        None,
        "LIC104",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();

    let aborted = TuneRunRow::abort(&pool, started.id, now + Duration::seconds(30))
        .await
        .unwrap();
    assert_eq!(aborted.outcome, TuneOutcome::Aborted);
    assert!(aborted.failure_reason.is_none());
}

#[tokio::test]
async fn get_returns_none_for_missing_id() {
    let pool = connect_in_memory().await.unwrap();
    assert!(TuneRunRow::get(&pool, 999).await.unwrap().is_none());
}

#[tokio::test]
async fn lifecycle_transition_on_missing_run_id_is_an_error_not_a_silent_noop() {
    let pool = connect_in_memory().await.unwrap();
    let result = TuneRunRow::complete(&pool, 999, Utc::now()).await;
    assert!(
        result.is_err(),
        "completing a run that doesn't exist must error, not silently succeed with nothing updated"
    );
}
// }}}1

// Filtering {{{1

#[tokio::test]
async fn list_filters_by_process_type_controller_type_outcome_and_backend() {
    let pool = connect_in_memory().await.unwrap();
    let t0 = Utc::now();

    let flow_pi_completed_opcda = seed_run(
        &pool,
        None,
        TuneBackend::Opcda,
        LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            ..sample_config()
        },
        TuneOutcome::Completed,
        t0,
    )
    .await;
    let level_p_failed_simulator = seed_run(
        &pool,
        None,
        TuneBackend::Simulator,
        LoopConfig {
            process_type: ProcessType::Level,
            controller_type: ControllerType::P,
            ..sample_config()
        },
        TuneOutcome::Failed,
        t0 + Duration::seconds(1),
    )
    .await;
    let flow_pid_running_replay = seed_run(
        &pool,
        None,
        TuneBackend::Replay,
        LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pid,
            ..sample_config()
        },
        TuneOutcome::Running,
        t0 + Duration::seconds(2),
    )
    .await;

    let flow_only = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default().with_process_type(ProcessType::Flow),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        flow_only.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![flow_pid_running_replay.id, flow_pi_completed_opcda.id],
        "newest-started first"
    );

    let pi_only = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default().with_controller_type(ControllerType::Pi),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(pi_only.len(), 1);
    assert_eq!(pi_only[0].id, flow_pi_completed_opcda.id);

    let completed_only = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default().with_outcome(TuneOutcome::Completed),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(completed_only.len(), 1);
    assert_eq!(completed_only[0].id, flow_pi_completed_opcda.id);

    let simulator_only = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default().with_backend(TuneBackend::Simulator),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(simulator_only.len(), 1);
    assert_eq!(simulator_only[0].id, level_p_failed_simulator.id);

    let combined = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default()
            .with_process_type(ProcessType::Flow)
            .with_outcome(TuneOutcome::Running),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0].id, flow_pid_running_replay.id);
}

/// Mirrors the test above but for the two filter fields findings 9 (`safety-run-snapshot`)
/// added -- seeded with `TuneRunRow::start` directly rather than [`seed_run`], since this is
/// the one test in the file that actually needs to vary the snapshotted template/origin
/// per-run rather than using the fixed [`sample_template`]/[`sample_tags`] pair.
#[tokio::test]
async fn list_filters_by_template_name_and_template_origin() {
    let pool = connect_in_memory().await.unwrap();
    let t0 = Utc::now();

    let yokogawa = built_in_templates().remove(0);
    let honeywell = built_in_templates()
        .into_iter()
        .find(|t| t.name == "Honeywell Experion")
        .expect("Honeywell Experion is a built-in template");

    let yokogawa_builtin = TuneRunRow::start(
        &pool,
        None,
        "LIC201",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &yokogawa,
        &LoopTags::derive_from_pv_tag("Unit1.LIC201.PV", &yokogawa),
        t0,
    )
    .await
    .unwrap();
    let honeywell_builtin = TuneRunRow::start(
        &pool,
        None,
        "LIC202",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &honeywell,
        &LoopTags::derive_from_pv_tag("Unit1.LIC202.PV", &honeywell),
        t0 + Duration::seconds(1),
    )
    .await
    .unwrap();
    let yokogawa_user = TuneRunRow::start(
        &pool,
        None,
        "LIC203",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::User,
        &yokogawa,
        &LoopTags::derive_from_pv_tag("Unit1.LIC203.PV", &yokogawa),
        t0 + Duration::seconds(2),
    )
    .await
    .unwrap();

    let yokogawa_only = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default().with_template_name(yokogawa.name.as_str()),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        yokogawa_only.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![yokogawa_user.id, yokogawa_builtin.id],
        "newest-started first"
    );

    let builtin_only = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default().with_template_origin(TemplateOrigin::Builtin),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        builtin_only.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![honeywell_builtin.id, yokogawa_builtin.id],
        "newest-started first"
    );

    let combined = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default()
            .with_template_name(yokogawa.name.as_str())
            .with_template_origin(TemplateOrigin::User),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0].id, yokogawa_user.id);
}

#[tokio::test]
async fn list_filters_by_loop_id() {
    let pool = connect_in_memory().await.unwrap();
    let template_id = seed_template(&pool).await;
    let loop_a = seed_loop_named(&pool, template_id, "LIC-A").await;
    let loop_b = seed_loop_named(&pool, template_id, "LIC-B").await;
    let t0 = Utc::now();

    let run_a = seed_run(
        &pool,
        Some(loop_a),
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        t0,
    )
    .await;
    seed_run(
        &pool,
        Some(loop_b),
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        t0 + Duration::seconds(1),
    )
    .await;
    seed_run(
        &pool,
        None,
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        t0 + Duration::seconds(2),
    )
    .await;

    let for_loop_a = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default().with_loop_id(loop_a),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(for_loop_a.len(), 1);
    assert_eq!(for_loop_a[0].id, run_a.id);
}

#[tokio::test]
async fn list_filters_by_started_at_range() {
    let pool = connect_in_memory().await.unwrap();
    let t0 = Utc::now();
    let early = seed_run(
        &pool,
        None,
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        t0 - Duration::days(2),
    )
    .await;
    let middle = seed_run(
        &pool,
        None,
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        t0 - Duration::days(1),
    )
    .await;
    let late = seed_run(
        &pool,
        None,
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        t0,
    )
    .await;

    let after_early = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default().with_started_after(t0 - Duration::hours(36)),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        after_early.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![late.id, middle.id]
    );

    let before_late = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default().with_started_before(t0 - Duration::hours(12)),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        before_late.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![middle.id, early.id]
    );

    let between = TuneRunRow::list(
        &pool,
        &TuneRunFilter::default()
            .with_started_after(t0 - Duration::hours(36))
            .with_started_before(t0 - Duration::hours(12)),
        Pagination::default(),
    )
    .await
    .unwrap();
    assert_eq!(between.len(), 1);
    assert_eq!(between[0].id, middle.id);
}

#[tokio::test]
async fn list_and_count_with_no_matches_returns_empty() {
    let pool = connect_in_memory().await.unwrap();
    seed_run(
        &pool,
        None,
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        Utc::now(),
    )
    .await;

    let filter = TuneRunFilter::default().with_outcome(TuneOutcome::Aborted);
    assert_eq!(TuneRunRow::count(&pool, &filter).await.unwrap(), 0);
    assert!(
        TuneRunRow::list(&pool, &filter, Pagination::default())
            .await
            .unwrap()
            .is_empty()
    );
}
// }}}1

// Pagination {{{1

#[tokio::test]
async fn list_paginates_and_count_matches_total_regardless_of_page_size() {
    let pool = connect_in_memory().await.unwrap();
    let t0 = Utc::now();
    let mut ids = Vec::new();
    for i in 0..5 {
        let run = seed_run(
            &pool,
            None,
            TuneBackend::Simulator,
            sample_config(),
            TuneOutcome::Completed,
            t0 + Duration::seconds(i),
        )
        .await;
        ids.push(run.id);
    }
    // `list` orders newest-started first, the reverse of insertion order here.
    ids.reverse();

    let total = TuneRunRow::count(&pool, &TuneRunFilter::default())
        .await
        .unwrap();
    assert_eq!(total, 5);

    let page1 = TuneRunRow::list(&pool, &TuneRunFilter::default(), Pagination::new(2, 0))
        .await
        .unwrap();
    assert_eq!(
        page1.iter().map(|r| r.id).collect::<Vec<_>>(),
        ids[0..2].to_vec()
    );

    let page2 = TuneRunRow::list(&pool, &TuneRunFilter::default(), Pagination::new(2, 2))
        .await
        .unwrap();
    assert_eq!(
        page2.iter().map(|r| r.id).collect::<Vec<_>>(),
        ids[2..4].to_vec()
    );

    let page3 = TuneRunRow::list(&pool, &TuneRunFilter::default(), Pagination::new(2, 4))
        .await
        .unwrap();
    assert_eq!(
        page3.iter().map(|r| r.id).collect::<Vec<_>>(),
        ids[4..5].to_vec()
    );

    let page4_empty = TuneRunRow::list(&pool, &TuneRunFilter::default(), Pagination::new(2, 6))
        .await
        .unwrap();
    assert!(page4_empty.is_empty());
}

#[tokio::test]
async fn pagination_default_is_50_rows_from_the_start() {
    let default = Pagination::default();
    assert_eq!(default.limit, 50);
    assert_eq!(default.offset, 0);
    assert_eq!(Pagination::first(10), Pagination::new(10, 0));
}
// }}}1

// tune_samples {{{1

#[tokio::test]
async fn tune_sample_insert_and_list_for_run_orders_by_tick() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC105",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();

    for tick in 0..3i64 {
        let sample = Tick {
            time: now + Duration::milliseconds(tick * 800),
            pv: 50.0 + tick as f32,
        };
        let state = MrftState {
            hysteresis: 1.0,
            mv_value_current: 55.0,
            mv_sign_next_step: 1,
            counter_all_switches: tick as u32,
            cycles_completed: 0,
            cycles_remaining: 2,
        };
        let inserted =
            TuneSampleRow::insert(&pool, run.id, tick, sample, state, SampleQuality::Good)
                .await
                .unwrap();
        assert_eq!(inserted.tick_index, tick);
        assert_eq!(inserted.sample, sample);
        assert_eq!(inserted.state, state);
        assert_eq!(inserted.run_id, run.id);
    }

    let samples = TuneSampleRow::list_for_run(&pool, run.id).await.unwrap();
    assert_eq!(
        samples.iter().map(|s| s.tick_index).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[tokio::test]
async fn tune_sample_list_for_run_is_empty_for_a_run_with_no_samples() {
    let pool = connect_in_memory().await.unwrap();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC106",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        Utc::now(),
    )
    .await
    .unwrap();
    assert!(
        TuneSampleRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn tune_sample_rejects_duplicate_tick_for_the_same_run() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC107",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();
    let sample = Tick {
        time: now,
        pv: 50.0,
    };
    let state = MrftState {
        hysteresis: 1.0,
        mv_value_current: 55.0,
        mv_sign_next_step: 1,
        counter_all_switches: 0,
        cycles_completed: 0,
        cycles_remaining: 2,
    };
    TuneSampleRow::insert(&pool, run.id, 0, sample, state, SampleQuality::Good)
        .await
        .unwrap();
    let dup = TuneSampleRow::insert(&pool, run.id, 0, sample, state, SampleQuality::Good).await;
    assert!(dup.is_err(), "duplicate (run_id, tick) must be rejected");
}

#[tokio::test]
async fn tune_sample_list_for_run_since_only_returns_ticks_after_the_given_one() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC108",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();

    for tick in 0..5i64 {
        let sample = Tick {
            time: now + Duration::milliseconds(tick * 800),
            pv: 50.0 + tick as f32,
        };
        let state = MrftState {
            hysteresis: 1.0,
            mv_value_current: 55.0,
            mv_sign_next_step: 1,
            counter_all_switches: tick as u32,
            cycles_completed: 0,
            cycles_remaining: 2,
        };
        TuneSampleRow::insert(&pool, run.id, tick, sample, state, SampleQuality::Good)
            .await
            .unwrap();
    }

    // `-1` (the "nothing sent yet" sentinel `GET /api/runs/{id}/stream`'s first poll uses)
    // must behave exactly like `list_for_run`: every sample, in order.
    let all = TuneSampleRow::list_for_run_since(&pool, run.id, -1)
        .await
        .unwrap();
    assert_eq!(
        all.iter().map(|s| s.tick_index).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );

    // A tick in the middle of the recorded range returns only the strictly-later ones.
    let since_2 = TuneSampleRow::list_for_run_since(&pool, run.id, 2)
        .await
        .unwrap();
    assert_eq!(
        since_2.iter().map(|s| s.tick_index).collect::<Vec<_>>(),
        vec![3, 4]
    );

    // The most recently sent tick: nothing new yet.
    let since_4 = TuneSampleRow::list_for_run_since(&pool, run.id, 4)
        .await
        .unwrap();
    assert!(since_4.is_empty());

    // A tick past the end of the recorded range (as if a caller's cursor were somehow ahead
    // of what's stored) is also just empty, not an error.
    let since_99 = TuneSampleRow::list_for_run_since(&pool, run.id, 99)
        .await
        .unwrap();
    assert!(since_99.is_empty());
}

#[tokio::test]
async fn tune_sample_list_for_run_since_is_scoped_to_the_given_run() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    let run_a = TuneRunRow::start(
        &pool,
        None,
        "LIC109A",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();
    let run_b = TuneRunRow::start(
        &pool,
        None,
        "LIC109B",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now,
    )
    .await
    .unwrap();

    let sample = Tick {
        time: now,
        pv: 50.0,
    };
    let state = MrftState {
        hysteresis: 1.0,
        mv_value_current: 55.0,
        mv_sign_next_step: 1,
        counter_all_switches: 0,
        cycles_completed: 0,
        cycles_remaining: 2,
    };
    TuneSampleRow::insert(&pool, run_a.id, 0, sample, state, SampleQuality::Good)
        .await
        .unwrap();
    TuneSampleRow::insert(&pool, run_b.id, 0, sample, state, SampleQuality::Good)
        .await
        .unwrap();

    let a_samples = TuneSampleRow::list_for_run_since(&pool, run_a.id, -1)
        .await
        .unwrap();
    assert_eq!(a_samples.len(), 1);
    assert_eq!(a_samples[0].run_id, run_a.id);
}
// }}}1

// tune_results {{{1

fn sample_tuning_result(level: ResponseLevel) -> TuningResult {
    TuningResult {
        response_level: level,
        kp: 1.5,
        ti_minutes: 2.5,
        td_minutes: 0.0,
    }
}

fn sample_pid_parameters(level: ResponseLevel) -> PidParameters {
    PidParameters {
        response_level: level,
        proportional: 66.7,
        integral: 2.5,
        derivative: 0.0,
    }
}

#[tokio::test]
async fn tune_result_insert_and_list_for_run_orders_by_response_level() {
    let pool = connect_in_memory().await.unwrap();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC108",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        Utc::now(),
    )
    .await
    .unwrap();

    for level in [
        ResponseLevel::Sluggish,
        ResponseLevel::Aggressive,
        ResponseLevel::Moderate,
    ] {
        let row = TuneResultRow::from_calculated(
            run.id,
            sample_tuning_result(level),
            sample_pid_parameters(level),
        );
        let inserted = TuneResultRow::insert(&pool, &row).await.unwrap();
        assert!(inserted.id > 0);
        assert_eq!(inserted.response_level, level);
        assert_eq!(inserted.run_id, run.id);
    }

    let results = TuneResultRow::list_for_run(&pool, run.id).await.unwrap();
    assert_eq!(
        results.iter().map(|r| r.response_level).collect::<Vec<_>>(),
        vec![
            ResponseLevel::Aggressive,
            ResponseLevel::Moderate,
            ResponseLevel::Sluggish,
        ],
        "must match bhtune_core::constants::ResponseLevel::ALL's order"
    );
}

#[tokio::test]
async fn tune_result_rejects_duplicate_response_level_for_the_same_run() {
    let pool = connect_in_memory().await.unwrap();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC109",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        Utc::now(),
    )
    .await
    .unwrap();
    let row = TuneResultRow::from_calculated(
        run.id,
        sample_tuning_result(ResponseLevel::Aggressive),
        sample_pid_parameters(ResponseLevel::Aggressive),
    );
    TuneResultRow::insert(&pool, &row).await.unwrap();
    let dup = TuneResultRow::insert(&pool, &row).await;
    assert!(
        dup.is_err(),
        "duplicate (run_id, response_level) must be rejected"
    );
}

#[tokio::test]
async fn tune_result_list_for_run_is_empty_for_an_incomplete_run() {
    let pool = connect_in_memory().await.unwrap();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC110",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        Utc::now(),
    )
    .await
    .unwrap();
    assert!(
        TuneResultRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .is_empty()
    );
}
// }}}1

// tune_writes {{{1

#[tokio::test]
async fn tune_write_insert_records_full_success_with_previous_and_no_rollback() {
    let pool = connect_in_memory().await.unwrap();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC111",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        Utc::now(),
    )
    .await
    .unwrap();

    let mut new = NewTuneWrite::new(ResponseLevel::Moderate, Utc::now());
    new.previous = Some(WriteReadback {
        proportional: 50.0,
        integral: 3.0,
        derivative: 0.0,
    });
    new.proportional_written = Some(66.7);
    new.integral_written = Some(2.5);
    new.derivative_written = Some(0.0);
    new.proportional_readback = Some(66.7);
    new.integral_readback = Some(2.5);
    new.derivative_readback = Some(0.0);
    new.success = true;

    let row = TuneWriteRow::insert(&pool, run.id, new).await.unwrap();
    assert_eq!(row.run_id, run.id);
    assert_eq!(row.response_level, ResponseLevel::Moderate);
    assert!(row.success);
    assert_eq!(
        row.previous,
        Some(WriteReadback {
            proportional: 50.0,
            integral: 3.0,
            derivative: 0.0,
        })
    );
    assert_eq!(row.proportional_written, Some(66.7));
    assert_eq!(row.proportional_readback, Some(66.7));
    assert_eq!(row.integral_readback, Some(2.5));
    assert_eq!(row.derivative_readback, Some(0.0));
    assert!(row.error_message.is_none());
    assert!(row.rollback_state.is_none());
    assert!(row.rollback_error.is_none());
}

#[tokio::test]
async fn tune_write_insert_records_pre_read_failure_with_nothing_attempted() {
    let pool = connect_in_memory().await.unwrap();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC112",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        Utc::now(),
    )
    .await
    .unwrap();

    let mut new = NewTuneWrite::new(ResponseLevel::Aggressive, Utc::now());
    new.error_message = Some("pre-read of Integral tag failed: bad quality".to_string());

    let row = TuneWriteRow::insert(&pool, run.id, new).await.unwrap();
    assert!(!row.success);
    assert!(row.previous.is_none());
    assert!(row.proportional_written.is_none());
    assert!(row.integral_written.is_none());
    assert!(row.derivative_written.is_none());
    assert!(row.proportional_readback.is_none());
    assert!(row.integral_readback.is_none());
    assert!(row.derivative_readback.is_none());
    assert_eq!(
        row.error_message.as_deref(),
        Some("pre-read of Integral tag failed: bad quality")
    );
    assert!(row.rollback_state.is_none());
    assert!(row.rollback_error.is_none());
}

#[tokio::test]
async fn tune_write_insert_records_partial_write_with_successful_rollback() {
    let pool = connect_in_memory().await.unwrap();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC113",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        Utc::now(),
    )
    .await
    .unwrap();

    let mut new = NewTuneWrite::new(ResponseLevel::Sluggish, Utc::now());
    new.previous = Some(WriteReadback {
        proportional: 50.0,
        integral: 3.0,
        derivative: 0.0,
    });
    // Proportional wrote and verified fine; Integral was rejected, so Derivative was never
    // attempted at all -- both left `None`, distinguishing "attempted and confirmed 3.0" from
    // "never attempted".
    new.proportional_written = Some(66.7);
    new.proportional_readback = Some(66.7);
    new.integral_written = Some(9.0);
    new.success = false;
    new.error_message = Some("Integral readback 3.0 outside tolerance of requested 9.0".into());
    new.rollback_state = Some(RollbackState::Succeeded);

    let row = TuneWriteRow::insert(&pool, run.id, new).await.unwrap();
    assert!(!row.success);
    assert_eq!(row.proportional_written, Some(66.7));
    assert_eq!(row.proportional_readback, Some(66.7));
    assert_eq!(row.integral_written, Some(9.0));
    assert!(row.integral_readback.is_none());
    assert!(row.derivative_written.is_none());
    assert!(row.derivative_readback.is_none());
    assert_eq!(row.rollback_state, Some(RollbackState::Succeeded));
    assert!(row.rollback_error.is_none());
}

#[tokio::test]
async fn tune_write_insert_records_partial_write_with_failed_rollback() {
    let pool = connect_in_memory().await.unwrap();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC114",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        Utc::now(),
    )
    .await
    .unwrap();

    let mut new = NewTuneWrite::new(ResponseLevel::Aggressive, Utc::now());
    new.previous = Some(WriteReadback {
        proportional: 50.0,
        integral: 3.0,
        derivative: 0.0,
    });
    new.proportional_written = Some(40.0);
    new.success = false;
    new.error_message = Some("Integral write rejected by DCS".to_string());
    new.rollback_state = Some(RollbackState::Failed);
    new.rollback_error = Some("Proportional rollback write rejected by DCS".to_string());

    let row = TuneWriteRow::insert(&pool, run.id, new).await.unwrap();
    assert!(!row.success);
    assert_eq!(row.rollback_state, Some(RollbackState::Failed));
    assert_eq!(
        row.rollback_error.as_deref(),
        Some("Proportional rollback write rejected by DCS")
    );
}

#[tokio::test]
async fn tune_write_list_for_run_orders_oldest_first() {
    let pool = connect_in_memory().await.unwrap();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC115",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        Utc::now(),
    )
    .await
    .unwrap();
    let t0 = Utc::now();

    let mut new_second = NewTuneWrite::new(ResponseLevel::Moderate, t0 + Duration::seconds(5));
    new_second.success = true;
    let second = TuneWriteRow::insert(&pool, run.id, new_second)
        .await
        .unwrap();

    let mut new_first = NewTuneWrite::new(ResponseLevel::Aggressive, t0);
    new_first.success = true;
    let first = TuneWriteRow::insert(&pool, run.id, new_first)
        .await
        .unwrap();

    let writes = TuneWriteRow::list_for_run(&pool, run.id).await.unwrap();
    assert_eq!(
        writes.iter().map(|w| w.id).collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
}

#[tokio::test]
async fn tune_write_list_for_run_is_empty_when_nothing_was_written() {
    let pool = connect_in_memory().await.unwrap();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC116",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        Utc::now(),
    )
    .await
    .unwrap();
    assert!(
        TuneWriteRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .is_empty()
    );
}
// }}}1

// delete_matching (history-retention) {{{1

#[tokio::test]
async fn delete_matching_deletes_only_runs_matching_the_filter_and_returns_the_count() {
    let pool = connect_in_memory().await.unwrap();
    let t0 = Utc::now();
    let old = seed_run(
        &pool,
        None,
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        t0 - Duration::days(100),
    )
    .await;
    let recent = seed_run(
        &pool,
        None,
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        t0,
    )
    .await;

    let cutoff = t0 - Duration::days(90);
    let deleted =
        TuneRunRow::delete_matching(&pool, &TuneRunFilter::default().with_started_before(cutoff))
            .await
            .unwrap();
    assert_eq!(deleted, 1);

    assert!(TuneRunRow::get(&pool, old.id).await.unwrap().is_none());
    assert!(TuneRunRow::get(&pool, recent.id).await.unwrap().is_some());
}

#[tokio::test]
async fn delete_matching_with_no_matches_deletes_nothing_and_returns_zero() {
    let pool = connect_in_memory().await.unwrap();
    let run = seed_run(
        &pool,
        None,
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        Utc::now(),
    )
    .await;

    let deleted = TuneRunRow::delete_matching(
        &pool,
        &TuneRunFilter::default().with_started_before(Utc::now() - Duration::days(365)),
    )
    .await
    .unwrap();
    assert_eq!(deleted, 0);
    assert!(TuneRunRow::get(&pool, run.id).await.unwrap().is_some());
}

#[tokio::test]
async fn delete_matching_cascades_to_samples_results_and_writes() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    let run = TuneRunRow::start(
        &pool,
        None,
        "LIC117",
        TuneBackend::Simulator,
        sample_config(),
        TemplateOrigin::Builtin,
        &sample_template(),
        &sample_tags(),
        now - Duration::days(200),
    )
    .await
    .unwrap();

    let tick = Tick {
        time: now,
        pv: 50.0,
    };
    let state = MrftState {
        hysteresis: 1.0,
        mv_value_current: 55.0,
        mv_sign_next_step: 1,
        counter_all_switches: 0,
        cycles_completed: 0,
        cycles_remaining: 2,
    };
    TuneSampleRow::insert(&pool, run.id, 0, tick, state, SampleQuality::Good)
        .await
        .unwrap();
    let result_row = TuneResultRow::from_calculated(
        run.id,
        sample_tuning_result(ResponseLevel::Moderate),
        sample_pid_parameters(ResponseLevel::Moderate),
    );
    TuneResultRow::insert(&pool, &result_row).await.unwrap();
    let mut new_write = NewTuneWrite::new(ResponseLevel::Moderate, now);
    new_write.success = true;
    TuneWriteRow::insert(&pool, run.id, new_write)
        .await
        .unwrap();

    let deleted = TuneRunRow::delete_matching(
        &pool,
        &TuneRunFilter::default().with_started_before(now - Duration::days(100)),
    )
    .await
    .unwrap();
    assert_eq!(deleted, 1);

    assert!(TuneRunRow::get(&pool, run.id).await.unwrap().is_none());
    assert!(
        TuneSampleRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        TuneResultRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        TuneWriteRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn delete_matching_with_an_empty_filter_deletes_every_run() {
    let pool = connect_in_memory().await.unwrap();
    seed_run(
        &pool,
        None,
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        Utc::now(),
    )
    .await;
    seed_run(
        &pool,
        None,
        TuneBackend::Simulator,
        sample_config(),
        TuneOutcome::Completed,
        Utc::now(),
    )
    .await;

    let deleted = TuneRunRow::delete_matching(&pool, &TuneRunFilter::default())
        .await
        .unwrap();
    assert_eq!(deleted, 2);
    assert_eq!(
        TuneRunRow::count(&pool, &TuneRunFilter::default())
            .await
            .unwrap(),
        0
    );
}
// }}}1
