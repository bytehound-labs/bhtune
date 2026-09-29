//! Shared test-only helpers for building an [`AppState`] backed by a seeded, in-memory
//! database -- every route test module needs one, so it lives here rather than being
//! copy-pasted per module.

#![cfg(test)]

use chrono::Utc;
use std::sync::{Arc, RwLock};

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
    let mut config_store =
        bhtune_cli::config::load_config_store_from(None, None, None, None, false)
            .expect("default test config store should load");
    // Keep route tests fast now that HTTP requests correctly inherit global timing settings
    // instead of carrying obsolete per-run timing fields.
    config_store.config.tuning = bhtune_cli::config::TuningConfig {
        mrft_delay_secs: Some(0),
        poll_interval_ms: Some(5),
        timeout_secs: Some(5),
        op_timeout_secs: Some(5),
        restore_timeout_secs: Some(5),
    };
    config_store.toml_tuning = config_store.config.tuning;
    config_store.tuning_sources =
        bhtune_cli::config::tuning_config_sources(&config_store.toml_tuning);
    AppState::for_mode(
        pool,
        Arc::new(RwLock::new(config_store)),
        bhtune_cli::config::ServerMode::Full,
        bhtune_cli::config::DemoPolicy::default(),
    )
}

/// Shared mock `Bridge` for route tests that need a real `OpcDaDriver` round trip.
///
/// The service and server live in `bhtune-test-support`. `good_reading` stays here
/// because only these route fixtures use that canned value.
pub(crate) mod mock_bridge {
    use opcda_bridge_proto::bridge::ReadResponse;

    pub(crate) use bhtune_test_support::{MockBridgeService, MockServerHandle};

    /// Binds `service` on an ephemeral localhost port. The returned handle must
    /// stay in scope so the server task is not dropped before the request finishes.
    pub(crate) async fn start_mock_server(
        service: MockBridgeService,
    ) -> (String, MockServerHandle) {
        bhtune_test_support::start_mock_server(service)
            .await
            .expect("bind mock bridge")
    }

    /// A "Good"-quality reading, regardless of which tag was requested --
    /// matching `bhtune-cli`'s own `history::revert` test fixtures' rationale: every
    /// pre-read and every write's confirmation readback returns this same value, so a
    /// fixture that also writes/reverts to `10.0` always sees a matching readback no
    /// matter which of the three PID constants is being processed.
    pub(crate) fn good_reading(value: &str) -> ReadResponse {
        ReadResponse {
            values: vec![opcda_bridge_proto::bridge::TagValue {
                tag_id: "ignored".to_string(),
                value: value.to_string(),
                quality: "Good".to_string(),
                timestamp: "2024-01-15 10:23:45".to_string(),
            }],
        }
    }
}
