//! Static files compiled into the binary. Everything here is public (no auth):
//! browsers fetch the manifest, icons and service worker without the session.

use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

macro_rules! asset {
    ($path:literal) => {
        include_bytes!(concat!("../static/", $path)).as_slice()
    };
}

fn lookup(path: &str) -> Option<(&'static [u8], &'static str)> {
    Some(match path {
        "style.css" => (asset!("style.css"), "text/css; charset=utf-8"),
        "app.js" => (asset!("app.js"), "text/javascript; charset=utf-8"),
        "sw.js" => (asset!("sw.js"), "text/javascript; charset=utf-8"),
        "manifest.webmanifest" => (asset!("manifest.webmanifest"), "application/manifest+json"),
        "offline.html" => (asset!("offline.html"), "text/html; charset=utf-8"),
        "icons/icon.svg" => (asset!("icons/icon.svg"), "image/svg+xml"),
        "icons/icon-192.png" => (asset!("icons/icon-192.png"), "image/png"),
        "icons/icon-512.png" => (asset!("icons/icon-512.png"), "image/png"),
        "icons/icon-maskable-512.png" => (asset!("icons/icon-maskable-512.png"), "image/png"),
        "icons/apple-touch-icon.png" => (asset!("icons/apple-touch-icon.png"), "image/png"),
        _ => return None,
    })
}

fn serve(path: &str) -> Response {
    let Some((body, mime)) = lookup(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // sw.js must revalidate or browsers keep running an old worker.
    let cache = if path == "sw.js" { "no-cache" } else { "public, max-age=3600" };
    ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, cache)], body).into_response()
}

pub async fn root_file(uri: axum::http::Uri) -> Response {
    serve(uri.path().trim_start_matches('/'))
}

pub async fn icon(Path(name): Path<String>) -> Response {
    serve(&format!("icons/{name}"))
}

pub async fn apple_touch_icon() -> Response {
    serve("icons/apple-touch-icon.png")
}
