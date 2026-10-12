//! Thin wrapper around the `yt-dlp` binary. Every call passes arguments as a
//! list (never via a shell) and puts the URL after `--`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use tokio::{process::Command, sync::Mutex};

use crate::config::Config;

const INFO_TIMEOUT: Duration = Duration::from_secs(90);
const INFO_CACHE_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
pub struct YtDlp {
    cfg: Arc<Config>,
    /// url -> (fetched_at, raw info json). Reused for transcripts via --load-info-json.
    cache: Arc<Mutex<HashMap<String, (Instant, Arc<Value>)>>>,
}

#[derive(Serialize)]
pub struct VideoInfo {
    pub id: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub webpage_url: String,
    pub language: Option<String>,
    pub formats: Vec<Format>,
    pub captions: Captions,
}

#[derive(Serialize)]
pub struct Format {
    pub id: String,
    pub ext: String,
    pub kind: &'static str, // "video+audio" | "video" | "audio"
    pub resolution: Option<String>,
    pub height: Option<u64>,
    pub fps: Option<f64>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub filesize: Option<u64>,
    pub note: Option<String>,
}

#[derive(Serialize, Default)]
pub struct Captions {
    pub manual: Vec<CaptionTrack>,
    pub auto: Vec<CaptionTrack>,
    pub translated: Vec<CaptionTrack>,
}

#[derive(Serialize)]
pub struct CaptionTrack {
    pub code: String,
    pub name: String,
}

impl YtDlp {
    pub fn new(cfg: Arc<Config>) -> Self {
        Self { cfg, cache: Default::default() }
    }

    /// Rejects anything that isn't a plain http(s) URL on an allowed host.
    pub fn validate_url(&self, raw: &str) -> Result<String> {
        let raw = raw.trim();
        if raw.len() > 2048 {
            bail!("URL too long");
        }
        let url = url::Url::parse(raw).map_err(|_| anyhow!("Invalid URL"))?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("Only http(s) URLs are allowed");
        }
        let host = url.host_str().ok_or_else(|| anyhow!("URL has no host"))?.to_lowercase();
        let allowed = &self.cfg.allowed_domains;
        if !allowed.is_empty()
            && !allowed.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
        {
            bail!("Host '{host}' is not allowed");
        }
        Ok(url.to_string())
    }

    /// Common args for every invocation.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.cfg.ytdlp_path);
        cmd.args(["--ignore-config", "--no-playlist", "--no-colors", "--no-warnings"]);
        if let Some(c) = self.cfg.cookies_file.as_ref().filter(|p| p.exists()) {
            cmd.arg("--cookies").arg(c);
        }
        cmd.stdin(Stdio::null()).kill_on_drop(true);
        cmd
    }

    pub async fn version(&self) -> Option<String> {
        let out = Command::new(&self.cfg.ytdlp_path).arg("--version").output().await.ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub async fn self_update(&self) {
        match Command::new(&self.cfg.ytdlp_path).arg("-U").output().await {
            Ok(o) => tracing::info!("yt-dlp -U: {}", String::from_utf8_lossy(&o.stdout).trim()),
            Err(e) => tracing::warn!("yt-dlp -U failed: {e}"),
        }
    }

    async fn raw_info(&self, url: &str) -> Result<Arc<Value>> {
        {
            let mut cache = self.cache.lock().await;
            cache.retain(|_, (t, _)| t.elapsed() < INFO_CACHE_TTL);
            if let Some((_, v)) = cache.get(url) {
                return Ok(v.clone());
            }
        }
        let mut cmd = self.command();
        cmd.args(["-J", "--", url]);
        let out = tokio::time::timeout(INFO_TIMEOUT, cmd.output())
            .await
            .map_err(|_| anyhow!("yt-dlp timed out"))?
            .context("failed to run yt-dlp")?;
        if !out.status.success() {
            bail!("{}", last_error(&out.stderr));
        }
        let v: Value = serde_json::from_slice(&out.stdout).context("invalid yt-dlp JSON")?;
        let v = Arc::new(v);
        self.cache.lock().await.insert(url.to_string(), (Instant::now(), v.clone()));
        Ok(v)
    }

    pub async fn info(&self, url: &str) -> Result<VideoInfo> {
        let v = self.raw_info(url).await?;
        Ok(parse_info(&v))
    }

    /// Downloads one caption track as VTT into `dir` and returns its contents.
    pub async fn fetch_vtt(&self, url: &str, lang: &str, auto: bool, dir: &Path) -> Result<String> {
        if lang.is_empty()
            || lang.len() > 32
            || !lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!("Invalid language code");
        }
        tokio::fs::create_dir_all(dir).await?;

        let mut cmd = self.command();
        cmd.args(["--skip-download", "--sub-format", "vtt/best", "--convert-subs", "vtt"])
            .arg(if auto { "--write-auto-subs" } else { "--write-subs" })
            // Anchored so "en" doesn't also match "en-GB" (sub-langs is a regex).
            .arg("--sub-langs")
            .arg(format!("^{lang}$"))
            .arg("-o")
            .arg(dir.join("sub.%(ext)s"));

        // Reuse cached metadata when we have it: much faster than re-extracting.
        let cached = self.cache.lock().await.get(url).map(|(_, v)| v.clone());
        if let Some(info) = cached {
            let p = dir.join("info.json");
            tokio::fs::write(&p, serde_json::to_vec(&*info)?).await?;
            cmd.arg("--load-info-json").arg(&p);
        } else {
            cmd.args(["--", url]);
        }

        let out = tokio::time::timeout(INFO_TIMEOUT, cmd.output())
            .await
            .map_err(|_| anyhow!("yt-dlp timed out"))??;
        if !out.status.success() {
            bail!("{}", last_error(&out.stderr));
        }
        let vtt = find_ext(dir, "vtt").await.ok_or_else(|| anyhow!("No captions found for '{lang}'"))?;
        Ok(tokio::fs::read_to_string(vtt).await?)
    }
}

/// Last non-empty stderr line, which is where yt-dlp puts its `ERROR:` message.
pub fn last_error(stderr: &[u8]) -> String {
    let s = String::from_utf8_lossy(stderr);
    s.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim_start_matches("ERROR:").trim().to_string())
        .unwrap_or_else(|| "yt-dlp failed".into())
}

pub async fn find_ext(dir: &Path, ext: &str) -> Option<PathBuf> {
    let mut rd = tokio::fs::read_dir(dir).await.ok()?;
    while let Ok(Some(e)) = rd.next_entry().await {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some(ext) {
            return Some(p);
        }
    }
    None
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn codec(v: &Value, k: &str) -> Option<String> {
    s(v, k).filter(|c| c != "none")
}

fn parse_info(v: &Value) -> VideoInfo {
    let mut formats: Vec<Format> = v
        .get("formats")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(parse_format).collect())
        .unwrap_or_default();
    // Best first: muxed/video by height, then audio.
    formats.sort_by(|a, b| b.height.unwrap_or(0).cmp(&a.height.unwrap_or(0)));

    VideoInfo {
        id: s(v, "id").unwrap_or_default(),
        title: s(v, "title").unwrap_or_else(|| "Untitled".into()),
        uploader: s(v, "uploader").or_else(|| s(v, "channel")),
        duration: v.get("duration").and_then(Value::as_f64),
        thumbnail: s(v, "thumbnail"),
        webpage_url: s(v, "webpage_url").unwrap_or_default(),
        language: s(v, "language"),
        formats,
        captions: parse_captions(v),
    }
}

fn parse_format(f: &Value) -> Option<Format> {
    let vcodec = codec(f, "vcodec");
    let acodec = codec(f, "acodec");
    let kind = match (&vcodec, &acodec) {
        (Some(_), Some(_)) => "video+audio",
        (Some(_), None) => "video",
        (None, Some(_)) => "audio",
        (None, None) => return None, // storyboards, etc.
    };
    // Skip manifests that aren't directly useful as a choice.
    if s(f, "protocol").is_some_and(|p| p.starts_with("mhtml")) {
        return None;
    }
    Some(Format {
        id: s(f, "format_id")?,
        ext: s(f, "ext").unwrap_or_default(),
        kind,
        resolution: s(f, "resolution"),
        height: f.get("height").and_then(Value::as_u64),
        fps: f.get("fps").and_then(Value::as_f64),
        vcodec,
        acodec,
        filesize: f
            .get("filesize")
            .and_then(Value::as_u64)
            .or_else(|| f.get("filesize_approx").and_then(Value::as_u64)),
        note: s(f, "format_note"),
    })
}

fn track_name(code: &str, entries: &[Value]) -> String {
    entries
        .iter()
        .find_map(|e| s(e, "name"))
        .unwrap_or_else(|| code.to_string())
}

fn parse_captions(v: &Value) -> Captions {
    let mut c = Captions::default();
    if let Some(subs) = v.get("subtitles").and_then(Value::as_object) {
        for (code, entries) in subs {
            if code == "live_chat" {
                continue;
            }
            let entries = entries.as_array().map(Vec::as_slice).unwrap_or_default();
            c.manual.push(CaptionTrack { code: code.clone(), name: track_name(code, entries) });
        }
    }
    if let Some(auto) = v.get("automatic_captions").and_then(Value::as_object) {
        for (code, entries) in auto {
            let entries = entries.as_array().map(Vec::as_slice).unwrap_or_default();
            // YouTube marks machine-translated tracks with a `tlang` URL param.
            let translated = entries
                .iter()
                .any(|e| s(e, "url").is_some_and(|u| u.contains("tlang=")));
            let track = CaptionTrack { code: code.clone(), name: track_name(code, entries) };
            if translated {
                c.translated.push(track);
            } else {
                c.auto.push(track);
            }
        }
    }
    c.manual.sort_by(|a, b| a.name.cmp(&b.name));
    c.auto.sort_by(|a, b| a.name.cmp(&b.name));
    c.translated.sort_by(|a, b| a.name.cmp(&b.name));
    c
}
