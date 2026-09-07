//! [`ApiError`]: the one error type every route handler returns, and its mapping onto an HTTP
//! status code plus a JSON `{"error": "..."}` body.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

/// Every way a request can fail, already carrying the HTTP status it maps to -- handlers
/// return `Result<_, ApiError>` and let `?` do the conversion (see the `From` impls below),
/// the same "one error enum, converted at the boundary" shape `bhtune-cli`'s commands use
/// with `anyhow::Result`.
#[derive(Debug)]
pub enum ApiError {
    /// The requested resource doesn't exist (a template name, a run id). 404.
    NotFound(String),
    /// The request conflicts with existing state (a name collision on create, a delete
    /// blocked by a foreign-key reference). 409.
    Conflict(String),
    /// The request body/query itself is malformed or fails domain validation
    /// (`DcsTemplate::validate`, an unparseable filter value axum's extractors didn't already
    /// reject). 400.
    BadRequest(String),
    /// The request was authenticated but is not permitted. 403.
    Forbidden(String),
    /// A per-client or per-session limit was exceeded. 429, with a `Retry-After` header.
    TooManyRequests {
        message: String,
        retry_after_secs: u64,
    },
    /// The public demo has exhausted a global capacity limit. 503, with a `Retry-After`
    /// header. Keeping this distinct from per-client throttling lets callers avoid telling a
    /// client that its own request rate caused shared service saturation.
    GlobalCapacity {
        message: String,
        retry_after_secs: u64,
    },
    /// No valid demo identity was supplied. 401.
    Unauthorized(String),
    /// Anything else: a database connection/query failure, or any other unexpected error.
    /// Deliberately doesn't echo the underlying error's `Display` text into the response body
    /// (logged via `tracing::error!` instead) -- an internal error's detail is for the
    /// server's own logs, not a client that can't act on it. 500.
    Internal(anyhow::Error),
}

/// The JSON body of every non-2xx response: `{"error": "<message>"}`. `pub`/`ToSchema` so
/// every fallible `#[utoipa::path]` response can reference it (`body = ErrorBody`) and the
/// generated OpenAPI spec -- and therefore the generated frontend TS client -- accurately
/// types error bodies instead of `content?: never`.
#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message, retry_after_secs) = match self {
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, message, None),
            ApiError::Conflict(message) => (StatusCode::CONFLICT, message, None),
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message, None),
            ApiError::Forbidden(message) => (StatusCode::FORBIDDEN, message, None),
            ApiError::TooManyRequests {
                message,
                retry_after_secs,
            } => (
                StatusCode::TOO_MANY_REQUESTS,
                message,
                Some(retry_after_secs),
            ),
            ApiError::GlobalCapacity {
                message,
                retry_after_secs,
            } => (
                StatusCode::SERVICE_UNAVAILABLE,
                message,
                Some(retry_after_secs),
            ),
            ApiError::Unauthorized(message) => (StatusCode::UNAUTHORIZED, message, None),
            ApiError::Internal(err) => {
                tracing::error!(error = %err, "internal error handling request");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                    None,
                )
            }
        };
        let mut response = (status, Json(ErrorBody { error: message })).into_response();
        if let Some(retry_after_secs) = retry_after_secs {
            response.headers_mut().insert(
                header::RETRY_AFTER,
                HeaderValue::from_str(&retry_after_secs.to_string())
                    .expect("an integer is always a valid Retry-After header value"),
            );
        }
        response
    }
}

impl From<bhtune_db::DbError> for ApiError {
    /// [`bhtune_db::DbError::TemplateInUse`] is the one variant a client can actually act on
    /// (stop trying to delete a template still referenced by a saved loop) -- everything else
    /// is an infrastructure-level failure the client can't distinguish or fix, so it collapses
    /// to [`ApiError::Internal`].
    fn from(err: bhtune_db::DbError) -> Self {
        match err {
            bhtune_db::DbError::TemplateInUse { id } => ApiError::Conflict(format!(
                "template {id} is still referenced by one or more saved loops and cannot be deleted"
            )),
            other => ApiError::Internal(other.into()),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError::Internal(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn not_found_maps_to_404_with_the_message() {
        let response = ApiError::NotFound("no template named 'X'".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            body_json(response).await,
            serde_json::json!({"error": "no template named 'X'"})
        );
    }

    #[tokio::test]
    async fn conflict_maps_to_409_with_the_message() {
        let response = ApiError::Conflict("already exists".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            body_json(response).await,
            serde_json::json!({"error": "already exists"})
        );
    }

    #[tokio::test]
    async fn bad_request_maps_to_400_with_the_message() {
        let response = ApiError::BadRequest("invalid".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            body_json(response).await,
            serde_json::json!({"error": "invalid"})
        );
    }

    #[tokio::test]
    async fn forbidden_maps_to_403_with_the_message() {
        let response = ApiError::Forbidden("not permitted".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            body_json(response).await,
            serde_json::json!({"error": "not permitted"})
        );
    }

    #[tokio::test]
    async fn too_many_requests_maps_to_429_with_retry_after() {
        let response = ApiError::TooManyRequests {
            message: "slow down".to_string(),
            retry_after_secs: 17,
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "17");
        assert_eq!(
            body_json(response).await,
            serde_json::json!({"error": "slow down"})
        );
    }

    #[tokio::test]
    async fn global_capacity_maps_to_503_with_retry_after() {
        let response = ApiError::GlobalCapacity {
            message: "demo capacity exhausted".to_string(),
            retry_after_secs: 3,
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "3");
        assert_eq!(
            body_json(response).await,
            serde_json::json!({"error": "demo capacity exhausted"})
        );
    }

    #[tokio::test]
    async fn internal_maps_to_500_and_never_leaks_the_underlying_message() {
        let response =
            ApiError::Internal(anyhow::anyhow!("secret db path leaked here")).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body_json(response).await,
            serde_json::json!({"error": "internal server error"})
        );
    }

    #[tokio::test]
    async fn template_in_use_db_error_maps_to_409() {
        let api_err: ApiError = bhtune_db::DbError::TemplateInUse { id: 7 }.into();
        assert!(matches!(api_err, ApiError::Conflict(_)));
        let response = api_err.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn other_db_errors_map_to_internal() {
        let api_err: ApiError = bhtune_db::DbError::InvalidBackup("bad file".to_string()).into();
        assert!(matches!(api_err, ApiError::Internal(_)));
        let response = api_err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn anyhow_errors_map_to_internal() {
        let api_err: ApiError = anyhow::anyhow!("unexpected failure").into();
        assert!(matches!(api_err, ApiError::Internal(_)));
    }
}
