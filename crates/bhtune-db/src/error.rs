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

    /// A JSON column (`tune_runs.template_snapshot_json`/`tags_json`) held syntactically
    /// valid JSON -- the schema's `CHECK (json_valid(...))` already guarantees that much --
    /// but it didn't deserialize into the Rust shape the column is supposed to hold. This
    /// can happen honestly, not just from external tampering: an older snapshot can predate
    /// a field a later `bhtune-core` release added to `DcsTemplate`/`LoopTags`.
    #[error("column {column:?} held JSON that didn't match the expected shape: {source}")]
    InvalidJsonShape {
        column: &'static str,
        #[source]
        source: serde_json::Error,
    },

    /// [`crate::backup::backup_to`]'s destination already exists. Refused rather than
    /// silently overwritten — clobbering a previous backup because of a reused filename
    /// would itself be a data-loss bug.
    #[error("backup destination already exists: {}", .0.display())]
    BackupDestinationExists(std::path::PathBuf),

    /// [`crate::backup::restore_from`] (or [`crate::backup::backup_to`]'s own post-write
    /// check) determined a file is not a usable bhtune backup: it failed
    /// `PRAGMA integrity_check`, or it doesn't contain a `tune_runs` table.
    #[error("not a valid bhtune backup file: {0}")]
    InvalidBackup(String),

    /// A filesystem operation during [`crate::backup::backup_to`]/
    /// [`crate::backup::restore_from`] (copying, renaming, or removing a file) failed.
    #[error("backup/restore file operation failed: {0}")]
    Io(#[source] std::io::Error),
}

pub type DbResult<T> = Result<T, DbError>;
