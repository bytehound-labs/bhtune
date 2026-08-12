//! `bhtune-db`'s error type.

/// Everything that can go wrong opening the database, running migrations, or executing a
/// query.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("failed to open or connect to the database: {0}")]
    Connect(#[source] sqlx::Error),

    #[error("failed to run database migrations: {0}")]
    Migrate(#[source] sqlx::migrate::MigrateError),

    #[error("database query failed: {0}")]
    Query(#[source] sqlx::Error),

    /// A value read back from a TEXT enum column didn't match any known
    /// variant. This can only happen if the row was written by something
    /// other than this crate (or a future version wrote a variant this
    /// version doesn't know about) — the `CHECK` constraints on every
    /// enum-shaped column prevent the database itself from ever storing an
    /// invalid value.
    #[error("column {column:?} held an unrecognized value: {value:?}")]
    InvalidEnumValue { column: &'static str, value: String },
}

pub type DbResult<T> = Result<T, DbError>;
