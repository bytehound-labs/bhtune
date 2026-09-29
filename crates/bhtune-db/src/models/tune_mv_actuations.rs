use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::{
    convert::{enum_to_text, option_enum_text, text_to_enum},
    error::{DbError, DbResult},
};

use super::tune_samples::SampleQuality;

#[cfg(doc)]
use super::tune_samples::TuneSampleRow;

// tune_mv_actuations {{{1

/// The physical purpose of one accepted manipulated-variable command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MvActuationKind {
    /// An MRFT relay step, including the engine's final snapback to the initial MV.
    Relay,
    /// The authoritative post-run restore write to the original MV.
    Restore,
}

/// Lifecycle state of one accepted manipulated-variable command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MvActuationStatus {
    /// The write was accepted, but the live MV has not yet produced terminal evidence.
    Pending,
    /// A live readback matched `target_mv` within the recorded `tolerance`.
    Confirmed,
    /// A finite, acceptable-quality live readback missed the target/tolerance requirement.
    Failed,
    /// The run ended before the command could be conclusively checked (for example, a
    /// cancellation, driver error, or operation timeout).
    Unverified,
    /// A later authoritative command deliberately replaced responsibility for confirming
    /// this command, such as the restore write taking over from the engine's final snapback.
    Superseded,
}

impl MvActuationStatus {
    fn ensure_terminal(self) -> DbResult<()> {
        if self == MvActuationStatus::Pending {
            Err(DbError::InvalidMvActuationFinalStatus)
        } else {
            Ok(())
        }
    }
}

/// Input for [`TuneMvActuationRow::insert_pending`].
///
/// `commanded_at` is the time the driver accepted the write, while
/// `confirmation_due_at` is the exact deadline selected by the actuation policy for this
/// command. Persisting both that deadline and `tolerance` makes historical evidence
/// interpretable even if the policy changes in a later release.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTuneMvActuation {
    pub sequence: i64,
    pub kind: MvActuationKind,
    pub commanded_at: DateTime<Utc>,
    pub target_mv: f32,
    /// The immediately preceding commanded value used to derive the relay-step-aware
    /// tolerance. `None` for the first command in a run and valid for either command kind.
    pub previous_commanded_mv: Option<f32>,
    pub tolerance: f32,
    pub confirmation_due_at: DateTime<Utc>,
}

/// One row of `tune_mv_actuations`: durable evidence for an accepted OPC DA MV write.
///
/// These rows are separate from [`TuneSampleRow`], whose MV remains the engine's commanded
/// series for trend/export compatibility. `readback_mv` and `readback_quality` are the most
/// recent physical observation made by the verifier; `attempt_count` counts every persisted
/// observation, including attempts where no numeric value or trustworthy quality could be
/// obtained.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneMvActuationRow {
    pub id: i64,
    pub run_id: i64,
    /// Monotonic command order within one run. Unique together with `run_id`.
    pub sequence: i64,
    pub kind: MvActuationKind,
    /// UTC projection of the instant at which the driver accepted the MV write.
    pub commanded_at: DateTime<Utc>,
    pub target_mv: f32,
    pub previous_commanded_mv: Option<f32>,
    /// Exact absolute target/readback tolerance applied to this command.
    pub tolerance: f32,
    /// Exact time at which the command must have terminal evidence under the active policy.
    pub confirmation_due_at: DateTime<Utc>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub readback_mv: Option<f32>,
    pub readback_quality: Option<SampleQuality>,
    pub attempt_count: i64,
    pub status: MvActuationStatus,
    /// Operator-facing explanation, primarily for failed/unverified/superseded rows.
    pub detail: Option<String>,
}

impl TuneMvActuationRow {
    /// Inserts one accepted command in [`MvActuationStatus::Pending`] state. Observation
    /// fields are empty and `attempt_count` is zero until [`Self::record_observation`] or
    /// [`Self::record_final_observation`] is called.
    pub async fn insert_pending(
        pool: &SqlitePool,
        run_id: i64,
        new: NewTuneMvActuation,
    ) -> DbResult<TuneMvActuationRow> {
        let row = sqlx::query(
            r"
            INSERT INTO tune_mv_actuations (
                run_id, sequence, kind, commanded_at, target_mv, previous_commanded_mv,
                tolerance, confirmation_due_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *
            ",
        )
        .bind(run_id)
        .bind(new.sequence)
        .bind(enum_to_text(&new.kind)?)
        .bind(new.commanded_at)
        .bind(new.target_mv)
        .bind(new.previous_commanded_mv)
        .bind(new.tolerance)
        .bind(new.confirmation_due_at)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_mv_actuation(row)
    }

    /// Records the latest verification attempt while leaving the command pending. Both
    /// observation values are optional so a caller can still persist that an attempted
    /// check produced no usable numeric value and/or no quality classification.
    pub async fn record_observation(
        pool: &SqlitePool,
        id: i64,
        checked_at: DateTime<Utc>,
        readback_mv: Option<f32>,
        readback_quality: Option<SampleQuality>,
    ) -> DbResult<TuneMvActuationRow> {
        let row = sqlx::query(
            r"
            UPDATE tune_mv_actuations
            SET last_checked_at = ?, readback_mv = ?, readback_quality = ?,
                attempt_count = attempt_count + 1
            WHERE id = ? AND status = 'pending'
            RETURNING *
            ",
        )
        .bind(checked_at)
        .bind(readback_mv)
        .bind(option_enum_text(readback_quality.as_ref())?)
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_mv_actuation(row)
    }

    /// Atomically records the final observation and terminal status. This is preferable to
    /// separate [`Self::record_observation`] and [`Self::finalize`] calls when one read
    /// conclusively confirms or rejects the target, because readers can never observe that
    /// final evidence while the row still misleadingly says `pending`.
    pub async fn record_final_observation(
        pool: &SqlitePool,
        id: i64,
        checked_at: DateTime<Utc>,
        readback_mv: Option<f32>,
        readback_quality: Option<SampleQuality>,
        status: MvActuationStatus,
        detail: Option<&str>,
    ) -> DbResult<TuneMvActuationRow> {
        status.ensure_terminal()?;
        let row = sqlx::query(
            r"
            UPDATE tune_mv_actuations
            SET last_checked_at = ?, readback_mv = ?, readback_quality = ?,
                attempt_count = attempt_count + 1, status = ?, detail = ?
            WHERE id = ? AND status = 'pending'
            RETURNING *
            ",
        )
        .bind(checked_at)
        .bind(readback_mv)
        .bind(option_enum_text(readback_quality.as_ref())?)
        .bind(enum_to_text(&status)?)
        .bind(detail)
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_mv_actuation(row)
    }

    /// Finalizes a pending command without inventing an observation. Used for terminal paths
    /// such as interruption (`Unverified`) or deliberate handoff (`Superseded`).
    pub async fn finalize(
        pool: &SqlitePool,
        id: i64,
        status: MvActuationStatus,
        detail: Option<&str>,
    ) -> DbResult<TuneMvActuationRow> {
        status.ensure_terminal()?;
        let row = sqlx::query(
            r"
            UPDATE tune_mv_actuations
            SET status = ?, detail = ?
            WHERE id = ? AND status = 'pending'
            RETURNING *
            ",
        )
        .bind(enum_to_text(&status)?)
        .bind(detail)
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_mv_actuation(row)
    }

    /// Finalizes every still-pending command belonging to `run_id`, returning the number of
    /// rows changed. Already-terminal rows and rows for other runs are never modified. This
    /// is the terminal-path backstop that prevents a completed/failed/aborted run from
    /// retaining misleading `pending` audit records.
    pub async fn finalize_pending_for_run(
        pool: &SqlitePool,
        run_id: i64,
        status: MvActuationStatus,
        detail: Option<&str>,
    ) -> DbResult<u64> {
        status.ensure_terminal()?;
        let result = sqlx::query(
            r"
            UPDATE tune_mv_actuations
            SET status = ?, detail = ?
            WHERE run_id = ? AND status = 'pending'
            ",
        )
        .bind(enum_to_text(&status)?)
        .bind(detail)
        .bind(run_id)
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
        Ok(result.rows_affected())
    }

    /// Lists every accepted MV command for `run_id` in command-sequence order.
    pub async fn list_for_run(pool: &SqlitePool, run_id: i64) -> DbResult<Vec<TuneMvActuationRow>> {
        let rows =
            sqlx::query("SELECT * FROM tune_mv_actuations WHERE run_id = ? ORDER BY sequence, id")
                .bind(run_id)
                .fetch_all(pool)
                .await
                .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_tune_mv_actuation).collect()
    }
}

fn row_to_tune_mv_actuation(row: SqliteRow) -> DbResult<TuneMvActuationRow> {
    let kind: String = row.try_get("kind").map_err(DbError::Query)?;
    let readback_quality: Option<String> =
        row.try_get("readback_quality").map_err(DbError::Query)?;
    let status: String = row.try_get("status").map_err(DbError::Query)?;
    Ok(TuneMvActuationRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        run_id: row.try_get("run_id").map_err(DbError::Query)?,
        sequence: row.try_get("sequence").map_err(DbError::Query)?,
        kind: text_to_enum("kind", &kind)?,
        commanded_at: row.try_get("commanded_at").map_err(DbError::Query)?,
        target_mv: row.try_get("target_mv").map_err(DbError::Query)?,
        previous_commanded_mv: row
            .try_get("previous_commanded_mv")
            .map_err(DbError::Query)?,
        tolerance: row.try_get("tolerance").map_err(DbError::Query)?,
        confirmation_due_at: row.try_get("confirmation_due_at").map_err(DbError::Query)?,
        last_checked_at: row.try_get("last_checked_at").map_err(DbError::Query)?,
        readback_mv: row.try_get("readback_mv").map_err(DbError::Query)?,
        readback_quality: readback_quality
            .map(|quality| text_to_enum("readback_quality", &quality))
            .transpose()?,
        attempt_count: row.try_get("attempt_count").map_err(DbError::Query)?,
        status: text_to_enum("status", &status)?,
        detail: row.try_get("detail").map_err(DbError::Query)?,
    })
}
// }}}1
