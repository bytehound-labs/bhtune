use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::error::{DbError, DbResult};

// settings {{{1

/// One row of `settings`: an app-wide key/value pair (e.g. the history retention policy).
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
