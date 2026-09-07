use bhtune_db::{connect, models::DemoSessionRow};
use chrono::Utc;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

#[tokio::test]
async fn migration_0008_upgrades_a_0007_database_without_losing_existing_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("before-demo.db");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE _sqlx_migrations (
            version BIGINT PRIMARY KEY, description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    for migration in sqlx::migrate!("./migrations").migrations.iter().take(7) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(migration.sql.as_str()))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO _sqlx_migrations
             (version, description, success, checksum, execution_time) VALUES (?, ?, 1, ?, 0)",
        )
        .bind(migration.version)
        .bind(migration.description.as_ref())
        .bind(migration.checksum.as_ref())
        .execute(&pool)
        .await
        .unwrap();
    }
    let now = Utc::now();
    let old_run: i64 = sqlx::query_scalar(
        "INSERT INTO tune_runs (
            loop_name, template_name, template_origin, template_snapshot_json, tags_json,
            driver, started_at, outcome, process_type, controller_type, relay_amp_percent,
            num_cycles_skip, num_cycles_count, noise_protection_secs, mrft_delay_secs, created_at
        ) VALUES ('legacy', 'fixture', 'builtin', '{}', '{}', 'simulator', ?,
            'completed', 'flow', 'pi', 5.0, 1, 2, 0, 0, ?) RETURNING id",
    )
    .bind(now)
    .bind(now)
    .fetch_one(&pool)
    .await
    .unwrap();
    pool.close().await;

    let upgraded = connect(&path).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tune_runs WHERE id = ?")
            .bind(old_run)
            .fetch_one(&upgraded)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'demo_sessions'",
        )
        .fetch_one(&upgraded)
        .await
        .unwrap(),
        1
    );
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('tune_runs')")
            .fetch_all(&upgraded)
            .await
            .unwrap();
    assert!(columns.iter().any(|name| name == "demo_session_id"));
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>("SELECT demo_session_id FROM tune_runs WHERE id = ?",)
            .bind(old_run)
            .fetch_one(&upgraded)
            .await
            .unwrap(),
        None
    );
    let indexes: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_index_list('tune_runs')")
            .fetch_all(&upgraded)
            .await
            .unwrap();
    assert!(
        indexes
            .iter()
            .any(|name| name == "idx_tune_runs_demo_session")
    );
    assert!(
        indexes
            .iter()
            .any(|name| name == "idx_tune_runs_demo_session_outcome")
    );
    let session_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('demo_sessions')")
            .fetch_all(&upgraded)
            .await
            .unwrap();
    assert!(!session_columns.iter().any(|name| name == "client_ip"));

    let session = DemoSessionRow::create(
        &upgraded,
        &"a".repeat(64),
        now,
        now + chrono::Duration::hours(1),
    )
    .await
    .unwrap();
    let error = sqlx::query("UPDATE tune_runs SET demo_session_id = ? WHERE id = ?")
        .bind(session.id)
        .bind(old_run)
        .execute(&upgraded)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("demo_session_id is immutable"));
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>("SELECT demo_session_id FROM tune_runs WHERE id = ?",)
            .bind(old_run)
            .fetch_one(&upgraded)
            .await
            .unwrap(),
        None
    );
}
