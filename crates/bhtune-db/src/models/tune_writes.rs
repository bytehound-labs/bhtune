use bhtune_core::ResponseLevel;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::{
    convert::{enum_to_text, option_enum_text, text_to_enum},
    error::{DbError, DbResult},
};

#[cfg(doc)]
use super::{tune_results::TuneResultRow, tune_runs::TuneRunRow};

// tune_writes {{{1

/// One row of `tune_writes`: an audit record of PID constants actually written back to the
/// DCS for one [`ResponseLevel`] of one run, distinct from what was merely *calculated*
/// ([`TuneResultRow`]). Flattened for the same reason as `TuneResultRow`.
///
/// `*_written`/`*_readback` are independently nullable (not all-or-nothing like `previous`)
/// because the operation writes and verifies P, then I, then D in sequence, stopping at the
/// first failure. A partial attempt leaves constants after the failure point at `None` rather
/// than 0, distinguishing "never attempted" from "attempted and confirmed zero".
#[derive(Debug, Clone, PartialEq)]
pub struct TuneWriteRow {
    pub id: i64,
    pub run_id: i64,
    pub response_level: ResponseLevel,
    pub written_at: DateTime<Utc>,
    /// Whether this row is a normal write-back or `bhtune history revert` undoing one. See
    /// [`WriteKind`].
    pub kind: WriteKind,
    /// Whether this operation allowed `Quality::Uncertain` readings. This is
    /// the explicit per-operation policy supplied at insertion time; it is
    /// independent of [`TuneRunRow::allow_uncertain_quality`].
    pub allow_uncertain_quality: bool,
    /// The P/I/D values read from the driver *before* any write was attempted. `None` only
    /// when the pre-read itself failed -- a hard stop before any write, so nothing else on
    /// this row was ever attempted either (`success = false`, every other field below `None`).
    pub previous: Option<WriteReadback>,
    pub proportional_written: Option<f32>,
    pub integral_written: Option<f32>,
    pub derivative_written: Option<f32>,
    /// Read back immediately after writing to confirm the DCS accepted the value within
    /// tolerance. `None` whenever the corresponding `*_written` field is `None`, or when the
    /// write was sent but the readback attempt itself failed.
    pub proportional_readback: Option<f32>,
    pub integral_readback: Option<f32>,
    pub derivative_readback: Option<f32>,
    pub success: bool,
    pub error_message: Option<String>,
    /// Set only when `success = false` and at least one constant had already been written
    /// before the failure, so a best-effort rollback to `previous` was attempted. `None`
    /// means rollback did not apply -- either every constant wrote successfully (`success =
    /// true`) or the pre-read failed before any write was attempted. Always `None` for a
    /// `kind = Revert` row.
    pub rollback_state: Option<RollbackState>,
    pub rollback_error: Option<String>,
}

/// A triple of proportional/integral/derivative values, read from the driver before any
/// write is attempted ([`TuneWriteRow::previous`]). Not a `bhtune-core` type like
/// `bhtune_core::tuning_math::OpcWriteValues`: this is a raw observation, not a
/// calculated/intended value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WriteReadback {
    pub proportional: f32,
    pub integral: f32,
    pub derivative: f32,
}

/// Whether a best-effort rollback of a partially-completed PID write was attempted and, if
/// so, whether it succeeded. See [`TuneWriteRow::rollback_state`] for when this is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RollbackState {
    /// Every constant that had been written was successfully written back to its `previous`
    /// value.
    Succeeded,
    /// At least one constant could not be written back to its `previous` value -- the loop
    /// may still hold a mismatched, partially-updated set of PID constants. See
    /// [`TuneWriteRow::rollback_error`] and `bhtune history revert` for recovering by hand.
    Failed,
}

/// Distinguishes a normal write-back from `bhtune history revert` undoing an earlier one.
/// Both share [`TuneWriteRow`]'s exact shape -- pre-read, write-and-verify each constant,
/// audit the outcome -- so they live in the same table rather than a second near-duplicate
/// one; `kind` is the one column that tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum WriteKind {
    /// A write-back of freshly calculated PID parameters (`maybe_write_back`).
    Write,
    /// `bhtune history revert` writing an earlier `Write` row's `previous` values back,
    /// undoing it. Never itself has a `rollback_state` -- a revert does not chain into a
    /// further rollback.
    Revert,
}

/// Everything needed to record one write-back attempt, successful or not. Built up by the
/// caller through the sequential pre-read / write-and-verify / rollback steps, then persisted
/// in a single [`TuneWriteRow::insert`] call so partial writes and rollback outcomes are
/// represented alongside complete success or pre-read failure.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTuneWrite {
    pub response_level: ResponseLevel,
    pub written_at: DateTime<Utc>,
    pub kind: WriteKind,
    pub allow_uncertain_quality: bool,
    pub previous: Option<WriteReadback>,
    pub proportional_written: Option<f32>,
    pub integral_written: Option<f32>,
    pub derivative_written: Option<f32>,
    pub proportional_readback: Option<f32>,
    pub integral_readback: Option<f32>,
    pub derivative_readback: Option<f32>,
    pub success: bool,
    pub error_message: Option<String>,
    pub rollback_state: Option<RollbackState>,
    pub rollback_error: Option<String>,
}

impl NewTuneWrite {
    /// Starts a record with every previous/written/readback/rollback field unset and
    /// `kind = WriteKind::Write`. New production write/revert operations must overwrite
    /// `allow_uncertain_quality` with the policy captured when that operation began; the
    /// permissive default keeps direct repository/test construction backward-compatible.
    pub fn new(response_level: ResponseLevel, written_at: DateTime<Utc>) -> Self {
        NewTuneWrite {
            response_level,
            written_at,
            kind: WriteKind::Write,
            allow_uncertain_quality: true,
            previous: None,
            proportional_written: None,
            integral_written: None,
            derivative_written: None,
            proportional_readback: None,
            integral_readback: None,
            derivative_readback: None,
            success: false,
            error_message: None,
            rollback_state: None,
            rollback_error: None,
        }
    }
}

impl TuneWriteRow {
    /// Records one write-back attempt exactly as `new` describes it -- see [`NewTuneWrite`].
    pub async fn insert(
        pool: &SqlitePool,
        run_id: i64,
        new: NewTuneWrite,
    ) -> DbResult<TuneWriteRow> {
        let row = sqlx::query(
            r"
            INSERT INTO tune_writes (
                run_id, response_level, written_at, kind,
                allow_uncertain_quality,
                proportional_previous, integral_previous, derivative_previous,
                proportional_written, integral_written, derivative_written,
                proportional_readback, integral_readback, derivative_readback,
                success, error_message, rollback_state, rollback_error
            ) VALUES (
                ?, ?, ?, ?,
                ?,
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
            )
            RETURNING *
            ",
        )
        .bind(run_id)
        .bind(enum_to_text(&new.response_level)?)
        .bind(new.written_at)
        .bind(enum_to_text(&new.kind)?)
        .bind(new.allow_uncertain_quality)
        .bind(new.previous.map(|p| p.proportional))
        .bind(new.previous.map(|p| p.integral))
        .bind(new.previous.map(|p| p.derivative))
        .bind(new.proportional_written)
        .bind(new.integral_written)
        .bind(new.derivative_written)
        .bind(new.proportional_readback)
        .bind(new.integral_readback)
        .bind(new.derivative_readback)
        .bind(new.success)
        .bind(new.error_message)
        .bind(option_enum_text(new.rollback_state.as_ref())?)
        .bind(new.rollback_error)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_write(row)
    }

    /// Lists every write-back attempt for `run_id`, oldest first — the full "who changed this
    /// loop and when" audit trail.
    pub async fn list_for_run(pool: &SqlitePool, run_id: i64) -> DbResult<Vec<TuneWriteRow>> {
        let rows = sqlx::query("SELECT * FROM tune_writes WHERE run_id = ? ORDER BY written_at")
            .bind(run_id)
            .fetch_all(pool)
            .await
            .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_tune_write).collect()
    }
}

fn row_to_tune_write(row: SqliteRow) -> DbResult<TuneWriteRow> {
    let response_level: String = row.try_get("response_level").map_err(DbError::Query)?;
    let kind: String = row.try_get("kind").map_err(DbError::Query)?;
    let proportional_previous: Option<f32> = row
        .try_get("proportional_previous")
        .map_err(DbError::Query)?;
    let integral_previous: Option<f32> =
        row.try_get("integral_previous").map_err(DbError::Query)?;
    let derivative_previous: Option<f32> =
        row.try_get("derivative_previous").map_err(DbError::Query)?;
    let previous = match (
        proportional_previous,
        integral_previous,
        derivative_previous,
    ) {
        (Some(proportional), Some(integral), Some(derivative)) => Some(WriteReadback {
            proportional,
            integral,
            derivative,
        }),
        _ => None,
    };
    let rollback_state: Option<String> = row.try_get("rollback_state").map_err(DbError::Query)?;
    Ok(TuneWriteRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        run_id: row.try_get("run_id").map_err(DbError::Query)?,
        response_level: text_to_enum("response_level", &response_level)?,
        written_at: row.try_get("written_at").map_err(DbError::Query)?,
        kind: text_to_enum("kind", &kind)?,
        allow_uncertain_quality: row
            .try_get("allow_uncertain_quality")
            .map_err(DbError::Query)?,
        previous,
        proportional_written: row
            .try_get("proportional_written")
            .map_err(DbError::Query)?,
        integral_written: row.try_get("integral_written").map_err(DbError::Query)?,
        derivative_written: row.try_get("derivative_written").map_err(DbError::Query)?,
        proportional_readback: row
            .try_get("proportional_readback")
            .map_err(DbError::Query)?,
        integral_readback: row.try_get("integral_readback").map_err(DbError::Query)?,
        derivative_readback: row.try_get("derivative_readback").map_err(DbError::Query)?,
        success: row.try_get("success").map_err(DbError::Query)?,
        error_message: row.try_get("error_message").map_err(DbError::Query)?,
        rollback_state: rollback_state
            .map(|s| text_to_enum("rollback_state", &s))
            .transpose()?,
        rollback_error: row.try_get("rollback_error").map_err(DbError::Query)?,
    })
}
// }}}1
