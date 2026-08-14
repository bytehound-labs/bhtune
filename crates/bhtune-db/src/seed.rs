//! Seeds the built-in DCS/PLC templates (`bhtune_core::built_in_templates()`) into
//! `dcs_templates` on startup, so a fresh database always has the four presets available
//! without a separate "first run" wizard step.
//!
//! This is an upsert, not a plain insert, because a template's suffix/unit conventions can
//! be corrected in a later bhtune release, and an existing install's `dcs_templates` table
//! should pick up that fix on upgrade rather than being frozen at whatever shipped when the
//! row was first created.

use bhtune_core::built_in_templates;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::{error::DbResult, models::DcsTemplateRow};

/// What [`seed_builtin_templates`] did with one built-in template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedOutcome {
    /// No row existed with this name; a new `is_builtin = 1` row was inserted.
    Inserted,
    /// A row already existed with `is_builtin = 1`; its fields were overwritten to match the
    /// current built-in definition.
    Updated,
    /// A row already existed with this name but `is_builtin = 0` — some past version of
    /// this function, or a user, created a custom template that happens to share a name
    /// with a built-in one. Left untouched: a user's own template is never silently
    /// overwritten, even if its name collides with a preset's.
    SkippedUserOwned,
}

/// One template's seeding result, returned so a caller (`cli-commands`, the desktop app's
/// startup routine) can log what happened — `bhtune-db` itself has no logging dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedResult {
    pub name: String,
    pub outcome: SeedOutcome,
}

/// Upserts every [`bhtune_core::built_in_templates`] entry into `dcs_templates`.
///
/// Safe to call on every startup: inserts any missing built-in, brings existing
/// `is_builtin = 1` rows in line with the current definition, and never touches a row whose
/// name collides with a built-in but which isn't itself marked `is_builtin`.
pub async fn seed_builtin_templates(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> DbResult<Vec<SeedResult>> {
    let mut results = Vec::new();

    for template in built_in_templates() {
        let outcome = match DcsTemplateRow::get_by_name(pool, &template.name).await? {
            None => {
                DcsTemplateRow::insert(pool, &template, true, now).await?;
                SeedOutcome::Inserted
            }
            Some(existing) if existing.is_builtin => {
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
        assert!(rows.iter().all(|r| r.is_builtin));

        // Every seeded row round-trips back to exactly the template that was seeded --
        // except `versions`/`description`/`source`, which `dcs_templates` has no columns
        // for yet (`template-provenance` adds `versions_json`/`description`/`source` and
        // makes this a full round-trip; until then `row_to_dcs_template` always returns
        // them empty/`None`, so this test normalizes them away rather than losing coverage
        // of every other field).
        for mut template in built_in_templates() {
            let row = DcsTemplateRow::get_by_name(&pool, &template.name)
                .await
                .unwrap()
                .expect("seeded template should be found by name");
            template.versions = Vec::new();
            template.description = None;
            template.source = None;
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
        let inserted = DcsTemplateRow::insert(&pool, &custom, false, now())
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
        assert!(!still_custom.is_builtin);
    }
}
