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
//! [`DcsTemplateRow`] and [`TuneRunRow`]/[`TuneSampleRow`]/[`TuneResultRow`]/[`TuneWriteRow`]
//! have full repository methods (insert, lifecycle transitions, filtering, pagination) —
//! covering `db-seed-templates` and `history-query-api`. [`LoopRow`] deliberately has none
//! yet: full CRUD for saved loops (list/update/delete) is a separate "loop management"
//! concern from history (which is about *runs*, not the loops they reference), left to
//! whichever future todo actually needs it. Until then, tests construct `loops` rows with
//! raw SQL (see `tests/schema.rs`'s `seed_loop` helper) purely as foreign-key setup.
//!
//! [`TuneRunRow::list`]/[`TuneRunRow::count`] build their `WHERE` clause dynamically with
//! `sqlx::QueryBuilder`, since [`TuneRunFilter`]'s fields are all optional and the set of
//! active conditions varies per call — a fixed `query!` string can't express that, and
//! `bhtune-db` uses runtime `query`/`query_as` throughout anyway (see `Cargo.toml`), so this
//! doesn't introduce a new query style, just the first dynamic one.

use bhtune_core::{
    ControllerDirection, ControllerType, DcsTemplate, LoopConfig, LoopTags, MrftState, ProcessType,
    ResponseLevel, Tick,
    tuning_math::{OpcWriteValues, PidParameters, TuningResult},
};
use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, sqlite::SqliteRow};

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

/// The outcome of a best-effort loop-restore attempt made after a run ended --
/// `safety-restore-guard` (finding 3 of the live-plant safety review). Recorded via
/// [`TuneRunRow::record_restore_status`]; `NULL` in the database (mapped to `None` on
/// [`TuneRunRow::restore_status`]) means no restore was ever attempted -- either the run
/// never mutated the loop at all, or it hasn't ended yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

/// Where a [`TuneRunRow`]'s snapshotted template came from, at the moment the run started.
/// Mirrors the origin the community template catalog gives a `dcs_templates` row (see
/// `template-provenance`), captured here so a run's own history never needs to look that row
/// back up -- which matters precisely because it might no longer exist, or might have been
/// re-imported under a different origin since.
///
/// `Catalog` isn't produced anywhere yet -- that lands with `template-provenance` itself --
/// but all three variants are defined together now so that landing doesn't require a
/// non-exhaustive-match-breaking enum change here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateOrigin {
    /// One of the templates `bhtune-core` ships built in.
    Builtin,
    /// Loaded from the community template catalog (`template-provenance`; not yet
    /// implemented).
    Catalog,
    /// Hand-imported or otherwise created by whoever is running bhtune.
    User,
}

impl TemplateOrigin {
    /// Bridges today's [`DcsTemplateRow::is_builtin`] boolean to this richer enum, until
    /// `template-provenance` gives `dcs_templates` its own real `origin` column that this
    /// type mirrors directly.
    pub fn from_is_builtin(is_builtin: bool) -> TemplateOrigin {
        if is_builtin {
            TemplateOrigin::Builtin
        } else {
            TemplateOrigin::User
        }
    }
}

/// The initial-readings snapshot for a [`TuneRunRow`] — known only once the backend's initial
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
    pub backend: TuneBackend,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub outcome: TuneOutcome,
    pub failure_reason: Option<String>,
    /// Snapshot of the `LoopConfig` this run was started with — always known up front, since
    /// it's user/schedule input rather than something read from the backend.
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
    pub initial_readings: Option<TuneRunInitialReadings>,
    /// Whether this run permitted `Quality::Uncertain` OPC readings via
    /// `--allow-uncertain-quality` (finding 5 of the live-plant safety review;
    /// `Quality::Bad` is never accepted regardless). `false` for every run started before
    /// [`TuneRunRow::record_allow_uncertain_quality`] is called -- see that method's doc
    /// comment for why it's a separate post-`start()` update rather than a `start()`
    /// parameter.
    pub allow_uncertain_quality: bool,
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
    pub backend: Option<TuneBackend>,
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

    pub fn with_backend(mut self, backend: TuneBackend) -> TuneRunFilter {
        self.backend = Some(backend);
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
        backend: TuneBackend,
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
                loop_id, loop_name, backend, started_at, outcome,
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
        .bind(enum_to_text(&backend))
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

    /// Records the backend's initial-readings snapshot (`ReadInitialOPCvalues` in the legacy
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

    /// Records whether this run permitted `Quality::Uncertain` OPC readings, via
    /// `--allow-uncertain-quality` (finding 5 of the live-plant safety review). A separate
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
    if let Some(backend) = filter.backend {
        builder
            .push(" AND backend = ")
            .push_bind(enum_to_text(&backend));
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

    let backend: String = row.try_get("backend").map_err(DbError::Query)?;
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

    Ok(TuneRunRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        loop_id: row.try_get("loop_id").map_err(DbError::Query)?,
        loop_name: row.try_get("loop_name").map_err(DbError::Query)?,
        backend: text_to_enum("backend", &backend)?,
        started_at: row.try_get("started_at").map_err(DbError::Query)?,
        completed_at: row.try_get("completed_at").map_err(DbError::Query)?,
        outcome: text_to_enum("outcome", &outcome)?,
        failure_reason: row.try_get("failure_reason").map_err(DbError::Query)?,
        config,
        template_origin: text_to_enum("template_origin", &template_origin)?,
        template,
        tags,
        initial_readings,
        allow_uncertain_quality: row
            .try_get("allow_uncertain_quality")
            .map_err(DbError::Query)?,
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
/// A `bhtune-db`-local mirror of [`bhtune_backend::Quality`], not a reuse of it directly:
/// `bhtune-db` deliberately doesn't depend on `bhtune-backend` (a leaf I/O-adapter crate with
/// a much heavier dependency tree -- `tokio`, `tonic`, `opcda-bridge` -- that has no business
/// in the persistence crate just to name one three-variant enum), so `bhtune-cli`, which
/// already depends on both, is the one place that converts between them. This mirrors
/// [`TemplateOrigin`]'s own precedent: a small, persistence-local enum rather than a second
/// dependency edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    /// The backend-reported quality of `sample.pv` at the moment it was read. See
    /// [`SampleQuality`].
    pub pv_quality: SampleQuality,
}

impl TuneSampleRow {
    /// Records one tick of a run, taking the exact [`Tick`]/[`MrftState`] pair
    /// [`bhtune_core::mrft::MrftEngine::step`] produced, plus the [`SampleQuality`] the
    /// backend reported for `sample.pv` at read time. `(run_id, tick_index)` is unique (see
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

/// The proportional/integral/derivative values read back from the backend immediately after
/// a [`TuneWriteRow::insert_success`] write, to confirm the DCS actually accepted them. Not a
/// `bhtune-core` type like [`OpcWriteValues`]: this isn't a calculated/intended value, just
/// raw floats the caller read back from the backend after writing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WriteReadback {
    pub proportional: f32,
    pub integral: f32,
    pub derivative: f32,
}

impl TuneWriteRow {
    /// Records a successful PID write-back, including what was read back afterward to
    /// confirm the DCS accepted it. `written`'s own `response_level` becomes the row's
    /// `response_level` — one source of truth, rather than a second field that could
    /// disagree with it.
    pub async fn insert_success(
        pool: &SqlitePool,
        run_id: i64,
        written: OpcWriteValues,
        readback: WriteReadback,
        written_at: DateTime<Utc>,
    ) -> DbResult<TuneWriteRow> {
        let row = sqlx::query(
            r#"
            INSERT INTO tune_writes (
                run_id, response_level, written_at, proportional_written, integral_written,
                derivative_written, proportional_readback, integral_readback,
                derivative_readback, success
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1)
            RETURNING *
            "#,
        )
        .bind(run_id)
        .bind(enum_to_text(&written.response_level))
        .bind(written_at)
        .bind(written.proportional)
        .bind(written.integral)
        .bind(written.derivative)
        .bind(readback.proportional)
        .bind(readback.integral)
        .bind(readback.derivative)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_write(row)
    }

    /// Records a PID write-back that failed — the DCS rejected the value, or the write
    /// itself errored — before any readback was possible. Readback columns are left `NULL`.
    pub async fn insert_failure(
        pool: &SqlitePool,
        run_id: i64,
        written: OpcWriteValues,
        written_at: DateTime<Utc>,
        error_message: &str,
    ) -> DbResult<TuneWriteRow> {
        let row = sqlx::query(
            r#"
            INSERT INTO tune_writes (
                run_id, response_level, written_at, proportional_written, integral_written,
                derivative_written, success, error_message
            ) VALUES (?, ?, ?, ?, ?, ?, 0, ?)
            RETURNING *
            "#,
        )
        .bind(run_id)
        .bind(enum_to_text(&written.response_level))
        .bind(written_at)
        .bind(written.proportional)
        .bind(written.integral)
        .bind(written.derivative)
        .bind(error_message)
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
    Ok(TuneWriteRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        run_id: row.try_get("run_id").map_err(DbError::Query)?,
        response_level: text_to_enum("response_level", &response_level)?,
        written_at: row.try_get("written_at").map_err(DbError::Query)?,
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
