use bhtune_core::{ControllerType, LoopConfig, LoopTags, ProcessType};
use bhtune_db::{
    connect, connect_in_memory,
    models::{
        DEMO_RESTART_INTERRUPTED_REASON, DemoSessionRow, Pagination, TemplateOrigin, TuneDriver,
        TuneOutcome, TuneRunRow,
    },
};
use chrono::{DateTime, Duration, Utc};

fn token_hash(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn run_fixture() -> (LoopConfig, bhtune_core::DcsTemplate, LoopTags) {
    let template = bhtune_core::built_in_templates().remove(0);
    let tags = LoopTags::derive_from_pv_tag("demo.PV", &template);
    let config = LoopConfig {
        process_type: ProcessType::Flow,
        controller_type: ControllerType::Pi,
        relay_amp_percent: 5.0,
        num_cycles_skip: 1,
        num_cycles_count: 2,
        noise_protection_secs: 0,
        mrft_delay_secs: 0,
    };
    (config, template, tags)
}

async fn create_session(pool: &sqlx::SqlitePool, hash: &str, now: DateTime<Utc>) -> DemoSessionRow {
    DemoSessionRow::create(pool, hash, now, now + Duration::hours(1))
        .await
        .unwrap()
}

async fn start_owned(
    pool: &sqlx::SqlitePool,
    session_id: i64,
    loop_name: &str,
    now: DateTime<Utc>,
) -> TuneRunRow {
    let (config, template, tags) = run_fixture();
    TuneRunRow::start_owned(
        pool,
        session_id,
        loop_name,
        config,
        TemplateOrigin::Builtin,
        &template,
        &tags,
        now,
    )
    .await
    .unwrap()
}

async fn insert_sample(pool: &sqlx::SqlitePool, run_id: i64, now: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO tune_samples (
            run_id, tick, time, pv, pv_quality, hysteresis, mv_value_current,
            mv_sign_next_step, counter_all_switches, cycles_completed, cycles_remaining
         ) VALUES (?, 0, ?, 50.0, 'good', 1.0, 45.0, 1, 0, 0, 2)",
    )
    .bind(run_id)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn lookup_and_touch_only_accept_valid_sessions_without_sliding_expiry() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    assert!(
        DemoSessionRow::create(&pool, "not-a-sha256-hash", now, now + Duration::hours(1))
            .await
            .is_err()
    );
    let hash = token_hash('a');
    let expires_at = now + Duration::hours(1);
    let session = DemoSessionRow::create(&pool, &hash, now, expires_at)
        .await
        .unwrap();

    let touched_at = now + Duration::minutes(15);
    let touched = DemoSessionRow::get_by_token_hash(&pool, &hash, touched_at)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(touched.last_seen_at, touched_at);
    assert_eq!(touched.expires_at, expires_at);

    assert!(
        DemoSessionRow::get_by_token_hash(&pool, &hash, expires_at)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        DemoSessionRow::touch_by_token_hash(&pool, &hash, expires_at)
            .await
            .unwrap()
            .is_none()
    );

    assert!(
        DemoSessionRow::revoke(&pool, session.id, touched_at)
            .await
            .unwrap()
    );
    assert!(
        DemoSessionRow::get_by_token_hash(&pool, &hash, touched_at)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        DemoSessionRow::touch_by_token_hash(&pool, &hash, touched_at)
            .await
            .unwrap()
            .is_none()
    );
    let last_seen: DateTime<Utc> =
        sqlx::query_scalar("SELECT last_seen_at FROM demo_sessions WHERE id = ?")
            .bind(session.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(last_seen, touched_at);
}

#[tokio::test]
async fn concurrent_first_use_converges_on_one_persisted_session() {
    let directory = tempfile::tempdir().unwrap();
    let pool = connect(&directory.path().join("demo-concurrency.db"))
        .await
        .unwrap();
    let now = Utc::now();
    let expires_at = now + Duration::hours(1);
    let hash = token_hash('b');

    let (left, right) = tokio::join!(
        DemoSessionRow::create(&pool, &hash, now, expires_at),
        DemoSessionRow::create(&pool, &hash, now, expires_at),
    );
    let left = left.unwrap();
    let right = right.unwrap();

    assert_eq!(left.id, right.id);
    assert_eq!(left.expires_at, expires_at);
    assert_eq!(right.expires_at, expires_at);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM demo_sessions WHERE token_hash = ?",)
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn get_or_create_does_not_revive_an_expired_or_revoked_token() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();

    let valid_hash = token_hash('6');
    let valid = create_session(&pool, &valid_hash, now).await;
    let existing = DemoSessionRow::get_or_create(
        &pool,
        &valid_hash,
        now + Duration::minutes(1),
        now + Duration::days(1),
    )
    .await
    .unwrap();
    assert_eq!(existing.id, valid.id);
    assert_eq!(existing.last_seen_at, now + Duration::minutes(1));
    assert_eq!(existing.expires_at, valid.expires_at);

    let expired_hash = token_hash('c');
    DemoSessionRow::create(
        &pool,
        &expired_hash,
        now - Duration::hours(2),
        now - Duration::hours(1),
    )
    .await
    .unwrap();
    assert!(
        DemoSessionRow::get_or_create(&pool, &expired_hash, now, now + Duration::hours(1),)
            .await
            .is_err()
    );

    let revoked_hash = token_hash('d');
    let revoked = create_session(&pool, &revoked_hash, now).await;
    DemoSessionRow::revoke(&pool, revoked.id, now)
        .await
        .unwrap();
    assert!(
        DemoSessionRow::get_or_create(&pool, &revoked_hash, now, now + Duration::hours(1),)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn owned_run_insert_is_atomic_immutable_and_simulator_only() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    let owner = create_session(&pool, &token_hash('e'), now).await;
    let other = create_session(&pool, &token_hash('f'), now).await;
    let (config, template, tags) = run_fixture();

    let error = TuneRunRow::start_with_demo_session(
        &pool,
        Some(owner.id),
        None,
        "demo.PV",
        TuneDriver::Opcda,
        config,
        TemplateOrigin::Builtin,
        &template,
        &tags,
        now,
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("demo_session_id requires simulator")
    );

    let owned = start_owned(&pool, owner.id, "owned", now).await;
    assert_eq!(owned.demo_session_id, Some(owner.id));
    let transfer_error = sqlx::query("UPDATE tune_runs SET demo_session_id = ? WHERE id = ?")
        .bind(other.id)
        .bind(owned.id)
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(
        transfer_error
            .to_string()
            .contains("demo_session_id is immutable")
    );
    let driver_error = sqlx::query("UPDATE tune_runs SET driver = 'opcda' WHERE id = ?")
        .bind(owned.id)
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(
        driver_error
            .to_string()
            .contains("demo_session_id requires simulator")
    );

    let full = TuneRunRow::start(
        &pool,
        None,
        "full",
        TuneDriver::Simulator,
        config,
        TemplateOrigin::Builtin,
        &template,
        &tags,
        now,
    )
    .await
    .unwrap();
    assert_eq!(full.demo_session_id, None);

    let expired_owner = DemoSessionRow::create(
        &pool,
        &token_hash('0'),
        now - Duration::hours(2),
        now - Duration::hours(1),
    )
    .await
    .unwrap();
    assert!(
        TuneRunRow::start_owned(
            &pool,
            expired_owner.id,
            "expired-owner",
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            now,
        )
        .await
        .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tune_runs WHERE demo_session_id = ?",)
            .bind(expired_owner.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );

    let revoked_owner = create_session(&pool, &token_hash('7'), now).await;
    DemoSessionRow::revoke(&pool, revoked_owner.id, now)
        .await
        .unwrap();
    assert!(
        TuneRunRow::start_owned(
            &pool,
            revoked_owner.id,
            "revoked-owner",
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            now,
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn owner_scoped_helpers_do_not_cross_session_boundaries_and_delete_cascades() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    let owner = create_session(&pool, &token_hash('1'), now).await;
    let other = create_session(&pool, &token_hash('2'), now).await;
    let older = start_owned(&pool, owner.id, "older", now).await;
    let newest = start_owned(&pool, owner.id, "newest", now + Duration::seconds(1)).await;
    let other_run = start_owned(&pool, other.id, "other", now + Duration::seconds(2)).await;
    insert_sample(&pool, older.id, now).await;

    assert_eq!(
        TuneRunRow::count_for_demo_session(&pool, owner.id)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        TuneRunRow::newest_for_demo_session(&pool, owner.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        newest.id
    );
    assert_eq!(
        TuneRunRow::list_for_demo_session(&pool, owner.id, Pagination::first(10))
            .await
            .unwrap()
            .iter()
            .map(|run| run.id)
            .collect::<Vec<_>>(),
        vec![newest.id, older.id]
    );
    assert!(
        TuneRunRow::get_for_demo_session(&pool, other_run.id, owner.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !TuneRunRow::delete_for_demo_session(&pool, older.id, other.id)
            .await
            .unwrap()
    );
    assert_eq!(
        TuneRunRow::count_rows_for_demo_session(&pool, owner.id)
            .await
            .unwrap(),
        3
    );

    assert!(
        TuneRunRow::delete_for_demo_session(&pool, older.id, owner.id)
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tune_samples WHERE run_id = ?")
            .bind(older.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert!(
        TuneRunRow::get(&pool, other_run.id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn global_count_and_terminal_pruning_preserve_running_full_and_other_owner_rows() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    let owner = create_session(&pool, &token_hash('8'), now).await;
    let other = create_session(&pool, &token_hash('9'), now).await;

    let oldest = start_owned(&pool, owner.id, "oldest", now).await;
    let middle = start_owned(&pool, owner.id, "middle", now + Duration::seconds(1)).await;
    let newest = start_owned(&pool, owner.id, "newest", now + Duration::seconds(2)).await;
    let running = start_owned(&pool, owner.id, "running", now + Duration::seconds(3)).await;
    let other_terminal = start_owned(&pool, other.id, "other", now + Duration::seconds(4)).await;
    for run in [&oldest, &middle, &newest, &other_terminal] {
        TuneRunRow::fail(&pool, run.id, now + Duration::seconds(5), "terminal")
            .await
            .unwrap();
    }
    insert_sample(&pool, oldest.id, now).await;

    let (config, template, tags) = run_fixture();
    let full = TuneRunRow::start(
        &pool,
        None,
        "full",
        TuneDriver::Simulator,
        config,
        TemplateOrigin::Builtin,
        &template,
        &tags,
        now + Duration::seconds(5),
    )
    .await
    .unwrap();
    TuneRunRow::fail(&pool, full.id, now + Duration::seconds(6), "full terminal")
        .await
        .unwrap();

    assert_eq!(TuneRunRow::count_demo_owned(&pool).await.unwrap(), 5);
    assert_eq!(
        TuneRunRow::prune_terminal_for_demo_session(&pool, owner.id, 2)
            .await
            .unwrap(),
        1
    );
    assert!(TuneRunRow::get(&pool, oldest.id).await.unwrap().is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tune_samples WHERE run_id = ?")
            .bind(oldest.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    for run_id in [middle.id, newest.id, running.id, other_terminal.id, full.id] {
        assert!(TuneRunRow::get(&pool, run_id).await.unwrap().is_some());
    }
    assert_eq!(TuneRunRow::count_demo_owned(&pool).await.unwrap(), 4);

    assert_eq!(
        TuneRunRow::prune_terminal_demo_owned(&pool, 1)
            .await
            .unwrap(),
        1
    );
    assert!(TuneRunRow::get(&pool, middle.id).await.unwrap().is_none());
    for run_id in [newest.id, running.id, other_terminal.id, full.id] {
        assert!(TuneRunRow::get(&pool, run_id).await.unwrap().is_some());
    }
    assert_eq!(TuneRunRow::count_demo_owned(&pool).await.unwrap(), 3);
}

#[tokio::test]
async fn database_enforces_the_fixed_global_demo_run_row_cap_atomically() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();
    let owner = create_session(&pool, &token_hash('a'), now).await;

    sqlx::query(
        "WITH RECURSIVE sequence(n) AS (
             SELECT 1
             UNION ALL
             SELECT n + 1 FROM sequence WHERE n < 5000
         )
         INSERT INTO tune_runs (
             demo_session_id, loop_name, template_name, template_origin,
             template_snapshot_json, tags_json, driver, started_at, outcome,
             process_type, controller_type, relay_amp_percent, num_cycles_skip,
             num_cycles_count, noise_protection_secs, mrft_delay_secs, created_at
         )
         SELECT ?, 'demo-cap-' || n, 'fixture', 'builtin', '{}', '{}', 'simulator',
                ?, 'completed', 'flow', 'pi', 5.0, 1, 2, 0, 0, ?
         FROM sequence",
    )
    .bind(owner.id)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(TuneRunRow::count_demo_owned(&pool).await.unwrap(), 5000);

    let (config, template, tags) = run_fixture();
    let error = TuneRunRow::start_owned(
        &pool,
        owner.id,
        "over-cap",
        config,
        TemplateOrigin::Builtin,
        &template,
        &tags,
        now,
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("demo tune run row limit reached")
    );
    assert_eq!(TuneRunRow::count_demo_owned(&pool).await.unwrap(), 5000);
}

#[tokio::test]
async fn cleanup_protects_running_history_until_restart_recovery_marks_it_failed() {
    let pool = connect_in_memory().await.unwrap();
    let created_at = Utc::now();
    let session = create_session(&pool, &token_hash('3'), created_at).await;
    let run = start_owned(&pool, session.id, "running", created_at).await;
    insert_sample(&pool, run.id, created_at).await;
    let cleanup_at = created_at + Duration::hours(2);

    assert_eq!(
        DemoSessionRow::cleanup_expired(&pool, cleanup_at)
            .await
            .unwrap(),
        0
    );
    let revoked_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT revoked_at FROM demo_sessions WHERE id = ?")
            .bind(session.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(revoked_at, None);
    assert!(TuneRunRow::get(&pool, run.id).await.unwrap().is_some());

    assert_eq!(
        DemoSessionRow::recover_running_demo_runs(&pool, cleanup_at)
            .await
            .unwrap(),
        1
    );
    let recovered = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
    assert_eq!(recovered.outcome, TuneOutcome::Failed);
    assert_eq!(recovered.completed_at, Some(cleanup_at));
    assert_eq!(
        recovered.failure_reason.as_deref(),
        Some(DEMO_RESTART_INTERRUPTED_REASON)
    );

    assert_eq!(
        DemoSessionRow::cleanup_expired(&pool, cleanup_at)
            .await
            .unwrap(),
        1
    );
    assert!(TuneRunRow::get(&pool, run.id).await.unwrap().is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tune_samples WHERE run_id = ?")
            .bind(run.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn cleanup_removes_expired_sessions_with_only_terminal_or_no_runs() {
    let pool = connect_in_memory().await.unwrap();
    let created_at = Utc::now();
    let terminal_owner = create_session(&pool, &token_hash('4'), created_at).await;
    let empty_owner = create_session(&pool, &token_hash('5'), created_at).await;
    let run = start_owned(&pool, terminal_owner.id, "terminal", created_at).await;
    TuneRunRow::fail(&pool, run.id, created_at, "finished before expiry")
        .await
        .unwrap();
    insert_sample(&pool, run.id, created_at).await;

    assert_eq!(
        DemoSessionRow::cleanup_expired(&pool, created_at + Duration::hours(2))
            .await
            .unwrap(),
        2
    );
    assert!(TuneRunRow::get(&pool, run.id).await.unwrap().is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM demo_sessions WHERE id IN (?, ?)",)
            .bind(terminal_owner.id)
            .bind(empty_owner.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}
