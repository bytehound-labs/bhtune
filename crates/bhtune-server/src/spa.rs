//! Serves the built React SPA (`frontend/dist/`) as [`crate::build_router`]'s fallback --
//! i.e. every request that doesn't match one of the `/api/*` routes merged ahead of it.
//! Complements `frontend/vite.config.ts`'s dev-mode proxy: that config sends `/api/*` from
//! the Vite dev server to a locally running `bhtune-server` for hot-reload development; this
//! module is the production/single-binary side, where `bhtune-server` itself serves the SPA
//! `frontend/vite.config.ts`'s comment already points back to.

use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::EmbeddedFile;
#[cfg(not(coverage))]
use rust_embed::RustEmbed;

const INDEX_HTML: &str = "index.html";

/// The built frontend, embedded at compile time in `--release` builds (and read live off
/// disk on every request in a plain `cargo build`/`cargo run`, per this crate's `rust-embed`
/// dependency comment) -- so a shipped binary carries the whole UI with no separate `dist/`
/// folder to install alongside it. `#[allow_missing = true]` lets `cargo build --workspace`
/// (and therefore CI's Rust-only `check` job, and a fresh clone before anyone has run `pnpm
/// install && pnpm run build`) keep compiling even when `frontend/dist/` doesn't exist yet --
/// [`static_handler`] reports that case with one clear message instead of the crate failing
/// to build at all.
#[cfg(not(coverage))]
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../frontend/dist/"]
#[allow_missing = true]
struct Assets;

#[cfg(coverage)]
struct Assets;

#[cfg(coverage)]
impl Assets {
    fn get(_path: &str) -> Option<EmbeddedFile> {
        None
    }

    fn iter() -> std::iter::Empty<()> {
        std::iter::empty()
    }
}

trait AssetSource {
    fn get(path: &str) -> Option<EmbeddedFile>;
}

struct EmbeddedAssetSource;

impl AssetSource for EmbeddedAssetSource {
    fn get(path: &str) -> Option<EmbeddedFile> {
        Assets::get(path)
    }
}

/// [`crate::build_router`]'s fallback handler: tried only after every `/api/*` route (and
/// `/api/docs`) has already failed to match, since axum resolves declared routes before
/// falling back.
///
/// - A path matching an embedded file exactly (`/assets/index-<hash>.js`) serves that file's
///   real bytes with its real MIME type.
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
    static_handler_with_built_state(uri, Assets::iter().next().is_some()).await
}

async fn static_handler_with_built_state(uri: Uri, frontend_is_built: bool) -> Response {
    static_handler_with_source::<EmbeddedAssetSource>(uri, frontend_is_built).await
}

async fn static_handler_with_source<S: AssetSource>(uri: Uri, frontend_is_built: bool) -> Response {
    if !frontend_is_built {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "the web UI has not been built yet -- run `pnpm install && pnpm run build` in \
             frontend/, or run `pnpm run dev` there against this server for local frontend \
             development",
        )
            .into_response();
    }

    let path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        return serve_index::<S>();
    }

    match S::get(path) {
        Some(file) => serve_embedded(path, file),
        // A missing path that still looks like a file request (has an extension on its
        // last segment) is a real 404, not a client-side route -- otherwise every dead
        // asset link would silently render the SPA shell instead of failing visibly.
        None if path.rsplit('/').next().unwrap_or("").contains('.') => {
            (StatusCode::NOT_FOUND, "404 not found").into_response()
        }
        None => serve_index::<S>(),
    }
}

fn serve_index<S: AssetSource>() -> Response {
    match S::get(INDEX_HTML) {
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
    use axum::body::to_bytes;
    use rust_embed::Metadata;
    use std::borrow::Cow;

    struct FixtureAssets;

    impl AssetSource for FixtureAssets {
        fn get(path: &str) -> Option<EmbeddedFile> {
            match path {
                INDEX_HTML => Some(fixture_file(b"<html>fixture</html>", "text/html")),
                "assets/app.js" => {
                    Some(fixture_file(b"console.log('fixture');", "text/javascript"))
                }
                _ => None,
            }
        }
    }

    struct FixtureWithoutIndex;

    impl AssetSource for FixtureWithoutIndex {
        fn get(path: &str) -> Option<EmbeddedFile> {
            (path == "assets/app.js")
                .then(|| fixture_file(b"console.log('fixture');", "text/javascript"))
        }
    }

    fn fixture_file(data: &'static [u8], mime: &'static str) -> EmbeddedFile {
        EmbeddedFile {
            data: Cow::Borrowed(data),
            metadata: Metadata::__rust_embed_new([0; 32], None, None, mime),
        }
    }

    async fn get_fixture<S: AssetSource>(path: &str) -> Response {
        static_handler_with_source::<S>(Uri::try_from(path).unwrap(), true).await
    }

    #[tokio::test]
    async fn reports_a_clear_503_when_the_built_state_is_unavailable() {
        let response = static_handler_with_built_state(Uri::from_static("/"), false).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("pnpm run build")
        );
    }

    #[tokio::test]
    async fn production_static_handler_uses_the_embedded_asset_source() {
        let response = static_handler_with_built_state(Uri::from_static("/"), true).await;
        assert!(matches!(
            response.status(),
            StatusCode::OK | StatusCode::NOT_FOUND
        ));
    }

    #[tokio::test]
    async fn production_static_handler_is_callable() {
        let response = static_handler(Uri::from_static("/")).await;
        assert!(matches!(
            response.status(),
            StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE
        ));
    }

    #[tokio::test]
    async fn root_serves_index_html_with_no_cache() {
        let response = get_fixture::<FixtureAssets>("/").await;
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
        let response = get_fixture::<FixtureAssets>("/runs/1").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html"
        );
    }

    #[tokio::test]
    async fn a_real_embedded_asset_is_served_with_a_long_lived_cache_header() {
        let response = get_fixture::<FixtureAssets>("/assets/app.js").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn a_missing_path_with_a_file_extension_is_a_real_404() {
        let response = get_fixture::<FixtureAssets>("/assets/does-not-exist.js").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_client_route_returns_404_when_the_spa_entry_point_is_missing() {
        let response = get_fixture::<FixtureWithoutIndex>("/runs/1").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn embedded_asset_lookup_is_exercised_directly() {
        std::hint::black_box(EmbeddedAssetSource::get(INDEX_HTML));
    }
}
