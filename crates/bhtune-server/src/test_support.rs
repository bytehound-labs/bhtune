//! Shared test-only helpers for building an [`AppState`] backed by a seeded, in-memory
//! database -- every route test module needs one, so it lives here rather than being
//! copy-pasted per module.

#![cfg(test)]

use chrono::Utc;

use crate::state::AppState;

/// An in-memory SQLite pool, migrated and seeded with the four built-in DCS/PLC templates
/// (Yokogawa CentumVP, Honeywell Experion, Schneider Modicon, Allen-Bradley PlantPAx) --
/// matching what any real bhtune install has from its first startup, so route tests can
/// exercise the "list/show an existing template" paths without each test seeding its own
/// fixture data.
pub(crate) async fn in_memory_state() -> AppState {
    let pool = bhtune_db::connect_in_memory()
        .await
        .expect("in-memory pool should always connect and migrate cleanly");
    bhtune_db::seed_builtin_templates(&pool, Utc::now())
        .await
        .expect("seeding the built-in templates into a fresh in-memory db should never fail");
    AppState { pool }
}
