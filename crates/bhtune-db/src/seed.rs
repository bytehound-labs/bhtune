//! Seeds a catalog of DCS/PLC templates into `dcs_templates` on startup, so a fresh database
//! always has the built-in presets available without a separate "first run" wizard step, and
//! so a future user-supplied catalog file can be kept in sync the same way.
//!
//! This is an upsert, not a plain insert, because a template's suffix/unit conventions can
//! be corrected in a later catalog revision, and an existing install's `dcs_templates` table
//! should pick up that fix on the next seed rather than being frozen at whatever was current
//! when the row was first created.

use bhtune_core::{DcsTemplate, built_in_templates};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::{
    error::DbResult,
    models::{DcsTemplateRow, TemplateOrigin},
};

/// What [`seed_templates`] did with one catalog template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedOutcome {
    /// No row existed with this name; a new row was inserted with the seeded `origin`.
    Inserted,
    /// A row already existed with the seeded `origin`; its fields were overwritten to match
    /// the current catalog definition.
    Updated,
    /// A row already existed with this name but a *different* `origin` — some other catalog
    /// (or a user, via `bhtune template import`) created a template that happens to share a
    /// name with one in this catalog. Left untouched: a row is never silently overwritten by
    /// a seed pass it doesn't belong to, even if its name collides with one that does.
    SkippedUserOwned,
}

/// One template's seeding result, returned so a caller (`cli-commands`, the web GUI's
/// startup routine) can log what happened — `bhtune-db` itself has no logging dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedResult {
    pub name: String,
    pub outcome: SeedOutcome,
}

/// Upserts every template in `templates` into `dcs_templates`, all attributed to `origin`.
///
/// Safe to call on every startup: inserts any missing template, brings existing rows that
/// already carry `origin` in line with the current definition, and never touches a row whose
/// name collides with one being seeded but which carries a *different* `origin` — that row
/// belongs to a different catalog (or a user), not this seed pass.
///
/// The one caller today is [`seed_builtin_templates`], seeding
/// [`bhtune_core::built_in_templates`] with [`TemplateOrigin::Builtin`]. `template-user-catalog`
/// will be the first caller to seed a user-supplied catalog file with [`TemplateOrigin::Catalog`],
/// reusing this exact upsert logic rather than duplicating it.
pub async fn seed_templates(
    pool: &SqlitePool,
    templates: Vec<DcsTemplate>,
    origin: TemplateOrigin,
    now: DateTime<Utc>,
) -> DbResult<Vec<SeedResult>> {
    let mut results = Vec::new();

    for template in templates {
        let outcome = match DcsTemplateRow::get_by_name(pool, &template.name).await? {
            None => {
                DcsTemplateRow::insert(pool, &template, origin, now).await?;
                SeedOutcome::Inserted
            }
            Some(existing) if existing.origin == origin => {
                DcsTemplateRow::update(pool, existing.id, &template, now).await?;
                SeedOutcome::Updated
            }
            Some(_) => SeedOutcome::SkippedUserOwned,
        };

        results.push(SeedResult {
            name: template.name,
            outcome,
        });
    }

    Ok(results)
}

/// Upserts every [`bhtune_core::built_in_templates`] entry into `dcs_templates` with
/// [`TemplateOrigin::Builtin`]. A thin, ergonomic wrapper around [`seed_templates`] for the
/// common startup case — see its docs for the upsert semantics.
pub async fn seed_builtin_templates(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> DbResult<Vec<SeedResult>> {
    seed_templates(pool, built_in_templates(), TemplateOrigin::Builtin, now).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::connect_in_memory;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    #[tokio::test]
    async fn seeds_all_builtins_into_empty_database() {
        let pool = connect_in_memory().await.unwrap();

        let results = seed_builtin_templates(&pool, now()).await.unwrap();

        assert_eq!(results.len(), built_in_templates().len());
        assert!(results.iter().all(|r| r.outcome == SeedOutcome::Inserted));

        let rows = DcsTemplateRow::list(&pool).await.unwrap();
        assert_eq!(rows.len(), built_in_templates().len());
        assert!(rows.iter().all(|r| r.origin == TemplateOrigin::Builtin));

        // Every seeded row round-trips back to exactly the template that was seeded.
        for template in built_in_templates() {
            let row = DcsTemplateRow::get_by_name(&pool, &template.name)
                .await
                .unwrap()
                .expect("seeded template should be found by name");
            assert_eq!(row.template, template);
        }
    }

    #[tokio::test]
    async fn reseeding_is_idempotent_and_does_not_duplicate() {
        let pool = connect_in_memory().await.unwrap();

        seed_builtin_templates(&pool, now()).await.unwrap();
        let second = seed_builtin_templates(&pool, now()).await.unwrap();

        assert!(second.iter().all(|r| r.outcome == SeedOutcome::Updated));
        let rows = DcsTemplateRow::list(&pool).await.unwrap();
        assert_eq!(rows.len(), built_in_templates().len());
    }

    #[tokio::test]
    async fn reseeding_corrects_a_drifted_builtin_row_in_place() {
        let pool = connect_in_memory().await.unwrap();
        seed_builtin_templates(&pool, now()).await.unwrap();

        // Simulate an older/corrupted row: hand-edit one builtin's suffix away from the
        // canonical value.
        let existing = DcsTemplateRow::get_by_name(&pool, "Yokogawa CentumVP")
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE dcs_templates SET manipulated_variable_suffix = 'WRONG' WHERE id = ?")
            .bind(existing.id)
            .execute(&pool)
            .await
            .unwrap();

        seed_builtin_templates(&pool, now()).await.unwrap();

        let corrected = DcsTemplateRow::get_by_name(&pool, "Yokogawa CentumVP")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            corrected.id, existing.id,
            "must update in place, not re-insert"
        );
        assert_eq!(corrected.template.manipulated_variable_suffix, "MV");
    }

    #[tokio::test]
    async fn never_overwrites_a_user_owned_row_with_a_colliding_name() {
        let pool = connect_in_memory().await.unwrap();

        let mut custom = built_in_templates().remove(0); // "Yokogawa CentumVP"
        custom.manipulated_variable_suffix = "CUSTOM_MV".to_string();
        let inserted = DcsTemplateRow::insert(&pool, &custom, TemplateOrigin::User, now())
            .await
            .unwrap();

        let results = seed_builtin_templates(&pool, now()).await.unwrap();

        let yokogawa_result = results
            .iter()
            .find(|r| r.name == "Yokogawa CentumVP")
            .unwrap();
        assert_eq!(yokogawa_result.outcome, SeedOutcome::SkippedUserOwned);

        let still_custom = DcsTemplateRow::get(&pool, inserted.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            still_custom.template.manipulated_variable_suffix,
            "CUSTOM_MV"
        );
        assert_eq!(still_custom.origin, TemplateOrigin::User);
    }
}
