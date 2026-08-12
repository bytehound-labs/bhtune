//! Row types mirroring the tables in `migrations/0001_initial_schema.sql`.
//!
//! These are deliberately *typed*, not raw-column, shapes: wherever a table's columns are a
//! clean 1:1 match for an existing `bhtune-core` type (`DcsTemplate`, `LoopTags`,
//! `LoopConfig`, `Tick` + `MrftState`), the row struct holds that type directly rather than
//! re-declaring its fields — one less place for the two to drift apart. Where a table
//! combines fields from two `bhtune-core` types that both carry their own overlapping field
//! (`TuningResult`/`PidParameters` both carry `response_level`; `InitialReadings`/`PvRange`
//! don't nest cleanly with the extra `controller_direction` column), the row struct is flat
//! instead, matching the table exactly and avoiding a redundant, only-sometimes-consistent
//! duplicate field.
//!
//! Only [`DcsTemplateRow`] has `insert`/`get` methods so far — enough to unblock
//! `db-seed-templates`, which needs exactly this. Full repository behavior (filtering,
//! pagination, updates, deletes) for the other tables is intentionally deferred to the todos
//! that actually need it (`history-query-api`, `db-seed-templates`'s later loop-CRUD
//! follow-ups, `backend-opcda`/`cli-commands` for writing `tune_runs`/`tune_samples`/
//! `tune_results`/`tune_writes`), so the API shape gets decided by its real call sites
//! instead of being guessed at here.

use bhtune_core::{
    ControllerDirection, DcsTemplate, LoopConfig, LoopTags, MrftState, ResponseLevel, Tick,
    tuning_math::{PidParameters, TuningResult},
};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::{
    convert::{enum_to_text, text_to_enum},
    error::{DbError, DbResult},
};

// dcs_templates {{{1

/// One row of `dcs_templates`: a [`DcsTemplate`] plus the database bookkeeping fields that
/// don't belong on the pure domain type itself.
#[derive(Debug, Clone, PartialEq)]
pub struct DcsTemplateRow {
    pub id: i64,
    pub is_builtin: bool,
    pub template: DcsTemplate,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DcsTemplateRow {
    /// Inserts `template`, returning the persisted row (with its assigned `id`). `now` is
    /// used for both `created_at` and `updated_at`; the caller supplies it rather than this
    /// function reading the clock, keeping "who reads the clock" consistent with the rest of
    /// bhtune's architecture (see `bhtune_core`'s crate docs).
    pub async fn insert(
        pool: &SqlitePool,
        template: &DcsTemplate,
        is_builtin: bool,
        now: DateTime<Utc>,
    ) -> DbResult<DcsTemplateRow> {
        let row = sqlx::query(
            r#"
            INSERT INTO dcs_templates (
                name, is_builtin, revert_mode, proportional_type, integral_type,
                integral_unit, derivative_type, derivative_unit,
                process_variable_suffix, manipulated_variable_suffix, setpoint_variable_suffix,
                controller_direction_suffix, controller_mode_suffix, mode_attribute_suffix,
                upper_pv_range_suffix, lower_pv_range_suffix, upper_mv_range_suffix,
                lower_mv_range_suffix, proportional_constant_suffix, integral_constant_suffix,
                derivative_constant_suffix, mode_manual_value, mode_auto_value,
                mode_attribute_program_value, controller_action_direct_value,
                created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *
            "#,
        )
        .bind(&template.name)
        .bind(is_builtin)
        .bind(template.revert_mode)
        .bind(enum_to_text(&template.proportional_type))
        .bind(enum_to_text(&template.integral_type))
        .bind(enum_to_text(&template.integral_unit))
        .bind(enum_to_text(&template.derivative_type))
        .bind(enum_to_text(&template.derivative_unit))
        .bind(&template.process_variable_suffix)
        .bind(&template.manipulated_variable_suffix)
        .bind(&template.setpoint_variable_suffix)
        .bind(&template.controller_direction_suffix)
        .bind(&template.controller_mode_suffix)
        .bind(&template.mode_attribute_suffix)
        .bind(&template.upper_pv_range_suffix)
        .bind(&template.lower_pv_range_suffix)
        .bind(&template.upper_mv_range_suffix)
        .bind(&template.lower_mv_range_suffix)
        .bind(&template.proportional_constant_suffix)
        .bind(&template.integral_constant_suffix)
        .bind(&template.derivative_constant_suffix)
        .bind(&template.mode_manual_value)
        .bind(&template.mode_auto_value)
        .bind(&template.mode_attribute_program_value)
        .bind(&template.controller_action_direct_value)
        .bind(now)
        .bind(now)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_dcs_template(row)
    }

    /// Fetches one row by id, or `None` if it doesn't exist.
    pub async fn get(pool: &SqlitePool, id: i64) -> DbResult<Option<DcsTemplateRow>> {
        let row = sqlx::query("SELECT * FROM dcs_templates WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(DbError::Query)?;
        row.map(row_to_dcs_template).transpose()
    }

    /// Fetches one row by its (unique) `name`, or `None` if it doesn't exist. Used by
    /// [`crate::seed::seed_builtin_templates`] to find any existing row before deciding
    /// whether to insert or update.
    pub async fn get_by_name(pool: &SqlitePool, name: &str) -> DbResult<Option<DcsTemplateRow>> {
        let row = sqlx::query("SELECT * FROM dcs_templates WHERE name = ?")
            .bind(name)
            .fetch_optional(pool)
            .await
            .map_err(DbError::Query)?;
        row.map(row_to_dcs_template).transpose()
    }

    /// Lists every row, ordered by `name`, for the loop-editor's template picker.
    pub async fn list(pool: &SqlitePool) -> DbResult<Vec<DcsTemplateRow>> {
        let rows = sqlx::query("SELECT * FROM dcs_templates ORDER BY name")
            .fetch_all(pool)
            .await
            .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_dcs_template).collect()
    }

    /// Overwrites every template field of the row at `id` with `template`'s, bumping
    /// `updated_at` to `now`. Deliberately does not touch `name` (the match key callers
    /// already looked the row up by) or `is_builtin` (ownership of a row never changes after
    /// creation) — only [`Self::insert`] sets those.
    pub async fn update(
        pool: &SqlitePool,
        id: i64,
        template: &DcsTemplate,
        now: DateTime<Utc>,
    ) -> DbResult<DcsTemplateRow> {
        let row = sqlx::query(
            r#"
            UPDATE dcs_templates SET
                revert_mode = ?, proportional_type = ?, integral_type = ?,
                integral_unit = ?, derivative_type = ?, derivative_unit = ?,
                process_variable_suffix = ?, manipulated_variable_suffix = ?,
                setpoint_variable_suffix = ?, controller_direction_suffix = ?,
                controller_mode_suffix = ?, mode_attribute_suffix = ?,
                upper_pv_range_suffix = ?, lower_pv_range_suffix = ?,
                upper_mv_range_suffix = ?, lower_mv_range_suffix = ?,
                proportional_constant_suffix = ?, integral_constant_suffix = ?,
                derivative_constant_suffix = ?, mode_manual_value = ?, mode_auto_value = ?,
                mode_attribute_program_value = ?, controller_action_direct_value = ?,
                updated_at = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(template.revert_mode)
        .bind(enum_to_text(&template.proportional_type))
        .bind(enum_to_text(&template.integral_type))
        .bind(enum_to_text(&template.integral_unit))
        .bind(enum_to_text(&template.derivative_type))
        .bind(enum_to_text(&template.derivative_unit))
        .bind(&template.process_variable_suffix)
        .bind(&template.manipulated_variable_suffix)
        .bind(&template.setpoint_variable_suffix)
        .bind(&template.controller_direction_suffix)
        .bind(&template.controller_mode_suffix)
        .bind(&template.mode_attribute_suffix)
        .bind(&template.upper_pv_range_suffix)
        .bind(&template.lower_pv_range_suffix)
        .bind(&template.upper_mv_range_suffix)
        .bind(&template.lower_mv_range_suffix)
        .bind(&template.proportional_constant_suffix)
        .bind(&template.integral_constant_suffix)
        .bind(&template.derivative_constant_suffix)
        .bind(&template.mode_manual_value)
        .bind(&template.mode_auto_value)
        .bind(&template.mode_attribute_program_value)
        .bind(&template.controller_action_direct_value)
        .bind(now)
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_dcs_template(row)
    }
}

fn row_to_dcs_template(row: SqliteRow) -> DbResult<DcsTemplateRow> {
    let get_enum = |column: &'static str| -> DbResult<String> {
        row.try_get::<String, _>(column).map_err(DbError::Query)
    };

    let template = DcsTemplate {
        name: row.try_get("name").map_err(DbError::Query)?,
        revert_mode: row.try_get("revert_mode").map_err(DbError::Query)?,
        proportional_type: text_to_enum("proportional_type", &get_enum("proportional_type")?)?,
        integral_type: text_to_enum("integral_type", &get_enum("integral_type")?)?,
        integral_unit: text_to_enum("integral_unit", &get_enum("integral_unit")?)?,
        derivative_type: text_to_enum("derivative_type", &get_enum("derivative_type")?)?,
        derivative_unit: text_to_enum("derivative_unit", &get_enum("derivative_unit")?)?,
        process_variable_suffix: row
            .try_get("process_variable_suffix")
            .map_err(DbError::Query)?,
        manipulated_variable_suffix: row
            .try_get("manipulated_variable_suffix")
            .map_err(DbError::Query)?,
        setpoint_variable_suffix: row
            .try_get("setpoint_variable_suffix")
            .map_err(DbError::Query)?,
        controller_direction_suffix: row
            .try_get("controller_direction_suffix")
            .map_err(DbError::Query)?,
        controller_mode_suffix: row
            .try_get("controller_mode_suffix")
            .map_err(DbError::Query)?,
        mode_attribute_suffix: row
            .try_get("mode_attribute_suffix")
            .map_err(DbError::Query)?,
        upper_pv_range_suffix: row
            .try_get("upper_pv_range_suffix")
            .map_err(DbError::Query)?,
        lower_pv_range_suffix: row
            .try_get("lower_pv_range_suffix")
            .map_err(DbError::Query)?,
        upper_mv_range_suffix: row
            .try_get("upper_mv_range_suffix")
            .map_err(DbError::Query)?,
        lower_mv_range_suffix: row
            .try_get("lower_mv_range_suffix")
            .map_err(DbError::Query)?,
        proportional_constant_suffix: row
            .try_get("proportional_constant_suffix")
            .map_err(DbError::Query)?,
        integral_constant_suffix: row
            .try_get("integral_constant_suffix")
            .map_err(DbError::Query)?,
        derivative_constant_suffix: row
            .try_get("derivative_constant_suffix")
            .map_err(DbError::Query)?,
        mode_manual_value: row.try_get("mode_manual_value").map_err(DbError::Query)?,
        mode_auto_value: row.try_get("mode_auto_value").map_err(DbError::Query)?,
        mode_attribute_program_value: row
            .try_get("mode_attribute_program_value")
            .map_err(DbError::Query)?,
        controller_action_direct_value: row
            .try_get("controller_action_direct_value")
            .map_err(DbError::Query)?,
    };

    Ok(DcsTemplateRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        is_builtin: row.try_get("is_builtin").map_err(DbError::Query)?,
        template,
        created_at: row.try_get("created_at").map_err(DbError::Query)?,
        updated_at: row.try_get("updated_at").map_err(DbError::Query)?,
    })
}
// }}}1

// loops {{{1

/// One row of `loops`: a saved, named tag mapping plus default MRFT parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopRow {
    pub id: i64,
    pub name: String,
    pub dcs_template_id: i64,
    pub tags: LoopTags,
    pub config: LoopConfig,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
// }}}1

// tune_runs {{{1

/// Which [`crate`]-agnostic I/O backend a run used. Lives in `bhtune-db` rather than
/// `bhtune-core` because it's a persistence/orchestration concept (which adapter drove this
/// run), not a domain concept the pure MRFT engine itself needs to know about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TuneBackend {
    Opcda,
    Simulator,
    Replay,
}

/// A run's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TuneOutcome {
    Running,
    Completed,
    Failed,
    Aborted,
}

/// The initial-readings snapshot for a [`TuneRunRow`] — known only once the backend's initial
/// read actually succeeds (`ReadInitialOPCvalues` in the legacy app); `None` for a run that
/// failed before or during that step. Combines
/// [`bhtune_core::mrft::InitialReadings`]/[`bhtune_core::tuning_math::PvRange`] with the
/// resolved [`ControllerDirection`] `core-tuning-math` needs alongside them, as one bespoke
/// type, since gluing the two existing structs together with one extra field isn't any
/// simpler than a purpose-built one here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TuneRunInitialReadings {
    pub pv_ini: f32,
    pub mv_ini: f32,
    pub mv_range_low: f32,
    pub mv_range_high: f32,
    pub pv_range_high: f32,
    pub pv_range_low: f32,
    pub controller_direction: ControllerDirection,
}

/// One row of `tune_runs`: a single MRFT (or future Step Test) execution against a loop.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneRunRow {
    pub id: i64,
    pub loop_id: Option<i64>,
    pub loop_name: String,
    pub backend: TuneBackend,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub outcome: TuneOutcome,
    pub failure_reason: Option<String>,
    /// Snapshot of the `LoopConfig` this run was started with — always known up front, since
    /// it's user/schedule input rather than something read from the backend.
    pub config: LoopConfig,
    pub initial_readings: Option<TuneRunInitialReadings>,
    pub created_at: DateTime<Utc>,
}
// }}}1

// tune_samples {{{1

/// One row of `tune_samples`: a single tick's [`Tick`] input and resulting [`MrftState`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TuneSampleRow {
    pub id: i64,
    pub run_id: i64,
    /// 0-based sample sequence number within the run. Named `tick_index` rather than `tick`
    /// to avoid colliding with the unrelated [`Tick`] type held in `sample`.
    pub tick_index: i64,
    pub sample: Tick,
    pub state: MrftState,
}
// }}}1

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
    pub kp: f32,
    pub ti_minutes: f32,
    pub td_minutes: f32,
    pub proportional: f32,
    pub integral: f32,
    pub derivative: f32,
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
            kp: tuning.kp,
            ti_minutes: tuning.ti_minutes,
            td_minutes: tuning.td_minutes,
            proportional: pid.proportional,
            integral: pid.integral,
            derivative: pid.derivative,
        }
    }
}
// }}}1

// tune_writes {{{1

/// One row of `tune_writes`: an audit record of PID constants actually written back to the
/// DCS for one [`ResponseLevel`] of one run, distinct from what was merely *calculated*
/// ([`TuneResultRow`]). Flattened for the same reason as `TuneResultRow`.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneWriteRow {
    pub id: i64,
    pub run_id: i64,
    pub response_level: ResponseLevel,
    pub written_at: DateTime<Utc>,
    pub proportional_written: f32,
    pub integral_written: f32,
    pub derivative_written: f32,
    /// Read back immediately after writing to confirm the DCS accepted the value. `None`
    /// when `success` is `false` and the write never got far enough to read back.
    pub proportional_readback: Option<f32>,
    pub integral_readback: Option<f32>,
    pub derivative_readback: Option<f32>,
    pub success: bool,
    pub error_message: Option<String>,
}
// }}}1

// settings {{{1

/// One row of `settings`: an app-wide key/value pair (e.g. the `history-retention` policy).
#[derive(Debug, Clone, PartialEq)]
pub struct SettingRow {
    pub key: String,
    pub value: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}
// }}}1

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::{enum_to_text, text_to_enum};

    #[test]
    fn tune_backend_round_trips_and_matches_check_constraint() {
        let cases = [
            (TuneBackend::Opcda, "opcda"),
            (TuneBackend::Simulator, "simulator"),
            (TuneBackend::Replay, "replay"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant), text);
            assert_eq!(
                text_to_enum::<TuneBackend>("backend", text).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn tune_outcome_round_trips_and_matches_check_constraint() {
        let cases = [
            (TuneOutcome::Running, "running"),
            (TuneOutcome::Completed, "completed"),
            (TuneOutcome::Failed, "failed"),
            (TuneOutcome::Aborted, "aborted"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant), text);
            assert_eq!(
                text_to_enum::<TuneOutcome>("outcome", text).unwrap(),
                variant
            );
        }
    }

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
        assert_eq!(row.kp, 1.0);
        assert_eq!(row.proportional, 3.0);
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
