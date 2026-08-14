//! `bhtune template list/show/import/export`.

use std::path::Path;

use bhtune_core::DcsTemplate;
use bhtune_db::SqlitePool;
use bhtune_db::models::{DcsTemplateRow, TemplateOrigin};

use crate::args::TemplateCommand;

pub async fn run(pool: &SqlitePool, command: TemplateCommand) -> anyhow::Result<()> {
    match command {
        TemplateCommand::List => list(pool).await,
        TemplateCommand::Show { name } => show(pool, &name).await,
        TemplateCommand::Import { path } => import(pool, &path).await,
        TemplateCommand::Export { name, path } => export(pool, &name, &path).await,
    }
}

async fn list(pool: &SqlitePool) -> anyhow::Result<()> {
    let templates = DcsTemplateRow::list(pool).await?;
    if templates.is_empty() {
        println!("No templates found.");
        return Ok(());
    }
    println!(
        "{:<28} {:<8} {:<12} {:<11} {:<11}",
        "NAME", "ORIGIN", "PROPORTIONAL", "INTEGRAL", "DERIVATIVE"
    );
    for row in templates {
        println!(
            "{:<28} {:<8} {:<12} {:<11} {:<11}",
            row.template.name,
            format!("{:?}", row.origin),
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

async fn import(pool: &SqlitePool, path: &Path) -> anyhow::Result<()> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read '{}': {e}", path.display()))?;
    let template: DcsTemplate = serde_json::from_str(&contents).map_err(|e| {
        anyhow::anyhow!(
            "'{}' is not a valid template JSON file: {e}",
            path.display()
        )
    })?;

    if DcsTemplateRow::get_by_name(pool, &template.name)
        .await?
        .is_some()
    {
        anyhow::bail!(
            "a template named '{}' already exists; rename it in the JSON file or remove the existing one first",
            template.name
        );
    }

    let row =
        DcsTemplateRow::insert(pool, &template, TemplateOrigin::User, chrono::Utc::now()).await?;
    println!("Imported template '{}' (id {}).", row.template.name, row.id);
    Ok(())
}

async fn export(pool: &SqlitePool, name: &str, path: &Path) -> anyhow::Result<()> {
    let row = DcsTemplateRow::get_by_name(pool, name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no template named '{name}'"))?;
    let json = serde_json::to_string_pretty(&row.template)?;
    std::fs::write(path, json)
        .map_err(|e| anyhow::anyhow!("failed to write '{}': {e}", path.display()))?;
    println!("Exported template '{name}' to '{}'.", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

        export(&pool, "Yokogawa CentumVP", &path).await.unwrap();

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
        export(&pool, "Yokogawa CentumVP", &path).await.unwrap();

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
        let err = export(&pool, "Nonexistent", &path).await.unwrap_err();
        assert!(err.to_string().contains("Nonexistent"));
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
    }
}
