//! `GET`/`PUT /api/runs/draft` for the app-wide New Tune form draft.
//!
//! This is deliberately separate from `tune_runs.request_json`: a draft is mutable, may be
//! incomplete while someone is editing it, and is not historical run data. Notes are omitted
//! from the DTO so transient operator context is never persisted as a form preference.

use axum::routing::get;
use axum::{Json, Router};
use bhtune_core::{ControllerDirection, ControllerType, ProcessType, ResponseLevel, TagOverrides};
use bhtune_db::models::{SettingRow, TuneDriver};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{ApiError, ErrorBody};
use crate::state::AppState;

const NEW_RUN_DRAFT_KEY: &str = "new_run_draft";

/// The editable state of the New Tune form.
///
/// Fields are optional because the form is allowed to be incomplete while it is being edited.
/// The frontend sends the complete shape on each save, using `null` for a cleared numeric or
/// enum field. Notes are intentionally absent: they describe one run, not a reusable draft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NewRunDraft {
    pub driver: Option<TuneDriver>,
    pub template: Option<String>,
    pub tagname: Option<String>,
    pub server: Option<String>,
    pub bridge_host: Option<String>,
    pub process_type: Option<ProcessType>,
    pub controller_type: Option<ControllerType>,
    pub relay_amp: Option<f32>,
    pub cycles_skip: Option<u32>,
    pub cycles_count: Option<u32>,
    pub noise_protection_secs: Option<u32>,
    pub mrft_delay: Option<u32>,
    pub poll_interval_ms: Option<u64>,
    pub timeout_secs: Option<u64>,
    pub op_timeout_secs: Option<u64>,
    pub restore_timeout_secs: Option<u64>,
    pub allow_uncertain_quality: Option<bool>,
    pub direction: Option<ControllerDirection>,
    pub tag_overrides: Option<TagOverrides>,
    pub pv_range_high: Option<f32>,
    pub pv_range_low: Option<f32>,
    pub mv_range_high: Option<f32>,
    pub mv_range_low: Option<f32>,
    pub sim_gain: Option<f32>,
    pub sim_tau: Option<f32>,
    pub sim_dead_time: Option<f32>,
    pub sim_noise: Option<f32>,
    pub sim_seed: Option<u64>,
    pub sim_initial_pv: Option<f32>,
    pub sim_initial_mv: Option<f32>,
    pub write_pid: Option<ResponseLevel>,
    pub yes: Option<bool>,
}

fn invalid_stored_draft(error: serde_json::Error) -> ApiError {
    ApiError::Internal(anyhow::anyhow!(
        "saved New Tune draft has an invalid shape: {error}"
    ))
}

/// Returns the saved New Tune draft, or `null` when no draft has been saved yet.
#[utoipa::path(
    get,
    path = "/api/runs/draft",
    tag = "runs",
    responses(
        (status = 200, description = "The saved New Tune draft, or null when none exists.", body = Option<NewRunDraft>),
        (status = 500, description = "The stored draft is malformed or the database failed.", body = ErrorBody),
    ),
)]
pub(crate) async fn get_draft(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<Option<NewRunDraft>>, ApiError> {
    let Some(setting) = SettingRow::get(&state.pool, NEW_RUN_DRAFT_KEY).await? else {
        return Ok(Json(None));
    };
    let draft = serde_json::from_value(setting.value).map_err(invalid_stored_draft)?;
    Ok(Json(Some(draft)))
}

/// Replaces the saved New Tune draft and returns the stored value.
#[utoipa::path(
    put,
    path = "/api/runs/draft",
    tag = "runs",
    request_body = NewRunDraft,
    responses(
        (status = 200, description = "The draft was saved.", body = NewRunDraft),
        (status = 500, description = "The database failed or the draft could not be stored.", body = ErrorBody),
    ),
)]
pub(crate) async fn put_draft(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(draft): Json<NewRunDraft>,
) -> Result<Json<NewRunDraft>, ApiError> {
    let value = serde_json::to_value(&draft)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!("failed to encode draft: {error}")))?;
    let stored = SettingRow::upsert(&state.pool, NEW_RUN_DRAFT_KEY, &value, Utc::now()).await?;
    let persisted = serde_json::from_value(stored.value).map_err(invalid_stored_draft)?;
    Ok(Json(persisted))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/runs/draft", get(get_draft).put(put_draft))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use bhtune_db::models::SettingRow;
    use serde_json::json;
    use tower::ServiceExt;

    #[tokio::test]
    async fn missing_draft_is_null() {
        let app = crate::build_router(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(
                Request::get("/api/runs/draft")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            json!(null)
        );
    }

    #[tokio::test]
    async fn put_replaces_draft_and_never_persists_notes() {
        let state = crate::test_support::in_memory_state().await;
        let pool = state.pool.clone();
        let app = crate::build_router(state);
        let request_body = json!({
            "driver": "opcda",
            "template": "Yokogawa CentumVP",
            "bridge_host": "localhost:7600",
            "notes": "transient operator context"
        });

        let response = app
            .clone()
            .oneshot(
                Request::put("/api/runs/draft")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::get("/api/runs/draft")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(saved["driver"], "opcda");
        assert_eq!(saved["bridge_host"], "localhost:7600");
        assert!(saved.get("notes").is_none());

        let raw = SettingRow::get(&pool, NEW_RUN_DRAFT_KEY)
            .await
            .unwrap()
            .unwrap();
        assert!(raw.value.get("notes").is_none());
    }

    #[tokio::test]
    async fn malformed_saved_draft_is_an_explicit_server_error() {
        let state = crate::test_support::in_memory_state().await;
        SettingRow::upsert(
            &state.pool,
            NEW_RUN_DRAFT_KEY,
            &json!({"driver": 42}),
            Utc::now(),
        )
        .await
        .unwrap();
        let app = crate::build_router(state);

        let response = app
            .oneshot(
                Request::get("/api/runs/draft")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            json!({"error": "internal server error"})
        );
    }
}
