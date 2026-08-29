//! Database connection setup: WAL journal mode, a busy timeout (so concurrent CLI + GUI
//! access to the same file doesn't immediately error out with `SQLITE_BUSY`), and
//! foreign-key enforcement (off by default in SQLite, and required here since every child
//! table relies on `ON DELETE CASCADE`/`ON DELETE SET NULL`).

use std::{path::Path, time::Duration};

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

use crate::error::{DbError, DbResult};

/// How long a connection waits on a locked database before giving up (`SQLITE_BUSY`).
/// Generous enough to ride out a retention sweep or a large history export running
/// concurrently in another process, without silently hanging forever if something is
/// actually deadlocked.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// Opens (creating if missing) the SQLite database at `path`, applies the standard pragmas,
/// and runs any pending migrations. This is the only supported way to get a `SqlitePool` in
/// bhtune — every caller (CLI, web server, tests) goes through this function so the pragmas and
/// migration state can never drift between adapters.
pub async fn connect(path: &Path) -> DbResult<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(BUSY_TIMEOUT)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .map_err(DbError::Connect)?;

    run_migrations(&pool).await?;

    Ok(pool)
}

/// Opens a private, in-process database for tests: same pragmas and migrations as
/// [`connect`], but nothing touches disk.
///
/// Capped at one pooled connection deliberately — a pooled in-memory SQLite database is
/// private per-connection, so a second connection would see an *empty* database rather than
/// the same one. That cap is what makes "in-memory" behave like a single shared database
/// across a test. WAL itself requires a real file (SQLite silently falls back to its
/// in-memory journal mode here regardless of the request below), so WAL is only actually
/// exercised by [`connect`]'s own tests, which use a real temp file.
pub async fn connect_in_memory() -> DbResult<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(BUSY_TIMEOUT)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(DbError::Connect)?;

    run_migrations(&pool).await?;

    Ok(pool)
}

async fn run_migrations(pool: &SqlitePool) -> DbResult<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(DbError::Migrate)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn apply_migrations_through(pool: &SqlitePool, migration_count: usize) {
        sqlx::query(
            r#"
            CREATE TABLE _sqlx_migrations (
                version BIGINT PRIMARY KEY,
                description TEXT NOT NULL,
                installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                success BOOLEAN NOT NULL,
                checksum BLOB NOT NULL,
                execution_time BIGINT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await
        .unwrap();

        let migrations = sqlx::migrate!("./migrations");
        for migration in migrations.migrations.iter().take(migration_count) {
            sqlx::raw_sql(sqlx::AssertSqlSafe(migration.sql.as_str()))
                .execute(pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO _sqlx_migrations \
                 (version, description, success, checksum, execution_time) VALUES (?, ?, 1, ?, 0)",
            )
            .bind(migration.version)
            .bind(migration.description.as_ref())
            .bind(migration.checksum.as_ref())
            .execute(pool)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn connect_creates_file_and_applies_wal_and_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.db");

        let pool = connect(&path).await.unwrap();
        assert!(path.exists());

        let (journal_mode,): (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(journal_mode, "wal");

        let (foreign_keys,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(foreign_keys, 1);
    }

    #[tokio::test]
    async fn connect_is_idempotent_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.db");

        connect(&path).await.unwrap();
        // Reopening an already-migrated database (the normal "app restart" case) must not
        // fail or try to reapply migrations.
        let pool = connect(&path).await.unwrap();

        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tune_runs'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn connect_in_memory_runs_migrations() {
        let pool = connect_in_memory().await.unwrap();
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'dcs_templates'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn connect_upgrades_a_pre_index_database_and_backfills_write_quality_policy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("before-history-indexes.db");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();

        apply_migrations_through(&pool, 1).await;
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES ('migration-fixture', ?, ?)",
        )
        .bind(r#"{"preserve":true}"#)
        .bind(chrono::Utc::now())
        .execute(&pool)
        .await
        .unwrap();

        let started_at = chrono::Utc::now();
        for allow_uncertain_quality in [1_i64, 0] {
            sqlx::query(
                r#"
                INSERT INTO tune_runs (
                    loop_name, template_name, template_origin, template_snapshot_json,
                    tags_json, driver, started_at, outcome, process_type, controller_type,
                    relay_amp_percent, num_cycles_skip, num_cycles_count,
                    noise_protection_secs, mrft_delay_secs, allow_uncertain_quality, created_at
                ) VALUES ('migration-fixture', 'fixture', 'builtin', '{}', '{}', 'simulator',
                          ?, 'completed', 'flow', 'pi', 5.0, 1, 2, 3, 0, ?, ?)
                "#,
            )
            .bind(started_at)
            .bind(allow_uncertain_quality)
            .bind(started_at)
            .execute(&pool)
            .await
            .unwrap();
        }
        let run_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM tune_runs WHERE loop_name = 'migration-fixture' ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        for (run_id, response_level) in run_ids.iter().zip(["moderate", "aggressive"]) {
            sqlx::query(
                "INSERT INTO tune_writes (run_id, response_level, written_at, kind, success) VALUES (?, ?, ?, 'write', 1)",
            )
            .bind(run_id)
            .bind(response_level)
            .bind(started_at)
            .execute(&pool)
            .await
            .unwrap();
        }
        pool.close().await;

        let upgraded = connect(&path).await.unwrap();
        let (value,): (String,) =
            sqlx::query_as("SELECT value FROM settings WHERE key = 'migration-fixture'")
                .fetch_one(&upgraded)
                .await
                .unwrap();
        assert_eq!(value, r#"{"preserve":true}"#);

        let indexes: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_index_list('tune_samples')")
                .fetch_all(&upgraded)
                .await
                .unwrap();
        assert!(
            indexes
                .iter()
                .any(|name| name == "idx_tune_samples_run_time")
        );
        let indexes: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_index_list('tune_writes')")
                .fetch_all(&upgraded)
                .await
                .unwrap();
        assert!(
            indexes
                .iter()
                .any(|name| name == "idx_tune_writes_run_written")
        );

        let policies: Vec<i64> = sqlx::query_scalar(
            r#"
            SELECT tw.allow_uncertain_quality
            FROM tune_writes tw
            JOIN tune_runs tr ON tr.id = tw.run_id
            WHERE tr.loop_name = 'migration-fixture'
            ORDER BY tw.id
            "#,
        )
        .fetch_all(&upgraded)
        .await
        .unwrap();
        assert_eq!(
            policies,
            vec![1, 0],
            "legacy writes inherit the global true default unless their parent run explicitly disabled it"
        );

        let timing_metrics: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT timing_metrics_json FROM tune_runs WHERE loop_name = 'migration-fixture' ORDER BY id",
        )
        .fetch_all(&upgraded)
        .await
        .unwrap();
        assert_eq!(
            timing_metrics,
            vec![None, None],
            "pre-diagnostics runs must remain readable with no invented timing metrics"
        );
    }

    #[tokio::test]
    async fn connect_upgrades_a_migration_0004_database_with_mv_actuation_audit_support() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("before-mv-actuation-verification.db");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();

        apply_migrations_through(&pool, 4).await;
        let started_at = chrono::Utc::now();
        let run_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO tune_runs (
                loop_name, template_name, template_origin, template_snapshot_json,
                tags_json, driver, started_at, outcome, process_type, controller_type,
                relay_amp_percent, num_cycles_skip, num_cycles_count,
                noise_protection_secs, mrft_delay_secs, created_at
            ) VALUES (
                'migration-0004-fixture', 'fixture', 'builtin', '{}', '{}', 'opcda',
                ?, 'completed', 'flow', 'pi', 5.0, 1, 2, 3, 0, ?
            )
            RETURNING id
            "#,
        )
        .bind(started_at)
        .bind(started_at)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES ('migration-0004-fixture', ?, ?)",
        )
        .bind(r#"{"preserve":true}"#)
        .bind(started_at)
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        let upgraded = connect(&path).await.unwrap();
        let preserved: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'migration-0004-fixture'")
                .fetch_one(&upgraded)
                .await
                .unwrap();
        assert_eq!(preserved, r#"{"preserve":true}"#);
        let preserved_run: String =
            sqlx::query_scalar("SELECT outcome FROM tune_runs WHERE id = ?")
                .bind(run_id)
                .fetch_one(&upgraded)
                .await
                .unwrap();
        assert_eq!(
            preserved_run, "completed",
            "migration 0005 must not rewrite existing run outcomes"
        );

        let commanded_at = started_at + chrono::Duration::seconds(1);
        sqlx::query(
            r#"
            INSERT INTO tune_mv_actuations (
                run_id, sequence, kind, commanded_at, target_mv, previous_commanded_mv,
                tolerance, confirmation_due_at
            ) VALUES (?, 0, 'relay', ?, 55.0, 45.0, 0.1, ?)
            "#,
        )
        .bind(run_id)
        .bind(commanded_at)
        .bind(commanded_at + chrono::Duration::seconds(4))
        .execute(&upgraded)
        .await
        .unwrap();

        sqlx::query("DELETE FROM tune_runs WHERE id = ?")
            .bind(run_id)
            .execute(&upgraded)
            .await
            .unwrap();
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tune_mv_actuations WHERE run_id = ?")
                .bind(run_id)
                .fetch_one(&upgraded)
                .await
                .unwrap();
        assert_eq!(
            remaining, 0,
            "the new audit table must cascade-delete with an upgraded run"
        );
    }
}
