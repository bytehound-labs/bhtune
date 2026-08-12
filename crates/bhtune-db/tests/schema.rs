//! Integration tests proving the migration in `migrations/0001_initial_schema.sql` actually
//! behaves the way its comments claim: every table exists, `CHECK` constraints reject bad
//! data, `ON DELETE CASCADE`/`SET NULL`/`RESTRICT` behave per-relationship as designed, and a
//! real `bhtune-core` value (a built-in `DcsTemplate`, a derived `LoopTags`) round-trips
//! through its column/JSON-blob storage exactly.

use bhtune_core::{LoopTags, built_in_templates};
use bhtune_db::{connect_in_memory, models::DcsTemplateRow};
use chrono::Utc;
use sqlx::Row;

#[tokio::test]
async fn dcs_template_round_trips_every_built_in_template() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();

    for template in built_in_templates() {
        let inserted = DcsTemplateRow::insert(&pool, &template, true, now)
            .await
            .unwrap();
        assert!(inserted.id > 0);
        assert!(inserted.is_builtin);
        assert_eq!(inserted.template, template);

        let fetched = DcsTemplateRow::get(&pool, inserted.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched, inserted);
    }
}

#[tokio::test]
async fn dcs_template_get_returns_none_for_missing_id() {
    let pool = connect_in_memory().await.unwrap();
    assert!(DcsTemplateRow::get(&pool, 999).await.unwrap().is_none());
}

#[tokio::test]
async fn dcs_template_name_must_be_unique() {
    let pool = connect_in_memory().await.unwrap();
    let template = built_in_templates().remove(0);
    let now = Utc::now();

    DcsTemplateRow::insert(&pool, &template, true, now)
        .await
        .unwrap();
    let dup = DcsTemplateRow::insert(&pool, &template, true, now).await;
    assert!(dup.is_err(), "duplicate template name must be rejected");
}

/// Inserts one built-in template and returns its id, for tests further down the FK chain.
async fn seed_template(pool: &sqlx::SqlitePool) -> i64 {
    let template = built_in_templates().remove(0);
    DcsTemplateRow::insert(pool, &template, true, Utc::now())
        .await
        .unwrap()
        .id
}

fn sample_loop_tags() -> LoopTags {
    let template = built_in_templates().remove(0);
    LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &template)
}

#[tokio::test]
async fn loop_tags_json_round_trips_exactly() {
    let pool = connect_in_memory().await.unwrap();
    let template_id = seed_template(&pool).await;
    let tags = sample_loop_tags();
    let tags_json = serde_json::to_string(&tags).unwrap();
    let now = Utc::now();

    sqlx::query(
        r#"
        INSERT INTO loops (
            name, dcs_template_id, tags_json, process_type, controller_type,
            relay_amp_percent, num_cycles_skip, num_cycles_count, noise_protection_secs,
            mrft_delay_secs, created_at, updated_at
        ) VALUES (?, ?, ?, 'flow', 'pi', 5.0, 1, 2, 3, 0, ?, ?)
        "#,
    )
    .bind("LIC101")
    .bind(template_id)
    .bind(&tags_json)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();

    let row = sqlx::query("SELECT tags_json FROM loops WHERE name = 'LIC101'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let fetched_json: String = row.try_get("tags_json").unwrap();
    let fetched_tags: LoopTags = serde_json::from_str(&fetched_json).unwrap();
    assert_eq!(fetched_tags, tags);
}

#[tokio::test]
async fn loops_reject_invalid_json_and_invalid_enum_values() {
    let pool = connect_in_memory().await.unwrap();
    let template_id = seed_template(&pool).await;
    let now = Utc::now();

    let bad_json = sqlx::query(
        r#"
        INSERT INTO loops (
            name, dcs_template_id, tags_json, process_type, controller_type,
            relay_amp_percent, num_cycles_skip, num_cycles_count, noise_protection_secs,
            mrft_delay_secs, created_at, updated_at
        ) VALUES ('bad-json', ?, 'not valid json', 'flow', 'pi', 5.0, 1, 2, 3, 0, ?, ?)
        "#,
    )
    .bind(template_id)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await;
    assert!(
        bad_json.is_err(),
        "invalid JSON in tags_json must be rejected"
    );

    let bad_enum = sqlx::query(
        r#"
        INSERT INTO loops (
            name, dcs_template_id, tags_json, process_type, controller_type,
            relay_amp_percent, num_cycles_skip, num_cycles_count, noise_protection_secs,
            mrft_delay_secs, created_at, updated_at
        ) VALUES ('bad-enum', ?, '{}', 'not_a_process_type', 'pi', 5.0, 1, 2, 3, 0, ?, ?)
        "#,
    )
    .bind(template_id)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await;
    assert!(bad_enum.is_err(), "invalid process_type must be rejected");
}

#[tokio::test]
async fn deleting_a_referenced_dcs_template_is_restricted() {
    let pool = connect_in_memory().await.unwrap();
    let template_id = seed_template(&pool).await;
    let now = Utc::now();

    sqlx::query(
        r#"
        INSERT INTO loops (
            name, dcs_template_id, tags_json, process_type, controller_type,
            relay_amp_percent, num_cycles_skip, num_cycles_count, noise_protection_secs,
            mrft_delay_secs, created_at, updated_at
        ) VALUES ('LIC101', ?, '{}', 'flow', 'pi', 5.0, 1, 2, 3, 0, ?, ?)
        "#,
    )
    .bind(template_id)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();

    let delete = sqlx::query("DELETE FROM dcs_templates WHERE id = ?")
        .bind(template_id)
        .execute(&pool)
        .await;
    assert!(
        delete.is_err(),
        "deleting a dcs_template referenced by a loop must be restricted"
    );
}

/// Inserts a loop (and its owning template) and returns the loop's id.
async fn seed_loop(pool: &sqlx::SqlitePool) -> i64 {
    let template_id = seed_template(pool).await;
    let now = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO loops (
            name, dcs_template_id, tags_json, process_type, controller_type,
            relay_amp_percent, num_cycles_skip, num_cycles_count, noise_protection_secs,
            mrft_delay_secs, created_at, updated_at
        ) VALUES ('LIC101', ?, '{}', 'flow', 'pi', 5.0, 1, 2, 3, 0, ?, ?)
        RETURNING id
        "#,
    )
    .bind(template_id)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await
    .unwrap()
    .try_get("id")
    .unwrap()
}

/// Inserts a `tune_runs` row attempted-and-failed before any backend I/O happened (all
/// initial-reading columns left `NULL`), proving the schema supports the "auditable failure"
/// case that motivated making those columns nullable in the first place.
async fn seed_failed_run(pool: &sqlx::SqlitePool, loop_id: Option<i64>) -> i64 {
    sqlx::query(
        r#"
        INSERT INTO tune_runs (
            loop_id, loop_name, backend, started_at, outcome, failure_reason,
            process_type, controller_type, relay_amp_percent, num_cycles_skip,
            num_cycles_count, noise_protection_secs, mrft_delay_secs, created_at
        ) VALUES (?, 'LIC101', 'opcda', ?, 'failed', 'InvalidCastException reading initial values',
                  'flow', 'pi', 5.0, 1, 2, 3, 0, ?)
        RETURNING id
        "#,
    )
    .bind(loop_id)
    .bind(Utc::now())
    .bind(Utc::now())
    .fetch_one(pool)
    .await
    .unwrap()
    .try_get("id")
    .unwrap()
}

#[tokio::test]
async fn tune_run_supports_failed_before_initial_read_with_all_readings_null() {
    let pool = connect_in_memory().await.unwrap();
    let loop_id = seed_loop(&pool).await;
    let run_id = seed_failed_run(&pool, Some(loop_id)).await;

    let row = sqlx::query(
        "SELECT outcome, pv_ini, mv_ini, controller_direction FROM tune_runs WHERE id = ?",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let outcome: String = row.try_get("outcome").unwrap();
    let pv_ini: Option<f64> = row.try_get("pv_ini").unwrap();
    let controller_direction: Option<String> = row.try_get("controller_direction").unwrap();
    assert_eq!(outcome, "failed");
    assert!(pv_ini.is_none());
    assert!(controller_direction.is_none());
}

#[tokio::test]
async fn tune_run_rejects_invalid_outcome_and_backend() {
    let pool = connect_in_memory().await.unwrap();
    let loop_id = seed_loop(&pool).await;

    let bad_outcome = sqlx::query(
        r#"
        INSERT INTO tune_runs (
            loop_id, loop_name, backend, started_at, outcome,
            process_type, controller_type, relay_amp_percent, num_cycles_skip,
            num_cycles_count, noise_protection_secs, mrft_delay_secs, created_at
        ) VALUES (?, 'LIC101', 'opcda', ?, 'not_a_real_outcome', 'flow', 'pi', 5.0, 1, 2, 3, 0, ?)
        "#,
    )
    .bind(loop_id)
    .bind(Utc::now())
    .bind(Utc::now())
    .execute(&pool)
    .await;
    assert!(bad_outcome.is_err());

    let bad_backend = sqlx::query(
        r#"
        INSERT INTO tune_runs (
            loop_id, loop_name, backend, started_at, outcome,
            process_type, controller_type, relay_amp_percent, num_cycles_skip,
            num_cycles_count, noise_protection_secs, mrft_delay_secs, created_at
        ) VALUES (?, 'LIC101', 'modbus', ?, 'running', 'flow', 'pi', 5.0, 1, 2, 3, 0, ?)
        "#,
    )
    .bind(loop_id)
    .bind(Utc::now())
    .bind(Utc::now())
    .execute(&pool)
    .await;
    assert!(
        bad_backend.is_err(),
        "backend outside the current roadmap must be rejected"
    );
}

#[tokio::test]
async fn deleting_a_loop_sets_tune_runs_loop_id_null_but_keeps_the_run() {
    let pool = connect_in_memory().await.unwrap();
    let loop_id = seed_loop(&pool).await;
    let run_id = seed_failed_run(&pool, Some(loop_id)).await;

    sqlx::query("DELETE FROM loops WHERE id = ?")
        .bind(loop_id)
        .execute(&pool)
        .await
        .unwrap();

    let row = sqlx::query("SELECT loop_id, loop_name FROM tune_runs WHERE id = ?")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let remaining_loop_id: Option<i64> = row.try_get("loop_id").unwrap();
    let loop_name: String = row.try_get("loop_name").unwrap();
    assert_eq!(remaining_loop_id, None);
    assert_eq!(
        loop_name, "LIC101",
        "the snapshot name must survive the loop's deletion"
    );
}

#[tokio::test]
async fn tune_samples_enforce_unique_tick_and_cascade_delete_with_the_run() {
    let pool = connect_in_memory().await.unwrap();
    let run_id = seed_failed_run(&pool, None).await;

    for tick in 0..3 {
        sqlx::query(
            r#"
            INSERT INTO tune_samples (
                run_id, tick, time, pv, hysteresis, mv_value_current, mv_sign_next_step,
                counter_all_switches, cycles_completed, cycles_remaining
            ) VALUES (?, ?, ?, 50.0, 1.0, 55.0, 1, 0, 0, 2)
            "#,
        )
        .bind(run_id)
        .bind(tick)
        .bind(Utc::now())
        .execute(&pool)
        .await
        .unwrap();
    }

    let dup = sqlx::query(
        r#"
        INSERT INTO tune_samples (
            run_id, tick, time, pv, hysteresis, mv_value_current, mv_sign_next_step,
            counter_all_switches, cycles_completed, cycles_remaining
        ) VALUES (?, 0, ?, 50.0, 1.0, 55.0, 1, 0, 0, 2)
        "#,
    )
    .bind(run_id)
    .bind(Utc::now())
    .execute(&pool)
    .await;
    assert!(dup.is_err(), "duplicate (run_id, tick) must be rejected");

    sqlx::query("DELETE FROM tune_runs WHERE id = ?")
        .bind(run_id)
        .execute(&pool)
        .await
        .unwrap();

    let (remaining,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tune_samples WHERE run_id = ?")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "samples must cascade-delete with their run");
}

#[tokio::test]
async fn tune_results_enforce_unique_response_level_and_cascade_delete_with_the_run() {
    let pool = connect_in_memory().await.unwrap();
    let run_id = seed_failed_run(&pool, None).await;

    for level in ["aggressive", "moderate", "sluggish"] {
        sqlx::query(
            r#"
            INSERT INTO tune_results (run_id, response_level, kp, ti_minutes, td_minutes, proportional, integral, derivative)
            VALUES (?, ?, 1.0, 2.0, 0.0, 3.0, 4.0, 0.0)
            "#,
        )
        .bind(run_id)
        .bind(level)
        .execute(&pool)
        .await
        .unwrap();
    }

    let dup = sqlx::query(
        r#"
        INSERT INTO tune_results (run_id, response_level, kp, ti_minutes, td_minutes, proportional, integral, derivative)
        VALUES (?, 'aggressive', 1.0, 2.0, 0.0, 3.0, 4.0, 0.0)
        "#,
    )
    .bind(run_id)
    .execute(&pool)
    .await;
    assert!(
        dup.is_err(),
        "duplicate (run_id, response_level) must be rejected"
    );

    sqlx::query("DELETE FROM tune_runs WHERE id = ?")
        .bind(run_id)
        .execute(&pool)
        .await
        .unwrap();

    let (remaining,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tune_results WHERE run_id = ?")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "results must cascade-delete with their run");
}

#[tokio::test]
async fn tune_writes_supports_failed_write_with_null_readback_and_cascade_deletes() {
    let pool = connect_in_memory().await.unwrap();
    let run_id = seed_failed_run(&pool, None).await;

    // A successful write, with readback confirmation.
    sqlx::query(
        r#"
        INSERT INTO tune_writes (
            run_id, response_level, written_at, proportional_written, integral_written,
            derivative_written, proportional_readback, integral_readback, derivative_readback,
            success
        ) VALUES (?, 'moderate', ?, 3.0, 4.0, 0.0, 3.0, 4.0, 0.0, 1)
        "#,
    )
    .bind(run_id)
    .bind(Utc::now())
    .execute(&pool)
    .await
    .unwrap();

    // A failed write: no readback, an error message instead.
    sqlx::query(
        r#"
        INSERT INTO tune_writes (
            run_id, response_level, written_at, proportional_written, integral_written,
            derivative_written, success, error_message
        ) VALUES (?, 'aggressive', ?, 1.0, 2.0, 0.0, 0, 'write rejected by DCS')
        "#,
    )
    .bind(run_id)
    .bind(Utc::now())
    .execute(&pool)
    .await
    .unwrap();

    let bad_success = sqlx::query(
        r#"
        INSERT INTO tune_writes (run_id, response_level, written_at, proportional_written, integral_written, derivative_written, success)
        VALUES (?, 'sluggish', ?, 1.0, 2.0, 0.0, 2)
        "#,
    )
    .bind(run_id)
    .bind(Utc::now())
    .execute(&pool)
    .await;
    assert!(
        bad_success.is_err(),
        "success must be constrained to 0 or 1"
    );

    sqlx::query("DELETE FROM tune_runs WHERE id = ?")
        .bind(run_id)
        .execute(&pool)
        .await
        .unwrap();

    let (remaining,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tune_writes WHERE run_id = ?")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "writes must cascade-delete with their run");
}

#[tokio::test]
async fn settings_reject_invalid_json_and_round_trip_valid_json() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();

    let bad =
        sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('x', 'not json', ?)")
            .bind(now)
            .execute(&pool)
            .await;
    assert!(bad.is_err());

    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES ('history_retention_days', '30', ?)",
    )
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();

    let row = sqlx::query("SELECT value FROM settings WHERE key = 'history_retention_days'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let value: String = row.try_get("value").unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&value).unwrap();
    assert_eq!(parsed, serde_json::json!(30));
}
