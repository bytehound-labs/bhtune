//! [`AppState`]: the shared state every route handler receives via `axum::extract::State`.

use bhtune_cli::config::BhtuneConfig;
use bhtune_db::SqlitePool;

use crate::active_run::ActiveRun;

/// Cheap to clone (an `Arc`-backed connection pool under the hood, per `sqlx`, and
/// [`ActiveRun`] is itself `Arc<Mutex<..>>`-backed) -- axum's `State` extractor just requires
/// `Clone` to hand a copy to each request.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    /// The single tune run (if any) currently executing in a background task -- see
    /// [`ActiveRun`]'s own doc comment for why v1 allows only one at a time.
    pub active_run: ActiveRun,
    /// Resolved once at process startup (`main.rs`'s `config::load_config`) and shared by
    /// every `POST /api/runs` call, exactly mirroring how `bhtune-cli` resolves the same
    /// `BhtuneConfig` once per process invocation -- so `--bridge-host`/`--server`
    /// resolution (`bhtune_cli::commands::tune::prepare`) behaves identically whether a run
    /// was started by the CLI or over HTTP.
    pub app_config: BhtuneConfig,
}
