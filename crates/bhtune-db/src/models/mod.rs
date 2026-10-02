//! Row types mirroring the tables in `migrations/`.
//!
//! These are deliberately *typed*, not raw-column, shapes: wherever a table's columns are a
//! clean 1:1 match for an existing `bhtune-core` type (`DcsTemplate`, `LoopTags`,
//! `LoopConfig`, `Tick` + `MrftState`), the row struct holds that type directly rather than
//! re-declaring its fields — one less place for the two to drift apart. Where a table
//! combines fields from two `bhtune-core` types that both carry their own overlapping field
//! (`TuningResult`/`PidParameters` both carry `response_level`; `InitialReadings`/`PvRange`
//! don't nest cleanly with the extra `controller_direction` column), the row struct is flat
//! instead, matching the table exactly and avoiding a redundant, only-sometimes-consistent
//! duplicate field.
//!
//! [`DemoSessionRow`], [`DcsTemplateRow`], and
//! [`TuneRunRow`]/[`TuneSampleRow`]/[`TuneResultRow`]/
//! [`TuneMvActuationRow`]/[`TuneWriteRow`] have full repository methods (insert, lifecycle
//! transitions, filtering, pagination). [`LoopRow`] deliberately has no repository methods:
//! full CRUD for saved loops (list/update/delete) is separate from run history, which is about
//! *runs*, not the loops they reference. Tests construct `loops` rows with raw SQL (see
//! `tests/schema.rs`'s `seed_loop` helper) purely as foreign-key setup.
//!
//! [`TuneRunRow::list`]/[`TuneRunRow::count`] build their `WHERE` clause dynamically with
//! `sqlx::QueryBuilder`, since [`TuneRunFilter`]'s fields are all optional and the set of
//! active conditions varies per call — a fixed `query!` string can't express that, and
//! `bhtune-db` uses runtime `query`/`query_as` throughout anyway (see `Cargo.toml`), so this
//! doesn't introduce a new query style, just the first dynamic one.

mod dcs_templates;
mod demo_sessions;
mod loops;
mod settings;
mod tune_mv_actuations;
mod tune_results;
mod tune_runs;
mod tune_samples;
mod tune_writes;

pub use dcs_templates::*;
pub use demo_sessions::*;
pub use loops::*;
pub use settings::*;
pub use tune_mv_actuations::*;
pub use tune_results::*;
pub use tune_runs::*;
pub use tune_samples::*;
pub use tune_writes::*;
