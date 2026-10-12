//! In-memory download queue. State is lost on restart; files on disk are
//! cleaned up by the TTL sweeper regardless.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    sync::{broadcast, RwLock, Semaphore},
};
use uuid::Uuid;

use crate::{config::Config, ytdlp::YtDlp};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct Job {
    pub id: Uuid,
    pub url: String,
    pub title: String,
    pub preset: String,
    pub status: Status,
    pub progress: f32,
    pub speed: Option<String>,
    pub eta: Option<String>,
    pub stage: Option<String>,
    pub filename: Option<String>,
    pub filesize: Option<u64>,
    pub error: Option<String>,
    pub created_at: u64,
    #[serde(skip)]
    pub file: Option<PathBuf>,
}

#[derive(Deserialize)]
pub struct NewJob {
    pub url: String,
    pub title: Option<String>,
    /// best | 1080 | 720 | 480 | mp3 | m4a | custom
    pub preset: String,
    pub format_id: Option<String>,
    /// kind of the custom format ("video" means it needs an audio track merged)
    pub format_kind: Option<String>,
}

#[derive(Clone)]
pub struct Jobs {
    cfg: Arc<Config>,
    ytdlp: YtDlp,
    jobs: Arc<RwLock<HashMap<Uuid, Job>>>,
    slots: Arc<Semaphore>,
    pub events: broadcast::Sender<Job>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Maps a preset to yt-dlp format arguments.
fn format_args(req: &NewJob) -> Result<Vec<String>> {
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Ok(match req.preset.as_str() {
        "best" => v(&["-f", "bv*+ba/b", "-S", "res,ext:mp4:m4a", "--merge-output-format", "mp4"]),
        h @ ("1080" | "720" | "480") => {
            let sort = format!("res:{h},ext:mp4:m4a");
            v(&["-f", "bv*+ba/b", "-S", &sort, "--merge-output-format", "mp4"])
        }
        "mp3" => v(&["-f", "ba/b", "-x", "--audio-format", "mp3", "--audio-quality", "0"]),
        "m4a" => v(&["-f", "ba[ext=m4a]/ba/b", "-x", "--audio-format", "m4a"]),
        "custom" => {
            let id = req.format_id.as_deref().unwrap_or_default();
            if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c)) {
                bail!("Invalid format id");
            }
            let f = if req.format_kind.as_deref() == Some("video") { format!("{id}+ba/{id}") } else { id.to_string() };
            vec!["-f".into(), f]
        }
        _ => bail!("Unknown preset"),
    })
}

impl Jobs {
    pub fn new(cfg: Arc<Config>, ytdlp: YtDlp) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            slots: Arc::new(Semaphore::new(cfg.max_concurrent_jobs)),
            cfg,
            ytdlp,
            jobs: Default::default(),
            events,
        }
    }

    pub async fn list(&self) -> Vec<Job> {
        let mut v: Vec<Job> = self.jobs.read().await.values().cloned().collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        v
    }

    pub async fn get(&self, id: Uuid) -> Option<Job> {
        self.jobs.read().await.get(&id).cloned()
    }

    async fn update(&self, id: Uuid, f: impl FnOnce(&mut Job)) {
        let snapshot = {
            let mut jobs = self.jobs.write().await;
            let Some(job) = jobs.get_mut(&id) else { return };
            f(job);
            job.clone()
        };
        let _ = self.events.send(snapshot);
    }

    pub async fn create(&self, req: NewJob) -> Result<Job> {
        let url = self.ytdlp.validate_url(&req.url)?;
        let fmt = format_args(&req)?;
        let job = Job {
            id: Uuid::new_v4(),
            url,
            title: req.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| req.url.clone()),
            preset: if req.preset == "custom" {
                format!("format {}", req.format_id.as_deref().unwrap_or_default())
            } else {
                req.preset.clone()
            },
            status: Status::Queued,
            progress: 0.0,
            speed: None,
            eta: None,
            stage: None,
            filename: None,
            filesize: None,
            error: None,
            created_at: now(),
            file: None,
        };
        self.jobs.write().await.insert(job.id, job.clone());
        let _ = self.events.send(job.clone());

        let this = self.clone();
        let id = job.id;
        tokio::spawn(async move {
            let _permit = this.slots.clone().acquire_owned().await;
            this.update(id, |j| j.status = Status::Running).await;
            let dir = this.cfg.download_dir.join(id.to_string());
            match this.run(id, &dir, fmt).await {
                Ok(file) => {
                    let size = tokio::fs::metadata(&file).await.ok().map(|m| m.len());
                    this.update(id, |j| {
                        j.status = Status::Done;
                        j.progress = 100.0;
                        j.speed = None;
                        j.eta = None;
                        j.stage = None;
                        j.filename = file.file_name().map(|n| n.to_string_lossy().into_owned());
                        j.filesize = size;
                        j.file = Some(file);
                    })
                    .await;
                }
                Err(e) => {
                    tracing::warn!(job = %id, "download failed: {e:#}");
                    let _ = tokio::fs::remove_dir_all(&dir).await;
                    this.update(id, |j| {
                        j.status = Status::Failed;
                        j.error = Some(e.to_string());
                    })
                    .await;
                }
            }
        });
        Ok(job)
    }

    async fn run(&self, id: Uuid, dir: &Path, fmt: Vec<String>) -> Result<PathBuf> {
        tokio::fs::create_dir_all(dir).await?;
        let url = self.get(id).await.map(|j| j.url).unwrap_or_default();

        let mut cmd = self.ytdlp.command();
        cmd.args(fmt)
            .args(["--newline", "--progress", "--restrict-filenames"])
            .args(["--progress-template", "download:PROG|%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s"])
            .args(["--print", "after_move:FILE|%(filepath)s"])
            .arg("-o")
            .arg(dir.join("%(title).120B [%(id)s].%(ext)s"))
            .args(["--", &url])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let err_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
            buf
        });

        let mut lines = BufReader::new(stdout).lines();
        let mut final_path: Option<PathBuf> = None;
        let mut part = 0u32;
        let mut last_pct = -1f32;
        while let Some(line) = lines.next_line().await? {
            if let Some(rest) = line.strip_prefix("PROG|") {
                let mut it = rest.split('|').map(str::trim);
                let pct = it.next().and_then(|p| p.trim_end_matches('%').trim().parse::<f32>().ok()).unwrap_or(0.0);
                let speed = it.next().map(str::to_string).filter(|s| !s.contains("Unknown") && s != "NA");
                let eta = it.next().map(str::to_string).filter(|s| !s.contains("Unknown") && s != "NA");
                // A new file (e.g. audio after video) restarts at 0%.
                if pct < last_pct - 50.0 || last_pct < 0.0 {
                    part += 1;
                }
                last_pct = pct;
                let stage = if part > 1 { Some(format!("part {part}")) } else { None };
                self.update(id, |j| {
                    j.progress = pct;
                    j.speed = speed;
                    j.eta = eta;
                    j.stage = stage;
                })
                .await;
            } else if let Some(p) = line.strip_prefix("FILE|") {
                final_path = Some(PathBuf::from(p.trim()));
            } else if line.starts_with("[Merger]") || line.starts_with("[ExtractAudio]") {
                self.update(id, |j| j.stage = Some("processing".into())).await;
            }
        }

        let status = child.wait().await?;
        let stderr = err_task.await.unwrap_or_default();
        if !status.success() {
            bail!("{}", crate::ytdlp::last_error(&stderr));
        }
        if let Some(p) = final_path.filter(|p| p.exists()) {
            return Ok(p);
        }
        largest_file(dir).await.ok_or_else(|| anyhow::anyhow!("Download finished but no file was found"))
    }

    pub async fn remove(&self, id: Uuid) -> bool {
        let removed = self.jobs.write().await.remove(&id).is_some();
        if removed {
            let _ = tokio::fs::remove_dir_all(self.cfg.download_dir.join(id.to_string())).await;
        }
        removed
    }

    /// Deletes finished jobs and stray directories older than the TTL.
    pub async fn sweep(&self) {
        let ttl = Duration::from_secs(self.cfg.file_ttl_hours * 3600);
        let cutoff = now().saturating_sub(ttl.as_secs());
        let expired: Vec<Uuid> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|j| matches!(j.status, Status::Done | Status::Failed) && j.created_at < cutoff)
            .map(|j| j.id)
            .collect();
        for id in expired {
            self.remove(id).await;
        }

        // Orphans from previous runs (job map is in-memory only).
        let live: Vec<String> = self.jobs.read().await.keys().map(|k| k.to_string()).collect();
        let Ok(mut rd) = tokio::fs::read_dir(&self.cfg.download_dir).await else { return };
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().into_owned();
            if live.contains(&name) {
                continue;
            }
            let old = e
                .metadata()
                .await
                .and_then(|m| m.modified())
                .map(|m| m.elapsed().unwrap_or_default() > ttl)
                .unwrap_or(false);
            if old {
                tracing::info!("removing expired {name}");
                let _ = tokio::fs::remove_dir_all(e.path()).await;
            }
        }
    }
}

async fn largest_file(dir: &Path) -> Option<PathBuf> {
    let mut rd = tokio::fs::read_dir(dir).await.ok()?;
    let mut best: Option<(u64, PathBuf)> = None;
    while let Ok(Some(e)) = rd.next_entry().await {
        if let Ok(m) = e.metadata().await {
            if m.is_file() && best.as_ref().is_none_or(|(s, _)| m.len() > *s) {
                best = Some((m.len(), e.path()));
            }
        }
    }
    best.map(|(_, p)| p)
}
