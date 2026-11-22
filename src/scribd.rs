//! Runs the bundled Scribd downloader (`scribd/run.py`: Python + Selenium +
//! headless Chromium). Like yt-dlp it is a child process with arguments
//! passed as a list and the URL after `--`.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use anyhow::{anyhow, bail, Result};
use tokio::process::Command;

use crate::config::Config;

pub const FORMATS: [&str; 3] = ["pdf", "docx", "txt"];

#[derive(Clone)]
pub struct Scribd {
    cfg: Arc<Config>,
}

pub struct DocUrl {
    pub url: String,
    pub id: String,
    /// Title guessed from the URL slug ("Document-Title" -> "Document Title").
    pub title: Option<String>,
}

impl Scribd {
    pub fn new(cfg: Arc<Config>) -> Self {
        Self { cfg }
    }

    /// None when ready, otherwise why the Scribd page can't work.
    pub async fn check(&self) -> Option<String> {
        if !Path::new(&self.cfg.scribd_script).exists() {
            return Some(format!("script not found at '{}'", self.cfg.scribd_script));
        }
        let out = Command::new(&self.cfg.scribd_python)
            .args(["-c", "import selenium, pypdf, docx, PIL"])
            .stdin(Stdio::null())
            .output()
            .await;
        match out {
            Err(e) => Some(format!("'{}' not runnable: {e}", self.cfg.scribd_python)),
            Ok(o) if !o.status.success() => Some(format!(
                "Python packages missing (pip install -r scribd/requirements.txt): {}",
                crate::ytdlp::last_error(&o.stderr)
            )),
            Ok(_) => None,
        }
    }

    /// Accepts scribd.com and its subdomains, `/document/<id>`, `/doc/<id>`
    /// or `/embeds/<id>`. A missing scheme is fine (people paste `scribd.com/...`).
    pub fn validate_url(raw: &str) -> Result<DocUrl> {
        let raw = raw.trim();
        if raw.len() > 2048 {
            bail!("URL too long");
        }
        let with_scheme = if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
        let url = url::Url::parse(&with_scheme).map_err(|_| anyhow!("Invalid URL"))?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("Only http(s) URLs are allowed");
        }
        let host = url.host_str().unwrap_or_default().to_lowercase();
        if host != "scribd.com" && !host.ends_with(".scribd.com") {
            bail!("Not a Scribd link");
        }
        let mut seg = url.path_segments().into_iter().flatten().filter(|s| !s.is_empty());
        let (kind, id) = (seg.next(), seg.next());
        let id = match (kind, id) {
            (Some("document" | "doc" | "embeds"), Some(id)) if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) => id,
            _ => bail!("Not a Scribd document link (expected scribd.com/document/<id>/…)"),
        };
        let title = seg
            .next()
            .filter(|s| *s != "content")
            .map(|s| {
                let s = url::form_urlencoded::parse(s.as_bytes()).map(|(k, _)| k.into_owned()).collect::<String>();
                s.replace(['-', '_'], " ")
            })
            .filter(|s| !s.trim().is_empty());
        Ok(DocUrl { url: url.to_string(), id: id.to_string(), title })
    }

    pub fn command(&self, url: &str, formats: &[String], out_dir: &Path) -> Command {
        let mut cmd = Command::new(&self.cfg.scribd_python);
        cmd.arg("-u")
            .arg(&self.cfg.scribd_script)
            .arg("--format")
            .arg(formats.join(","))
            .arg("-o")
            .arg(out_dir)
            .args(["--", url])
            // Leave TMPDIR alone: Chrome's profile holds a Unix socket, and a
            // long temp path pushes it past the ~108-byte socket path limit.
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Own process group, so a timeout can also kill Chrome.
        #[cfg(unix)]
        cmd.process_group(0);
        cmd
    }
}

/// Progress parsed from one line of the script's output.
pub enum Line {
    Pages(u32),
    /// Page `n` of the PDF has been printed.
    Printed(u32, u32),
    /// Pages up to `n` have been loaded (TXT/DOCX-only runs have no print step).
    Loaded(u32, u32),
    Stage(&'static str),
    Failed(String),
    /// Follows "No printable document pages were detected." (bot check, not found, …)
    Reason(String),
    Other,
}

fn frac(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('/')?;
    let a = a.rsplit('-').next()?.trim().parse().ok()?;
    let b = b.trim_end_matches(|c: char| !c.is_ascii_digit()).trim().parse().ok()?;
    Some((a, b))
}

pub fn parse_line(line: &str) -> Line {
    let t = line.trim();
    if let Some(r) = t.strip_prefix("Viewer ready: ") {
        return r.split_whitespace().next().and_then(|n| n.parse().ok()).map_or(Line::Other, Line::Pages);
    }
    if let Some(r) = t.strip_prefix("Page ") {
        if let Some((a, b)) = r.split_whitespace().next().and_then(frac) {
            return Line::Printed(a, b);
        }
    }
    if let Some(r) = t.strip_prefix("Loading page batch ") {
        if let Some((a, b)) = frac(r.trim_end_matches('.')) {
            return Line::Loaded(a, b);
        }
    }
    if t.starts_with("Starting Chrome") {
        return Line::Stage("starting browser");
    }
    if t.starts_with("Merging ") {
        return Line::Stage("merging PDF");
    }
    if t.starts_with("Building DOCX") {
        return Line::Stage("building DOCX");
    }
    if let Some(e) = t.strip_prefix("Export failed: ").or_else(|| t.strip_prefix("Error: ")) {
        return Line::Failed(e.to_string());
    }
    if let Some(r) = t.strip_prefix("Reason: ") {
        return Line::Reason(r.to_string());
    }
    Line::Other
}

/// The exported files, in the order the formats were asked for.
pub async fn outputs(dir: &Path, formats: &[String]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else { return Vec::new() };
    while let Ok(Some(e)) = rd.next_entry().await {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or_default().to_lowercase();
        if p.is_file() && !name.starts_with("scribd-debug-") && formats.contains(&ext) {
            found.push((formats.iter().position(|f| *f == ext).unwrap_or(99), p));
        }
    }
    found.sort();
    found.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let d = Scribd::validate_url("scribd.com/document/123/My-Doc-Title").unwrap();
        assert_eq!(d.id, "123");
        assert_eq!(d.title.as_deref(), Some("My Doc Title"));
        assert!(Scribd::validate_url("https://de.scribd.com/doc/42?secret_password=x").is_ok());
        assert!(Scribd::validate_url("https://www.scribd.com/embeds/42/content").unwrap().title.is_none());
        assert!(Scribd::validate_url("https://notscribd.com/document/1").is_err());
        assert!(Scribd::validate_url("https://scribd.com.evil.com/document/1").is_err());
        assert!(Scribd::validate_url("https://www.scribd.com/user/1/x").is_err());
        assert!(Scribd::validate_url("ftp://scribd.com/document/1").is_err());
    }

    #[test]
    fn lines() {
        assert!(matches!(parse_line("Viewer ready: 316 page(s) detected."), Line::Pages(316)));
        assert!(matches!(parse_line("  Page 5/316 1002x1296px -> 10.438\"x13.500\""), Line::Printed(5, 316)));
        assert!(matches!(parse_line("  Loading page batch 9-16/316..."), Line::Loaded(16, 316)));
        assert!(matches!(parse_line("Export failed: boom"), Line::Failed(e) if e == "boom"));
        assert!(matches!(parse_line("  Reason: The document was not found"), Line::Reason(r) if r == "The document was not found"));
    }
}
