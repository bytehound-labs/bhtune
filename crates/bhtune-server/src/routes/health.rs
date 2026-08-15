//! `GET /api/health` -- an unauthenticated liveness probe, deliberately the only endpoint
//! that touches neither [`crate::state::AppState`] nor the database: a load balancer or the
//! Windows Service manager (`server-windows-service`) needs to be able to tell "the process
//! is up and answering HTTP" apart from "the process is up but the database is unreachable"
//! (the latter would fail on essentially every other route already).

use axum::Json;
use axum::routing::get;
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
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
        assert_eq!(body, serde_json::json!({"status": "ok"}));
    }
}
