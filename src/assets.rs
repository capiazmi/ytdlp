//! Static files compiled into the binary. Everything here is public (no auth):
//! browsers fetch the manifest, icons and service worker without the session.
//!
//! Cache busting: pages reference `/app.js?v=<hash>`, `/style.css?v=<hash>` etc.,
//! where the hash covers the frontend files. A deploy that changes them changes
//! the URLs, so browsers can never pair a new page with an old script.

use std::{borrow::Cow, sync::LazyLock};

use axum::{
    extract::Path,
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
};
use sha2::{Digest, Sha256};

macro_rules! asset {
    ($path:literal) => {
        include_bytes!(concat!("../static/", $path)).as_slice()
    };
}

pub static VERSION: LazyLock<String> = LazyLock::new(|| {
    let mut h = Sha256::new();
    for part in [
        asset!("common.js"),
        asset!("app.js"),
        asset!("scribd.js"),
        asset!("style.css"),
        asset!("sw.js"),
        asset!("index.html"),
        asset!("scribd.html"),
        asset!("login.html"),
    ] {
        h.update(part);
    }
    hex::encode(h.finalize())[..10].to_string()
});

fn versioned(html: &str) -> String {
    let v = VERSION.as_str();
    ["/common.js", "/app.js", "/scribd.js", "/style.css"]
        .iter()
        .fold(html.to_string(), |h, f| h.replace(&format!("\"{f}\""), &format!("\"{f}?v={v}\"")))
}

pub static INDEX: LazyLock<String> = LazyLock::new(|| versioned(include_str!("../static/index.html")));
pub static SCRIBD: LazyLock<String> = LazyLock::new(|| versioned(include_str!("../static/scribd.html")));
pub static LOGIN: LazyLock<String> = LazyLock::new(|| versioned(include_str!("../static/login.html")));
static OFFLINE: LazyLock<String> = LazyLock::new(|| versioned(include_str!("../static/offline.html")));
// New cache name per version so the worker drops old shell files on activate.
static SW: LazyLock<String> =
    LazyLock::new(|| include_str!("../static/sw.js").replace("'ytdlp-v1'", &format!("'ytdlp-{}'", *VERSION)));

pub fn index_page() -> Html<&'static str> {
    Html(INDEX.as_str())
}

pub fn scribd_page() -> Html<&'static str> {
    Html(SCRIBD.as_str())
}

pub fn login_page() -> Html<&'static str> {
    Html(LOGIN.as_str())
}

fn lookup(path: &str) -> Option<(Cow<'static, [u8]>, &'static str)> {
    let b = |s: &'static [u8]| Cow::Borrowed(s);
    Some(match path {
        "style.css" => (b(asset!("style.css")), "text/css; charset=utf-8"),
        "common.js" => (b(asset!("common.js")), "text/javascript; charset=utf-8"),
        "app.js" => (b(asset!("app.js")), "text/javascript; charset=utf-8"),
        "scribd.js" => (b(asset!("scribd.js")), "text/javascript; charset=utf-8"),
        "sw.js" => (b(SW.as_bytes()), "text/javascript; charset=utf-8"),
        "offline.html" => (b(OFFLINE.as_bytes()), "text/html; charset=utf-8"),
        "manifest.webmanifest" => (b(asset!("manifest.webmanifest")), "application/manifest+json"),
        "icons/icon.svg" => (b(asset!("icons/icon.svg")), "image/svg+xml"),
        "icons/icon-192.png" => (b(asset!("icons/icon-192.png")), "image/png"),
        "icons/icon-512.png" => (b(asset!("icons/icon-512.png")), "image/png"),
        "icons/icon-maskable-512.png" => (b(asset!("icons/icon-maskable-512.png")), "image/png"),
        "icons/apple-touch-icon.png" => (b(asset!("icons/apple-touch-icon.png")), "image/png"),
        _ => return None,
    })
}

fn serve(path: &str, headers: &HeaderMap) -> Response {
    let Some((body, mime)) = lookup(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Always revalidate; the ETag makes that a cheap 304.
    let etag = format!("\"{}\"", *VERSION);
    if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CACHE_CONTROL, "no-cache".to_string()),
            (header::ETAG, etag),
        ],
        body.into_owned(),
    )
        .into_response()
}

pub async fn root_file(uri: axum::http::Uri, headers: HeaderMap) -> Response {
    serve(uri.path().trim_start_matches('/'), &headers)
}

pub async fn icon(Path(name): Path<String>, headers: HeaderMap) -> Response {
    serve(&format!("icons/{name}"), &headers)
}

pub async fn apple_touch_icon(headers: HeaderMap) -> Response {
    serve("icons/apple-touch-icon.png", &headers)
}
