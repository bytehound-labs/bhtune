//! Serves the built React SPA (`frontend/dist/`) as [`crate::build_router`]'s fallback --
//! i.e. every request that doesn't match one of the declared routes merged ahead of it.
//! Complements `frontend/vite.config.ts`'s dev-mode proxy: that config sends `/api/*` from
//! the Vite dev server to a locally running `bhtune-server` for hot-reload development; this
//! module is the production/single-binary side, where `bhtune-server` itself serves the SPA
//! `frontend/vite.config.ts`'s comment already points back to.

use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::{EmbeddedFile, RustEmbed};

const INDEX_HTML: &str = "index.html";

/// The built frontend, embedded at compile time in `--release` builds (and read live off
/// disk on every request in a plain `cargo build`/`cargo run`, per this crate's `rust-embed`
/// dependency comment) -- so a shipped binary carries the whole UI with no separate `dist/`
/// folder to install alongside it. `#[allow_missing = true]` lets `cargo build --workspace`
/// (and therefore CI's Rust-only `check` job, and a fresh clone before anyone has run `pnpm
/// install && pnpm run build`) keep compiling even when `frontend/dist/` doesn't exist yet --
/// [`static_handler`] reports that case with one clear message instead of the crate failing
/// to build at all.
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../frontend/dist/"]
#[allow_missing = true]
struct Assets;

/// [`crate::build_router`]'s fallback handler: tried only after every declared route has
/// failed to match, since axum resolves declared routes before falling back.
///
/// - A path matching an embedded file exactly (`/assets/index-<hash>.js`) serves that file's
///   real bytes with its real MIME type.
/// - An unmatched `/api` path returns a JSON 404 rather than the SPA shell. This keeps a
///   stale or incompatible server from masquerading as a successful API response.
/// - Anything else falls back to `index.html`, so React Router's client-side routes
///   (`/runs/1`, `/templates/new`, ...) resolve on a direct navigation or hard refresh, not
///   only after client-side navigation from `/` -- **except** a path whose last segment
///   contains a `.`, which is treated as a genuinely missing static asset (a stale bookmark,
///   a typo'd script `src`) and gets a real 404 rather than silently serving the SPA shell.
/// - If the SPA was never built at all (`Assets::iter()` is empty -- see the `allow_missing`
///   note on [`Assets`]), every request gets one explicit, actionable message instead of a
///   confusing blank 404. This is the one case a contributor running `cargo run -p
///   bhtune-server` straight after cloning, without having built the frontend, will hit.
pub(crate) async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(crate::error::ErrorBody {
                error: format!("API route not found: /{path}"),
            }),
        )
            .into_response();
    }

    if Assets::iter().next().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "the web UI has not been built yet -- run `pnpm install && pnpm run build` in \
             frontend/, or run `pnpm run dev` there against this server for local frontend \
             development",
        )
            .into_response();
    }

    if path.is_empty() {
        return serve_index();
    }

    match Assets::get(path) {
        Some(file) => serve_embedded(path, file),
        // A missing path that still looks like a file request (has an extension on its
        // last segment) is a real 404, not a client-side route -- otherwise every dead
        // asset link would silently render the SPA shell instead of failing visibly.
        None if path.rsplit('/').next().unwrap_or("").contains('.') => {
            (StatusCode::NOT_FOUND, "404 not found").into_response()
        }
        None => serve_index(),
    }
}

fn serve_index() -> Response {
    match Assets::get(INDEX_HTML) {
        Some(file) => serve_embedded(INDEX_HTML, file),
        None => (StatusCode::NOT_FOUND, "404 not found").into_response(),
    }
}

fn serve_embedded(path: &str, file: EmbeddedFile) -> Response {
    let mime = file.metadata.mimetype();
    let cache_control = if path == INDEX_HTML {
        // The entry point must be revalidated on every load: it's what names the *current*
        // set of content-hashed asset filenames below, so caching it would pin a client to
        // whatever build happened to be current when it first loaded.
        HeaderValue::from_static("no-cache")
    } else {
        // Every other embedded path is one of Vite's content-hashed filenames
        // (`assets/index-<hash>.js`) -- a new build always emits a new filename, so it's
        // safe to tell browsers/proxies to cache these indefinitely.
        HeaderValue::from_static("public, max-age=31536000, immutable")
    };
    let mut response = file.data.into_response();
    let headers = response.headers_mut();
    if let Ok(content_type) = HeaderValue::from_str(mime) {
        headers.insert(header::CONTENT_TYPE, content_type);
    }
    headers.insert(header::CACHE_CONTROL, cache_control);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    // These exercise `static_handler` against whatever `frontend/dist/` actually contains at
    // test time -- built (a real `pnpm run build` output, matching manual/CI verification)
    // or absent (a fresh checkout, matching CI's Rust-only `check` job). Skipping the
    // built-only assertions when assets are missing keeps this suite green in both cases
    // rather than requiring every contributor/CI job to build the frontend first just to run
    // `cargo test -p bhtune-server`.
    fn frontend_is_built() -> bool {
        Assets::iter().next().is_some()
    }

    async fn get(path: &str) -> Response {
        axum::Router::new()
            .fallback(static_handler)
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn reports_a_clear_503_when_the_spa_has_not_been_built() {
        if frontend_is_built() {
            return;
        }
        let response = get("/").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("pnpm run build")
        );
    }

    #[tokio::test]
    async fn root_serves_index_html_with_no_cache() {
        if !frontend_is_built() {
            return;
        }
        let response = get("/").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-cache"
        );
    }

    #[tokio::test]
    async fn an_unknown_client_side_route_falls_back_to_index_html() {
        if !frontend_is_built() {
            return;
        }
        let response = get("/runs/1").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html"
        );
    }

    #[tokio::test]
    async fn a_real_embedded_asset_is_served_with_a_long_lived_cache_header() {
        if !frontend_is_built() {
            return;
        }
        let some_asset = Assets::iter()
            .find(|p| p.as_ref() != INDEX_HTML)
            .expect("a real build always emits at least one hashed asset alongside index.html");
        let response = get(&format!("/{some_asset}")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn a_missing_path_with_a_file_extension_is_a_real_404() {
        if !frontend_is_built() {
            return;
        }
        let response = get("/assets/does-not-exist.js").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_unknown_api_path_returns_a_json_404() {
        let response = get("/api/does-not-exist").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "application/json"
        );
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "API route not found: /api/does-not-exist");
    }
}
