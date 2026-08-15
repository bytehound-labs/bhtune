//! [`AppState`]: the shared state every route handler receives via `axum::extract::State`.

use bhtune_db::SqlitePool;

/// Cheap to clone (an `Arc`-backed connection pool under the hood, per `sqlx`), matching
/// every other adapter in this workspace's convention of passing `&SqlitePool` around rather
/// than a bespoke wrapper -- axum's `State` extractor just requires `Clone` to hand a copy to
/// each request.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
}
