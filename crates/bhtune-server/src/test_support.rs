//! Shared test-only helpers for building an [`AppState`] backed by a seeded database --
//! every route test module needs one, so it lives here rather than being copy-pasted per
//! module.

#![cfg(test)]

use chrono::Utc;
use std::sync::{Arc, RwLock};

use crate::state::AppState;

async fn seeded_state(pool: bhtune_db::SqlitePool) -> AppState {
    bhtune_db::seed_builtin_templates(&pool, Utc::now())
        .await
        .expect("seeding the built-in templates into a fresh test database should never fail");
    let mut config_store =
        bhtune_runtime::config::load_config_store_from(None, None, None, None, false)
            .expect("default test config store should load");
    // Keep route tests fast now that HTTP requests correctly inherit global timing settings
    // instead of carrying obsolete per-run timing fields.
    config_store.config.tuning = bhtune_runtime::config::TuningConfig {
        mrft_delay_secs: Some(0),
        poll_interval_ms: Some(5),
        timeout_secs: Some(5),
        op_timeout_secs: Some(5),
        restore_timeout_secs: Some(5),
    };
    config_store.toml_tuning = config_store.config.tuning;
    config_store.tuning_sources =
        bhtune_runtime::config::tuning_config_sources(&config_store.toml_tuning);
    AppState::for_mode(
        pool,
        Arc::new(RwLock::new(config_store)),
        bhtune_runtime::config::ServerMode::Full,
        bhtune_runtime::config::DemoPolicy::default(),
    )
}

/// A private in-memory database, migrated and seeded with the four built-in DCS/PLC
/// templates. Use this for route tests that do not initiate live ownership.
pub(crate) async fn in_memory_state() -> AppState {
    let pool = bhtune_db::connect_in_memory()
        .await
        .expect("in-memory pool should always connect and migrate cleanly");
    seeded_state(pool).await
}

/// A private file-backed test database for route tests that exercise live write/revert
/// ownership, which requires a stable filesystem path.
pub(crate) async fn file_backed_state() -> (AppState, tempfile::TempDir) {
    let directory = tempfile::tempdir().expect("temporary test database directory should exist");
    let pool = bhtune_db::connect(&directory.path().join("route-test.db"))
        .await
        .expect("file-backed pool should connect and migrate cleanly");
    let state = seeded_state(pool).await;
    (state, directory)
}

/// A "Good"-quality reading for fixtures whose handler does not echo the requested tag.
pub(crate) fn good_reading(value: &str) -> opcda_bridge_proto::bridge::ReadResponse {
    opcda_bridge_proto::bridge::ReadResponse {
        values: vec![opcda_bridge_proto::bridge::TagValue {
            tag_id: "ignored".to_string(),
            value: value.to_string(),
            quality: "Good".to_string(),
            timestamp: "2024-01-15 10:23:45".to_string(),
        }],
    }
}
