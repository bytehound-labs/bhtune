use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::error::{DbError, DbResult};

// demo_sessions {{{1

/// Failure reason persisted when startup recovery terminates an owned demo run that was still
/// marked as running.
pub const DEMO_RESTART_INTERRUPTED_REASON: &str = "demo run was interrupted by a server restart";

/// A short-lived anonymous owner for simulator-only public demo runs. The raw bearer token is
/// never stored; callers pass its lowercase hexadecimal SHA-256 hash to the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoSessionRow {
    pub id: i64,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl DemoSessionRow {
    /// Persists a session on first use, or returns the already-persisted valid row when another
    /// request won the same token-hash race.
    pub async fn create(
        pool: &SqlitePool,
        token_hash: &str,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> DbResult<Self> {
        Self::get_or_create(pool, token_hash, now, expires_at).await
    }

    /// Lazily persists a token hash on first use. Concurrent callers presenting the same token
    /// converge on one row: the unique-token loser reloads the winner's valid row rather than
    /// surfacing a uniqueness error.
    ///
    /// An expired or revoked conflicting row remains authoritative and is returned as
    /// `RowNotFound`; it is never revived and its fixed expiry is never extended.
    pub async fn get_or_create(
        pool: &SqlitePool,
        token_hash: &str,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> DbResult<Self> {
        let inserted = sqlx::query(
            "INSERT INTO demo_sessions (token_hash, created_at, last_seen_at, expires_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(token_hash) DO NOTHING \
             RETURNING *",
        )
        .bind(token_hash)
        .bind(now)
        .bind(now)
        .bind(expires_at)
        .fetch_optional(pool)
        .await
        .map_err(DbError::Query)?;

        match inserted {
            Some(row) => row_to_demo_session(row),
            None => Self::get_by_token_hash(pool, token_hash, now)
                .await?
                .ok_or_else(|| DbError::Query(sqlx::Error::RowNotFound)),
        }
    }

    /// Looks up an authorization session and atomically records its latest activity. Expired
    /// and revoked rows are deliberately indistinguishable from a missing row, and the fixed
    /// expiry is never extended.
    pub async fn get_by_token_hash(
        pool: &SqlitePool,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> DbResult<Option<Self>> {
        let row = sqlx::query(
            "UPDATE demo_sessions \
             SET last_seen_at = MAX(last_seen_at, ?) \
             WHERE token_hash = ? AND revoked_at IS NULL AND expires_at > ? \
             RETURNING *",
        )
        .bind(now)
        .bind(token_hash)
        .bind(now)
        .fetch_optional(pool)
        .await
        .map_err(DbError::Query)?;
        row.map(row_to_demo_session).transpose()
    }

    /// Records activity only while the session remains valid. `expires_at` is never changed:
    /// demo sessions have an absolute lifetime, not a sliding one.
    pub async fn touch_by_token_hash(
        pool: &SqlitePool,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> DbResult<Option<Self>> {
        Self::get_by_token_hash(pool, token_hash, now).await
    }

    pub async fn revoke(pool: &SqlitePool, id: i64, now: DateTime<Utc>) -> DbResult<bool> {
        let result = sqlx::query(
            "UPDATE demo_sessions SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(id)
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
        Ok(result.rows_affected() != 0)
    }

    /// Removes expired/revoked sessions and cascades their terminal owned history.
    ///
    /// A session owning a running run is protected even after expiry or revocation. Startup
    /// recovery must first mark that run failed via [`Self::recover_running_demo_runs`]; only a
    /// later cleanup can remove the session and its now-terminal history.
    pub async fn cleanup_expired(pool: &SqlitePool, now: DateTime<Utc>) -> DbResult<u64> {
        let result = sqlx::query(
            "DELETE FROM demo_sessions \
             WHERE (expires_at <= ? OR revoked_at IS NOT NULL) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM tune_runs \
                   WHERE tune_runs.demo_session_id = demo_sessions.id \
                     AND tune_runs.outcome = 'running' \
               )",
        )
        .bind(now)
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
        Ok(result.rows_affected())
    }

    /// Marks demo runs left in `running` state by a process restart as failed. Demo work is
    /// intentionally not resumed after a crash or restart.
    pub async fn recover_running_demo_runs(pool: &SqlitePool, now: DateTime<Utc>) -> DbResult<u64> {
        let result = sqlx::query(
            "UPDATE tune_runs SET outcome = 'failed', completed_at = ?, \
             failure_reason = ? \
             WHERE demo_session_id IS NOT NULL AND outcome = 'running'",
        )
        .bind(now)
        .bind(DEMO_RESTART_INTERRUPTED_REASON)
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
        Ok(result.rows_affected())
    }
}

fn row_to_demo_session(row: SqliteRow) -> DbResult<DemoSessionRow> {
    Ok(DemoSessionRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        token_hash: row.try_get("token_hash").map_err(DbError::Query)?,
        created_at: row.try_get("created_at").map_err(DbError::Query)?,
        last_seen_at: row.try_get("last_seen_at").map_err(DbError::Query)?,
        expires_at: row.try_get("expires_at").map_err(DbError::Query)?,
        revoked_at: row.try_get("revoked_at").map_err(DbError::Query)?,
    })
}
