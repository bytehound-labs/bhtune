//! `GET /api/health` -- an unauthenticated liveness probe, deliberately the only endpoint
//! that touches neither [`crate::state::AppState`] nor the database: a load balancer or the
//! Windows Service manager (`server-windows-service`) needs to be able to tell "the process
//! is up and answering HTTP" apart from "the process is up but the database is unreachable"
//! (the latter would fail on essentially every other route already). It also exposes the
//! server package version for the web application's shell.

use axum::Json;
use axum::routing::get;
use serde::Serialize;
use utoipa::ToSchema;

use crate::state::AppState;

#[derive(Serialize, ToSchema)]
pub(crate) struct Health {
    status: &'static str,
    version: &'static str,
}

/// Liveness probe.
#[utoipa::path(
    get,
    path = "/api/health",
    tag = "health",
    responses(
        (
            status = 200,
            description = "The process is up and answering HTTP, with its application version.",
            body = Health
        ),
    ),
)]
pub(crate) async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route("/api/health", get(health))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_200_and_ok_status() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "status": "ok",
                "version": env!("CARGO_PKG_VERSION"),
            })
        );
    }
}
