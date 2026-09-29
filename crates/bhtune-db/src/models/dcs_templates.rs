use bhtune_core::DcsTemplate;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::{
    convert::{enum_to_text, json_text, text_to_enum},
    error::{DbError, DbResult},
};

#[cfg(doc)]
use super::tune_runs::TuneRunRow;

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
        let versions_json = json_text("template versions", &template.versions)?;
        let row = sqlx::query(
            r"
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
            ",
        )
        .bind(&template.name)
        .bind(enum_to_text(&origin)?)
        .bind(template.revert_mode)
        .bind(enum_to_text(&template.proportional_type)?)
        .bind(enum_to_text(&template.integral_type)?)
        .bind(enum_to_text(&template.integral_unit)?)
        .bind(enum_to_text(&template.derivative_type)?)
        .bind(enum_to_text(&template.derivative_unit)?)
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
        let versions_json = json_text("template versions", &template.versions)?;
        let row = sqlx::query(
            r"
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
            ",
        )
        .bind(template.revert_mode)
        .bind(enum_to_text(&template.proportional_type)?)
        .bind(enum_to_text(&template.integral_type)?)
        .bind(enum_to_text(&template.integral_unit)?)
        .bind(enum_to_text(&template.derivative_type)?)
        .bind(enum_to_text(&template.derivative_unit)?)
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
