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

use crate::{
    config::Config,
    scribd::{self, Scribd},
    ytdlp::YtDlp,
};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Video,
    Scribd,
}

#[derive(Clone, Debug, Serialize)]
pub struct OutFile {
    pub name: String,
    pub size: u64,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct Job {
    pub id: Uuid,
    pub kind: Kind,
    pub url: String,
    pub title: String,
    pub preset: String,
    pub status: Status,
    pub progress: f32,
    pub speed: Option<String>,
    pub eta: Option<String>,
    pub stage: Option<String>,
    pub files: Vec<OutFile>,
    pub error: Option<String>,
    pub created_at: u64,
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

#[derive(Deserialize)]
pub struct NewScribdJob {
    pub url: String,
    /// any of pdf | docx | txt
    pub formats: Vec<String>,
}

/// What a job runs.
enum Plan {
    Video(Vec<String>),
    Scribd(Vec<String>),
}

#[derive(Clone)]
pub struct Jobs {
    cfg: Arc<Config>,
    ytdlp: YtDlp,
    scribd: Scribd,
    jobs: Arc<RwLock<HashMap<Uuid, Job>>>,
    slots: Arc<Semaphore>,
    scribd_slots: Arc<Semaphore>,
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
    pub fn new(cfg: Arc<Config>, ytdlp: YtDlp, scribd: Scribd) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            slots: Arc::new(Semaphore::new(cfg.max_concurrent_jobs)),
            scribd_slots: Arc::new(Semaphore::new(cfg.scribd_max_concurrent)),
            cfg,
            ytdlp,
            scribd,
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
        let preset = if req.preset == "custom" {
            format!("format {}", req.format_id.as_deref().unwrap_or_default())
        } else {
            req.preset.clone()
        };
        let title = req.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| req.url.clone());
        Ok(self.start(Kind::Video, url, title, preset, Plan::Video(fmt)).await)
    }

    pub async fn create_scribd(&self, req: NewScribdJob) -> Result<Job> {
        let doc = Scribd::validate_url(&req.url)?;
        let mut formats: Vec<String> = Vec::new();
        for f in &req.formats {
            let f = f.trim().to_lowercase();
            if !scribd::FORMATS.contains(&f.as_str()) {
                bail!("Unknown format '{f}'");
            }
            if !formats.contains(&f) {
                formats.push(f);
            }
        }
        if formats.is_empty() {
            bail!("Pick at least one format");
        }
        let title = doc.title.unwrap_or_else(|| format!("Scribd document {}", doc.id));
        Ok(self.start(Kind::Scribd, doc.url, title, formats.join(" + "), Plan::Scribd(formats)).await)
    }

    async fn start(&self, kind: Kind, url: String, title: String, preset: String, plan: Plan) -> Job {
        let job = Job {
            id: Uuid::new_v4(),
            kind,
            url,
            title,
            preset,
            status: Status::Queued,
            progress: 0.0,
            speed: None,
            eta: None,
            stage: None,
            files: Vec::new(),
            error: None,
            created_at: now(),
        };
        self.jobs.write().await.insert(job.id, job.clone());
        let _ = self.events.send(job.clone());

        let this = self.clone();
        let id = job.id;
        tokio::spawn(async move {
            let slots = match plan {
                Plan::Video(_) => this.slots.clone(),
                Plan::Scribd(_) => this.scribd_slots.clone(),
            };
            let _permit = slots.acquire_owned().await;
            this.update(id, |j| j.status = Status::Running).await;
            let dir = this.cfg.download_dir.join(id.to_string());
            let res = match plan {
                Plan::Video(fmt) => this.run(id, &dir, fmt).await.map(|p| vec![p]),
                Plan::Scribd(formats) => this.run_scribd(id, &dir, formats).await,
            };
            match res {
                Ok(paths) => {
                    let mut files = Vec::new();
                    for path in paths {
                        let size = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        files.push(OutFile { name, size, path });
                    }
                    this.update(id, |j| {
                        j.status = Status::Done;
                        j.progress = 100.0;
                        j.speed = None;
                        j.eta = None;
                        j.stage = None;
                        j.files = files;
                    })
                    .await;
                }
                Err(e) => {
                    tracing::warn!(job = %id, "job failed: {e:#}");
                    let _ = tokio::fs::remove_dir_all(&dir).await;
                    this.update(id, |j| {
                        j.status = Status::Failed;
                        j.stage = None;
                        j.error = Some(e.to_string());
                    })
                    .await;
                }
            }
        });
        job
    }

    async fn run_scribd(&self, id: Uuid, dir: &Path, formats: Vec<String>) -> Result<Vec<PathBuf>> {
        tokio::fs::create_dir_all(dir).await?;
        let url = self.get(id).await.map(|j| j.url).unwrap_or_default();
        let mut child = self.scribd.command(&url, &formats, dir).spawn()?;
        let pgid = child.id();
        let stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let err_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
            buf
        });

        let printing = formats.iter().any(|f| f == "pdf");
        let work = async {
            let mut lines = BufReader::new(stdout).lines();
            let mut failure: Option<String> = None;
            while let Some(line) = lines.next_line().await? {
                // 0-90% for loading/printing pages; merging and DOCX building take the rest.
                let (pct, stage) = match scribd::parse_line(&line) {
                    scribd::Line::Pages(n) => (Some(2.0), Some(format!("{n} pages"))),
                    scribd::Line::Printed(a, b) if b > 0 => (Some(90.0 * a as f32 / b as f32), Some(format!("page {a}/{b}"))),
                    scribd::Line::Loaded(a, b) if b > 0 && !printing => {
                        (Some(90.0 * a as f32 / b as f32), Some(format!("reading page {a}/{b}")))
                    }
                    scribd::Line::Stage(s) => (None, Some(s.to_string())),
                    scribd::Line::Failed(e) => {
                        failure.get_or_insert(e);
                        (None, None)
                    }
                    scribd::Line::Reason(r) => {
                        if let Some(f) = failure.as_mut() {
                            f.push_str(&format!(" {}.", r.trim_end_matches('.')));
                        }
                        (None, None)
                    }
                    _ => (None, None),
                };
                if pct.is_some() || stage.is_some() {
                    self.update(id, |j| {
                        if let Some(p) = pct {
                            j.progress = p;
                        }
                        if stage.is_some() {
                            j.stage = stage;
                        }
                    })
                    .await;
                }
            }
            anyhow::Ok((child.wait().await?, failure))
        };
        let limit = Duration::from_secs(self.cfg.scribd_timeout_minutes * 60);
        let res = tokio::time::timeout(limit, work).await;
        // Chrome is a grandchild; take down the whole group, finished or not.
        #[cfg(unix)]
        if let Some(pid) = pgid {
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
        let (status, failure) = match res {
            Err(_) => bail!("Timed out after {} minutes", self.cfg.scribd_timeout_minutes),
            Ok(r) => r?,
        };
        let stderr = err_task.await.unwrap_or_default();
        if !status.success() {
            let msg = failure.unwrap_or_else(|| {
                let s = String::from_utf8_lossy(&stderr);
                s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("Scribd export failed").trim().to_string()
            });
            bail!("{msg}");
        }
        let files = scribd::outputs(dir, &formats).await;
        if files.is_empty() {
            bail!("Export finished but no file was found");
        }
        Ok(files)
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
