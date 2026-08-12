//! `bhtune-db` — persistence.
//!
//! A single, plain, open SQLite database (via `sqlx`, WAL mode + busy timeout + foreign keys
//! enforced) holds everything: DCS/PLC templates, loop configuration, and tune run history.
//! There is deliberately no encryption and no licensing/usage-gating table — see AGENTS.md
//! for why.
//!
//! - [`pool`] — opens the database and runs migrations (`connect`/`connect_in_memory`). The
//!   only supported way to get a `SqlitePool` in bhtune.
//! - [`convert`] — maps `bhtune-core`'s `serde`-tagged enums to/from the `TEXT` columns
//!   SQLite stores them as.
//! - [`models`] — row types for every table in `migrations/0001_initial_schema.sql`.
//! - [`seed`] — upserts the built-in DCS/PLC templates on startup.
//! - [`error`] — the crate's error type, [`error::DbError`].
//!
//! Tables: `dcs_templates`, `loops`, `tune_runs`, `tune_samples`, `tune_results`,
//! `tune_writes`, `settings` — see the migration file for the full schema and the rationale
//! behind each design decision (nullability, JSON-vs-columns, cascade rules).

pub mod convert;
pub mod error;
pub mod models;
pub mod pool;
pub mod seed;

pub use error::{DbError, DbResult};
pub use pool::{connect, connect_in_memory};
pub use seed::{SeedOutcome, SeedResult, seed_builtin_templates};
