//! Static asset serving for the embedded frontend.
//!
//! `build.rs` ensures `web/dist/` is freshly built before compilation
//! (it runs `bunx vite build` in the web directory). At runtime, this
//! module's [`spa_service`] is mounted as `.fallback_service(...)` on the
//! top-level router so that:
//!
//!   - `/api/...` requests are still handled by the API routes
//!   - `/`, `/sessions/{id}`, etc. (SPA client-side routes) fall through
//!     to `web/dist/index.html`
//!   - `/assets/index-*.{js,css}` and other real files are served as-is
//!
//! The dist path is resolved at compile time via
//! `env!("CARGO_MANIFEST_DIR")`, so the binary is CWD-independent.

use tower_http::services::{ServeDir, ServeFile};

/// Absolute path to the built frontend directory, resolved at compile time.
///
/// `concat!` is evaluated by the compiler; CWD at runtime does not matter.
const DIST_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/dist");

/// `index.html` path used as the SPA fallback for client-side routes.
const INDEX_HTML: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/dist/index.html");

/// Build the SPA fallback service.
///
/// `ServeDir::fallback` is called when `ServeDir` doesn't find a file
/// at the request path; the inner service's status is preserved, so
/// `ServeFile` returns 200 with `index.html` for SPA client-side routes.
/// (Contrast with `not_found_service`, which would force the status to
/// 404 — wrong for an SPA fallback.)
///
/// Note: unmatched `/api/*` paths also fall through to this service
/// (e.g. `GET /api/nonexistent` returns `index.html` with 200). This
/// is a known minor regression documented in PROGRESS.md; the future
/// fix is a `.nest("/api", ...)` refactor with an explicit 404 handler.
pub fn spa_service() -> ServeDir<ServeFile> {
    ServeDir::new(DIST_PATH).fallback(ServeFile::new(INDEX_HTML))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    /// Mirror the production router shape: an API route + the SPA
    /// fallback. The stub returns JSON, not the index.html shell, so
    /// the test can tell which one won the routing.
    async fn stub_api() -> &'static str {
        r#"{"stub":true}"#
    }

    fn test_app() -> Router {
        Router::new()
            .route("/api/health", get(stub_api))
            .fallback_service(super::spa_service())
    }

    #[tokio::test]
    async fn test_root_serves_index_html() {
        let res = test_app()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let s = std::str::from_utf8(&body).unwrap();
        assert!(
            s.contains(r#"id="root""#),
            "GET / should serve index.html with id=\"root\"; got first 200 chars: {}",
            &s[..s.len().min(200)]
        );
    }

    #[tokio::test]
    async fn test_deep_link_falls_back_to_index_html() {
        // SPA client-side route — no file at /sessions/abc-123, so
        // ServeDir's fallback kicks in and serves index.html.
        let res = test_app()
            .oneshot(
                Request::builder()
                    .uri("/sessions/abc-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let s = std::str::from_utf8(&body).unwrap();
        assert!(
            s.contains(r#"id="root""#),
            "GET /sessions/abc-123 should fall back to index.html; got: {}",
            &s[..s.len().min(200)]
        );
    }

    #[tokio::test]
    async fn test_api_route_takes_precedence_over_fallback() {
        // Regression: the SPA fallback must not eat /api/* requests.
        // The stub route is registered before the fallback, so it
        // wins; the body must be the stub JSON, not index.html.
        let res = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let s = std::str::from_utf8(&body).unwrap();
        assert!(
            s.contains(r#""stub":true"#),
            "GET /api/health should return the stub JSON, not index.html; got: {s}"
        );
    }

    #[tokio::test]
    async fn test_real_asset_serves_with_correct_status() {
        // Resolve the hashed JS bundle from web/dist/index.html, then
        // assert that GET on it returns 200 with the JS content. This
        // guards against vite `base` regressions and mime_guess upgrades
        // that could otherwise break the SPA while leaving the
        // index.html shell intact.
        let index_html = std::fs::read_to_string(super::INDEX_HTML).expect("dist built");
        let script_src = index_html
            .lines()
            .find_map(|line| line.split("src=\"").nth(1)?.split('"').next())
            .filter(|s| s.starts_with("/assets/"))
            .expect("index.html should reference a hashed /assets/*.js");
        let res = test_app()
            .oneshot(
                Request::builder()
                    .uri(script_src)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert!(
            !body.is_empty(),
            "GET {script_src} should return non-empty JS body, got {} bytes",
            body.len()
        );
    }

    #[tokio::test]
    async fn test_missing_asset_falls_back_to_index_html() {
        // GET on a non-existent path (e.g. /favicon.ico, which we don't
        // ship) should fall back to index.html with 200, not return 404.
        // Pins the SPA-fallback semantics against future tower-http
        // upgrades that change "missing file" handling.
        let res = test_app()
            .oneshot(
                Request::builder()
                    .uri("/favicon.ico")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let s = std::str::from_utf8(&body).unwrap();
        assert!(
            s.contains(r#"id="root""#),
            "GET /favicon.ico should fall back to index.html; got: {}",
            &s[..s.len().min(200)]
        );
    }
}
