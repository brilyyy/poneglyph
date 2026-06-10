//! Embedded viewer assets (PRD §8.12).
//!
//! With the `embed-viewer` feature, `viewer/dist` (built by
//! `scripts/build-release.sh`) is compiled in via rust-embed and served with
//! an SPA fallback. Without it, a placeholder page keeps `/` alive so plain
//! `cargo build` never needs Node.

#[cfg(feature = "embed-viewer")]
use axum::http::{header, StatusCode};
use axum::http::Uri;
#[cfg(not(feature = "embed-viewer"))]
use axum::response::Html;
use axum::response::{IntoResponse, Response};

#[cfg(feature = "embed-viewer")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../viewer/dist/"]
struct ViewerAssets;

#[cfg(feature = "embed-viewer")]
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // SPA routes (no extension) and the root fall back to index.html.
    let serve_index = path.is_empty() || !path.contains('.');
    let asset_path = if serve_index { "index.html" } else { path };

    match ViewerAssets::get(asset_path) {
        Some(file) => {
            let mime = mime_guess::from_path(asset_path).first_or_octet_stream();
            // Vite emits content-hashed filenames under assets/ — cache hard.
            let cache = if asset_path.starts_with("assets/") {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            (
                [
                    (header::CONTENT_TYPE, mime.as_ref().to_string()),
                    (header::CACHE_CONTROL, cache.to_string()),
                ],
                file.data,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(not(feature = "embed-viewer"))]
pub async fn static_handler(_uri: Uri) -> Response {
    Html(
        "<!doctype html><html><head><title>poneglyph</title></head><body style=\"font-family: system-ui; max-width: 40rem; margin: 4rem auto;\">\
         <h1>poneglyph is running</h1>\
         <p>The HTTP API is live at <code>/api</code> and passive capture at <code>/ingest</code>.</p>\
         <p>This binary was built without the embedded viewer. Build with <code>scripts/build-release.sh</code> (or <code>--features viewer</code>) to include it.</p>\
         </body></html>",
    )
    .into_response()
}
