//! `bhtune-server` — the Axum HTTP/REST adapter over `bhtune-core`/`bhtune-db`, and (once
//! `server-embed-spa`/`frontend-shell` land) the host for the React SPA. See AGENTS.md's
//! "Web app architecture" section for why this, rather than a desktop GUI, is the primary
//! v1 GUI adapter.
//!
//! Split into a lib (this crate, `bhtune_server`) and a thin `main.rs` binary shell so route
//! handlers are directly testable via [`tower::ServiceExt::oneshot`] against
//! [`build_router`]'s output, with no bound TCP socket needed -- the same lib/bin split
//! `bhtune-cli` already uses for the same reason.

pub mod active_run;
pub mod error;
pub mod openapi;
pub mod routes;
pub mod state;

#[cfg(test)]
mod test_support;

use utoipa::OpenApi as _;
use utoipa_scalar::Servable as _;

pub use state::AppState;

/// Assembles every route module into one [`axum::Router`], ready to serve or to drive
/// directly in a test via `tower::ServiceExt::oneshot`.
///
/// Alongside the JSON API routes, this mounts the OpenAPI contract itself two ways: the raw
/// document at `GET /api/openapi.json` (for tooling -- CI's spec-diff gate, and eventually
/// `frontend-shell`'s generated TS client) and an interactive Scalar UI at `/api/docs` (for a
/// human exploring the API in a browser). [`utoipa_scalar::Scalar::with_url`] returns a
/// state-generic `axum::Router<S>` with the UI's one route already attached, so it merges in
/// directly rather than needing its own handler function.
pub fn build_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(routes::health::router())
        .merge(routes::templates::router())
        .merge(routes::history::router())
        .merge(routes::runs::router())
        .route("/api/openapi.json", axum::routing::get(openapi_json))
        .merge(utoipa_scalar::Scalar::with_url(
            "/api/docs",
            openapi::ApiDoc::openapi(),
        ))
        .with_state(state)
}

async fn openapi_json() -> axum::Json<utoipa::openapi::OpenApi> {
    axum::Json(openapi::ApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
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
            .clone()
            .oneshot(Request::get("/api/runs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(runs.status(), StatusCode::OK);

        // `history::router()` registers `GET /api/runs` and `runs::router()` registers
        // `POST /api/runs` at that same path string -- proving axum actually merges the two
        // method routers onto one path (rather than the second `.merge()` silently
        // dropping/overwriting the first) is exactly the design question this route was
        // built to resolve. A malformed body still reaches the handler and fails with `400`
        // (validation), not `404`/`405` (routing) -- routing succeeding is all this
        // assertion cares about.
        let post_runs = app
            .clone()
            .oneshot(
                Request::post("/api/runs")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(post_runs.status(), StatusCode::NOT_FOUND);
        assert_ne!(post_runs.status(), StatusCode::METHOD_NOT_ALLOWED);

        let openapi_json = app
            .clone()
            .oneshot(
                Request::get("/api/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(openapi_json.status(), StatusCode::OK);
        let bytes = to_bytes(openapi_json.into_body(), usize::MAX)
            .await
            .unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(spec["info"]["title"], "BHTune API");
        assert_eq!(spec["paths"]["/api/health"]["get"]["tags"][0], "health");

        let docs = app
            .oneshot(Request::get("/api/docs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(docs.status(), StatusCode::OK);
        let content_type = docs
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(content_type.starts_with("text/html"));
    }
}
