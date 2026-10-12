//! Transcript -> PDF via the Typst CLI. Typst handles shaping, RTL and CJK;
//! fonts come from the system (Noto packages in the Docker image).

use std::{path::Path, time::Duration};

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use tokio::process::Command;

use crate::subtitles::{short_clock, Segment};

const TEMPLATE: &str = include_str!("../static/transcript.typ");

#[derive(Serialize)]
struct Data<'a> {
    title: &'a str,
    uploader: Option<&'a str>,
    duration: Option<String>,
    url: &'a str,
    source: &'a str,
    code: &'a str,
    lang_name: &'a str,
    kind: &'a str,
    lang: String,
    region: Option<String>,
    cjk: Vec<&'static str>,
    timestamps: bool,
    segments: Vec<Seg>,
    paragraphs: Vec<String>,
}

#[derive(Serialize)]
struct Seg {
    t: String,
    link: Option<String>,
    text: String,
}

pub struct Meta<'a> {
    pub title: &'a str,
    pub uploader: Option<&'a str>,
    pub duration: Option<f64>,
    pub url: &'a str,
    /// YouTube video id, enables clickable timestamps
    pub youtube_id: Option<&'a str>,
    pub code: &'a str,
    pub lang_name: &'a str,
    pub auto: bool,
}

/// YouTube codes -> something Typst accepts (ISO 639 letters + optional region).
fn typst_lang(code: &str) -> (String, Option<String>) {
    let mut parts = code.split(['-', '_']);
    let base = parts.next().unwrap_or("").to_ascii_lowercase();
    let base = match base.as_str() {
        "iw" => "he".to_string(),
        "jw" => "jv".to_string(),
        "in" => "id".to_string(),
        b if (2..=3).contains(&b.len()) && b.chars().all(|c| c.is_ascii_alphabetic()) => b.to_string(),
        _ => "en".to_string(),
    };
    let region = parts
        .next()
        .filter(|r| r.len() == 2 && r.chars().all(|c| c.is_ascii_alphabetic()))
        .map(|r| r.to_ascii_uppercase());
    (base, region)
}

/// Han characters differ per locale; put the matching CJK face first.
fn cjk_order(code: &str) -> Vec<&'static str> {
    let c = code.to_ascii_lowercase();
    let first = if c.starts_with("ja") {
        "Noto Sans CJK JP"
    } else if c.starts_with("ko") {
        "Noto Sans CJK KR"
    } else if c.contains("hk") {
        "Noto Sans CJK HK"
    } else if c.contains("tw") || c.contains("hant") {
        "Noto Sans CJK TC"
    } else {
        "Noto Sans CJK SC"
    };
    let mut v = vec![first];
    for f in ["Noto Sans CJK SC", "Noto Sans CJK JP", "Noto Sans CJK KR", "Noto Sans CJK TC", "Noto Sans CJK HK"] {
        if f != first {
            v.push(f);
        }
    }
    v
}

/// Groups caption lines into readable paragraphs: break on pauses or length.
fn paragraphs(segs: &[Segment]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut len = 0;
    for (i, s) in segs.iter().enumerate() {
        cur.push(&s.text);
        len += s.text.len();
        let pause = segs.get(i + 1).is_some_and(|n| n.start - s.end > 2.0);
        if pause || len > 600 {
            out.push(cur.join(" "));
            cur.clear();
            len = 0;
        }
    }
    if !cur.is_empty() {
        out.push(cur.join(" "));
    }
    out
}

pub async fn render(typst: &str, dir: &Path, meta: Meta<'_>, segs: &[Segment], timestamps: bool) -> Result<Vec<u8>> {
    let (lang, region) = typst_lang(meta.code);
    let data = Data {
        title: meta.title,
        uploader: meta.uploader,
        duration: meta.duration.map(short_clock),
        url: meta.url,
        source: "ytdlp-web",
        code: meta.code,
        lang_name: meta.lang_name,
        kind: if meta.auto { "Auto-generated" } else { "Manual" },
        lang,
        region,
        cjk: cjk_order(meta.code),
        timestamps,
        segments: if timestamps {
            segs.iter()
                .map(|s| Seg {
                    t: short_clock(s.start),
                    link: meta
                        .youtube_id
                        .map(|id| format!("https://www.youtube.com/watch?v={id}&t={}s", s.start as u64)),
                    text: s.text.clone(),
                })
                .collect()
        } else {
            Vec::new()
        },
        paragraphs: if timestamps { Vec::new() } else { paragraphs(segs) },
    };

    tokio::fs::create_dir_all(dir).await?;
    tokio::fs::write(dir.join("main.typ"), TEMPLATE).await?;
    tokio::fs::write(dir.join("data.json"), serde_json::to_vec(&data)?).await?;

    let out = tokio::time::timeout(
        Duration::from_secs(90),
        Command::new(typst)
            .arg("compile")
            .arg("--root")
            .arg(dir)
            .arg(dir.join("main.typ"))
            .arg(dir.join("out.pdf"))
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| anyhow!("PDF rendering timed out"))?
    .map_err(|e| anyhow!("failed to run typst: {e}"))?;
    if !out.status.success() {
        tracing::warn!("typst failed: {}", String::from_utf8_lossy(&out.stderr));
        bail!("PDF rendering failed");
    }
    Ok(tokio::fs::read(dir.join("out.pdf")).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_languages() {
        assert_eq!(typst_lang("pt-BR"), ("pt".into(), Some("BR".into())));
        assert_eq!(typst_lang("iw"), ("he".into(), None));
        assert_eq!(typst_lang("zh-Hans"), ("zh".into(), None));
        assert_eq!(typst_lang("en-orig"), ("en".into(), None));
        assert_eq!(typst_lang("???"), ("en".into(), None));
        assert_eq!(cjk_order("ja")[0], "Noto Sans CJK JP");
        assert_eq!(cjk_order("zh-TW")[0], "Noto Sans CJK TC");
    }
}
