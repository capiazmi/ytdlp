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

use crate::{jobs::NewJob, pdf, subtitles, AppState};

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
    /// txt | pdf | srt | vtt
    #[serde(default = "default_fmt")]
    format: String,
    /// txt/pdf only; srt/vtt are always timed
    #[serde(default)]
    timestamps: bool,
    #[serde(default)]
    download: bool,
}

fn default_fmt() -> String {
    "txt".into()
}

pub async fn transcript(State(s): State<AppState>, Query(q): Query<TranscriptQuery>) -> ApiResult<Response> {
    let url = s.ytdlp.validate_url(&q.url)?;
    let auto = q.kind == "auto";
    // Also warms the metadata cache so fetch_vtt can skip re-extraction.
    let info = s.ytdlp.info(&url).await?;
    let tmp = s.cfg.download_dir.join(".tmp").join(Uuid::new_v4().to_string());
    let res = transcript_body(&s, &q, &url, &info, auto, &tmp).await;
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let (body, ext, mime) = res?;

    let mut resp = ([(header::CONTENT_TYPE, mime)], body).into_response();
    if q.download {
        let id: String = info.id.chars().filter(|c| c.is_ascii_alphanumeric() || "-_".contains(*c)).collect();
        let lang: String = q.lang.chars().filter(|c| c.is_ascii_alphanumeric() || "-_".contains(*c)).collect();
        let suffix = if q.timestamps && matches!(ext, "txt" | "pdf") { ".timestamps" } else { "" };
        let disp = format!("attachment; filename=\"{id}.{lang}{suffix}.{ext}\"");
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, disp.parse().unwrap());
    }
    Ok(resp)
}

async fn transcript_body(
    s: &AppState,
    q: &TranscriptQuery,
    url: &str,
    info: &crate::ytdlp::VideoInfo,
    auto: bool,
    tmp: &std::path::Path,
) -> anyhow::Result<(Body, &'static str, &'static str)> {
    let vtt = s.ytdlp.fetch_vtt(url, &q.lang, auto, &tmp.join("sub")).await?;
    let segs = subtitles::parse_vtt(&vtt);
    Ok(match q.format.as_str() {
        "srt" => (subtitles::to_srt(&segs).into(), "srt", "application/x-subrip; charset=utf-8"),
        "vtt" => (vtt.into(), "vtt", "text/vtt; charset=utf-8"),
        "pdf" => {
            let c = &info.captions;
            let list = if auto { c.auto.iter().chain(&c.translated).collect::<Vec<_>>() } else { c.manual.iter().collect() };
            let lang_name = list.iter().find(|t| t.code == q.lang).map(|t| t.name.as_str()).unwrap_or(&q.lang);
            let is_youtube = url::Url::parse(&info.webpage_url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.ends_with("youtube.com") || h.ends_with("youtu.be")))
                .unwrap_or(false);
            let meta = pdf::Meta {
                title: &info.title,
                uploader: info.uploader.as_deref(),
                duration: info.duration,
                url: if info.webpage_url.is_empty() { url } else { &info.webpage_url },
                youtube_id: is_youtube.then_some(info.id.as_str()),
                code: &q.lang,
                lang_name,
                auto,
            };
            let bytes = pdf::render(&s.cfg.typst_path, &tmp.join("pdf"), meta, &segs, q.timestamps).await?;
            (bytes.into(), "pdf", "application/pdf")
        }
        _ if q.timestamps => (subtitles::to_timestamped(&segs).into(), "txt", "text/plain; charset=utf-8"),
        _ => (subtitles::to_text(&segs).into(), "txt", "text/plain; charset=utf-8"),
    })
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
