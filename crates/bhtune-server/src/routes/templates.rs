//! Template CRUD routes: `GET /api/templates`, `GET /api/templates/{name}`,
//! `POST /api/templates`, `DELETE /api/templates/{name}`.
//!
//! Mirrors `bhtune-cli`'s `commands::template` behavior exactly (see `import_one` there):
//! validate, then check for a name collision, then insert. HTTP-created templates are
//! always [`TemplateOrigin::User`] -- the same origin the CLI's single-template `import`
//! path assigns, as opposed to the auto-loaded `Builtin`/`Catalog` origins, which only ever
//! come from the embedded catalog or a user catalog file, never from this endpoint.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use bhtune_core::DcsTemplate;
use bhtune_db::models::{DcsTemplateRow, TemplateOrigin};
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::{ApiError, ErrorBody};
use crate::state::AppState;

/// The HTTP-facing shape of a stored template: the caller-supplied [`DcsTemplate`] fields
/// flattened alongside the database-assigned `id`/`origin`/timestamps. Per this workspace's
/// established DTO-decoupling convention (see `bhtune-cli`'s `commands::history` module doc
/// comments), [`DcsTemplateRow`] itself deliberately does not derive `Serialize` -- every
/// JSON-facing consumer builds its own projection rather than the DB row shape leaking
/// straight onto the wire.
#[derive(Debug, Serialize, ToSchema)]
pub struct TemplateResponse {
    pub id: i64,
    pub origin: TemplateOrigin,
    #[serde(flatten)]
    pub template: DcsTemplate,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<DcsTemplateRow> for TemplateResponse {
    fn from(row: DcsTemplateRow) -> Self {
        TemplateResponse {
            id: row.id,
            origin: row.origin,
            template: row.template,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// List every stored template.
///
/// `GET /api/templates` -- every stored template (built-in, catalog, and user-created
/// alike), ordered by name, for a template picker.
#[utoipa::path(
    get,
    path = "/api/templates",
    tag = "templates",
    responses(
        (status = 200, description = "Every stored template, ordered by name.", body = Vec<TemplateResponse>),
    ),
)]
pub(crate) async fn list_templates(
    State(state): State<AppState>,
) -> Result<Json<Vec<TemplateResponse>>, ApiError> {
    let rows = DcsTemplateRow::list(&state.pool).await?;
    Ok(Json(rows.into_iter().map(TemplateResponse::from).collect()))
}

/// Fetch one template by name.
///
/// `GET /api/templates/{name}` -- 404 if no template has that name.
#[utoipa::path(
    get,
    path = "/api/templates/{name}",
    tag = "templates",
    params(
        ("name" = String, Path, description = "Template name"),
    ),
    responses(
        (status = 200, body = TemplateResponse),
        (status = 404, description = "No template with that name.", body = ErrorBody),
    ),
)]
pub(crate) async fn get_template(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<TemplateResponse>, ApiError> {
    let row = DcsTemplateRow::get_by_name(&state.pool, &name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no template named '{name}'")))?;
    Ok(Json(row.into()))
}

/// Create a new, user-owned template.
///
/// `POST /api/templates` -- 400 if [`DcsTemplate::validate`] rejects the body, 409 if a
/// template with the same name already exists.
#[utoipa::path(
    post,
    path = "/api/templates",
    tag = "templates",
    request_body = DcsTemplate,
    responses(
        (status = 201, description = "Template created.", body = TemplateResponse),
        (status = 400, description = "The template failed validation.", body = ErrorBody),
        (status = 409, description = "A template with this name already exists.", body = ErrorBody),
    ),
)]
pub(crate) async fn create_template(
    State(state): State<AppState>,
    Json(template): Json<DcsTemplate>,
) -> Result<(StatusCode, Json<TemplateResponse>), ApiError> {
    template
        .validate()
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    if DcsTemplateRow::get_by_name(&state.pool, &template.name)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "a template named '{}' already exists",
            template.name
        )));
    }
    let row =
        DcsTemplateRow::insert(&state.pool, &template, TemplateOrigin::User, Utc::now()).await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Delete a template by name.
///
/// `DELETE /api/templates/{name}` -- 404 if no template has that name, 409 if it is still
/// referenced by a saved loop (`bhtune_db::DbError::TemplateInUse`, mapped by
/// `From<DbError> for ApiError`).
#[utoipa::path(
    delete,
    path = "/api/templates/{name}",
    tag = "templates",
    params(
        ("name" = String, Path, description = "Template name"),
    ),
    responses(
        (status = 204, description = "Template deleted."),
        (status = 404, description = "No template with that name.", body = ErrorBody),
        (status = 409, description = "The template is still referenced by one or more saved loops.", body = ErrorBody),
    ),
)]
pub(crate) async fn delete_template(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let row = DcsTemplateRow::get_by_name(&state.pool, &name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no template named '{name}'")))?;
    DcsTemplateRow::delete(&state.pool, row.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/templates", get(list_templates).post(create_template))
        .route(
            "/api/templates/{name}",
            get(get_template).delete(delete_template),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn minimal_valid_template(name: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "revert_mode": true,
            "proportional_type": "gain",
            "integral_type": "reset_time",
            "integral_unit": "minutes",
            "derivative_type": "derivative_time",
            "derivative_unit": "minutes",
            "process_variable_suffix": ".PV",
            "manipulated_variable_suffix": ".MV",
            "setpoint_variable_suffix": ".SV",
            "controller_direction_suffix": "",
            "controller_mode_suffix": "",
            "mode_attribute_suffix": "",
            "upper_pv_range_suffix": ".PVHR",
            "lower_pv_range_suffix": ".PVLR",
            "upper_mv_range_suffix": ".MVHR",
            "lower_mv_range_suffix": ".MVLR",
            "proportional_constant_suffix": ".KP",
            "integral_constant_suffix": ".TI",
            "derivative_constant_suffix": ".TD",
            "mode_manual_value": "1",
            "mode_auto_value": "0",
            "mode_attribute_program_value": null,
            "controller_action_direct_value": "0",
        })
    }

    #[tokio::test]
    async fn list_returns_the_four_seeded_builtin_templates() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(Request::get("/api/templates").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body.as_array().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn get_by_name_returns_the_matching_template() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(
                Request::get("/api/templates/Yokogawa%20CentumVP")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["name"], "Yokogawa CentumVP");
        assert_eq!(body["origin"], "builtin");
    }

    #[tokio::test]
    async fn get_by_name_404s_for_an_unknown_name() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(
                Request::get("/api/templates/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn create_then_get_round_trips_a_new_user_template() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let body = minimal_valid_template("My Custom PLC");
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/templates")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = body_json(response).await;
        assert_eq!(created["origin"], "user");
        assert_eq!(created["name"], "My Custom PLC");

        let response = app
            .oneshot(
                Request::get("/api/templates/My%20Custom%20PLC")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_rejects_an_invalid_template_with_400() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let mut body = minimal_valid_template("");
        body["name"] = serde_json::json!("");
        let response = app
            .oneshot(
                Request::post("/api/templates")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_rejects_a_duplicate_name_with_409() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let body = minimal_valid_template("Yokogawa CentumVP");
        let response = app
            .oneshot(
                Request::post("/api/templates")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn delete_removes_a_user_template() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let body = minimal_valid_template("Deletable Template");
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/templates")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = app
            .clone()
            .oneshot(
                Request::delete("/api/templates/Deletable%20Template")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .oneshot(
                Request::get("/api/templates/Deletable%20Template")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_404s_for_an_unknown_name() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(
                Request::delete("/api/templates/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
