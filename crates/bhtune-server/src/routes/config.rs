//! `GET`/`PUT /api/config` for the two mutable global TOML policies.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use bhtune_cli::config::{
    ConfigPolicyUpdate, ConfigStoreError, LoadedConfigStore, resolve_retention_days,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{ApiError, ErrorBody};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConfigValues {
    pub allow_uncertain_quality: bool,
    #[schema(minimum = 1)]
    pub retention_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConfigSources {
    pub allow_uncertain_quality: String,
    pub retention_days: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConfigTomlValues {
    pub allow_uncertain_quality: Option<bool>,
    #[schema(minimum = 1)]
    pub retention_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConfigResponse {
    pub revision: String,
    pub config_path: String,
    pub toml: ConfigTomlValues,
    pub effective: ConfigValues,
    pub source: ConfigSources,
    pub backup_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct UpdateConfigRequest {
    pub revision: String,
    pub allow_uncertain_quality: bool,
    #[schema(minimum = 1)]
    pub retention_days: Option<u32>,
}

fn response_from_store(store: &LoadedConfigStore, backup_path: Option<String>) -> ConfigResponse {
    let env_retention = std::env::var("BHTUNE_RETENTION_DAYS")
        .ok()
        .and_then(|value| value.parse().ok());
    response_from_store_with_retention(store, backup_path, env_retention)
}

fn response_from_store_with_retention(
    store: &LoadedConfigStore,
    backup_path: Option<String>,
    env_retention: Option<u32>,
) -> ConfigResponse {
    let effective_retention = resolve_retention_days(env_retention, &store.config);
    let sources = ConfigSources {
        allow_uncertain_quality: if store.toml_allow_uncertain_quality.is_some() {
            "config_file".to_string()
        } else {
            "default".to_string()
        },
        retention_days: if env_retention.is_some() {
            "environment".to_string()
        } else if store.config.retention_days.is_some() {
            "config_file".to_string()
        } else {
            "default".to_string()
        },
    };
    let effective = ConfigValues {
        allow_uncertain_quality: store.config.allow_uncertain_quality,
        retention_days: effective_retention,
    };
    ConfigResponse {
        revision: store.revision.clone(),
        config_path: store
            .path
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        toml: ConfigTomlValues {
            allow_uncertain_quality: store.toml_allow_uncertain_quality,
            retention_days: store.config.retention_days,
        },
        source: sources,
        effective,
        backup_path,
    }
}

fn map_store_error(error: ConfigStoreError) -> ApiError {
    match error {
        ConfigStoreError::Conflict { message, .. } => ApiError::Conflict(message),
        other => ApiError::Internal(anyhow::anyhow!(other.to_string())),
    }
}

#[utoipa::path(
    get,
    path = "/api/config",
    tag = "config",
    responses(
        (status = 200, description = "The TOML and effective global configuration.", body = ConfigResponse),
        (status = 500, description = "The configuration store could not be read.", body = ErrorBody),
    ),
)]
pub(crate) async fn get_config(
    State(state): State<AppState>,
) -> Result<Json<ConfigResponse>, ApiError> {
    let store = state
        .config_store
        .read()
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("configuration store lock is poisoned")))?;
    Ok(Json(response_from_store(&store, None)))
}

#[utoipa::path(
    put,
    path = "/api/config",
    tag = "config",
    request_body = UpdateConfigRequest,
    responses(
        (status = 200, description = "The configuration was saved.", body = ConfigResponse),
        (status = 400, description = "The request contains an invalid retention policy.", body = ErrorBody),
        (status = 409, description = "The supplied revision is stale or the file changed on disk.", body = ErrorBody),
        (status = 500, description = "The configuration could not be written.", body = ErrorBody),
    ),
)]
pub(crate) async fn put_config(
    State(state): State<AppState>,
    Json(request): Json<UpdateConfigRequest>,
) -> Result<Json<ConfigResponse>, ApiError> {
    if request.retention_days == Some(0) {
        return Err(ApiError::BadRequest(
            "retention_days must be at least 1 or null".to_string(),
        ));
    }
    let mut store = state
        .config_store
        .write()
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("configuration store lock is poisoned")))?;
    let saved = bhtune_cli::config::save_config_store(
        &store,
        &request.revision,
        &ConfigPolicyUpdate {
            allow_uncertain_quality: request.allow_uncertain_quality,
            retention_days: request.retention_days,
        },
    )
    .map_err(map_store_error)?;
    let backup_path = saved
        .backup_path
        .as_deref()
        .map(|path| path.display().to_string());
    *store = saved.state;
    Ok(Json(response_from_store(&store, backup_path)))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/config", get(get_config).put(put_config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use std::sync::{Arc, RwLock};
    use tempfile::tempdir;
    use tower::ServiceExt;

    async fn state_for(path: &std::path::Path) -> AppState {
        let mut state = crate::test_support::in_memory_state().await;
        let loaded =
            bhtune_cli::config::load_config_store_from(Some(path), None, None, None, false)
                .unwrap();
        state.config_store = Arc::new(RwLock::new(loaded));
        state
    }

    async fn json_body(response: axum::http::Response<Body>) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn get_returns_default_policy_and_revision() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        std::fs::write(&path, "bridge_host = \"gateway:7600\"\n").unwrap();
        let state = state_for(&path).await;
        let revision = state.config_store.read().unwrap().revision.clone();
        let response = crate::build_router(state)
            .oneshot(Request::get("/api/config").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["revision"], revision);
        assert_eq!(
            body["toml"]["allow_uncertain_quality"],
            serde_json::Value::Null
        );
        assert_eq!(body["effective"]["allow_uncertain_quality"], true);
        assert_eq!(body["source"]["allow_uncertain_quality"], "default");
        assert!(
            body["config_path"]
                .as_str()
                .unwrap()
                .ends_with("bhtune.toml")
        );
    }

    #[tokio::test]
    async fn put_patches_only_supported_keys_and_returns_new_revision() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        std::fs::write(
            &path,
            "# preserve\nbridge_host = \"gateway:7600\"\nunknown = \"keep\"\n",
        )
        .unwrap();
        let state = state_for(&path).await;
        let revision = state.config_store.read().unwrap().revision.clone();
        let app = crate::build_router(state);
        let response = app
            .oneshot(
                Request::put("/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "revision": revision,
                            "allow_uncertain_quality": false,
                            "retention_days": 30
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_ne!(body["revision"], revision);
        assert_eq!(body["effective"]["allow_uncertain_quality"], false);
        assert_eq!(body["effective"]["retention_days"], 30);
        let saved = std::fs::read_to_string(path).unwrap();
        assert!(saved.contains("# preserve"));
        assert!(saved.contains("unknown = \"keep\""));
        assert!(saved.contains("allow_uncertain_quality = false"));
        assert!(saved.contains("retention_days = 30"));
    }

    #[tokio::test]
    async fn put_creates_an_auto_discovered_missing_config_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bhtune").join("bhtune.toml");
        let loaded = bhtune_cli::config::load_config_store_from(
            None,
            Some(dir.path().to_str().unwrap()),
            None,
            None,
            false,
        )
        .unwrap();
        let revision = loaded.revision.clone();
        let mut state = crate::test_support::in_memory_state().await;
        state.config_store = Arc::new(RwLock::new(loaded));
        let response = crate::build_router(state)
            .oneshot(
                Request::put("/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "revision": revision,
                            "allow_uncertain_quality": true,
                            "retention_days": null
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(path.exists());
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("allow_uncertain_quality = true")
        );
    }

    #[tokio::test]
    async fn put_rejects_stale_revision_and_external_disk_changes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        std::fs::write(&path, "bridge_host = \"gateway:7600\"\n").unwrap();
        let state = state_for(&path).await;
        let revision = state.config_store.read().unwrap().revision.clone();
        std::fs::write(&path, "bridge_host = \"other:7600\"\n").unwrap();
        let response = crate::build_router(state)
            .oneshot(
                Request::put("/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "revision": revision,
                            "allow_uncertain_quality": false,
                            "retention_days": null
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(
            json_body(response).await["error"]
                .as_str()
                .unwrap()
                .contains("changed on disk")
        );
    }

    #[test]
    fn response_reports_environment_retention_and_unresolved_path() {
        let store = LoadedConfigStore {
            path: None,
            missing_is_allowed: true,
            original_raw: None,
            config: bhtune_cli::config::BhtuneConfig {
                retention_days: Some(30),
                ..Default::default()
            },
            revision: "revision".to_string(),
            toml_allow_uncertain_quality: None,
        };

        let response = response_from_store_with_retention(&store, None, Some(90));

        assert_eq!(response.config_path, "");
        assert_eq!(response.source.retention_days, "environment");
        assert_eq!(response.effective.retention_days, Some(90));
    }

    #[test]
    fn map_store_error_maps_non_conflicts_to_internal_errors() {
        let error = map_store_error(ConfigStoreError::PathNotResolved);
        assert!(matches!(error, ApiError::Internal(_)));

        let error = map_store_error(ConfigStoreError::Conflict {
            path: None,
            message: "stale".to_string(),
        });
        assert!(matches!(error, ApiError::Conflict(message) if message == "stale"));
    }

    #[tokio::test]
    async fn put_rejects_zero_retention_days() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        std::fs::write(&path, "").unwrap();
        let state = state_for(&path).await;
        let revision = state.config_store.read().unwrap().revision.clone();
        let response = crate::build_router(state)
            .oneshot(
                Request::put("/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "revision": revision,
                            "allow_uncertain_quality": true,
                            "retention_days": 0
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            json_body(response).await["error"],
            "retention_days must be at least 1 or null"
        );
    }

    #[tokio::test]
    async fn get_returns_internal_error_when_config_store_lock_is_poisoned() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        std::fs::write(&path, "").unwrap();
        let state = state_for(&path).await;
        let store = Arc::clone(&state.config_store);
        std::thread::spawn(move || {
            let _guard = store.write().unwrap();
            panic!("poison configuration store");
        })
        .join()
        .unwrap_err();

        let response = crate::build_router(state)
            .oneshot(Request::get("/api/config").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn put_returns_internal_error_when_config_store_lock_is_poisoned() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        std::fs::write(&path, "").unwrap();
        let state = state_for(&path).await;
        let revision = state.config_store.read().unwrap().revision.clone();
        let store = Arc::clone(&state.config_store);
        std::thread::spawn(move || {
            let _guard = store.write().unwrap();
            panic!("poison configuration store");
        })
        .join()
        .unwrap_err();

        let response = crate::build_router(state)
            .oneshot(
                Request::put("/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "revision": revision,
                            "allow_uncertain_quality": true,
                            "retention_days": null
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
