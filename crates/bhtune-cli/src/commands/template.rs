//! `bhtune template list/show/import/export/delete`.

use std::path::Path;

use bhtune_core::DcsTemplate;
use bhtune_db::SqlitePool;
use bhtune_db::models::{DcsTemplateRow, TemplateOrigin};

use crate::args::{TemplateCommand, TemplateFileFormat};

pub async fn run(pool: &SqlitePool, command: TemplateCommand) -> anyhow::Result<()> {
    match command {
        TemplateCommand::List => list(pool).await,
        TemplateCommand::Show { name } => show(pool, &name).await,
        TemplateCommand::Import { path } => import(pool, &path).await,
        TemplateCommand::Export { name, path, format } => export(pool, &name, &path, format).await,
        TemplateCommand::Delete { name } => delete(pool, &name).await,
    }
}

async fn list(pool: &SqlitePool) -> anyhow::Result<()> {
    let templates = DcsTemplateRow::list(pool).await?;
    if templates.is_empty() {
        println!("No templates found.");
        return Ok(());
    }
    println!(
        "{:<28} {:<8} {:<20} {:<12} {:<11} {:<11}",
        "NAME", "ORIGIN", "VERSIONS", "PROPORTIONAL", "INTEGRAL", "DERIVATIVE"
    );
    for row in templates {
        let versions = if row.template.versions.is_empty() {
            "-".to_string()
        } else {
            row.template.versions.join(", ")
        };
        println!(
            "{:<28} {:<8} {:<20} {:<12} {:<11} {:<11}",
            row.template.name,
            format!("{:?}", row.origin),
            versions,
            format!("{:?}", row.template.proportional_type),
            format!("{:?}", row.template.integral_type),
            format!("{:?}", row.template.derivative_type),
        );
    }
    Ok(())
}

async fn show(pool: &SqlitePool, name: &str) -> anyhow::Result<()> {
    let row = DcsTemplateRow::get_by_name(pool, name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no template named '{name}'"))?;
    println!("{}", serde_json::to_string_pretty(&row.template)?);
    Ok(())
}

/// A file "looks like" single-template JSON if, ignoring leading whitespace, it starts with
/// `{`. Real TOML catalog files always start with a comment (`#`), blank lines, or a
/// `[[template]]` table header -- never a bare `{` at the document root, since that isn't
/// valid top-level TOML syntax at all -- so this heuristic never misclassifies legitimate
/// content of either format. It exists purely so a malformed file gets a format-specific
/// parse error (matching what the user most likely intended to write) rather than always
/// showing the TOML parser's complaint about content that was actually meant as JSON.
fn looks_like_json_object(contents: &str) -> bool {
    contents.trim_start().starts_with('{')
}

/// Parses either a single-template JSON document or a multi-template TOML catalog without
/// touching the database. The shared helper is used by the import command and by the fuzz
/// target under `fuzz/`, keeping the high-risk format boundary independently testable.
pub fn parse_import_contents(contents: &str) -> anyhow::Result<Vec<DcsTemplate>> {
    if looks_like_json_object(contents) {
        let template: DcsTemplate = serde_json::from_str(contents)?;
        template.validate()?;
        Ok(vec![template])
    } else {
        Ok(bhtune_core::template::parse_catalog(contents)?)
    }
}

async fn import(pool: &SqlitePool, path: &Path) -> anyhow::Result<()> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read '{}': {e}", path.display()))?;

    if looks_like_json_object(&contents) {
        let template = parse_import_contents(&contents)
            .map_err(|e| {
                anyhow::anyhow!(
                    "'{}' is not a valid template JSON file: {e}",
                    path.display()
                )
            })?
            .remove(0);
        return import_one(pool, template).await;
    }

    let templates = parse_import_contents(&contents).map_err(|e| {
        anyhow::anyhow!(
            "'{}' is not a valid template TOML catalog: {e}",
            path.display()
        )
    })?;
    import_catalog(pool, templates).await
}

/// Imports a single template (the JSON path). Hard-fails on a name collision, matching this
/// command's existing behavior -- a JSON import is one deliberate template, so an existing
/// row with that name is treated as a mistake to fix, not something to silently skip.
async fn import_one(pool: &SqlitePool, template: DcsTemplate) -> anyhow::Result<()> {
    template
        .validate()
        .map_err(|e| anyhow::anyhow!("'{}' is not a valid template: {e}", template.name))?;

    if DcsTemplateRow::get_by_name(pool, &template.name)
        .await?
        .is_some()
    {
        anyhow::bail!(
            "a template named '{}' already exists; rename it in the file or remove the existing one first",
            template.name
        );
    }

    let row =
        DcsTemplateRow::insert(pool, &template, TemplateOrigin::User, chrono::Utc::now()).await?;
    println!("Imported template '{}' (id {}).", row.template.name, row.id);
    Ok(())
}

/// Imports a multi-template TOML catalog (a single-template TOML file is just a one-entry
/// catalog, so it goes through this same path). Best-effort rather than all-or-nothing: a
/// template whose name already exists is skipped and reported rather than aborting the
/// whole import, since the expected use is re-importing an updated community catalog file
/// that overlaps with templates already present -- the useful outcome is "add what's new",
/// not "fail because some of this was already here".
async fn import_catalog(pool: &SqlitePool, templates: Vec<DcsTemplate>) -> anyhow::Result<()> {
    if templates.is_empty() {
        println!("The catalog contained no templates.");
        return Ok(());
    }

    let now = chrono::Utc::now();
    let mut imported = Vec::new();
    let mut skipped = Vec::new();

    for template in templates {
        if DcsTemplateRow::get_by_name(pool, &template.name)
            .await?
            .is_some()
        {
            skipped.push(template.name);
            continue;
        }
        DcsTemplateRow::insert(pool, &template, TemplateOrigin::User, now).await?;
        imported.push(template.name);
    }

    for message in catalog_import_messages(&imported, &skipped) {
        println!("{message}");
    }
    Ok(())
}

fn catalog_import_messages(imported: &[String], skipped: &[String]) -> Vec<String> {
    let mut messages = Vec::new();
    if imported.is_empty() {
        messages.push("Imported no new templates.".to_string());
    } else {
        messages.push(format!(
            "Imported {} template{}: {}.",
            imported.len(),
            if imported.len() == 1 { "" } else { "s" },
            imported.join(", ")
        ));
    }
    if !skipped.is_empty() {
        messages.push(format!(
            "Skipped {} already-existing template{}: {}.",
            skipped.len(),
            if skipped.len() == 1 { "" } else { "s" },
            skipped.join(", ")
        ));
    }
    messages
}

async fn export(
    pool: &SqlitePool,
    name: &str,
    path: &Path,
    format: TemplateFileFormat,
) -> anyhow::Result<()> {
    let row = DcsTemplateRow::get_by_name(pool, name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no template named '{name}'"))?;

    let contents = match format {
        TemplateFileFormat::Json => serde_json::to_string_pretty(&row.template)?,
        TemplateFileFormat::Toml => bhtune_core::template::to_catalog_toml(vec![row.template])
            .map_err(|e| anyhow::anyhow!("failed to serialize template as TOML: {e}"))?,
    };
    std::fs::write(path, contents)
        .map_err(|e| anyhow::anyhow!("failed to write '{}': {e}", path.display()))?;
    println!("Exported template '{name}' to '{}'.", path.display());
    Ok(())
}

async fn delete(pool: &SqlitePool, name: &str) -> anyhow::Result<()> {
    let row = DcsTemplateRow::get_by_name(pool, name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no template named '{name}'"))?;

    match DcsTemplateRow::delete(pool, row.id).await {
        Ok(true) => {
            println!("Deleted template '{name}'.");
            match row.origin {
                TemplateOrigin::Builtin => println!(
                    "Note: '{name}' ships as a Builtin template and will be re-added \
                     automatically the next time bhtune starts."
                ),
                TemplateOrigin::Catalog => println!(
                    "Note: '{name}' comes from your user template catalog and will be \
                     re-added automatically the next time bhtune starts, unless it's also \
                     removed from that catalog file."
                ),
                TemplateOrigin::User => {}
            }
            Ok(())
        }
        // TOCTOU: something else deleted the row between the lookup above and here.
        Ok(false) => {
            println!("Template '{name}' was already deleted.");
            Ok(())
        }
        Err(e) => classify_delete_error(name, e),
    }
}

fn classify_delete_error(name: &str, error: bhtune_db::DbError) -> anyhow::Result<()> {
    match error {
        bhtune_db::DbError::TemplateInUse { .. } => anyhow::bail!(
            "cannot delete template '{name}': it is still referenced by one or more saved loops; delete or reassign those loops first"
        ),
        error => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    async fn seeded_pool() -> SqlitePool {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        bhtune_db::seed_builtin_templates(&pool, chrono::Utc::now())
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn list_runs_against_a_seeded_database() {
        let pool = seeded_pool().await;
        list(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn list_handles_an_empty_database() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        list(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn list_propagates_database_errors() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        pool.close().await;

        assert!(list(&pool).await.is_err());
    }

    #[tokio::test]
    async fn list_shows_a_dash_for_a_template_with_no_recorded_versions() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let mut template = bhtune_core::built_in_templates().remove(0);
        template.versions = Vec::new();
        DcsTemplateRow::insert(&pool, &template, TemplateOrigin::User, chrono::Utc::now())
            .await
            .unwrap();

        // Exercises the empty-`versions` "-" formatting branch; `list` itself only prints,
        // so success here (rather than a panic on the empty-vec join) is what matters.
        list(&pool).await.unwrap();
    }

    #[test]
    fn catalog_import_messages_report_imports_and_skips() {
        let imported = vec!["New Site".to_string()];
        let skipped = vec!["Existing Site".to_string(), "Another Site".to_string()];
        assert_eq!(
            catalog_import_messages(&imported, &skipped),
            vec![
                "Imported 1 template: New Site.".to_string(),
                "Skipped 2 already-existing templates: Existing Site, Another Site.".to_string(),
            ]
        );
    }

    #[test]
    fn catalog_import_messages_omit_the_skip_line_when_nothing_was_skipped() {
        assert_eq!(
            catalog_import_messages(&[], &[]),
            vec!["Imported no new templates.".to_string()]
        );
    }

    #[test]
    fn delete_passes_through_non_template_in_use_database_errors() {
        let error = classify_delete_error(
            "Broken",
            bhtune_db::DbError::InvalidBackup("test error".to_string()),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "not a valid bhtune backup file: test error"
        );
    }

    #[tokio::test]
    async fn show_prints_a_known_template() {
        let pool = seeded_pool().await;
        show(&pool, "Yokogawa CentumVP").await.unwrap();
    }

    #[tokio::test]
    async fn show_errors_for_an_unknown_template() {
        let pool = seeded_pool().await;
        let err = show(&pool, "Nonexistent").await.unwrap_err();
        assert!(err.to_string().contains("Nonexistent"));
    }

    #[tokio::test]
    async fn export_then_import_round_trips_under_a_new_name() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("template.json");

        export(&pool, "Yokogawa CentumVP", &path, TemplateFileFormat::Json)
            .await
            .unwrap();

        // Rename before importing, since the name already exists (seeded as builtin).
        let contents = std::fs::read_to_string(&path).unwrap();
        let mut template: DcsTemplate = serde_json::from_str(&contents).unwrap();
        template.name = "My Custom Site".to_string();
        std::fs::write(&path, serde_json::to_string_pretty(&template).unwrap()).unwrap();

        import(&pool, &path).await.unwrap();

        let row = DcsTemplateRow::get_by_name(&pool, "My Custom Site")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.origin, TemplateOrigin::User);
        assert_eq!(
            row.template.process_variable_suffix,
            template.process_variable_suffix
        );
    }

    #[tokio::test]
    async fn import_rejects_a_colliding_name() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("template.json");
        export(&pool, "Yokogawa CentumVP", &path, TemplateFileFormat::Json)
            .await
            .unwrap();

        let err = import(&pool, &path).await.unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn import_rejects_invalid_json() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{ not json").unwrap();

        let err = import(&pool, &path).await.unwrap_err();
        assert!(err.to_string().contains("not a valid template"));
    }

    #[tokio::test]
    async fn import_rejects_invalid_toml() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is not [[ valid toml").unwrap();

        let err = import(&pool, &path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("not a valid template TOML catalog")
        );
    }

    #[tokio::test]
    async fn import_rejects_a_json_template_that_fails_validation() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.json");

        let mut template = bhtune_core::built_in_templates().remove(0);
        template.name = "Incomplete Site".to_string();
        template.process_variable_suffix = String::new();
        std::fs::write(&path, serde_json::to_string_pretty(&template).unwrap()).unwrap();

        let err = import(&pool, &path).await.unwrap_err();
        assert!(err.to_string().contains("not a valid template"));
        assert!(
            DcsTemplateRow::get_by_name(&pool, "Incomplete Site")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn import_reports_a_missing_file_clearly() {
        let pool = seeded_pool().await;
        let err = import(&pool, Path::new("/nonexistent/template.json"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("failed to read"));
    }

    #[tokio::test]
    async fn export_errors_for_an_unknown_template() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("template.json");
        let err = export(&pool, "Nonexistent", &path, TemplateFileFormat::Json)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Nonexistent"));
    }

    #[tokio::test]
    async fn export_toml_then_import_round_trips_under_a_new_name() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("template.toml");

        export(&pool, "Yokogawa CentumVP", &path, TemplateFileFormat::Toml)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.matches("[[template]]").count(), 1);

        // Rename before importing, since the name already exists (seeded as builtin).
        let renamed = contents.replacen("Yokogawa CentumVP", "My TOML Site", 1);
        std::fs::write(&path, renamed).unwrap();

        import(&pool, &path).await.unwrap();

        let row = DcsTemplateRow::get_by_name(&pool, "My TOML Site")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.origin, TemplateOrigin::User);
    }

    #[tokio::test]
    async fn import_toml_catalog_adds_multiple_new_templates_and_skips_existing_ones() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.toml");

        // One brand-new template plus one whose name collides with a seeded builtin --
        // the catalog import must add the former and skip (not fail on) the latter.
        let mut templates = bhtune_core::built_in_templates();
        templates.truncate(1);
        assert_eq!(templates[0].name, "Yokogawa CentumVP");
        let mut new_template = templates[0].clone();
        new_template.name = "Brand New Site".to_string();
        templates.push(new_template);

        let toml = bhtune_core::template::to_catalog_toml(templates).unwrap();
        assert_eq!(toml.matches("[[template]]").count(), 2);
        std::fs::write(&path, toml).unwrap();

        import(&pool, &path).await.unwrap();

        assert!(
            DcsTemplateRow::get_by_name(&pool, "Brand New Site")
                .await
                .unwrap()
                .is_some()
        );
        // The colliding "Yokogawa CentumVP" entry must not have been touched/duplicated.
        let all = DcsTemplateRow::list(&pool).await.unwrap();
        assert_eq!(
            all.iter()
                .filter(|row| row.template.name == "Yokogawa CentumVP")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn import_toml_catalog_with_no_templates_is_a_no_op() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.toml");
        std::fs::write(&path, "template = []").unwrap();

        import(&pool, &path).await.unwrap();
    }

    #[tokio::test]
    async fn import_toml_catalog_where_every_template_already_exists_imports_nothing() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("all_existing.toml");

        // The seeded pool already has every built-in template by name, so re-importing the
        // exact same catalog must skip every entry and import none -- exercising the "0
        // imported" reporting branch distinctly from the "some imported, some skipped" case
        // covered above.
        let toml =
            bhtune_core::template::to_catalog_toml(bhtune_core::built_in_templates()).unwrap();
        std::fs::write(&path, toml).unwrap();

        let before = DcsTemplateRow::list(&pool).await.unwrap().len();
        import(&pool, &path).await.unwrap();
        let after = DcsTemplateRow::list(&pool).await.unwrap().len();
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn delete_removes_an_unreferenced_user_template() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("template.json");
        export(&pool, "Yokogawa CentumVP", &path, TemplateFileFormat::Json)
            .await
            .unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let mut template: DcsTemplate = serde_json::from_str(&contents).unwrap();
        template.name = "Deletable Site".to_string();
        std::fs::write(&path, serde_json::to_string_pretty(&template).unwrap()).unwrap();
        import(&pool, &path).await.unwrap();

        delete(&pool, "Deletable Site").await.unwrap();

        assert!(
            DcsTemplateRow::get_by_name(&pool, "Deletable Site")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn delete_reports_a_row_removed_by_a_concurrent_delete_as_already_deleted() {
        let pool = seeded_pool().await;
        sqlx::query(
            "CREATE TRIGGER remove_before_delete
             BEFORE DELETE ON dcs_templates
             BEGIN
                 DELETE FROM dcs_templates WHERE name = 'Yokogawa CentumVP';
             END",
        )
        .execute(&pool)
        .await
        .unwrap();

        delete(&pool, "Yokogawa CentumVP").await.unwrap();
        assert!(
            DcsTemplateRow::get_by_name(&pool, "Yokogawa CentumVP")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn delete_succeeds_for_a_builtin_template() {
        let pool = seeded_pool().await;
        delete(&pool, "Yokogawa CentumVP").await.unwrap();
        assert!(
            DcsTemplateRow::get_by_name(&pool, "Yokogawa CentumVP")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn delete_succeeds_for_a_catalog_origin_template() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let mut template = bhtune_core::built_in_templates().remove(0);
        template.name = "User Catalog Site".to_string();
        DcsTemplateRow::insert(
            &pool,
            &template,
            TemplateOrigin::Catalog,
            chrono::Utc::now(),
        )
        .await
        .unwrap();

        // Exercises the Catalog-origin reseed note branch distinctly from Builtin/User.
        delete(&pool, "User Catalog Site").await.unwrap();

        assert!(
            DcsTemplateRow::get_by_name(&pool, "User Catalog Site")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn delete_errors_for_an_unknown_template() {
        let pool = seeded_pool().await;
        let err = delete(&pool, "Nonexistent").await.unwrap_err();
        assert!(err.to_string().contains("Nonexistent"));
    }

    #[tokio::test]
    async fn delete_refuses_a_template_still_referenced_by_a_loop() {
        let pool = seeded_pool().await;
        let row = DcsTemplateRow::get_by_name(&pool, "Yokogawa CentumVP")
            .await
            .unwrap()
            .unwrap();
        let now = chrono::Utc::now();
        sqlx::query(
            r#"
            INSERT INTO loops (
                name, dcs_template_id, tags_json, process_type, controller_type,
                relay_amp_percent, num_cycles_skip, num_cycles_count, noise_protection_secs,
                mrft_delay_secs, created_at, updated_at
            ) VALUES ('LIC101', ?, '{}', 'flow', 'pi', 5.0, 1, 2, 3, 0, ?, ?)
            "#,
        )
        .bind(row.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let err = delete(&pool, "Yokogawa CentumVP").await.unwrap_err();
        assert!(err.to_string().contains("still referenced"));
        assert!(
            DcsTemplateRow::get_by_name(&pool, "Yokogawa CentumVP")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn run_dispatches_every_subcommand() {
        let pool = seeded_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("template.json");

        run(&pool, TemplateCommand::List).await.unwrap();
        run(
            &pool,
            TemplateCommand::Show {
                name: "Yokogawa CentumVP".to_string(),
            },
        )
        .await
        .unwrap();
        run(
            &pool,
            TemplateCommand::Export {
                name: "Yokogawa CentumVP".to_string(),
                path: path.clone(),
                format: TemplateFileFormat::Json,
            },
        )
        .await
        .unwrap();

        // Rename before importing, since the name already exists (seeded as builtin).
        let contents = std::fs::read_to_string(&path).unwrap();
        let mut template: DcsTemplate = serde_json::from_str(&contents).unwrap();
        template.name = "Dispatch Import Target".to_string();
        std::fs::write(&path, serde_json::to_string_pretty(&template).unwrap()).unwrap();

        run(&pool, TemplateCommand::Import { path }).await.unwrap();

        assert!(
            DcsTemplateRow::get_by_name(&pool, "Dispatch Import Target")
                .await
                .unwrap()
                .is_some()
        );

        run(
            &pool,
            TemplateCommand::Delete {
                name: "Dispatch Import Target".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(
            DcsTemplateRow::get_by_name(&pool, "Dispatch Import Target")
                .await
                .unwrap()
                .is_none()
        );
    }

    proptest::proptest! {
        #[test]
        fn exported_json_templates_parse_as_imports(
            name in "[A-Za-z][A-Za-z0-9 _-]{0,24}",
        ) {
            let mut template = bhtune_core::built_in_templates().remove(0);
            template.name = name;
            let encoded = serde_json::to_string(&template).unwrap();
            prop_assert_eq!(parse_import_contents(&encoded).unwrap(), vec![template]);
        }

        #[test]
        fn exported_toml_catalogs_parse_as_imports(
            name in "[A-Za-z][A-Za-z0-9 _-]{0,24}",
        ) {
            let mut template = bhtune_core::built_in_templates().remove(0);
            template.name = name;
            let encoded = bhtune_core::template::to_catalog_toml(vec![template.clone()]).unwrap();
            prop_assert_eq!(parse_import_contents(&encoded).unwrap(), vec![template]);
        }

        #[test]
        fn arbitrary_import_text_never_panics(input in any::<String>()) {
            let _ = parse_import_contents(&input);
        }
    }
}
