use std::{env, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub download_dir: PathBuf,
    pub file_ttl_hours: u64,
    pub max_concurrent_jobs: usize,
    pub ytdlp_path: String,
    pub cookies_file: Option<PathBuf>,
    pub auto_update: bool,
    /// Empty = allow every host yt-dlp supports.
    pub allowed_domains: Vec<String>,
    /// None = auth disabled.
    pub app_password: Option<String>,
    pub session_secret: String,
    pub cookie_secure: bool,
}

fn var(key: &str) -> Option<String> {
    env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    var(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

impl Config {
    /// Loads `.env.local` then `.env`. dotenvy never overrides an already-set
    /// variable, so precedence is: real env > .env.local > .env.
    pub fn load() -> Self {
        dotenvy::from_filename(".env.local").ok();
        dotenvy::dotenv().ok();

        let app_password = var("APP_PASSWORD");
        let session_secret = var("SESSION_SECRET").unwrap_or_else(|| {
            if app_password.is_some() {
                tracing::warn!("SESSION_SECRET not set; sessions reset on every restart");
            }
            uuid::Uuid::new_v4().to_string()
        });

        Self {
            port: parse("APP_PORT", 3000),
            download_dir: PathBuf::from(var("DOWNLOAD_DIR").unwrap_or_else(|| "./data/downloads".into())),
            file_ttl_hours: parse("FILE_TTL_HOURS", 24),
            max_concurrent_jobs: parse("MAX_CONCURRENT_JOBS", 2).max(1),
            ytdlp_path: var("YTDLP_PATH").unwrap_or_else(|| "yt-dlp".into()),
            cookies_file: var("YTDLP_COOKIES_FILE").map(PathBuf::from),
            auto_update: parse("YTDLP_AUTO_UPDATE", false),
            allowed_domains: var("ALLOWED_DOMAINS")
                .map(|v| {
                    v.split(',')
                        .map(|d| d.trim().trim_start_matches('.').to_lowercase())
                        .filter(|d| !d.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            app_password,
            session_secret,
            cookie_secure: parse("COOKIE_SECURE", false),
        }
    }
}
