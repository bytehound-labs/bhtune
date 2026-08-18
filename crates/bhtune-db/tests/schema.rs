//! Integration tests proving the migration in `migrations/0001_initial_schema.sql` actually
//! behaves the way its comments claim: every table exists, `CHECK` constraints reject bad
//! data, `ON DELETE CASCADE`/`SET NULL`/`RESTRICT` behave per-relationship as designed, and a
//! real `bhtune-core` value (a built-in `DcsTemplate`, a derived `LoopTags`) round-trips
//! through its column/JSON-blob storage exactly.

use bhtune_core::{LoopTags, built_in_templates};
use bhtune_db::{
    DbError, connect_in_memory,
    models::{DcsTemplateRow, TemplateOrigin},
};
use chrono::Utc;
use sqlx::Row;

#[tokio::test]
async fn dcs_template_round_trips_every_built_in_template() {
    let pool = connect_in_memory().await.unwrap();
    let now = Utc::now();

    for template in built_in_templates() {
        let inserted = DcsTemplateRow::insert(&pool, &template, TemplateOrigin::Builtin, now)
            .await
            .unwrap();
        assert!(inserted.id > 0);
        assert_eq!(inserted.origin, TemplateOrigin::Builtin);
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

    DcsTemplateRow::insert(&pool, &template, TemplateOrigin::Builtin, now)
        .await
        .unwrap();
    let dup = DcsTemplateRow::insert(&pool, &template, TemplateOrigin::Builtin, now).await;
    assert!(dup.is_err(), "duplicate template name must be rejected");
}

#[tokio::test]
async fn dcs_templates_reject_invalid_origin_and_invalid_versions_json() {
    let pool = connect_in_memory().await.unwrap();
    let template = built_in_templates().remove(0);
    let id = DcsTemplateRow::insert(&pool, &template, TemplateOrigin::Builtin, Utc::now())
        .await
        .unwrap()
        .id;

    let bad_origin = sqlx::query("UPDATE dcs_templates SET origin = ? WHERE id = ?")
        .bind("not_a_real_origin")
        .bind(id)
        .execute(&pool)
        .await;
    assert!(
        bad_origin.is_err(),
        "origin outside builtin/catalog/user must be rejected"
    );

    let bad_versions_json = sqlx::query("UPDATE dcs_templates SET versions_json = ? WHERE id = ?")
        .bind("not valid json")
        .bind(id)
        .execute(&pool)
        .await;
    assert!(
        bad_versions_json.is_err(),
        "invalid JSON in versions_json must be rejected"
    );
}

/// `json_valid` only proves `versions_json` is syntactically valid JSON, not that it
/// deserializes into the `Vec<String>` shape the column is supposed to hold -- `"123"` is
/// valid JSON (a bare number) but the wrong shape. This proves `row_to_dcs_template`'s
/// `DbError::InvalidJsonShape` mapping actually fires for that case, rather than only being
/// reachable in theory.
#[tokio::test]
async fn dcs_template_get_rejects_versions_json_with_the_wrong_shape() {
    let pool = connect_in_memory().await.unwrap();
    let template = built_in_templates().remove(0);
    let id = DcsTemplateRow::insert(&pool, &template, TemplateOrigin::Builtin, Utc::now())
        .await
        .unwrap()
        .id;

    sqlx::query("UPDATE dcs_templates SET versions_json = '123' WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    let err = DcsTemplateRow::get(&pool, id).await.unwrap_err();
    assert!(matches!(
        err,
        bhtune_db::DbError::InvalidJsonShape {
            column: "versions_json",
            ..
        }
    ));
}

/// Distinct from `dcs_template_round_trips_every_built_in_template`, which only ever seeds
/// (and therefore only ever exercises) `origin = 'builtin'`: nothing in `bhtune-cli` produces
/// `TemplateOrigin::Catalog` yet (`template-user-catalog` is what will), so this is the only
/// place any test proves `dcs_templates.origin` round-trips *every* variant the `CHECK`
/// constraint allows, not just the one variant real callers currently use.
#[tokio::test]
async fn dcs_template_origin_round_trips_every_variant() {
    let pool = connect_in_memory().await.unwrap();
    let template = built_in_templates().remove(0);
    let id = DcsTemplateRow::insert(&pool, &template, TemplateOrigin::Builtin, Utc::now())
        .await
        .unwrap()
        .id;

    for (raw, expected) in [
        ("builtin", TemplateOrigin::Builtin),
        ("catalog", TemplateOrigin::Catalog),
        ("user", TemplateOrigin::User),
    ] {
        sqlx::query("UPDATE dcs_templates SET origin = ? WHERE id = ?")
            .bind(raw)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let fetched = DcsTemplateRow::get(&pool, id).await.unwrap().unwrap();
        assert_eq!(fetched.origin, expected);
    }
}

/// Inserts one built-in template and returns its id, for tests further down the FK chain.
async fn seed_template(pool: &sqlx::SqlitePool) -> i64 {
    let template = built_in_templates().remove(0);
    DcsTemplateRow::insert(pool, &template, TemplateOrigin::Builtin, Utc::now())
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

#[tokio::test]
async fn dcs_template_delete_removes_an_unreferenced_template() {
    let pool = connect_in_memory().await.unwrap();
    let template_id = seed_template(&pool).await;

    let deleted = DcsTemplateRow::delete(&pool, template_id).await.unwrap();
    assert!(deleted, "delete of an existing, unreferenced row");
    assert!(
        DcsTemplateRow::get(&pool, template_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn dcs_template_delete_returns_false_for_a_nonexistent_id() {
    let pool = connect_in_memory().await.unwrap();
    let deleted = DcsTemplateRow::delete(&pool, 999).await.unwrap();
    assert!(!deleted, "deleting a nonexistent id is not an error");
}

#[tokio::test]
async fn dcs_template_delete_fails_with_template_in_use_when_referenced_by_a_loop() {
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

    let err = DcsTemplateRow::delete(&pool, template_id)
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::TemplateInUse { id } if id == template_id));

    // The row must still be there -- the rejected delete must not have partially applied.
    assert!(
        DcsTemplateRow::get(&pool, template_id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn dcs_template_delete_reports_a_non_database_error_as_query_not_template_in_use() {
    let pool = connect_in_memory().await.unwrap();
    let template_id = seed_template(&pool).await;

    // Closing the pool first forces `execute` to fail with `sqlx::Error::PoolClosed` --
    // not a `Database` error -- proving `delete`'s classification correctly falls through
    // to `DbError::Query` rather than misreporting every failure as `TemplateInUse`.
    pool.close().await;

    let err = DcsTemplateRow::delete(&pool, template_id)
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::Query(_)));
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

/// Inserts a `tune_runs` row attempted-and-failed before any driver I/O happened (all
/// initial-reading columns left `NULL`), proving the schema supports the "auditable failure"
/// case that motivated making those columns nullable in the first place.
async fn seed_failed_run(pool: &sqlx::SqlitePool, loop_id: Option<i64>) -> i64 {
    sqlx::query(
        r#"
        INSERT INTO tune_runs (
            loop_id, loop_name, driver, started_at, outcome, failure_reason,
            process_type, controller_type, relay_amp_percent, num_cycles_skip,
            num_cycles_count, noise_protection_secs, mrft_delay_secs,
            template_name, template_origin, template_snapshot_json, tags_json, created_at
        ) VALUES (?, 'LIC101', 'opcda', ?, 'failed', 'InvalidCastException reading initial values',
                  'flow', 'pi', 5.0, 1, 2, 3, 0, 'Test Template', 'builtin', '{}', '{}', ?)
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
async fn tune_run_rejects_invalid_outcome_and_driver() {
    let pool = connect_in_memory().await.unwrap();
    let loop_id = seed_loop(&pool).await;

    let bad_outcome = sqlx::query(
        r#"
        INSERT INTO tune_runs (
            loop_id, loop_name, driver, started_at, outcome,
            process_type, controller_type, relay_amp_percent, num_cycles_skip,
            num_cycles_count, noise_protection_secs, mrft_delay_secs,
            template_name, template_origin, template_snapshot_json, tags_json, created_at
        ) VALUES (?, 'LIC101', 'opcda', ?, 'not_a_real_outcome', 'flow', 'pi', 5.0, 1, 2, 3, 0,
                  'Test Template', 'builtin', '{}', '{}', ?)
        "#,
    )
    .bind(loop_id)
    .bind(Utc::now())
    .bind(Utc::now())
    .execute(&pool)
    .await;
    assert!(bad_outcome.is_err());

    let bad_driver = sqlx::query(
        r#"
        INSERT INTO tune_runs (
            loop_id, loop_name, driver, started_at, outcome,
            process_type, controller_type, relay_amp_percent, num_cycles_skip,
            num_cycles_count, noise_protection_secs, mrft_delay_secs,
            template_name, template_origin, template_snapshot_json, tags_json, created_at
        ) VALUES (?, 'LIC101', 'modbus', ?, 'running', 'flow', 'pi', 5.0, 1, 2, 3, 0,
                  'Test Template', 'builtin', '{}', '{}', ?)
        "#,
    )
    .bind(loop_id)
    .bind(Utc::now())
    .bind(Utc::now())
    .execute(&pool)
    .await;
    assert!(
        bad_driver.is_err(),
        "driver outside the current roadmap must be rejected"
    );
}

/// Covers the three `CHECK` constraints `safety-run-snapshot` added to `tune_runs`: a
/// `template_origin` outside the three known values, and invalid JSON in either of the two
/// new JSON blob columns. Mirrors `loops_reject_invalid_json_and_invalid_enum_values` above,
/// which covers the same shape of constraint on `loops.tags_json`/`process_type`.
#[tokio::test]
async fn tune_runs_reject_invalid_template_origin_and_invalid_json() {
    let pool = connect_in_memory().await.unwrap();
    let loop_id = seed_loop(&pool).await;

    let insert = |template_origin: &'static str,
                  template_snapshot_json: &'static str,
                  tags_json: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                r#"
                INSERT INTO tune_runs (
                    loop_id, loop_name, driver, started_at, outcome,
                    process_type, controller_type, relay_amp_percent, num_cycles_skip,
                    num_cycles_count, noise_protection_secs, mrft_delay_secs,
                    template_name, template_origin, template_snapshot_json, tags_json, created_at
                ) VALUES (?, 'LIC101', 'opcda', ?, 'running', 'flow', 'pi', 5.0, 1, 2, 3, 0,
                          'Test Template', ?, ?, ?, ?)
                "#,
            )
            .bind(loop_id)
            .bind(Utc::now())
            .bind(template_origin)
            .bind(template_snapshot_json)
            .bind(tags_json)
            .bind(Utc::now())
            .execute(&pool)
            .await
        }
    };

    assert!(
        insert("not_a_real_origin", "{}", "{}").await.is_err(),
        "template_origin outside builtin/catalog/user must be rejected"
    );
    assert!(
        insert("builtin", "not valid json", "{}").await.is_err(),
        "invalid JSON in template_snapshot_json must be rejected"
    );
    assert!(
        insert("builtin", "{}", "not valid json").await.is_err(),
        "invalid JSON in tags_json must be rejected"
    );
    assert!(
        insert("builtin", "{}", "{}").await.is_ok(),
        "the same statement with valid values must succeed"
    );
}

/// Covers `db-run-request-snapshot`'s `request_json` `CHECK (json_valid(request_json))`
/// constraint, and confirms the column's `DEFAULT '{}'` lets a plain `INSERT` that omits it
/// (exactly what `TuneRunRow::start`'s own `INSERT` does) succeed unchanged.
#[tokio::test]
async fn tune_runs_default_and_reject_invalid_request_json() {
    let pool = connect_in_memory().await.unwrap();
    let loop_id = seed_loop(&pool).await;

    let omitted = sqlx::query(
        r#"
        INSERT INTO tune_runs (
            loop_id, loop_name, driver, started_at, outcome,
            process_type, controller_type, relay_amp_percent, num_cycles_skip,
            num_cycles_count, noise_protection_secs, mrft_delay_secs,
            template_name, template_origin, template_snapshot_json, tags_json, created_at
        ) VALUES (?, 'LIC101', 'opcda', ?, 'running', 'flow', 'pi', 5.0, 1, 2, 3, 0,
                  'Test Template', 'builtin', '{}', '{}', ?)
        RETURNING request_json
        "#,
    )
    .bind(loop_id)
    .bind(Utc::now())
    .bind(Utc::now())
    .fetch_one(&pool)
    .await
    .unwrap();
    let default_request_json: String = omitted.try_get("request_json").unwrap();
    assert_eq!(
        default_request_json, "{}",
        "an INSERT that never mentions request_json must default to an empty JSON object"
    );

    let insert_with_request_json = |request_json: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                r#"
                INSERT INTO tune_runs (
                    loop_id, loop_name, driver, started_at, outcome,
                    process_type, controller_type, relay_amp_percent, num_cycles_skip,
                    num_cycles_count, noise_protection_secs, mrft_delay_secs,
                    template_name, template_origin, template_snapshot_json, tags_json,
                    request_json, created_at
                ) VALUES (?, 'LIC101', 'opcda', ?, 'running', 'flow', 'pi', 5.0, 1, 2, 3, 0,
                          'Test Template', 'builtin', '{}', '{}', ?, ?)
                "#,
            )
            .bind(loop_id)
            .bind(Utc::now())
            .bind(request_json)
            .bind(Utc::now())
            .execute(&pool)
            .await
        }
    };

    assert!(
        insert_with_request_json("not valid json").await.is_err(),
        "invalid JSON in request_json must be rejected"
    );
    assert!(
        insert_with_request_json(r#"{"tagname":"LIC101"}"#)
            .await
            .is_ok(),
        "a well-formed JSON object must be accepted"
    );
}

/// Covers the `CHECK` constraints `safety-quality` added: `tune_runs.allow_uncertain_quality`
/// must be `0` or `1`, and `tune_samples.pv_quality` must be one of `good`/`uncertain`/`bad`.
#[tokio::test]
async fn tune_runs_and_tune_samples_reject_invalid_quality_columns() {
    let pool = connect_in_memory().await.unwrap();
    let loop_id = seed_loop(&pool).await;

    let insert_run = |allow_uncertain_quality: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                r#"
                INSERT INTO tune_runs (
                    loop_id, loop_name, driver, started_at, outcome,
                    process_type, controller_type, relay_amp_percent, num_cycles_skip,
                    num_cycles_count, noise_protection_secs, mrft_delay_secs,
                    template_name, template_origin, template_snapshot_json, tags_json,
                    allow_uncertain_quality, created_at
                ) VALUES (?, 'LIC101', 'opcda', ?, 'running', 'flow', 'pi', 5.0, 1, 2, 3, 0,
                          'Test Template', 'builtin', '{}', '{}', ?, ?)
                "#,
            )
            .bind(loop_id)
            .bind(Utc::now())
            .bind(allow_uncertain_quality)
            .bind(Utc::now())
            .execute(&pool)
            .await
        }
    };
    assert!(
        insert_run(2).await.is_err(),
        "allow_uncertain_quality outside 0/1 must be rejected"
    );
    assert!(insert_run(0).await.is_ok());
    assert!(insert_run(1).await.is_ok());

    let run_id = seed_failed_run(&pool, Some(loop_id)).await;
    let insert_sample = |pv_quality: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                r#"
                INSERT INTO tune_samples (
                    run_id, tick, time, pv, pv_quality, hysteresis, mv_value_current,
                    mv_sign_next_step, counter_all_switches, cycles_completed, cycles_remaining
                ) VALUES (?, 0, ?, 50.0, ?, 1.0, 55.0, 1, 0, 0, 2)
                "#,
            )
            .bind(run_id)
            .bind(Utc::now())
            .bind(pv_quality)
            .execute(&pool)
            .await
        }
    };
    assert!(
        insert_sample("stale").await.is_err(),
        "pv_quality outside good/uncertain/bad must be rejected"
    );
    assert!(insert_sample("good").await.is_ok());
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
                run_id, tick, time, pv, pv_quality, hysteresis, mv_value_current,
                mv_sign_next_step, counter_all_switches, cycles_completed, cycles_remaining
            ) VALUES (?, ?, ?, 50.0, 'good', 1.0, 55.0, 1, 0, 0, 2)
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
            run_id, tick, time, pv, pv_quality, hysteresis, mv_value_current,
            mv_sign_next_step, counter_all_switches, cycles_completed, cycles_remaining
        ) VALUES (?, 0, ?, 50.0, 'good', 1.0, 55.0, 1, 0, 0, 2)
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
            run_id, response_level, written_at, kind, proportional_written, integral_written,
            derivative_written, proportional_readback, integral_readback, derivative_readback,
            success
        ) VALUES (?, 'moderate', ?, 'write', 3.0, 4.0, 0.0, 3.0, 4.0, 0.0, 1)
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
            run_id, response_level, written_at, kind, proportional_written, integral_written,
            derivative_written, success, error_message
        ) VALUES (?, 'aggressive', ?, 'write', 1.0, 2.0, 0.0, 0, 'write rejected by DCS')
        "#,
    )
    .bind(run_id)
    .bind(Utc::now())
    .execute(&pool)
    .await
    .unwrap();

    // A revert, undoing an earlier write -- same shape, `kind = 'revert'`.
    sqlx::query(
        r#"
        INSERT INTO tune_writes (
            run_id, response_level, written_at, kind, proportional_written, integral_written,
            derivative_written, proportional_readback, integral_readback, derivative_readback,
            success
        ) VALUES (?, 'moderate', ?, 'revert', 1.0, 2.0, 0.0, 1.0, 2.0, 0.0, 1)
        "#,
    )
    .bind(run_id)
    .bind(Utc::now())
    .execute(&pool)
    .await
    .unwrap();

    let bad_success = sqlx::query(
        r#"
        INSERT INTO tune_writes (run_id, response_level, written_at, kind, proportional_written, integral_written, derivative_written, success)
        VALUES (?, 'sluggish', ?, 'write', 1.0, 2.0, 0.0, 2)
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

    let bad_kind = sqlx::query(
        r#"
        INSERT INTO tune_writes (run_id, response_level, written_at, kind, proportional_written, integral_written, derivative_written, success)
        VALUES (?, 'sluggish', ?, 'undo', 1.0, 2.0, 0.0, 1)
        "#,
    )
    .bind(run_id)
    .bind(Utc::now())
    .execute(&pool)
    .await;
    assert!(
        bad_kind.is_err(),
        "kind must be constrained to 'write' or 'revert'"
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
