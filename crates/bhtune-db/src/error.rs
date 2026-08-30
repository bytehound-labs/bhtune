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

    /// An MV-actuation finalization API was given [`crate::models::MvActuationStatus::Pending`].
    /// Pending is the initial state inserted by
    /// [`crate::models::TuneMvActuationRow::insert_pending`], not a terminal result.
    #[error("pending is not a terminal MV actuation status")]
    InvalidMvActuationFinalStatus,

    /// A JSON column (`tune_runs.template_snapshot_json`/`tags_json`/
    /// `timing_metrics_json`, `dcs_templates.versions_json`) held syntactically valid JSON
    /// -- the schema's `CHECK (json_valid(...))` already guarantees that much -- but it
    /// didn't deserialize into the Rust shape the column is supposed to hold. This can
    /// happen honestly, not just from external tampering: an older snapshot can predate a
    /// field a later release added to its typed representation.
    #[error("column {column:?} held JSON that didn't match the expected shape: {source}")]
    InvalidJsonShape {
        column: &'static str,
        #[source]
        source: serde_json::Error,
    },

    /// [`crate::models::DcsTemplateRow::delete`] targeted a template that at least one
    /// `loops` row still references. The schema's `ON DELETE RESTRICT` foreign key is what
    /// actually enforces this; this variant exists so `bhtune-cli`'s `template delete` can
    /// turn the resulting SQLite foreign-key-violation error into a message naming the
    /// template rather than a raw SQL error, without needing its own `sqlx` dependency just
    /// to inspect the error kind.
    #[error("template {id} is still referenced by one or more saved loops and cannot be deleted")]
    TemplateInUse { id: i64 },

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

    /// [`crate::backup::restore_from`] found another connection (in this process or
    /// another) still attached to the live database. Restoring while that's true risks
    /// desynchronizing whatever holds it: its view of the file would silently stop matching
    /// what's on disk the moment the restore replaces it.
    #[error(
        "database at {} appears to be in use by another connection or process -- close it before restoring",
        .0.display()
    )]
    DatabaseInUse(std::path::PathBuf),
}

pub type DbResult<T> = Result<T, DbError>;
