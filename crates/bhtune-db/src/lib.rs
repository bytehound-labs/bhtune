//! `bhtune-db` — persistence.
//!
//! A single, plain, open SQLite database (via `sqlx`, WAL mode + busy
//! timeout) holds everything: DCS/PLC templates, loop configuration, and
//! tune run history. There is deliberately no encryption and no
//! licensing/usage-gating table — see AGENTS.md for why.
//!
//! Planned tables: `dcs_templates`, `loops`, `tune_runs`, `tune_samples`,
//! `tune_results`, `settings`.
//!
//! Not yet implemented — this crate is scaffolding only until the
//! `db-schema` phase.
