use bhtune_core::{
    ResponseLevel,
    tuning_math::{
        CheckedTuningResult, PidParameters, TuningResult, TuningResultInvalidReason,
        TuningResultStatus,
    },
};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::{
    convert::{enum_to_text, option_enum_text, text_to_enum},
    error::{DbError, DbResult},
};

// tune_results {{{1

/// One row of `tune_results`: the calculated PID result for one [`ResponseLevel`] of one run.
///
/// Flattened rather than nesting [`TuningResult`]/[`PidParameters`] directly, since both of
/// those types carry their own `response_level` field — nesting both would mean either two
/// redundant copies that could disagree, or an awkward "just trust the outer one" rule. One
/// flat set of columns matching the table exactly avoids that.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TuneResultRow {
    pub id: i64,
    pub run_id: i64,
    pub response_level: ResponseLevel,
    pub kp: Option<f32>,
    pub ti_minutes: Option<f32>,
    pub td_minutes: Option<f32>,
    pub proportional: Option<f32>,
    pub integral: Option<f32>,
    pub derivative: Option<f32>,
    pub status: TuningResultStatus,
    pub invalid_reason: Option<TuningResultInvalidReason>,
}

impl TuneResultRow {
    /// Builds a (not-yet-inserted, `id = 0`) row from a matching [`TuningResult`]/
    /// [`PidParameters`] pair, as produced together by
    /// [`bhtune_core::tuning_math::calculate_tuning_result`]/
    /// [`bhtune_core::tuning_math::calculate_pid_parameters`] for the same [`ResponseLevel`].
    ///
    /// # Panics
    /// Panics if `tuning.response_level != pid.response_level`: pairing results for different
    /// response levels together is always a caller bug, never a runtime data problem, the
    /// same contract [`bhtune_core::tuning_math::measure_oscillation`] documents for its own
    /// caller-contract panics.
    pub fn from_calculated(run_id: i64, tuning: TuningResult, pid: PidParameters) -> TuneResultRow {
        assert_eq!(
            tuning.response_level, pid.response_level,
            "TuningResult and PidParameters must be for the same ResponseLevel"
        );
        TuneResultRow {
            id: 0,
            run_id,
            response_level: tuning.response_level,
            kp: Some(tuning.kp),
            ti_minutes: Some(tuning.ti_minutes),
            td_minutes: Some(tuning.td_minutes),
            proportional: Some(pid.proportional),
            integral: Some(pid.integral),
            derivative: Some(pid.derivative),
            status: TuningResultStatus::Valid,
            invalid_reason: None,
        }
    }

    /// Builds a row from the checked calculation path. Invalid results retain their response
    /// level and reason while leaving every numeric value absent, so they cannot be mistaken for
    /// writable PID constants.
    pub fn from_checked(run_id: i64, checked: CheckedTuningResult) -> TuneResultRow {
        let values = checked.usable_values();
        TuneResultRow {
            id: 0,
            run_id,
            response_level: checked.response_level,
            kp: values.map(|(tuning, _)| tuning.kp),
            ti_minutes: values.map(|(tuning, _)| tuning.ti_minutes),
            td_minutes: values.map(|(tuning, _)| tuning.td_minutes),
            proportional: values.map(|(_, pid)| pid.proportional),
            integral: values.map(|(_, pid)| pid.integral),
            derivative: values.map(|(_, pid)| pid.derivative),
            status: checked.status,
            invalid_reason: checked.invalid_reason,
        }
    }

    /// Inserts `row` (typically built via [`Self::from_calculated`]), returning the persisted
    /// copy with its assigned `id`. `(run_id, response_level)` is unique (see the migration)
    /// — a successfully completed run writes exactly the 3 [`ResponseLevel`] rows once, at
    /// completion.
    pub async fn insert(pool: &SqlitePool, row: &TuneResultRow) -> DbResult<TuneResultRow> {
        let inserted = sqlx::query(
            r"
            INSERT INTO tune_results (
                run_id, response_level, kp, ti_minutes, td_minutes,
                proportional, integral, derivative, status, invalid_reason
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *
            ",
        )
        .bind(row.run_id)
        .bind(enum_to_text(&row.response_level)?)
        .bind(row.kp)
        .bind(row.ti_minutes)
        .bind(row.td_minutes)
        .bind(row.proportional)
        .bind(row.integral)
        .bind(row.derivative)
        .bind(enum_to_text(&row.status)?)
        .bind(option_enum_text(row.invalid_reason.as_ref())?)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_result(inserted)
    }

    /// Lists every calculated result of `run_id`, ordered by [`ResponseLevel`] (which sorts
    /// alphabetically as Aggressive, Moderate, Sluggish — the same order
    /// [`bhtune_core::constants::ResponseLevel::ALL`] enumerates them in). 0 rows for a run
    /// that never completed, up to 3 for one that did.
    pub async fn list_for_run(pool: &SqlitePool, run_id: i64) -> DbResult<Vec<TuneResultRow>> {
        let rows =
            sqlx::query("SELECT * FROM tune_results WHERE run_id = ? ORDER BY response_level")
                .bind(run_id)
                .fetch_all(pool)
                .await
                .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_tune_result).collect()
    }
}

fn row_to_tune_result(row: SqliteRow) -> DbResult<TuneResultRow> {
    let response_level: String = row.try_get("response_level").map_err(DbError::Query)?;
    Ok(TuneResultRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        run_id: row.try_get("run_id").map_err(DbError::Query)?,
        response_level: text_to_enum("response_level", &response_level)?,
        kp: row.try_get("kp").map_err(DbError::Query)?,
        ti_minutes: row.try_get("ti_minutes").map_err(DbError::Query)?,
        td_minutes: row.try_get("td_minutes").map_err(DbError::Query)?,
        proportional: row.try_get("proportional").map_err(DbError::Query)?,
        integral: row.try_get("integral").map_err(DbError::Query)?,
        derivative: row.try_get("derivative").map_err(DbError::Query)?,
        status: {
            let status: String = row.try_get("status").map_err(DbError::Query)?;
            text_to_enum("status", &status)?
        },
        invalid_reason: {
            let reason: Option<String> = row.try_get("invalid_reason").map_err(DbError::Query)?;
            reason
                .as_deref()
                .map(|reason| text_to_enum("invalid_reason", reason))
                .transpose()?
        },
    })
}
// }}}1

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_calculated_builds_matching_row() {
        let tuning = TuningResult {
            response_level: ResponseLevel::Moderate,
            kp: 1.0,
            ti_minutes: 2.0,
            td_minutes: 0.0,
        };
        let pid = PidParameters {
            response_level: ResponseLevel::Moderate,
            proportional: 3.0,
            integral: 4.0,
            derivative: 0.0,
        };
        let row = TuneResultRow::from_calculated(42, tuning, pid);
        assert_eq!(row.run_id, 42);
        assert_eq!(row.response_level, ResponseLevel::Moderate);
        assert_eq!(row.kp, Some(1.0));
        assert_eq!(row.proportional, Some(3.0));
    }

    #[test]
    #[should_panic(expected = "same ResponseLevel")]
    fn from_calculated_panics_on_mismatched_response_level() {
        let tuning = TuningResult {
            response_level: ResponseLevel::Aggressive,
            kp: 1.0,
            ti_minutes: 2.0,
            td_minutes: 0.0,
        };
        let pid = PidParameters {
            response_level: ResponseLevel::Sluggish,
            proportional: 3.0,
            integral: 4.0,
            derivative: 0.0,
        };
        TuneResultRow::from_calculated(1, tuning, pid);
    }
}
