//! `GET`/`PUT /api/config` for the mutable global TOML policies and tune settings.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use bhtune_cli::config::{
    ConfigPolicyUpdate, ConfigStoreError, LoadedConfigStore, TuningConfig, TuningConfigSource,
    resolve_retention_days, resolve_tuning_config, validate_tuning_config,
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
    pub tuning: ConfigTuningValues,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConfigTuningValues {
    pub mrft_delay_secs: u32,
    pub poll_interval_ms: u64,
    pub timeout_secs: u64,
    pub op_timeout_secs: u64,
    pub restore_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConfigSources {
    pub allow_uncertain_quality: String,
    pub retention_days: String,
    pub tuning: ConfigTuningSources,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConfigTuningSources {
    pub mrft_delay_secs: String,
    pub poll_interval_ms: String,
    pub timeout_secs: String,
    pub op_timeout_secs: String,
    pub restore_timeout_secs: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConfigTomlValues {
    pub allow_uncertain_quality: Option<bool>,
    #[schema(minimum = 1)]
    pub retention_days: Option<u32>,
    pub tuning: ConfigTuningTomlValues,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConfigTuningTomlValues {
    #[schema(maximum = 3600)]
    pub mrft_delay_secs: Option<u32>,
    #[schema(minimum = 1)]
    pub poll_interval_ms: Option<u64>,
    #[schema(minimum = 1)]
    pub timeout_secs: Option<u64>,
    #[schema(minimum = 1)]
    pub op_timeout_secs: Option<u64>,
    #[schema(minimum = 1)]
    pub restore_timeout_secs: Option<u64>,
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
    /// Omit this field (or send it as JSON `null`) to preserve the existing `[tuning]`
    /// overrides for compatibility with older clients. When an object is supplied, it
    /// replaces the complete tuning block: every nested field is written as an override
    /// when it has a value, and a nested `null` (including an omitted nested field, which
    /// deserializes to `None`) removes that field's override. An all-null object therefore
    /// resets the whole tuning block to built-in defaults.
    pub tuning: Option<UpdateTuningRequest>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct UpdateTuningRequest {
    #[schema(maximum = 3600)]
    pub mrft_delay_secs: Option<u32>,
    #[schema(minimum = 1)]
    pub poll_interval_ms: Option<u64>,
    #[schema(minimum = 1)]
    pub timeout_secs: Option<u64>,
    #[schema(minimum = 1)]
    pub op_timeout_secs: Option<u64>,
    #[schema(minimum = 1)]
    pub restore_timeout_secs: Option<u64>,
}

impl From<UpdateTuningRequest> for TuningConfig {
    fn from(request: UpdateTuningRequest) -> Self {
        Self {
            mrft_delay_secs: request.mrft_delay_secs,
            poll_interval_ms: request.poll_interval_ms,
            timeout_secs: request.timeout_secs,
            op_timeout_secs: request.op_timeout_secs,
            restore_timeout_secs: request.restore_timeout_secs,
        }
    }
}

fn tuning_source_label(source: TuningConfigSource) -> String {
    match source {
        TuningConfigSource::Toml => "config_file",
        TuningConfigSource::BuiltInDefault => "default",
    }
    .to_string()
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
    let effective_tuning = resolve_tuning_config(&store.toml_tuning);
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
        tuning: ConfigTuningSources {
            mrft_delay_secs: tuning_source_label(store.tuning_sources.mrft_delay_secs),
            poll_interval_ms: tuning_source_label(store.tuning_sources.poll_interval_ms),
            timeout_secs: tuning_source_label(store.tuning_sources.timeout_secs),
            op_timeout_secs: tuning_source_label(store.tuning_sources.op_timeout_secs),
            restore_timeout_secs: tuning_source_label(store.tuning_sources.restore_timeout_secs),
        },
    };
    let effective = ConfigValues {
        allow_uncertain_quality: store.config.allow_uncertain_quality,
        retention_days: effective_retention,
        tuning: ConfigTuningValues {
            mrft_delay_secs: effective_tuning.mrft_delay_secs,
            poll_interval_ms: effective_tuning.poll_interval_ms,
            timeout_secs: effective_tuning.timeout_secs,
            op_timeout_secs: effective_tuning.op_timeout_secs,
            restore_timeout_secs: effective_tuning.restore_timeout_secs,
        },
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
            tuning: ConfigTuningTomlValues {
                mrft_delay_secs: store.toml_tuning.mrft_delay_secs,
                poll_interval_ms: store.toml_tuning.poll_interval_ms,
                timeout_secs: store.toml_tuning.timeout_secs,
                op_timeout_secs: store.toml_tuning.op_timeout_secs,
                restore_timeout_secs: store.toml_tuning.restore_timeout_secs,
            },
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
        (status = 400, description = "The request contains an invalid retention or tuning policy.", body = ErrorBody),
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
    let tuning = request
        .tuning
        .map(TuningConfig::from)
        .unwrap_or(store.toml_tuning);
    let effective_tuning = resolve_tuning_config(&tuning);
    validate_tuning_config(&effective_tuning, false)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let saved = bhtune_cli::config::save_config_store(
        &store,
        &request.revision,
        &ConfigPolicyUpdate {
            allow_uncertain_quality: request.allow_uncertain_quality,
            retention_days: request.retention_days,
            mrft_delay_secs: tuning.mrft_delay_secs,
            poll_interval_ms: tuning.poll_interval_ms,
            timeout_secs: tuning.timeout_secs,
            op_timeout_secs: tuning.op_timeout_secs,
            restore_timeout_secs: tuning.restore_timeout_secs,
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
        assert_eq!(
            body["toml"]["tuning"]["mrft_delay_secs"],
            serde_json::Value::Null
        );
        assert_eq!(body["effective"]["tuning"]["mrft_delay_secs"], 0);
        assert_eq!(body["effective"]["tuning"]["poll_interval_ms"], 800);
        assert_eq!(body["effective"]["tuning"]["timeout_secs"], 3600);
        assert_eq!(body["effective"]["tuning"]["op_timeout_secs"], 30);
        assert_eq!(body["effective"]["tuning"]["restore_timeout_secs"], 30);
        assert_eq!(body["source"]["tuning"]["poll_interval_ms"], "default");
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
    async fn put_persists_tuning_values_and_reports_effective_sources() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        std::fs::write(&path, "# preserve\n").unwrap();
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
                            "retention_days": null,
                            "tuning": {
                                "mrft_delay_secs": 12,
                                "poll_interval_ms": 250,
                                "timeout_secs": 900,
                                "op_timeout_secs": 45,
                                "restore_timeout_secs": 8
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["toml"]["tuning"]["mrft_delay_secs"], 12);
        assert_eq!(body["effective"]["tuning"]["poll_interval_ms"], 250);
        assert_eq!(body["effective"]["tuning"]["timeout_secs"], 900);
        assert_eq!(body["effective"]["tuning"]["op_timeout_secs"], 45);
        assert_eq!(body["effective"]["tuning"]["restore_timeout_secs"], 8);
        assert_eq!(body["source"]["tuning"]["mrft_delay_secs"], "config_file");
        assert_eq!(
            body["source"]["tuning"]["restore_timeout_secs"],
            "config_file"
        );
        let saved = std::fs::read_to_string(path).unwrap();
        assert!(saved.contains("[tuning]"));
        assert!(saved.contains("mrft_delay_secs = 12"));
        assert!(saved.contains("poll_interval_ms = 250"));
        assert!(saved.contains("restore_timeout_secs = 8"));
    }

    #[tokio::test]
    async fn put_omitting_tuning_preserves_existing_overrides() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        std::fs::write(
            &path,
            "[tuning]\nmrft_delay_secs = 12\npoll_interval_ms = 250\n",
        )
        .unwrap();
        let state = state_for(&path).await;
        let revision = state.config_store.read().unwrap().revision.clone();
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

        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["toml"]["tuning"]["mrft_delay_secs"], 12);
        assert_eq!(body["toml"]["tuning"]["poll_interval_ms"], 250);
        assert_eq!(body["effective"]["tuning"]["mrft_delay_secs"], 12);
        assert_eq!(body["effective"]["tuning"]["poll_interval_ms"], 250);
        assert_eq!(body["effective"]["allow_uncertain_quality"], false);
    }

    #[tokio::test]
    async fn put_supplied_partial_tuning_object_replaces_the_complete_block() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        std::fs::write(
            &path,
            "[tuning]\nmrft_delay_secs = 12\npoll_interval_ms = 250\ntimeout_secs = 900\nop_timeout_secs = 45\nrestore_timeout_secs = 8\n",
        )
        .unwrap();
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
                            "retention_days": null,
                            "tuning": {
                                "poll_interval_ms": 500
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(
            body["toml"]["tuning"]["mrft_delay_secs"],
            serde_json::Value::Null
        );
        assert_eq!(body["toml"]["tuning"]["poll_interval_ms"], 500);
        assert_eq!(
            body["toml"]["tuning"]["timeout_secs"],
            serde_json::Value::Null
        );
        assert_eq!(
            body["toml"]["tuning"]["op_timeout_secs"],
            serde_json::Value::Null
        );
        assert_eq!(
            body["toml"]["tuning"]["restore_timeout_secs"],
            serde_json::Value::Null
        );
        assert_eq!(body["effective"]["tuning"]["mrft_delay_secs"], 0);
        assert_eq!(body["effective"]["tuning"]["poll_interval_ms"], 500);
        assert_eq!(body["effective"]["tuning"]["timeout_secs"], 3600);
        assert_eq!(body["effective"]["tuning"]["op_timeout_secs"], 30);
        assert_eq!(body["effective"]["tuning"]["restore_timeout_secs"], 30);
        let saved = std::fs::read_to_string(path).unwrap();
        assert!(saved.contains("poll_interval_ms = 500"));
        assert!(!saved.contains("mrft_delay_secs"));
        assert!(!saved.contains("timeout_secs"));
        assert!(!saved.contains("op_timeout_secs"));
        assert!(!saved.contains("restore_timeout_secs"));
    }

    #[tokio::test]
    async fn put_all_null_tuning_resets_to_built_in_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        std::fs::write(
            &path,
            "[tuning]\nmrft_delay_secs = 12\npoll_interval_ms = 250\ntimeout_secs = 900\nop_timeout_secs = 45\nrestore_timeout_secs = 8\n",
        )
        .unwrap();
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
                            "retention_days": null,
                            "tuning": {
                                "mrft_delay_secs": null,
                                "poll_interval_ms": null,
                                "timeout_secs": null,
                                "op_timeout_secs": null,
                                "restore_timeout_secs": null
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(
            body["toml"]["tuning"]["mrft_delay_secs"],
            serde_json::Value::Null
        );
        assert_eq!(
            body["toml"]["tuning"]["poll_interval_ms"],
            serde_json::Value::Null
        );
        assert_eq!(body["effective"]["tuning"]["mrft_delay_secs"], 0);
        assert_eq!(body["effective"]["tuning"]["poll_interval_ms"], 800);
        assert_eq!(body["effective"]["tuning"]["timeout_secs"], 3600);
        assert_eq!(body["source"]["tuning"]["restore_timeout_secs"], "default");
        let saved = std::fs::read_to_string(path).unwrap();
        assert!(!saved.contains("mrft_delay_secs"));
        assert!(!saved.contains("restore_timeout_secs"));
    }

    #[tokio::test]
    async fn put_rejects_invalid_tuning_values() {
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
                            "retention_days": null,
                            "tuning": {
                                "mrft_delay_secs": 3601,
                                "poll_interval_ms": 0,
                                "timeout_secs": 1,
                                "op_timeout_secs": 1,
                                "restore_timeout_secs": 1
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            json_body(response).await["error"]
                .as_str()
                .unwrap()
                .contains("mrft_delay_secs")
        );
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
            toml_tuning: Default::default(),
            tuning_sources: bhtune_cli::config::tuning_config_sources(
                &bhtune_cli::config::TuningConfig::default(),
            ),
        };

        let response = response_from_store_with_retention(&store, None, Some(90));

        assert_eq!(response.config_path, "");
        assert_eq!(response.source.retention_days, "environment");
        assert_eq!(response.effective.retention_days, Some(90));
        assert_eq!(response.effective.tuning.mrft_delay_secs, 0);
        assert_eq!(response.source.tuning.op_timeout_secs, "default");
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
