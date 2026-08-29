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
//! [`DcsTemplateRow`] and [`TuneRunRow`]/[`TuneSampleRow`]/[`TuneResultRow`]/
//! [`TuneMvActuationRow`]/[`TuneWriteRow`] have full repository methods (insert, lifecycle
//! transitions, filtering, pagination) — covering `db-seed-templates` and
//! `history-query-api`. [`LoopRow`] deliberately has none yet: full CRUD for saved loops
//! (list/update/delete) is a separate "loop management" concern from history (which is about
//! *runs*, not the loops they reference), left to whichever future todo actually needs it.
//! Until then, tests construct `loops` rows with raw SQL (see `tests/schema.rs`'s `seed_loop`
//! helper) purely as foreign-key setup.
//!
//! [`TuneRunRow::list`]/[`TuneRunRow::count`] build their `WHERE` clause dynamically with
//! `sqlx::QueryBuilder`, since [`TuneRunFilter`]'s fields are all optional and the set of
//! active conditions varies per call — a fixed `query!` string can't express that, and
//! `bhtune-db` uses runtime `query`/`query_as` throughout anyway (see `Cargo.toml`), so this
//! doesn't introduce a new query style, just the first dynamic one.

use bhtune_core::{
    ControllerDirection, ControllerType, DcsTemplate, LoopConfig, LoopTags, MrftState, ProcessType,
    ResponseLevel, Tick,
    tuning_math::{PidParameters, TuningResult},
};
use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, sqlite::SqliteRow};

use crate::{
    convert::{enum_to_text, text_to_enum},
    error::{DbError, DbResult},
};

// dcs_templates {{{1

/// Where a `dcs_templates` row came from, and -- since [`TuneRunRow`] snapshots a copy of one
/// at [`TuneRunRow::start`] time -- where a run's snapshotted template came from too. Kept as
/// one definition reused by both tables (`dcs_templates.origin`, `tune_runs.template_origin`)
/// rather than two, so they can never drift on what the possible origins even are, and so a
/// run's history never needs to look the original row back up to know its provenance --
/// which matters precisely because that row might no longer exist, or might have been
/// re-imported under a different origin since.
///
/// `builtin` and `catalog` rows are re-upserted from their respective data files on every
/// startup ([`crate::seed::seed_templates`]); `user` rows (hand-imported via `bhtune template
/// import`, or created through a future GUI editor) are never auto-touched -- auto-reseeding
/// a hand-edited row would silently discard someone's own customization, while *not*
/// reseeding a shipped preset would mean a suffix/unit fix in a later release never reaches
/// existing installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TemplateOrigin {
    /// One of the templates `bhtune-core` ships embedded in its own binary (see
    /// `template-catalog`).
    Builtin,
    /// Loaded from a user-supplied catalog file (`$XDG_CONFIG_HOME/bhtune/templates.toml`
    /// and platform equivalents, or an explicit `--templates`/`BHTUNE_TEMPLATES` override --
    /// see `template-user-catalog`), auto-seeded on every startup the same way `Builtin`
    /// rows are.
    Catalog,
    /// Hand-imported (`bhtune template import`) or otherwise created by whoever is running
    /// bhtune.
    User,
}

/// One row of `dcs_templates`: a [`DcsTemplate`] plus the database bookkeeping fields that
/// don't belong on the pure domain type itself.
#[derive(Debug, Clone, PartialEq)]
pub struct DcsTemplateRow {
    pub id: i64,
    pub origin: TemplateOrigin,
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
        origin: TemplateOrigin,
        now: DateTime<Utc>,
    ) -> DbResult<DcsTemplateRow> {
        let versions_json = serde_json::to_string(&template.versions)
            .expect("Vec<String> serialization is infallible");
        let row = sqlx::query(
            r#"
            INSERT INTO dcs_templates (
                name, origin, revert_mode, proportional_type, integral_type,
                integral_unit, derivative_type, derivative_unit,
                process_variable_suffix, manipulated_variable_suffix, setpoint_variable_suffix,
                controller_direction_suffix, controller_mode_suffix, mode_attribute_suffix,
                upper_pv_range_suffix, lower_pv_range_suffix, upper_mv_range_suffix,
                lower_mv_range_suffix, proportional_constant_suffix, integral_constant_suffix,
                derivative_constant_suffix, mode_manual_value, mode_auto_value,
                mode_attribute_program_value, controller_action_direct_value,
                versions_json, description, source,
                created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *
            "#,
        )
        .bind(&template.name)
        .bind(enum_to_text(&origin))
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
        .bind(versions_json)
        .bind(&template.description)
        .bind(&template.source)
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
    /// [`crate::seed::seed_templates`] to find any existing row before deciding whether to
    /// insert or update.
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
    /// already looked the row up by) or `origin` (ownership of a row never changes after
    /// creation) — only [`Self::insert`] sets those.
    pub async fn update(
        pool: &SqlitePool,
        id: i64,
        template: &DcsTemplate,
        now: DateTime<Utc>,
    ) -> DbResult<DcsTemplateRow> {
        let versions_json = serde_json::to_string(&template.versions)
            .expect("Vec<String> serialization is infallible");
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
                versions_json = ?, description = ?, source = ?,
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
        .bind(versions_json)
        .bind(&template.description)
        .bind(&template.source)
        .bind(now)
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_dcs_template(row)
    }

    /// Deletes the row at `id`. Returns `Ok(true)` if a row existed and was removed,
    /// `Ok(false)` if no row with that id existed (not an error -- deciding whether "nothing
    /// to delete" should itself be an error is the caller's call; `bhtune-cli`'s `template
    /// delete` already resolves `id` from a name via [`Self::get_by_name`] and produces its
    /// own "no template named" error before ever calling this).
    ///
    /// Fails with [`DbError::TemplateInUse`] if `loops.dcs_template_id`'s `ON DELETE
    /// RESTRICT` foreign key rejects the delete because a saved loop still references this
    /// template. Classified as "any database-level error on this specific statement",
    /// rather than solely `sqlx`'s `DatabaseError::is_foreign_key_violation()` -- confirmed
    /// empirically that SQLite's C implementation reports an *immediate* `RESTRICT`
    /// violation like this one under the extended result code `SQLITE_CONSTRAINT_TRIGGER`
    /// (its FK-action enforcement runs through the same internal machinery as a trigger
    /// body), not `SQLITE_CONSTRAINT_FOREIGNKEY` -- the latter is what `sqlx-sqlite` maps to
    /// `is_foreign_key_violation()`, and it's only what a *deferred* FK check reports at
    /// commit time, which this crate never uses. This is safe to broaden to "any database
    /// error" specifically because `dcs_templates` has exactly one foreign key pointing at
    /// it (`loops.dcs_template_id`), no triggers exist anywhere in the schema, and a bare
    /// `DELETE` can't violate this table's own `CHECK` constraints (they all apply to
    /// column values, which a delete-by-id never touches) -- so a database-level failure of
    /// this exact statement structurally has only one possible cause.
    pub async fn delete(pool: &SqlitePool, id: i64) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM dcs_templates WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| {
                if e.as_database_error().is_some() {
                    DbError::TemplateInUse { id }
                } else {
                    DbError::Query(e)
                }
            })?;
        Ok(result.rows_affected() > 0)
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
        versions: {
            let versions_json: String = row.try_get("versions_json").map_err(DbError::Query)?;
            serde_json::from_str(&versions_json).map_err(|source| DbError::InvalidJsonShape {
                column: "versions_json",
                source,
            })?
        },
        description: row.try_get("description").map_err(DbError::Query)?,
        source: row.try_get("source").map_err(DbError::Query)?,
    };

    Ok(DcsTemplateRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        origin: text_to_enum("origin", &get_enum("origin")?)?,
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

/// Which [`crate`]-agnostic I/O driver a run used. Lives in `bhtune-db` rather than
/// `bhtune-core` because it's a persistence/orchestration concept (which adapter drove this
/// run), not a domain concept the pure MRFT engine itself needs to know about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TuneDriver {
    Opcda,
    Simulator,
    Replay,
}

/// A run's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TuneOutcome {
    Running,
    Completed,
    Failed,
    Aborted,
}

/// The clock basis used for a run's persisted polling-cadence diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TimingBasis {
    /// Simulator process evolution and MRFT timestamps both advance by one exact configured
    /// poll interval per successful PV sample.
    SimulatedFixedStep,
    /// Live OPC DA timestamps are UTC projections of monotonic elapsed time, preserving real
    /// scheduling and driver delays without exposure to wall-clock adjustments.
    LiveMonotonic,
}

/// Polling-cadence diagnostics captured over one run's successful PV samples.
///
/// The two optional gap fields are `None` when fewer than two samples were observed. The
/// measured oscillation fields are populated only for a completed MRFT run.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct TimingMetrics {
    pub basis: TimingBasis,
    pub requested_interval_ms: u64,
    pub sample_gap_count: u64,
    pub mean_sample_gap_ms: Option<f64>,
    pub max_sample_gap_ms: Option<f64>,
    /// Number of adjacent sample gaps at least twice the requested interval. Each such gap
    /// proves that at least one complete polling opportunity was missed.
    pub missed_poll_opportunity_count: u64,
    pub measured_oscillation_period_ms: Option<f64>,
    pub approximate_samples_per_period: Option<f64>,
}

/// The outcome of a best-effort loop-restore attempt made after a run ended --
/// `safety-restore-guard` (finding 3 of the live-plant safety review). Recorded via
/// [`TuneRunRow::record_restore_status`]; `NULL` in the database (mapped to `None` on
/// [`TuneRunRow::restore_status`]) means no restore was ever attempted -- either the run
/// never mutated the loop at all, or it hasn't ended yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RestoreStatus {
    /// `restore()` ran every applicable step to completion with no failures.
    Confirmed,
    /// A second Ctrl+C arrived, `--restore-timeout-secs` elapsed, or one or more individual
    /// restore steps themselves failed, before the restore could be confirmed complete. The
    /// loop may still be at a relay-test MV/mode -- see `restore_detail` for what an
    /// operator (or `bhtune restore-loop`) needs to check by hand.
    Incomplete,
}

/// The initial-readings snapshot for a [`TuneRunRow`] — known only once the driver's initial
/// read actually succeeds (`ReadInitialOPCvalues` in the legacy app); `None` for a run that
/// failed before or during that step. Combines
/// [`bhtune_core::mrft::InitialReadings`]/[`bhtune_core::range::PvRange`] with the
/// resolved [`ControllerDirection`] `core-tuning-math` needs alongside them, as one bespoke
/// type, since gluing the two existing structs together with one extra field isn't any
/// simpler than a purpose-built one here.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneRunInitialReadings {
    pub pv_ini: f32,
    pub mv_ini: f32,
    pub mv_range_low: f32,
    pub mv_range_high: f32,
    pub pv_range_high: f32,
    pub pv_range_low: f32,
    pub controller_direction: ControllerDirection,
    /// The controller mode tag's raw value at read time, before any mutation --
    /// `None` when the template/loop has no mode tag at all. Persisted (rather than kept
    /// only in-process) so a crashed run's restore intent survives the process dying
    /// outright -- `safety-restore-guard` (finding 3 of the live-plant safety review).
    pub mode_raw: Option<String>,
    /// The mode-attribute tag's raw value at read time, before any mutation -- `None` when
    /// the template/loop has no mode-attribute tag at all. See `mode_raw`.
    pub mode_attribute_raw: Option<String>,
    /// The setpoint read while the loop was still in its original mode, captured only when
    /// that mode was Auto (mirrors `SvValueIni` in the legacy app) -- `None` otherwise. Read
    /// here rather than during the mode transition itself (unlike the legacy app), since a
    /// plain read has no mutation risk and can safely happen before the loop is touched at
    /// all. See `mode_raw`.
    pub setpoint_ini: Option<f32>,
}

/// One row of `tune_runs`: a single MRFT (or future Step Test) execution against a loop.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneRunRow {
    pub id: i64,
    pub loop_id: Option<i64>,
    pub loop_name: String,
    pub driver: TuneDriver,
    /// The OPC DA server ProgID this run actually used -- `None` for a non-opcda run, or for
    /// any run started before [`TuneRunRow::record_connection`] is called (see that method's
    /// doc comment). Flat and filterable rather than folded into `request_json`, since
    /// `bhtune history revert` must know exactly which plant a past run touched.
    pub opc_server: Option<String>,
    /// The opcda-bridge gateway host this run actually used -- `None` for a non-opcda run,
    /// or before [`TuneRunRow::record_connection`] is called. See `opc_server`.
    pub bridge_host: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub outcome: TuneOutcome,
    pub failure_reason: Option<String>,
    /// Snapshot of the `LoopConfig` this run was started with — always known up front, since
    /// it's user/schedule input rather than something read from the driver.
    pub config: LoopConfig,
    /// Where the snapshotted `template` below came from (see [`TemplateOrigin`]).
    pub template_origin: TemplateOrigin,
    /// Snapshot of the exact [`DcsTemplate`] this run was configured against, deserialized
    /// from `template_snapshot_json`. Held as the full struct rather than just its `name` --
    /// which is what makes a historical run stay interpretable once the template catalog
    /// changes underneath it (`safety-run-snapshot`). There's no separate `template_name`
    /// field here even though the table has a `template_name` column: `.name` on this field
    /// already carries that value, and the column exists purely so it's filterable/indexable
    /// without `json_extract` (see this module's own doc comment).
    pub template: DcsTemplate,
    /// Snapshot of the resolved [`LoopTags`] this run actually used, deserialized from
    /// `tags_json`.
    pub tags: LoopTags,
    /// The complete run request exactly as submitted (CLI flags or the HTTP
    /// `POST /api/runs` body), before any config-driven defaulting -- raw JSON rather than a
    /// typed struct, since its shape is owned by `bhtune-cli`/`bhtune-server`, not
    /// `bhtune-db`. `"{}"` for any run started before
    /// [`TuneRunRow::record_connection`] is called. Powers `ui-prefill-last-run` and
    /// "duplicate this run"; never treat this as the source of truth for connection
    /// facts -- that's `opc_server`/`bridge_host` above.
    pub request_json: String,
    /// Mutable operator notes for this run. `None` means no note is recorded.
    pub notes: Option<String>,
    pub initial_readings: Option<TuneRunInitialReadings>,
    /// Whether this run permitted `Quality::Uncertain` OPC readings under the global
    /// `allow_uncertain_quality` policy (finding 5 of the live-plant safety review;
    /// `Quality::Bad` is never accepted regardless). `false` for every run started before
    /// [`TuneRunRow::record_allow_uncertain_quality`] is called -- see that method's doc
    /// comment for why it's a separate post-`start()` update rather than a `start()`
    /// parameter.
    pub allow_uncertain_quality: bool,
    /// Polling-cadence diagnostics collected from successful PV samples. `None` for runs
    /// created before timing diagnostics existed or attempts that ended before polling began.
    pub timing_metrics: Option<TimingMetrics>,
    /// Outcome of the best-effort restore attempted after this run ended -- `None` if no
    /// restore was ever attempted (the run never mutated the loop, or hasn't ended yet). See
    /// [`RestoreStatus`] and [`TuneRunRow::record_restore_status`].
    pub restore_status: Option<RestoreStatus>,
    /// Set only alongside `restore_status = Some(RestoreStatus::Incomplete)`: what a second
    /// Ctrl+C, `--restore-timeout-secs`, or an individual failed restore step prevented from
    /// being confirmed.
    pub restore_detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Filter criteria for [`TuneRunRow::list`]/[`TuneRunRow::count`]. Every field is optional;
/// the all-`None` default matches every run. Build one with [`TuneRunFilter::default`] and
/// the `with_*` methods, e.g. `TuneRunFilter::default().with_outcome(TuneOutcome::Failed)`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TuneRunFilter {
    pub loop_id: Option<i64>,
    pub process_type: Option<ProcessType>,
    pub controller_type: Option<ControllerType>,
    pub outcome: Option<TuneOutcome>,
    pub driver: Option<TuneDriver>,
    /// Exact match against the stored `opc_server` column. See
    /// [`TuneRunRow::record_connection`].
    pub opc_server: Option<String>,
    /// Exact match against the stored `bridge_host` column. See
    /// [`TuneRunRow::record_connection`].
    pub bridge_host: Option<String>,
    /// Matches runs with `started_at >= started_after` (inclusive).
    pub started_after: Option<DateTime<Utc>>,
    /// Matches runs with `started_at <= started_before` (inclusive).
    pub started_before: Option<DateTime<Utc>>,
    pub template_name: Option<String>,
    pub template_origin: Option<TemplateOrigin>,
}

impl TuneRunFilter {
    pub fn with_loop_id(mut self, loop_id: i64) -> TuneRunFilter {
        self.loop_id = Some(loop_id);
        self
    }

    pub fn with_process_type(mut self, process_type: ProcessType) -> TuneRunFilter {
        self.process_type = Some(process_type);
        self
    }

    pub fn with_controller_type(mut self, controller_type: ControllerType) -> TuneRunFilter {
        self.controller_type = Some(controller_type);
        self
    }

    pub fn with_outcome(mut self, outcome: TuneOutcome) -> TuneRunFilter {
        self.outcome = Some(outcome);
        self
    }

    pub fn with_driver(mut self, driver: TuneDriver) -> TuneRunFilter {
        self.driver = Some(driver);
        self
    }

    pub fn with_opc_server(mut self, opc_server: impl Into<String>) -> TuneRunFilter {
        self.opc_server = Some(opc_server.into());
        self
    }

    pub fn with_bridge_host(mut self, bridge_host: impl Into<String>) -> TuneRunFilter {
        self.bridge_host = Some(bridge_host.into());
        self
    }

    pub fn with_started_after(mut self, started_after: DateTime<Utc>) -> TuneRunFilter {
        self.started_after = Some(started_after);
        self
    }

    pub fn with_started_before(mut self, started_before: DateTime<Utc>) -> TuneRunFilter {
        self.started_before = Some(started_before);
        self
    }

    pub fn with_template_name(mut self, template_name: impl Into<String>) -> TuneRunFilter {
        self.template_name = Some(template_name.into());
        self
    }

    pub fn with_template_origin(mut self, template_origin: TemplateOrigin) -> TuneRunFilter {
        self.template_origin = Some(template_origin);
        self
    }
}

/// A page of [`TuneRunRow::list`] results: `limit` rows starting at `offset`, ordered newest
/// first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pagination {
    pub limit: i64,
    pub offset: i64,
}

impl Pagination {
    pub fn new(limit: i64, offset: i64) -> Pagination {
        Pagination { limit, offset }
    }

    /// The first `limit` rows.
    pub fn first(limit: i64) -> Pagination {
        Pagination { limit, offset: 0 }
    }
}

impl Default for Pagination {
    /// 50 rows, offset 0 — a reasonable default page size for a CLI/GUI run list.
    fn default() -> Pagination {
        Pagination {
            limit: 50,
            offset: 0,
        }
    }
}

impl TuneRunRow {
    /// Starts a new run: inserts a `tune_runs` row with `outcome = 'running'` and no initial
    /// readings yet (see [`Self::record_initial_readings`]). `now` is used for both
    /// `started_at` and `created_at`, which are naturally the same instant for a run that's
    /// only just begun. `loop_id` may be `None` for an ad-hoc run against tags that were
    /// never saved as a reusable [`LoopRow`].
    ///
    /// `template_origin`/`template`/`tags` snapshot exactly what this run was configured
    /// against (`safety-run-snapshot`), so a historical run stays interpretable even after
    /// the template catalog changes underneath it. Serializing `template`/`tags` is treated
    /// as infallible here, the same way [`enum_to_text`] treats enum serialization as
    /// infallible: both types are plain, `derive`d, string/enum-only structures with no maps,
    /// and every `f32` field they can carry is validated finite well before a run reaches
    /// this call (see `safety-validation`) -- a panic here would mean that contract regressed
    /// upstream, not a normal runtime failure this function's `DbResult` should model.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        pool: &SqlitePool,
        loop_id: Option<i64>,
        loop_name: &str,
        driver: TuneDriver,
        config: LoopConfig,
        template_origin: TemplateOrigin,
        template: &DcsTemplate,
        tags: &LoopTags,
        now: DateTime<Utc>,
    ) -> DbResult<TuneRunRow> {
        let template_snapshot_json =
            serde_json::to_string(template).expect("DcsTemplate serialization is infallible");
        let tags_json = serde_json::to_string(tags).expect("LoopTags serialization is infallible");

        let row = sqlx::query(
            r#"
            INSERT INTO tune_runs (
                loop_id, loop_name, driver, started_at, outcome,
                process_type, controller_type, relay_amp_percent, num_cycles_skip,
                num_cycles_count, noise_protection_secs, mrft_delay_secs,
                template_name, template_origin, template_snapshot_json, tags_json,
                created_at
            ) VALUES (?, ?, ?, ?, 'running', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *
            "#,
        )
        .bind(loop_id)
        .bind(loop_name)
        .bind(enum_to_text(&driver))
        .bind(now)
        .bind(enum_to_text(&config.process_type))
        .bind(enum_to_text(&config.controller_type))
        .bind(config.relay_amp_percent)
        .bind(config.num_cycles_skip)
        .bind(config.num_cycles_count)
        .bind(config.noise_protection_secs)
        .bind(config.mrft_delay_secs)
        .bind(&template.name)
        .bind(enum_to_text(&template_origin))
        .bind(template_snapshot_json)
        .bind(tags_json)
        .bind(now)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Records this run's connection provenance and the exact request it was started with
    /// (`db-run-request-snapshot`): the OPC DA server ProgID and opcda-bridge gateway host
    /// actually used (`None`/`None` for a non-opcda run), and a JSON snapshot of the complete
    /// submitted request (CLI flags or the HTTP `POST /api/runs` body), captured *before* any
    /// config-driven defaulting so it reflects what the caller actually asked for.
    ///
    /// A separate post-`start()` update rather than three more `start()` parameters, matching
    /// [`Self::record_allow_uncertain_quality`]'s precedent -- `start()` already has 8
    /// positional parameters across dozens of call sites in this workspace's test suites
    /// alone, and three more would make every one of them noisier for no benefit, since none
    /// of those tests care about connection provenance. Unlike that method, this data *is*
    /// normally known the instant a run begins; the one production caller (`bhtune-cli`'s
    /// `prepare()`) calls this immediately after `start()` succeeds, before any driver I/O.
    /// `opc_server`/`bridge_host` default to `NULL` and `request_json` defaults to `"{}"`
    /// (see the migration), so every existing `start()` call site keeps compiling and
    /// behaving exactly as before.
    pub async fn record_connection(
        pool: &SqlitePool,
        run_id: i64,
        opc_server: Option<&str>,
        bridge_host: Option<&str>,
        request_json: &str,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r#"
            UPDATE tune_runs SET opc_server = ?, bridge_host = ?, request_json = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(opc_server)
        .bind(bridge_host)
        .bind(request_json)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Replaces this run's operator notes. Passing `None` clears the note, which is the
    /// persistence-layer implementation of the GUI's delete-note action. This deliberately
    /// has no lifecycle restriction: notes remain editable while a run is active and after it
    /// reaches a terminal outcome.
    pub async fn update_notes(
        pool: &SqlitePool,
        run_id: i64,
        notes: Option<&str>,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r#"
            UPDATE tune_runs SET notes = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(notes)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Records the driver's initial-readings snapshot (`ReadInitialOPCvalues` in the legacy
    /// app) for an already-started run. Called at most once per run, right after that read
    /// succeeds -- and, deliberately, *before* `transition_to_manual`'s first mutating write
    /// rather than after it (`safety-restore-guard`, finding 3 of the live-plant safety
    /// review), so `mode_raw`/`mode_attribute_raw`/`setpoint_ini` are always durably
    /// persisted before the loop is touched at all, letting a crashed run be reconstructed
    /// and restored later via `bhtune restore-loop`. A run that fails before or during the
    /// read instead goes straight to [`Self::fail`] with `initial_readings` left `None`.
    pub async fn record_initial_readings(
        pool: &SqlitePool,
        run_id: i64,
        readings: TuneRunInitialReadings,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r#"
            UPDATE tune_runs SET
                pv_ini = ?, mv_ini = ?, mv_range_low = ?, mv_range_high = ?,
                pv_range_high = ?, pv_range_low = ?, controller_direction = ?,
                mode_raw = ?, mode_attribute_raw = ?, setpoint_ini = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(readings.pv_ini)
        .bind(readings.mv_ini)
        .bind(readings.mv_range_low)
        .bind(readings.mv_range_high)
        .bind(readings.pv_range_high)
        .bind(readings.pv_range_low)
        .bind(enum_to_text(&readings.controller_direction))
        .bind(readings.mode_raw)
        .bind(readings.mode_attribute_raw)
        .bind(readings.setpoint_ini)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Records whether this run permitted `Quality::Uncertain` OPC readings under the global
    /// configuration policy (finding 5 of the live-plant safety review). A separate
    /// post-`start()` update rather than a new `start()` parameter deliberately: `start()`
    /// already has 8 positional parameters across 28 call sites in this crate's own test
    /// suite alone, and this is a rarely-used escape hatch, not information every caller
    /// naturally has on hand at the moment a run begins the way `template_origin`/`template`/
    /// `tags` are. The column defaults to `0`/`false` (see the migration), so every existing
    /// `start()` call site keeps compiling and behaving exactly as before; only the one
    /// production caller in `bhtune-cli`'s `run()` needs to call this, right after `start()`
    /// succeeds.
    pub async fn record_allow_uncertain_quality(
        pool: &SqlitePool,
        run_id: i64,
        allow_uncertain_quality: bool,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r#"
            UPDATE tune_runs SET allow_uncertain_quality = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(allow_uncertain_quality)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Records the polling cadence observed while this run was active. Kept as one typed JSON
    /// snapshot because these diagnostics are nested, evolve together, and have no SQL-level
    /// filtering requirement. Normal completed/aborted tune orchestration uses
    /// [`Self::complete_with_timing_metrics`] or [`Self::abort_with_timing_metrics`] so the
    /// terminal outcome and diagnostics become visible atomically; this standalone update is
    /// retained for non-terminal/failure paths and direct repository consumers.
    pub async fn record_timing_metrics(
        pool: &SqlitePool,
        run_id: i64,
        metrics: TimingMetrics,
    ) -> DbResult<TuneRunRow> {
        let metrics_json =
            serde_json::to_string(&metrics).expect("TimingMetrics serialization is infallible");
        let row = sqlx::query(
            r#"
            UPDATE tune_runs SET timing_metrics_json = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(metrics_json)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Records the outcome of a best-effort loop-restore attempt made after this run ended
    /// (`safety-restore-guard`, finding 3 of the live-plant safety review). Called once,
    /// after `complete`/`fail`/`abort` (whichever applies) and after `attempt_restore` has
    /// actually run -- never before, and never for a run that ended without ever mutating
    /// the loop (nothing to restore, so nothing to record). `detail` should be `Some(..)`
    /// whenever `status` is [`RestoreStatus::Incomplete`], naming what could not be
    /// confirmed; pass `None` for [`RestoreStatus::Confirmed`]. A separate post-hoc update
    /// rather than a `complete`/`fail`/`abort` parameter, matching
    /// [`Self::record_allow_uncertain_quality`]'s precedent: the restore attempt always
    /// happens strictly after one of those three, never alongside it.
    pub async fn record_restore_status(
        pool: &SqlitePool,
        run_id: i64,
        status: RestoreStatus,
        detail: Option<&str>,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r#"
            UPDATE tune_runs SET restore_status = ?, restore_detail = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(enum_to_text(&status))
        .bind(detail)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Marks a run `completed` — a full MRFT test that ran to its natural end. The calculated
    /// results themselves are recorded separately via [`TuneResultRow::insert`].
    pub async fn complete(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            "UPDATE tune_runs SET outcome = 'completed', completed_at = ? WHERE id = ? RETURNING *",
        )
        .bind(completed_at)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Atomically marks a run completed and publishes its timing diagnostics. This prevents
    /// readers that react to the terminal outcome (notably the SSE stream) from observing a
    /// completed run before its timing snapshot is visible.
    pub async fn complete_with_timing_metrics(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
        timing_metrics: Option<TimingMetrics>,
    ) -> DbResult<TuneRunRow> {
        Self::set_terminal_outcome_with_timing_metrics(
            pool,
            run_id,
            completed_at,
            TuneOutcome::Completed,
            timing_metrics,
            None,
        )
        .await
    }

    /// Marks a run `failed`, recording why. Valid whether or not
    /// [`Self::record_initial_readings`] was ever called for this run — a run can fail before,
    /// during, or after the initial read.
    pub async fn fail(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
        failure_reason: &str,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r#"
            UPDATE tune_runs SET outcome = 'failed', completed_at = ?, failure_reason = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(completed_at)
        .bind(failure_reason)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Marks a run `aborted` — stopped deliberately (by a human, or `cli-safety`'s
    /// wall-clock timeout guardrail) rather than failing on its own.
    pub async fn abort(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            "UPDATE tune_runs SET outcome = 'aborted', completed_at = ? WHERE id = ? RETURNING *",
        )
        .bind(completed_at)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Atomically marks a run aborted and publishes any timing diagnostics collected before
    /// the abort, so terminal-state readers cannot miss the final timing snapshot.
    pub async fn abort_with_timing_metrics(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
        timing_metrics: Option<TimingMetrics>,
    ) -> DbResult<TuneRunRow> {
        Self::set_terminal_outcome_with_timing_metrics(
            pool,
            run_id,
            completed_at,
            TuneOutcome::Aborted,
            timing_metrics,
            None,
        )
        .await
    }

    /// Atomically marks a run aborted, publishes its timing diagnostics, and persists the
    /// operator-facing reason for the abort in the existing `failure_reason` column. This is
    /// used when an abort has a durable safety explanation (for example an MV command whose
    /// live readback did not reach its target), while preserving the database's existing
    /// [`TuneOutcome::Aborted`] value.
    pub async fn abort_with_timing_metrics_and_reason(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
        timing_metrics: Option<TimingMetrics>,
        reason: &str,
    ) -> DbResult<TuneRunRow> {
        Self::set_terminal_outcome_with_timing_metrics(
            pool,
            run_id,
            completed_at,
            TuneOutcome::Aborted,
            timing_metrics,
            Some(reason),
        )
        .await
    }

    async fn set_terminal_outcome_with_timing_metrics(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
        outcome: TuneOutcome,
        timing_metrics: Option<TimingMetrics>,
        failure_reason: Option<&str>,
    ) -> DbResult<TuneRunRow> {
        let timing_metrics_json = timing_metrics.map(|metrics| {
            serde_json::to_string(&metrics).expect("TimingMetrics serialization is infallible")
        });
        let row = sqlx::query(
            r#"
            UPDATE tune_runs
            SET outcome = ?, completed_at = ?, timing_metrics_json = ?, failure_reason = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(enum_to_text(&outcome))
        .bind(completed_at)
        .bind(timing_metrics_json)
        .bind(failure_reason)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Fetches one row by id, or `None` if it doesn't exist.
    pub async fn get(pool: &SqlitePool, id: i64) -> DbResult<Option<TuneRunRow>> {
        let row = sqlx::query("SELECT * FROM tune_runs WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(DbError::Query)?;
        row.map(row_to_tune_run).transpose()
    }

    /// Lists runs matching `filter`, newest-started first, one `pagination` page at a time.
    /// See [`Self::count`] for the total number of rows `filter` matches across all pages.
    pub async fn list(
        pool: &SqlitePool,
        filter: &TuneRunFilter,
        pagination: Pagination,
    ) -> DbResult<Vec<TuneRunRow>> {
        let mut builder: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT * FROM tune_runs");
        push_filter(&mut builder, filter);
        builder.push(" ORDER BY started_at DESC LIMIT ");
        builder.push_bind(pagination.limit);
        builder.push(" OFFSET ");
        builder.push_bind(pagination.offset);

        let rows = builder
            .build()
            .fetch_all(pool)
            .await
            .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_tune_run).collect()
    }

    /// Counts every run matching `filter`, ignoring pagination — the total [`Self::list`]
    /// would page through.
    pub async fn count(pool: &SqlitePool, filter: &TuneRunFilter) -> DbResult<i64> {
        let mut builder: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT COUNT(*) FROM tune_runs");
        push_filter(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(pool)
            .await
            .map_err(DbError::Query)
    }

    /// Deletes every run matching `filter` in one statement (SQLite treats a single
    /// statement as its own transaction, so no explicit `BEGIN`/`COMMIT` is needed). Returns
    /// the number of runs deleted. `tune_samples`/`tune_results`/`tune_writes`'s `ON DELETE
    /// CASCADE` foreign keys (see `db-schema`'s migration) remove each deleted run's samples,
    /// results, and write-back audit rows automatically.
    ///
    /// Shares [`push_filter`] with [`Self::list`]/[`Self::count`], so "what a `--dry-run`
    /// preview reports" and "what an actual sweep deletes" can never disagree — used this way
    /// by `history-retention`'s automatic sweep and `bhtune history prune`.
    ///
    /// An empty `filter` (every field `None`) matches and deletes every run in the table —
    /// callers that mean to scope a deletion must build a `filter` that says so explicitly;
    /// this function has no separate "are you sure" guard of its own, matching `count`/`list`
    /// treating an empty filter as "everything" rather than "nothing".
    pub async fn delete_matching(pool: &SqlitePool, filter: &TuneRunFilter) -> DbResult<u64> {
        let mut builder: QueryBuilder<Sqlite> = QueryBuilder::new("DELETE FROM tune_runs");
        push_filter(&mut builder, filter);
        let result = builder
            .build()
            .execute(pool)
            .await
            .map_err(DbError::Query)?;
        Ok(result.rows_affected())
    }

    /// Deletes exactly one run by id (`history-explorer-ui`'s delete action). Returns
    /// whether a row was actually deleted -- `false` if no run has that id, letting the
    /// caller map that to a 404 rather than a silent no-op. Unlike
    /// [`DcsTemplateRow::delete`], no foreign key ever blocks this: `tune_runs` has no
    /// parent-side `RESTRICT` reference pointing at it, only the `ON DELETE CASCADE`
    /// children (`tune_samples`/`tune_results`/`tune_writes`, see `db-schema`'s migration),
    /// which SQLite removes automatically as part of the same statement.
    pub async fn delete(pool: &SqlitePool, id: i64) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM tune_runs WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map_err(DbError::Query)?;
        Ok(result.rows_affected() > 0)
    }
}

/// Appends `WHERE <conditions>` to `builder` for every `Some` field in `filter`, or nothing
/// at all if every field is `None`. Shared by [`TuneRunRow::list`]/[`TuneRunRow::count`] so
/// the two can never disagree about which rows match a given filter.
fn push_filter(builder: &mut QueryBuilder<Sqlite>, filter: &TuneRunFilter) {
    // `1=1` makes every real condition an unconditional `AND`, rather than needing to track
    // whether it's the first one (and therefore needs `WHERE` instead of `AND`).
    builder.push(" WHERE 1=1");

    if let Some(loop_id) = filter.loop_id {
        builder.push(" AND loop_id = ").push_bind(loop_id);
    }
    if let Some(process_type) = filter.process_type {
        builder
            .push(" AND process_type = ")
            .push_bind(enum_to_text(&process_type));
    }
    if let Some(controller_type) = filter.controller_type {
        builder
            .push(" AND controller_type = ")
            .push_bind(enum_to_text(&controller_type));
    }
    if let Some(outcome) = filter.outcome {
        builder
            .push(" AND outcome = ")
            .push_bind(enum_to_text(&outcome));
    }
    if let Some(driver) = filter.driver {
        builder
            .push(" AND driver = ")
            .push_bind(enum_to_text(&driver));
    }
    if let Some(opc_server) = &filter.opc_server {
        builder
            .push(" AND opc_server = ")
            .push_bind(opc_server.clone());
    }
    if let Some(bridge_host) = &filter.bridge_host {
        builder
            .push(" AND bridge_host = ")
            .push_bind(bridge_host.clone());
    }
    if let Some(started_after) = filter.started_after {
        builder.push(" AND started_at >= ").push_bind(started_after);
    }
    if let Some(started_before) = filter.started_before {
        builder
            .push(" AND started_at <= ")
            .push_bind(started_before);
    }
    if let Some(template_name) = &filter.template_name {
        builder
            .push(" AND template_name = ")
            .push_bind(template_name.clone());
    }
    if let Some(template_origin) = filter.template_origin {
        builder
            .push(" AND template_origin = ")
            .push_bind(enum_to_text(&template_origin));
    }
}

fn row_to_tune_run(row: SqliteRow) -> DbResult<TuneRunRow> {
    let pv_ini: Option<f32> = row.try_get("pv_ini").map_err(DbError::Query)?;
    let initial_readings = match pv_ini {
        Some(pv_ini) => {
            let controller_direction: String = row
                .try_get("controller_direction")
                .map_err(DbError::Query)?;
            Some(TuneRunInitialReadings {
                pv_ini,
                mv_ini: row.try_get("mv_ini").map_err(DbError::Query)?,
                mv_range_low: row.try_get("mv_range_low").map_err(DbError::Query)?,
                mv_range_high: row.try_get("mv_range_high").map_err(DbError::Query)?,
                pv_range_high: row.try_get("pv_range_high").map_err(DbError::Query)?,
                pv_range_low: row.try_get("pv_range_low").map_err(DbError::Query)?,
                controller_direction: text_to_enum("controller_direction", &controller_direction)?,
                mode_raw: row.try_get("mode_raw").map_err(DbError::Query)?,
                mode_attribute_raw: row.try_get("mode_attribute_raw").map_err(DbError::Query)?,
                setpoint_ini: row.try_get("setpoint_ini").map_err(DbError::Query)?,
            })
        }
        None => None,
    };

    let process_type: String = row.try_get("process_type").map_err(DbError::Query)?;
    let controller_type: String = row.try_get("controller_type").map_err(DbError::Query)?;
    let config = LoopConfig {
        process_type: text_to_enum("process_type", &process_type)?,
        controller_type: text_to_enum("controller_type", &controller_type)?,
        relay_amp_percent: row.try_get("relay_amp_percent").map_err(DbError::Query)?,
        num_cycles_skip: row.try_get("num_cycles_skip").map_err(DbError::Query)?,
        num_cycles_count: row.try_get("num_cycles_count").map_err(DbError::Query)?,
        noise_protection_secs: row
            .try_get("noise_protection_secs")
            .map_err(DbError::Query)?,
        mrft_delay_secs: row.try_get("mrft_delay_secs").map_err(DbError::Query)?,
    };

    let driver: String = row.try_get("driver").map_err(DbError::Query)?;
    let outcome: String = row.try_get("outcome").map_err(DbError::Query)?;

    let restore_status_text: Option<String> =
        row.try_get("restore_status").map_err(DbError::Query)?;
    let restore_status = restore_status_text
        .map(|text| text_to_enum("restore_status", &text))
        .transpose()?;

    let template_origin: String = row.try_get("template_origin").map_err(DbError::Query)?;
    let template_snapshot_json: String = row
        .try_get("template_snapshot_json")
        .map_err(DbError::Query)?;
    let tags_json: String = row.try_get("tags_json").map_err(DbError::Query)?;
    let request_json: String = row.try_get("request_json").map_err(DbError::Query)?;
    let timing_metrics_json: Option<String> =
        row.try_get("timing_metrics_json").map_err(DbError::Query)?;
    let template: DcsTemplate =
        serde_json::from_str(&template_snapshot_json).map_err(|source| {
            DbError::InvalidJsonShape {
                column: "template_snapshot_json",
                source,
            }
        })?;
    let tags: LoopTags =
        serde_json::from_str(&tags_json).map_err(|source| DbError::InvalidJsonShape {
            column: "tags_json",
            source,
        })?;
    let timing_metrics = timing_metrics_json
        .map(|json| {
            serde_json::from_str(&json).map_err(|source| DbError::InvalidJsonShape {
                column: "timing_metrics_json",
                source,
            })
        })
        .transpose()?;

    Ok(TuneRunRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        loop_id: row.try_get("loop_id").map_err(DbError::Query)?,
        loop_name: row.try_get("loop_name").map_err(DbError::Query)?,
        driver: text_to_enum("driver", &driver)?,
        opc_server: row.try_get("opc_server").map_err(DbError::Query)?,
        bridge_host: row.try_get("bridge_host").map_err(DbError::Query)?,
        started_at: row.try_get("started_at").map_err(DbError::Query)?,
        completed_at: row.try_get("completed_at").map_err(DbError::Query)?,
        outcome: text_to_enum("outcome", &outcome)?,
        failure_reason: row.try_get("failure_reason").map_err(DbError::Query)?,
        config,
        template_origin: text_to_enum("template_origin", &template_origin)?,
        template,
        tags,
        request_json,
        notes: row.try_get("notes").map_err(DbError::Query)?,
        initial_readings,
        allow_uncertain_quality: row
            .try_get("allow_uncertain_quality")
            .map_err(DbError::Query)?,
        timing_metrics,
        restore_status,
        restore_detail: row.try_get("restore_detail").map_err(DbError::Query)?,
        created_at: row.try_get("created_at").map_err(DbError::Query)?,
    })
}
// }}}1

// tune_samples {{{1

/// How much a [`TuneSampleRow`]'s `sample.pv` reading should be trusted, as recorded at the
/// moment it was read (finding 5 of the live-plant safety review).
///
/// A `bhtune-db`-local mirror of [`bhtune_driver::Quality`], not a reuse of it directly:
/// `bhtune-db` deliberately doesn't depend on `bhtune-driver` (a leaf I/O-adapter crate with
/// a much heavier dependency tree -- `tokio`, `tonic`, `opcda-bridge` -- that has no business
/// in the persistence crate just to name one three-variant enum), so `bhtune-cli`, which
/// already depends on both, is the one place that converts between them. This mirrors
/// [`TemplateOrigin`]'s own precedent: a small, persistence-local enum rather than a second
/// dependency edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SampleQuality {
    Good,
    Uncertain,
    Bad,
}

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
    /// The driver-reported quality of `sample.pv` at the moment it was read. See
    /// [`SampleQuality`].
    pub pv_quality: SampleQuality,
}

impl TuneSampleRow {
    /// Records one tick of a run, taking the exact [`Tick`]/[`MrftState`] pair
    /// [`bhtune_core::mrft::MrftEngine::step`] produced, plus the [`SampleQuality`] the
    /// driver reported for `sample.pv` at read time. `(run_id, tick_index)` is unique (see
    /// the migration), so re-recording the same tick twice is a caller bug, not a silent
    /// overwrite.
    pub async fn insert(
        pool: &SqlitePool,
        run_id: i64,
        tick_index: i64,
        sample: Tick,
        state: MrftState,
        pv_quality: SampleQuality,
    ) -> DbResult<TuneSampleRow> {
        let row = sqlx::query(
            r#"
            INSERT INTO tune_samples (
                run_id, tick, time, pv, pv_quality, hysteresis, mv_value_current,
                mv_sign_next_step, counter_all_switches, cycles_completed, cycles_remaining
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *
            "#,
        )
        .bind(run_id)
        .bind(tick_index)
        .bind(sample.time)
        .bind(sample.pv)
        .bind(enum_to_text(&pv_quality))
        .bind(state.hysteresis)
        .bind(state.mv_value_current)
        .bind(state.mv_sign_next_step)
        .bind(state.counter_all_switches)
        .bind(state.cycles_completed)
        .bind(state.cycles_remaining)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_sample(row)
    }

    /// Lists every sample of `run_id`, ordered by tick — the full per-tick trend the history
    /// explorer's chart (`history-explorer-ui`) plots.
    pub async fn list_for_run(pool: &SqlitePool, run_id: i64) -> DbResult<Vec<TuneSampleRow>> {
        let rows = sqlx::query("SELECT * FROM tune_samples WHERE run_id = ? ORDER BY tick")
            .bind(run_id)
            .fetch_all(pool)
            .await
            .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_tune_sample).collect()
    }

    /// Lists only the samples of `run_id` recorded *after* `after_tick`, ordered by tick --
    /// what `bhtune-server`'s `GET /api/runs/{id}/stream` (`frontend-live-stream`) polls on
    /// every iteration so it never re-sends a tick it has already pushed to the browser.
    /// Pass `-1` to fetch every sample from the very first tick (`tune_samples.tick` is
    /// `>= 0`, so nothing is ever excluded by that sentinel).
    pub async fn list_for_run_since(
        pool: &SqlitePool,
        run_id: i64,
        after_tick: i64,
    ) -> DbResult<Vec<TuneSampleRow>> {
        let rows =
            sqlx::query("SELECT * FROM tune_samples WHERE run_id = ? AND tick > ? ORDER BY tick")
                .bind(run_id)
                .bind(after_tick)
                .fetch_all(pool)
                .await
                .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_tune_sample).collect()
    }
}

fn row_to_tune_sample(row: SqliteRow) -> DbResult<TuneSampleRow> {
    Ok(TuneSampleRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        run_id: row.try_get("run_id").map_err(DbError::Query)?,
        tick_index: row.try_get("tick").map_err(DbError::Query)?,
        sample: Tick {
            time: row.try_get("time").map_err(DbError::Query)?,
            pv: row.try_get("pv").map_err(DbError::Query)?,
        },
        state: MrftState {
            hysteresis: row.try_get("hysteresis").map_err(DbError::Query)?,
            mv_value_current: row.try_get("mv_value_current").map_err(DbError::Query)?,
            mv_sign_next_step: row.try_get("mv_sign_next_step").map_err(DbError::Query)?,
            counter_all_switches: row
                .try_get("counter_all_switches")
                .map_err(DbError::Query)?,
            cycles_completed: row.try_get("cycles_completed").map_err(DbError::Query)?,
            cycles_remaining: row.try_get("cycles_remaining").map_err(DbError::Query)?,
        },
        pv_quality: {
            let pv_quality: String = row.try_get("pv_quality").map_err(DbError::Query)?;
            text_to_enum("pv_quality", &pv_quality)?
        },
    })
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

    /// Inserts `row` (typically built via [`Self::from_calculated`]), returning the persisted
    /// copy with its assigned `id`. `(run_id, response_level)` is unique (see the migration)
    /// — a successfully completed run writes exactly the 3 [`ResponseLevel`] rows once, at
    /// completion.
    pub async fn insert(pool: &SqlitePool, row: &TuneResultRow) -> DbResult<TuneResultRow> {
        let inserted = sqlx::query(
            r#"
            INSERT INTO tune_results (
                run_id, response_level, kp, ti_minutes, td_minutes,
                proportional, integral, derivative
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *
            "#,
        )
        .bind(row.run_id)
        .bind(enum_to_text(&row.response_level))
        .bind(row.kp)
        .bind(row.ti_minutes)
        .bind(row.td_minutes)
        .bind(row.proportional)
        .bind(row.integral)
        .bind(row.derivative)
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
    })
}
// }}}1

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
            r#"
            INSERT INTO tune_mv_actuations (
                run_id, sequence, kind, commanded_at, target_mv, previous_commanded_mv,
                tolerance, confirmation_due_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *
            "#,
        )
        .bind(run_id)
        .bind(new.sequence)
        .bind(enum_to_text(&new.kind))
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
            r#"
            UPDATE tune_mv_actuations
            SET last_checked_at = ?, readback_mv = ?, readback_quality = ?,
                attempt_count = attempt_count + 1
            WHERE id = ? AND status = 'pending'
            RETURNING *
            "#,
        )
        .bind(checked_at)
        .bind(readback_mv)
        .bind(readback_quality.map(|quality| enum_to_text(&quality)))
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
            r#"
            UPDATE tune_mv_actuations
            SET last_checked_at = ?, readback_mv = ?, readback_quality = ?,
                attempt_count = attempt_count + 1, status = ?, detail = ?
            WHERE id = ? AND status = 'pending'
            RETURNING *
            "#,
        )
        .bind(checked_at)
        .bind(readback_mv)
        .bind(readback_quality.map(|quality| enum_to_text(&quality)))
        .bind(enum_to_text(&status))
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
            r#"
            UPDATE tune_mv_actuations
            SET status = ?, detail = ?
            WHERE id = ? AND status = 'pending'
            RETURNING *
            "#,
        )
        .bind(enum_to_text(&status))
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
            r#"
            UPDATE tune_mv_actuations
            SET status = ?, detail = ?
            WHERE run_id = ? AND status = 'pending'
            "#,
        )
        .bind(enum_to_text(&status))
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

// tune_writes {{{1

/// One row of `tune_writes`: an audit record of PID constants actually written back to the
/// DCS for one [`ResponseLevel`] of one run, distinct from what was merely *calculated*
/// ([`TuneResultRow`]). Flattened for the same reason as `TuneResultRow`.
///
/// `*_written`/`*_readback` are independently nullable (not all-or-nothing like `previous`)
/// because `safety-writeback-rollback` writes and verifies P, then I, then D in sequence,
/// stopping at the first failure -- so a partial attempt leaves the constants after the
/// failure point at `None` rather than 0, distinguishing "never attempted" from "attempted
/// and confirmed zero".
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
/// caller as it works through the sequential pre-read / write-and-verify / rollback steps,
/// then persisted in a single [`TuneWriteRow::insert`] call -- replacing the old two-outcome
/// `insert_success`/`insert_failure` split, which could not represent a partial write or a
/// rollback attempt at all. See `safety-writeback-rollback` in AGENTS.md for the four
/// distinguishable outcomes this shape exists to capture.
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
            r#"
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
            "#,
        )
        .bind(run_id)
        .bind(enum_to_text(&new.response_level))
        .bind(new.written_at)
        .bind(enum_to_text(&new.kind))
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
        .bind(new.rollback_state.map(|s| enum_to_text(&s)))
        .bind(new.rollback_error)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_write(row)
    }

    /// Lists every write-back attempt for `run_id`, oldest first — the full "who changed this
    /// loop and when" audit trail `history-writeback-audit` exists to provide.
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

// settings {{{1

/// One row of `settings`: an app-wide key/value pair (e.g. the `history-retention` policy).
#[derive(Debug, Clone, PartialEq)]
pub struct SettingRow {
    pub key: String,
    pub value: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}

impl SettingRow {
    /// Loads one app-wide setting by key, returning `None` when it has not been stored yet.
    ///
    /// The database constraint guarantees that `value` is syntactically valid JSON, but the
    /// shape is intentionally left to the feature that owns the key. Parsing it here keeps
    /// callers from having to duplicate the TEXT-to-JSON conversion.
    pub async fn get(pool: &SqlitePool, key: &str) -> DbResult<Option<Self>> {
        let row = sqlx::query("SELECT key, value, updated_at FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(pool)
            .await
            .map_err(DbError::Query)?;

        row.map(setting_from_row).transpose()
    }

    /// Inserts or replaces one app-wide setting and returns the stored row.
    ///
    /// `updated_at` is supplied by the caller so database tests and clock ownership remain
    /// deterministic, matching the other repository methods in this module.
    pub async fn upsert(
        pool: &SqlitePool,
        key: &str,
        value: &serde_json::Value,
        updated_at: DateTime<Utc>,
    ) -> DbResult<Self> {
        let value_json = value.to_string();
        let row = sqlx::query(
            "INSERT INTO settings (key, value, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET
                 value = excluded.value,
                 updated_at = excluded.updated_at
             RETURNING key, value, updated_at",
        )
        .bind(key)
        .bind(value_json)
        .bind(updated_at)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        setting_from_row(row)
    }
}

fn setting_from_row(row: SqliteRow) -> DbResult<SettingRow> {
    let value_json: String = row.try_get("value").map_err(DbError::Query)?;
    let value = serde_json::from_str(&value_json).map_err(|source| DbError::InvalidJsonShape {
        column: "settings.value",
        source,
    })?;

    Ok(SettingRow {
        key: row.try_get("key").map_err(DbError::Query)?,
        value,
        updated_at: row.try_get("updated_at").map_err(DbError::Query)?,
    })
}
// }}}1

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::{enum_to_text, text_to_enum};

    async fn sample_run() -> (crate::SqlitePool, i64) {
        let pool = crate::connect_in_memory().await.unwrap();
        let template = bhtune_core::built_in_templates().remove(0);
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Unit1.FIC101.PV", &template);
        let config = bhtune_core::LoopConfig {
            process_type: bhtune_core::ProcessType::Flow,
            controller_type: bhtune_core::ControllerType::Pi,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 3,
            mrft_delay_secs: 0,
        };
        let run = TuneRunRow::start(
            &pool,
            None,
            "Unit1.FIC101.PV",
            TuneDriver::Simulator,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        (pool, run.id)
    }

    #[test]
    fn tune_driver_round_trips_and_matches_check_constraint() {
        let cases = [
            (TuneDriver::Opcda, "opcda"),
            (TuneDriver::Simulator, "simulator"),
            (TuneDriver::Replay, "replay"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant), text);
            assert_eq!(text_to_enum::<TuneDriver>("driver", text).unwrap(), variant);
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

    #[tokio::test]
    async fn record_restore_status_round_trips_both_status_shapes() {
        let (pool, run_id) = sample_run().await;
        let confirmed =
            TuneRunRow::record_restore_status(&pool, run_id, RestoreStatus::Confirmed, None)
                .await
                .unwrap();
        assert_eq!(confirmed.restore_status, Some(RestoreStatus::Confirmed));
        assert_eq!(confirmed.restore_detail, None);

        let incomplete = TuneRunRow::record_restore_status(
            &pool,
            run_id,
            RestoreStatus::Incomplete,
            Some("MV restore failed"),
        )
        .await
        .unwrap();
        assert_eq!(incomplete.restore_status, Some(RestoreStatus::Incomplete));
        assert_eq!(
            incomplete.restore_detail.as_deref(),
            Some("MV restore failed")
        );
    }

    #[tokio::test]
    async fn tune_run_delete_reports_both_existing_and_missing_ids() {
        let (pool, run_id) = sample_run().await;
        assert!(TuneRunRow::delete(&pool, run_id).await.unwrap());
        assert!(!TuneRunRow::delete(&pool, run_id).await.unwrap());
    }

    #[tokio::test]
    async fn malformed_tune_run_json_is_reported_with_the_respective_column() {
        for (column, expected) in [
            ("template_snapshot_json", "template_snapshot_json"),
            ("tags_json", "tags_json"),
            ("timing_metrics_json", "timing_metrics_json"),
        ] {
            let (pool, run_id) = sample_run().await;
            let query = match column {
                "template_snapshot_json" => {
                    sqlx::query("UPDATE tune_runs SET template_snapshot_json = ? WHERE id = ?")
                }
                "tags_json" => sqlx::query("UPDATE tune_runs SET tags_json = ? WHERE id = ?"),
                "timing_metrics_json" => {
                    sqlx::query("UPDATE tune_runs SET timing_metrics_json = ? WHERE id = ?")
                }
                _ => unreachable!(),
            };
            query
                .bind("\"wrong-shape\"")
                .bind(run_id)
                .execute(&pool)
                .await
                .unwrap();
            let err = TuneRunRow::get(&pool, run_id).await.unwrap_err();
            assert!(matches!(
                err,
                DbError::InvalidJsonShape { column: actual, .. } if actual == expected
            ));
        }
    }

    #[tokio::test]
    async fn malformed_setting_json_is_reported_as_invalid_shape() {
        let pool = crate::connect_in_memory().await.unwrap();
        sqlx::query("CREATE TABLE malformed_settings (key TEXT, value TEXT, updated_at TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO malformed_settings (key, value, updated_at) VALUES (?, ?, ?)")
            .bind("draft")
            .bind("{not-json")
            .bind(chrono::Utc::now())
            .execute(&pool)
            .await
            .unwrap();

        let row = sqlx::query("SELECT key, value, updated_at FROM malformed_settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        let err = setting_from_row(row).unwrap_err();
        assert!(matches!(
            err,
            DbError::InvalidJsonShape {
                column: "settings.value",
                ..
            }
        ));
    }
}
