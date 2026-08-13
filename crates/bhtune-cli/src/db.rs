//! Opens the CLI's database and seeds the built-in DCS/PLC templates on every startup.

use std::path::Path;

use bhtune_db::SqlitePool;

/// Opens (creating if necessary) the database at `path`, running migrations, then upserts the
/// built-in templates via [`bhtune_db::seed_builtin_templates`] so a fresh database is
/// immediately usable without a separate setup step.
pub async fn open(path: &Path) -> anyhow::Result<SqlitePool> {
    let pool = bhtune_db::connect(path).await?;
    bhtune_db::seed_builtin_templates(&pool, chrono::Utc::now()).await?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_seeds_builtin_templates_on_a_fresh_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.db");
        let pool = open(&path).await.unwrap();
        let templates = bhtune_db::models::DcsTemplateRow::list(&pool)
            .await
            .unwrap();
        assert_eq!(templates.len(), 4);
    }

    #[tokio::test]
    async fn open_is_idempotent_across_repeated_calls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.db");
        open(&path).await.unwrap();
        let pool = open(&path).await.unwrap();
        let templates = bhtune_db::models::DcsTemplateRow::list(&pool)
            .await
            .unwrap();
        assert_eq!(templates.len(), 4);
    }
}
