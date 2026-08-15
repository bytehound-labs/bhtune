//! Opens the CLI's database and seeds the built-in and user-catalog DCS/PLC templates on
//! every startup.

use std::path::Path;

use bhtune_core::DcsTemplate;
use bhtune_db::SqlitePool;
use bhtune_db::models::TemplateOrigin;

/// Opens (creating if necessary) the database at `path`, running migrations, then upserts the
/// built-in templates via [`bhtune_db::seed_builtin_templates`] so a fresh database is
/// immediately usable without a separate setup step. If `user_templates` is `Some` (the
/// caller found and parsed a user catalog file -- see `crate::config::load_user_templates`,
/// `template-user-catalog`), those templates are additionally upserted with
/// [`TemplateOrigin::Catalog`] via [`bhtune_db::seed_templates`]. `None` means no user
/// catalog file was found at all, which is not an error and simply skips this second seed
/// pass -- the common case, since most installs never create `templates.toml`.
///
/// Creates `path`'s parent directory tree first: `bhtune_db::connect`'s
/// `SqliteConnectOptions::create_if_missing(true)` only creates the database *file*, not any
/// missing parent directories -- necessary now that the default database path (see
/// `crate::config::default_db_path_from`) is a nested, not-yet-existing platform directory
/// (e.g. `~/.local/share/bhtune/`) on a genuinely fresh install.
pub async fn open(
    path: &Path,
    user_templates: Option<Vec<DcsTemplate>>,
) -> anyhow::Result<SqlitePool> {
    tracing::info!(db_path = %path.display(), "opening database");
    ensure_parent_dir(path)?;
    let pool = bhtune_db::connect(path).await?;
    let now = chrono::Utc::now();
    let seeded = bhtune_db::seed_builtin_templates(&pool, now).await?;
    tracing::debug!(templates = seeded.len(), "seeded built-in DCS templates");
    if let Some(templates) = user_templates {
        let seeded =
            bhtune_db::seed_templates(&pool, templates, TemplateOrigin::Catalog, now).await?;
        let count = seeded.len();
        tracing::debug!(templates = count, "seeded user catalog DCS templates");
    }
    Ok(pool)
}

/// Creates `path`'s parent directory tree if it doesn't already exist. A no-op (not an
/// error) for a bare filename with no directory component at all -- `Path::parent()` returns
/// `Some("")` in that case, and `std::fs::create_dir_all("")` is a documented no-op success,
/// so this never needs to special-case "no parent" separately from "empty parent".
fn ensure_parent_dir(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("failed to create database directory {parent:?}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhtune_db::models::DcsTemplateRow;

    fn sample_catalog_template() -> DcsTemplate {
        let mut template = bhtune_core::built_in_templates().remove(0);
        template.name = "Test Catalog Template".to_string();
        template
    }

    #[tokio::test]
    async fn open_seeds_builtin_templates_on_a_fresh_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.db");
        let pool = open(&path, None).await.unwrap();
        let templates = bhtune_db::models::DcsTemplateRow::list(&pool)
            .await
            .unwrap();
        assert_eq!(templates.len(), 4);
    }

    #[tokio::test]
    async fn open_is_idempotent_across_repeated_calls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.db");
        open(&path, None).await.unwrap();
        let pool = open(&path, None).await.unwrap();
        let templates = bhtune_db::models::DcsTemplateRow::list(&pool)
            .await
            .unwrap();
        assert_eq!(templates.len(), 4);
    }

    #[tokio::test]
    async fn open_creates_missing_nested_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deeper").join("bhtune.db");
        assert!(!path.parent().unwrap().exists());
        let pool = open(&path, None).await.unwrap();
        assert!(path.exists());
        let templates = bhtune_db::models::DcsTemplateRow::list(&pool)
            .await
            .unwrap();
        assert_eq!(templates.len(), 4);
    }

    #[tokio::test]
    async fn open_seeds_user_catalog_templates_with_catalog_origin_when_provided() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.db");
        let pool = open(&path, Some(vec![sample_catalog_template()]))
            .await
            .unwrap();
        let templates = DcsTemplateRow::list(&pool).await.unwrap();
        assert_eq!(templates.len(), 5);
        let seeded = templates
            .iter()
            .find(|t| t.template.name == "Test Catalog Template")
            .expect("user catalog template should have been seeded");
        assert_eq!(seeded.origin, TemplateOrigin::Catalog);
    }

    #[tokio::test]
    async fn open_reseeding_the_same_user_catalog_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.db");
        open(&path, Some(vec![sample_catalog_template()]))
            .await
            .unwrap();
        let pool = open(&path, Some(vec![sample_catalog_template()]))
            .await
            .unwrap();
        let templates = DcsTemplateRow::list(&pool).await.unwrap();
        assert_eq!(templates.len(), 5);
    }

    #[test]
    fn ensure_parent_dir_is_a_no_op_for_a_bare_filename() {
        // Doesn't touch the real filesystem at all -- `Path::new("bhtune.db").parent()` is
        // `Some("")`, and `create_dir_all("")` is a documented no-op success -- so this is
        // safe to call regardless of the test process's current working directory.
        ensure_parent_dir(Path::new("bhtune.db")).unwrap();
    }
}
