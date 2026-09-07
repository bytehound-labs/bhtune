use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, Request, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use bhtune_cli::config::ServerMode;

// React's tag tree and uPlot set layout values through element `style` attributes. Keep
// stylesheet sources self-only while permitting only that narrow inline-style surface.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; \
    style-src 'self'; style-src-attr 'unsafe-inline'; connect-src 'self'; img-src 'self'; \
    font-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; \
    form-action 'self'";
const PERMISSIONS_POLICY: &str = "accelerometer=(), autoplay=(), camera=(), \
    clipboard-read=(), clipboard-write=(), display-capture=(), encrypted-media=(), \
    fullscreen=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), midi=(), \
    payment=(), picture-in-picture=(), publickey-credentials-get=(), screen-wake-lock=(), \
    usb=(), web-share=(), xr-spatial-tracking=()";

fn is_state_changing(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn origin_is_allowed(
    mode: ServerMode,
    headers: &HeaderMap,
    configured_origin: Option<&str>,
) -> bool {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let origin = match (origins.next(), origins.next()) {
        (Some(origin), None) => origin.to_str().ok(),
        (None, None) => None,
        _ => return false,
    };

    if mode == ServerMode::Demo {
        return origin.is_some_and(|origin| configured_origin == Some(origin));
    }

    // Full mode predates browser-only operation and must retain CLI/curl compatibility. A Vite
    // development page reaches the backend through its same-origin proxy, so its browser
    // Origin is the Vite origin rather than the backend's configured origin. Fetch Metadata is
    // browser-controlled; accept that narrow same-origin case while retaining the exact-origin
    // check for clients that do not provide it and rejecting cross-site browser requests.
    match headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
    {
        Some(fetch_site) if fetch_site.eq_ignore_ascii_case("same-origin") => origin.is_some(),
        Some(_) => origin.is_some_and(|origin| configured_origin == Some(origin)),
        None => origin.is_none_or(|origin| configured_origin == Some(origin)),
    }
}

fn response_is_private(mode: ServerMode, path: &str) -> bool {
    mode == ServerMode::Demo || path == "/api" || path.starts_with("/api/")
}

fn is_scalar_docs(path: &str) -> bool {
    path == "/api/docs" || path.starts_with("/api/docs/")
}

fn apply_security_headers(response: &mut Response, include_csp: bool) {
    let headers = response.headers_mut();
    if include_csp {
        headers.insert(
            "content-security-policy",
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
        );
    }
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static(PERMISSIONS_POLICY),
    );
}

fn apply_private_response_headers(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.append(header::VARY, HeaderValue::from_static("Cookie"));
    headers.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow, noarchive"),
    );
}

/// Reject cross-site state-changing browser requests and protect public Demo/private API
/// responses.
///
/// Demo mode requires the exact configured Origin on every state-changing request. Full mode
/// retains CLI/curl compatibility, accepts a browser-controlled `same-origin` request from the
/// Vite development proxy, and otherwise requires the configured Origin. This middleware never
/// emits CORS response headers.
///
/// Baseline browser protections apply to every response. Demo responses additionally receive
/// private/no-index caching headers on every path, including the embedded SPA; Full mode
/// receives those privacy headers on `/api` only so immutable static-asset caching is preserved.
/// Full mode's existing Scalar documentation page is the sole CSP exception because its upstream
/// HTML loads the Scalar bundle from jsDelivr; Demo mode does not expose that page.
pub async fn origin_and_security_headers(
    State(state): State<crate::state::AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let private_response = response_is_private(state.mode, path);
    let include_csp = state.mode == ServerMode::Demo || !is_scalar_docs(path);
    if is_state_changing(request.method())
        && !origin_is_allowed(
            state.mode,
            request.headers(),
            state.allowed_origin.as_deref(),
        )
    {
        let mut response =
            crate::error::ApiError::Forbidden("cross-origin request rejected".into())
                .into_response();
        apply_security_headers(&mut response, include_csp);
        if private_response {
            apply_private_response_headers(&mut response);
        }
        return response;
    }

    let mut response = next.run(request).await;
    apply_security_headers(&mut response, include_csp);
    if private_response {
        apply_private_response_headers(&mut response);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::to_bytes,
        http::{Request, StatusCode},
        middleware,
        routing::{get, post},
    };
    use tower::ServiceExt;

    #[test]
    fn demo_origin_check_requires_an_exact_match_and_rejects_missing_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://bhtunedemo.bytehound.ca"),
        );
        assert!(origin_is_allowed(
            ServerMode::Demo,
            &headers,
            Some("https://bhtunedemo.bytehound.ca")
        ));

        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://bhtunedemo.bytehound.ca.attacker.example"),
        );
        assert!(!origin_is_allowed(
            ServerMode::Demo,
            &headers,
            Some("https://bhtunedemo.bytehound.ca")
        ));

        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://bhtunedemo.bytehound.ca"),
        );
        assert!(!origin_is_allowed(
            ServerMode::Demo,
            &headers,
            Some("https://bhtunedemo.bytehound.ca")
        ));
        assert!(!origin_is_allowed(ServerMode::Demo, &headers, None));

        headers.remove(header::ORIGIN);
        assert!(!origin_is_allowed(
            ServerMode::Demo,
            &headers,
            Some("https://bhtunedemo.bytehound.ca")
        ));

        headers.append(
            header::ORIGIN,
            HeaderValue::from_static("https://bhtunedemo.bytehound.ca"),
        );
        headers.append(
            header::ORIGIN,
            HeaderValue::from_static("https://bhtunedemo.bytehound.ca"),
        );
        assert!(!origin_is_allowed(
            ServerMode::Demo,
            &headers,
            Some("https://bhtunedemo.bytehound.ca")
        ));
    }

    #[test]
    fn full_mode_preserves_cli_and_vite_proxy_mutations() {
        let mut headers = HeaderMap::new();
        assert!(origin_is_allowed(
            ServerMode::Full,
            &headers,
            Some("http://127.0.0.1:8787")
        ));

        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(!origin_is_allowed(
            ServerMode::Full,
            &headers,
            Some("http://127.0.0.1:8787")
        ));

        headers.insert(header::ORIGIN, HeaderValue::from_static("http://asus:5173"));
        assert!(origin_is_allowed(
            ServerMode::Full,
            &headers,
            Some("http://127.0.0.1:8787")
        ));

        headers.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert!(!origin_is_allowed(
            ServerMode::Full,
            &headers,
            Some("http://127.0.0.1:8787")
        ));

        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        headers.remove(header::ORIGIN);
        assert!(!origin_is_allowed(
            ServerMode::Full,
            &headers,
            Some("http://127.0.0.1:8787")
        ));
    }

    async fn app(mode: ServerMode) -> Router {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = mode;
        state.allowed_origin = Some("https://bhtunedemo.bytehound.ca".into());
        Router::new()
            .route("/api/change", post(|| async { StatusCode::NO_CONTENT }))
            .route(
                "/api/protected",
                get(|| async {
                    (
                        [
                            (header::CACHE_CONTROL, "public, max-age=60"),
                            (header::VARY, "Accept-Encoding"),
                        ],
                        "ok",
                    )
                }),
            )
            .route(
                "/asset.js",
                get(|| async { ([(header::CACHE_CONTROL, "public, immutable")], "asset") }),
            )
            .route("/api/docs", get(|| async { "Scalar" }))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                origin_and_security_headers,
            ))
            .with_state(state)
    }

    #[tokio::test]
    async fn demo_mutation_requires_the_configured_origin() {
        let app = app(ServerMode::Demo).await;
        let missing = app
            .clone()
            .oneshot(Request::post("/api/change").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::FORBIDDEN);
        assert_eq!(missing.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(missing.headers()["x-frame-options"], "DENY");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &to_bytes(missing.into_body(), usize::MAX).await.unwrap()
            )
            .unwrap(),
            serde_json::json!({"error": "cross-origin request rejected"})
        );

        let exact = app
            .oneshot(
                Request::post("/api/change")
                    .header(header::ORIGIN, "https://bhtunedemo.bytehound.ca")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(exact.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn full_mode_allows_vite_proxy_mutations_without_widening_cross_site_access() {
        let app = app(ServerMode::Full).await;
        let cli = app
            .clone()
            .oneshot(Request::post("/api/change").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(cli.status(), StatusCode::NO_CONTENT);

        let vite = app
            .clone()
            .oneshot(
                Request::post("/api/change")
                    .header("sec-fetch-site", "same-origin")
                    .header(header::ORIGIN, "http://asus:5173")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(vite.status(), StatusCode::NO_CONTENT);

        let cross_site = app
            .oneshot(
                Request::post("/api/change")
                    .header("sec-fetch-site", "cross-site")
                    .header(header::ORIGIN, "https://attacker.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cross_site.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn protected_responses_receive_security_headers_without_enabling_cors() {
        let response = app(ServerMode::Demo)
            .await
            .oneshot(Request::get("/api/protected").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let headers = response.headers();
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        let vary = headers
            .get_all(header::VARY)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(vary, ["Accept-Encoding", "Cookie"]);
        assert_eq!(headers["x-frame-options"], "DENY");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["referrer-policy"], "no-referrer");
        assert_eq!(headers["cross-origin-resource-policy"], "same-origin");
        assert_eq!(headers["cross-origin-opener-policy"], "same-origin");
        assert_eq!(headers["x-robots-tag"], "noindex, nofollow, noarchive");
        assert!(
            headers["permissions-policy"]
                .to_str()
                .unwrap()
                .contains("camera=()")
        );
        let csp = headers["content-security-policy"].to_str().unwrap();
        for directive in [
            "script-src 'self'",
            "style-src 'self'",
            "style-src-attr 'unsafe-inline'",
            "connect-src 'self'",
            "img-src 'self'",
            "font-src 'self'",
            "object-src 'none'",
            "base-uri 'self'",
            "frame-ancestors 'none'",
        ] {
            assert!(
                csp.contains(directive),
                "missing CSP directive: {directive}"
            );
        }
        assert!(!headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
        assert!(!headers.contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS));
    }

    #[tokio::test]
    async fn full_mode_preserves_scalar_without_weakening_demo_csp() {
        let full = app(ServerMode::Full)
            .await
            .oneshot(Request::get("/api/docs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(!full.headers().contains_key("content-security-policy"));
        assert_eq!(full.headers()["x-frame-options"], "DENY");

        let demo = app(ServerMode::Demo)
            .await
            .oneshot(Request::get("/api/docs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(demo.headers().contains_key("content-security-policy"));
    }

    #[tokio::test]
    async fn full_mode_api_responses_receive_private_response_headers() {
        let response = app(ServerMode::Full)
            .await
            .oneshot(Request::get("/api/protected").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()["x-robots-tag"],
            "noindex, nofollow, noarchive"
        );
    }

    #[tokio::test]
    async fn demo_static_responses_are_private_and_no_index() {
        let response = app(ServerMode::Demo)
            .await
            .oneshot(Request::get("/asset.js").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()["x-robots-tag"],
            "noindex, nofollow, noarchive"
        );
        assert!(response.headers().contains_key("content-security-policy"));
    }

    #[tokio::test]
    async fn demo_router_protects_the_embedded_spa_fallback() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.allowed_origin = Some("https://bhtunedemo.bytehound.ca".into());
        let response = crate::build_router(state)
            .oneshot(
                Request::get("/client-side-route")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()["x-frame-options"], "DENY");
        assert!(response.headers().contains_key("content-security-policy"));
    }

    #[tokio::test]
    async fn full_mode_keeps_non_api_asset_caching_unchanged() {
        let response = app(ServerMode::Full)
            .await
            .oneshot(Request::get("/asset.js").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, immutable"
        );
        assert!(response.headers().contains_key("content-security-policy"));
        assert!(!response.headers().contains_key("x-robots-tag"));
    }
}
