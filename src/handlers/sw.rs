//! Serves the service worker script dynamically (rather than as a static
//! file) so `crate::APP_VERSION` gets baked into its bytes on every
//! request. This is what makes the browser's own update-detection fire at
//! all — it works by byte-diffing a refetched `/sw.js` against the
//! installed one — and it keeps the service worker's cache-naming scheme
//! atomically in sync with the running binary. See `templates/sw.js` for
//! the actual caching logic.

use askama::Template;
use axum::http::header;
use axum::response::IntoResponse;

#[derive(Template)]
#[template(path = "sw.js", escape = "none")]
struct ServiceWorkerTemplate<'a> {
    app_version: &'a str,
}

pub async fn serve_sw() -> impl IntoResponse {
    let body = ServiceWorkerTemplate {
        app_version: crate::APP_VERSION,
    }
    .render()
    .expect("sw.js template is static and always renders");

    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
}
