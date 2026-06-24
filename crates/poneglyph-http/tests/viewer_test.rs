//! Embedded-viewer smoke tests. Only meaningful with the `embed-viewer`
//! feature (requires `pnpm -C viewer build` first); compiled out otherwise.
#![cfg(feature = "embed-viewer")]

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use poneglyph_core::config::Config;
use poneglyph_core::store::Store;
use poneglyph_http::{AppState, build_router};

fn router() -> axum::Router {
    let state = AppState {
        store: Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
        embedder: None,
        config: Arc::new(Config::default()),
        enrich: None,
        graph_dirty: None,
    };
    build_router(state)
}

#[tokio::test]
async fn root_serves_spa_index() {
    let resp = router()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(ct.starts_with("text/html"));
}

#[tokio::test]
async fn spa_routes_fall_back_to_index() {
    let resp = router()
        .oneshot(Request::builder().uri("/memories/whatever").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(ct.starts_with("text/html"));
}

#[tokio::test]
async fn missing_asset_with_extension_is_404() {
    let resp = router()
        .oneshot(Request::builder().uri("/assets/missing.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
