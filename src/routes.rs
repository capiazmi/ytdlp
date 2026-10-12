use std::convert::Infallible;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Response,
    },
    Json,
};
use serde::Deserialize;
use serde_json::json;
use tokio_stream::{wrappers::BroadcastStream, Stream, StreamExt};
use uuid::Uuid;

use crate::{jobs::NewJob, subtitles, AppState};

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(StatusCode::BAD_REQUEST, e.to_string())
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "not found".into())
}

pub async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

pub async fn app_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], include_str!("../static/app.js"))
}

pub async fn style_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], include_str!("../static/style.css"))
}

pub async fn healthz() -> &'static str {
    "ok"
}

pub async fn status(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "ytdlp_version": s.ytdlp.version().await,
        "auth": s.cfg.app_password.is_some(),
        "file_ttl_hours": s.cfg.file_ttl_hours,
        "cookies": s.cfg.cookies_file.as_ref().is_some_and(|p| p.exists()),
    }))
}

#[derive(Deserialize)]
pub struct UrlQuery {
    url: String,
}

pub async fn info(State(s): State<AppState>, Query(q): Query<UrlQuery>) -> ApiResult<impl IntoResponse> {
    let url = s.ytdlp.validate_url(&q.url)?;
    Ok(Json(s.ytdlp.info(&url).await?))
}

#[derive(Deserialize)]
pub struct TranscriptQuery {
    url: String,
    lang: String,
    /// manual | auto
    kind: String,
    /// txt | ts | srt | vtt
    #[serde(default = "default_fmt")]
    format: String,
    #[serde(default)]
    download: bool,
    /// used for the download filename
    id: Option<String>,
}

fn default_fmt() -> String {
    "txt".into()
}

pub async fn transcript(State(s): State<AppState>, Query(q): Query<TranscriptQuery>) -> ApiResult<Response> {
    let url = s.ytdlp.validate_url(&q.url)?;
    let tmp = s.cfg.download_dir.join(".tmp").join(Uuid::new_v4().to_string());
    let res = s.ytdlp.fetch_vtt(&url, &q.lang, q.kind == "auto", &tmp).await;
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let vtt = res?;

    let segs = subtitles::parse_vtt(&vtt);
    let (body, ext, mime) = match q.format.as_str() {
        "ts" => (subtitles::to_timestamped(&segs), "txt", "text/plain"),
        "srt" => (subtitles::to_srt(&segs), "srt", "application/x-subrip"),
        "vtt" => (vtt, "vtt", "text/vtt"),
        _ => (subtitles::to_text(&segs), "txt", "text/plain"),
    };

    let mut resp = ([(header::CONTENT_TYPE, format!("{mime}; charset=utf-8"))], body).into_response();
    if q.download {
        let id: String = q
            .id
            .unwrap_or_else(|| "transcript".into())
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || "-_".contains(*c))
            .collect();
        let suffix = if q.format == "ts" { ".timestamps" } else { "" };
        let disp = format!("attachment; filename=\"{id}.{}{suffix}.{ext}\"", q.lang);
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, disp.parse().unwrap());
    }
    Ok(resp)
}

pub async fn list_jobs(State(s): State<AppState>) -> impl IntoResponse {
    Json(s.jobs.list().await)
}

pub async fn create_job(State(s): State<AppState>, Json(req): Json<NewJob>) -> ApiResult<impl IntoResponse> {
    Ok((StatusCode::CREATED, Json(s.jobs.create(req).await?)))
}

pub async fn delete_job(State(s): State<AppState>, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    if s.jobs.remove(id).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}

/// Server-Sent Events: one `job` event per job state change.
pub async fn events(State(s): State<AppState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(s.jobs.events.subscribe()).filter_map(|msg| {
        let job = msg.ok()?;
        Some(Ok(Event::default().event("job").json_data(job).ok()?))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub async fn download_file(State(s): State<AppState>, Path(id): Path<Uuid>) -> ApiResult<Response> {
    let job = s.jobs.get(id).await.ok_or_else(not_found)?;
    let path = job.file.ok_or_else(not_found)?;
    let file = tokio::fs::File::open(&path).await.map_err(|_| not_found())?;
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let name: String = job
        .filename
        .unwrap_or_else(|| "download".into())
        .chars()
        .map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' })
        .collect();

    let body = Body::from_stream(tokio_util::io::ReaderStream::new(file));
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        body,
    )
        .into_response())
}
