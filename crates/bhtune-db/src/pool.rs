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
/// bhtune — every caller (CLI, desktop, tests) goes through this function so the pragmas and
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
}
