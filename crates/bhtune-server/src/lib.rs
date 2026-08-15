//! `bhtune-server` — the Axum HTTP/REST adapter over `bhtune-core`/`bhtune-db`, and (once
//! `server-embed-spa`/`frontend-shell` land) the host for the React SPA. See AGENTS.md's
//! "Web app architecture" section for why this, rather than a desktop GUI, is the primary
//! v1 GUI adapter.
//!
//! Split into a lib (this crate, `bhtune_server`) and a thin `main.rs` binary shell so route
//! handlers are directly testable via [`tower::ServiceExt::oneshot`] against
//! [`build_router`]'s output, with no bound TCP socket needed -- the same lib/bin split
//! `bhtune-cli` already uses for the same reason.

pub mod error;
pub mod routes;
pub mod state;

#[cfg(test)]
mod test_support;

pub use state::AppState;

/// Assembles every route module into one [`axum::Router`], ready to serve or to drive
/// directly in a test via `tower::ServiceExt::oneshot`.
pub fn build_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(routes::health::router())
        .merge(routes::templates::router())
        .merge(routes::history::router())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn build_router_serves_every_merged_route_module() {
        let app = build_router(test_support::in_memory_state().await);

        let health = app
            .clone()
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);

        let templates = app
            .clone()
            .oneshot(Request::get("/api/templates").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(templates.status(), StatusCode::OK);

        let runs = app
            .oneshot(Request::get("/api/runs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(runs.status(), StatusCode::OK);
    }
}
