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
