//! [`AppState`]: the shared state every route handler receives via `axum::extract::State`.

use bhtune_db::SqlitePool;
use std::sync::{Arc, RwLock};

use crate::active_run::ActiveRun;

/// Cheap to clone (an `Arc`-backed connection pool under the hood, per `sqlx`, and
/// [`ActiveRun`] is itself `Arc<Mutex<..>>`-backed) -- axum's `State` extractor just requires
/// `Clone` to hand a copy to each request.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    /// The in-flight tune registry and exclusive post-hoc write/revert reservation -- see
    /// [`ActiveRun`]'s own doc comment for the concurrency and shutdown behavior.
    pub active_run: ActiveRun,
    /// The live, revisioned TOML configuration. Route handlers take a fresh snapshot for
    /// every operation so a configuration-page save is visible without restarting the server.
    pub config_store: Arc<RwLock<bhtune_cli::config::LoadedConfigStore>>,
}

impl AppState {
    pub fn config_snapshot(&self) -> anyhow::Result<bhtune_cli::config::BhtuneConfig> {
        self.config_store
            .read()
            .map(|store| store.config.clone())
            .map_err(|_| anyhow::anyhow!("configuration store lock is poisoned"))
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn config_snapshot_returns_an_independent_config_copy() {
        let state = crate::test_support::in_memory_state().await;
        let mut snapshot = state.config_snapshot().unwrap();
        snapshot.allow_uncertain_quality = false;

        assert!(state.config_snapshot().unwrap().allow_uncertain_quality);
    }

    #[tokio::test]
    async fn config_snapshot_reports_a_poisoned_store_lock() {
        let state = crate::test_support::in_memory_state().await;
        let store = state.config_store.clone();
        let _ = std::thread::spawn(move || {
            let _guard = store.write().unwrap();
            panic!("deliberately poison the test lock");
        })
        .join();

        assert!(
            state
                .config_snapshot()
                .unwrap_err()
                .to_string()
                .contains("poisoned")
        );
    }
}
