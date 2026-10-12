//! Single shared password/PIN from APP_PASSWORD. The session cookie holds
//! sha256(secret|password), so changing either logs everyone out.

use std::time::Duration;

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
    Form,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::AppState;

const COOKIE: &str = "ytdlp_session";

fn token(state: &AppState) -> Option<String> {
    let pw = state.cfg.app_password.as_ref()?;
    let mut h = Sha256::new();
    h.update(state.cfg.session_secret.as_bytes());
    h.update(b"|");
    h.update(pw.as_bytes());
    Some(hex::encode(h.finalize()))
}

fn cookie_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(&format!("{COOKIE}=")).map(str::to_string))
}

pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = token(&state) else {
        return next.run(req).await; // auth disabled
    };
    let ok = cookie_value(req.headers())
        .is_some_and(|v| bool::from(v.as_bytes().ct_eq(expected.as_bytes())));
    if ok {
        return next.run(req).await;
    }
    let path = req.uri().path();
    if path.starts_with("/api/") || path.starts_with("/files/") {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    // Keep where they were going (e.g. /share?url=... from the share sheet).
    let target = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
    if target == "/" {
        Redirect::to("/login").into_response()
    } else {
        let next: String = url::form_urlencoded::byte_serialize(target.as_bytes()).collect();
        Redirect::to(&format!("/login?next={next}")).into_response()
    }
}

/// Only same-site relative paths; blocks `//evil.com` and `/\\evil.com`.
fn safe_next(next: Option<&str>) -> &str {
    match next {
        Some(n)
            if n.starts_with('/')
                && !n.starts_with("//")
                && !n.starts_with("/\\")
                && n.bytes().all(|b| b.is_ascii_graphic()) =>
        {
            n
        }
        _ => "/",
    }
}

pub async fn login_page() -> Html<&'static str> {
    Html(include_str!("../static/login.html"))
}

#[derive(Deserialize)]
pub struct LoginForm {
    password: String,
    next: Option<String>,
}

pub async fn login(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    let next = safe_next(form.next.as_deref());
    let Some(pw) = state.cfg.app_password.as_ref() else {
        return Redirect::to(next).into_response();
    };
    if !bool::from(form.password.as_bytes().ct_eq(pw.as_bytes())) {
        // Slow down brute force on short PINs.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let enc: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
        return Redirect::to(&format!("/login?error=1&next={enc}")).into_response();
    }
    let secure = if state.cfg.cookie_secure { "; Secure" } else { "" };
    let cookie = format!(
        "{COOKIE}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000{secure}",
        token(&state).unwrap_or_default()
    );
    ([(header::SET_COOKIE, cookie)], Redirect::to(next)).into_response()
}

pub async fn logout() -> Response {
    let cookie = format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    ([(header::SET_COOKIE, cookie)], Redirect::to("/login")).into_response()
}
