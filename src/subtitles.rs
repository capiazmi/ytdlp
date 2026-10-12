//! WebVTT -> clean transcript. YouTube auto-captions "roll": each cue repeats
//! the previous line plus a new one, with inline `<c>`/timestamp tags. We strip
//! tags and drop lines we've just emitted.

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

pub fn parse_vtt(vtt: &str) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut recent: VecDeque<String> = VecDeque::with_capacity(4);
    let normalized = vtt.replace("\r\n", "\n");

    for block in normalized.split("\n\n") {
        let mut lines = block.lines();
        let Some((start, end)) = lines.by_ref().find_map(parse_timing) else {
            continue;
        };
        // Dedupe per line (rolling captions), then join what's new in this cue.
        let mut fresh = Vec::new();
        for line in lines {
            let text = clean_line(line);
            if text.is_empty() || recent.contains(&text) {
                continue;
            }
            if recent.len() == 4 {
                recent.pop_front();
            }
            recent.push_back(text.clone());
            fresh.push(text);
        }
        if !fresh.is_empty() {
            segments.push(Segment { start, end, text: fresh.join(" ") });
        }
    }

    // Rolling cues overlap; clamp each end to the next start so SRT is sane.
    for i in 0..segments.len().saturating_sub(1) {
        let next = segments[i + 1].start;
        if segments[i].end > next && next > segments[i].start {
            segments[i].end = next;
        }
    }
    segments
}

fn parse_timing(line: &str) -> Option<(f64, f64)> {
    let (a, b) = line.split_once("-->")?;
    let start = parse_ts(a.trim())?;
    let end = parse_ts(b.split_whitespace().next()?)?;
    Some((start, end))
}

/// `HH:MM:SS.mmm` or `MM:SS.mmm`
fn parse_ts(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn clean_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_tag = false;
    for ch in line.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clock(t: f64, sep: char) -> String {
    let ms = (t * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}{sep}{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

pub fn short_clock(t: f64) -> String {
    let s = t as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub fn to_text(segs: &[Segment]) -> String {
    segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n")
}

pub fn to_timestamped(segs: &[Segment]) -> String {
    segs.iter()
        .map(|s| format!("[{}] {}", short_clock(s.start), s.text))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn to_srt(segs: &[Segment]) -> String {
    segs.iter()
        .enumerate()
        .map(|(i, s)| format!("{}\n{} --> {}\n{}\n", i + 1, clock(s.start, ','), clock(s.end, ','), s.text))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes_rolling_auto_captions() {
        let vtt = "WEBVTT\nKind: captions\nLanguage: en\n\n\
00:00:00.000 --> 00:00:02.000 align:start position:0%\n\
hello<00:00:00.500><c> world</c>\n\n\
00:00:02.000 --> 00:00:02.010 align:start position:0%\n\
hello world\n \n\n\
00:00:02.010 --> 00:00:04.000 align:start position:0%\n\
hello world\nhow<00:00:02.500><c> are</c><c> you</c>\n\n\
00:00:04.000 --> 00:00:04.010\nhow are you\n";
        let segs = parse_vtt(vtt);
        assert_eq!(to_text(&segs), "hello world\nhow are you");
        assert_eq!(segs[1].start, 2.010);
        assert_eq!(to_timestamped(&segs), "[00:00] hello world\n[00:02] how are you");
    }

    #[test]
    fn srt_output() {
        let vtt = "WEBVTT\n\n00:01.200 --> 00:03.360\nA &amp; B\n";
        assert_eq!(to_srt(&parse_vtt(vtt)), "1\n00:00:01,200 --> 00:00:03,360\nA & B\n");
    }
}
