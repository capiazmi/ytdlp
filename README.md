# ytdlp-web

Small Rust (Axum) web UI around `yt-dlp`: fetch video info, download video/audio
with live progress, and get transcripts from existing YouTube captions
(manual, auto-generated, auto-translated) as PDF, TXT, SRT or VTT.
PDF and TXT can include timestamps; in the PDF they link to that moment in the video.

A second page, **/scribd**, downloads Scribd documents as PDF, DOCX or TXT
using the bundled [scribd-downloader](https://github.com/themrsami/scribd-downloader)
(Python + Selenium + headless Chromium, see `scribd/`).

Installable as a PWA. On Android it appears in the share sheet (YouTube → Share → ytdlp);
on iOS use the Shortcut described in the app's "Install & share shortcuts" panel.

No database: jobs live in memory, files live under `DOWNLOAD_DIR` and are
deleted after `FILE_TTL_HOURS`.

## Dev

Needs `yt-dlp`, `ffmpeg`, `deno` and `typst` on `PATH`, plus Noto fonts
(incl. Noto Sans CJK) for PDF export.

```sh
cp .env.example .env.local   # set APP_PASSWORD
cargo run                    # http://localhost:3000
```

For the Scribd page you also need Chrome/Chromium and a Python venv:

```sh
python3 -m venv .venv && .venv/bin/pip install -r scribd/requirements.txt
# .env.local: SCRIBD_PYTHON=.venv/bin/python
# optional:   CHROME_BIN=/usr/bin/chromium CHROMEDRIVER=/usr/bin/chromedriver
```

If it isn't set up, the app still runs; the Scribd page shows why it's unavailable.

Or in Docker:

```sh
docker compose -f docker-compose.yml -f docker-compose.dev.yml up --build
```

## Deploy (Dokploy + cloudflared)

1. Create a **Compose** service pointing at this repo (`docker-compose.yml`).
2. In the Environment tab, set the vars from `.env.example`. At minimum:
   `APP_PASSWORD`, `SESSION_SECRET` (`openssl rand -hex 32`), `COOKIE_SECURE=true`.
3. Add a domain → service `app`, container port `3000`. No host ports are
   published. Public traffic goes through your cloudflared tunnel.

Data (downloads, yt-dlp cache, optional `cookies.txt`) lives on the
`ytdlp-data` volume at `/data`.

### "Sign in to confirm you're not a bot"

YouTube often challenges server IPs. Export cookies from a logged-in browser
(Netscape `cookies.txt` format), copy it to `/data/cookies.txt` in the
volume, and set `YTDLP_COOKIES_FILE=/data/cookies.txt`. A throwaway account
is a good idea.

### Scribd

The image ships Debian's Chromium and chromedriver. Each export runs one
headless Chrome (1.5-2.5 GB RAM on big documents), so `SCRIBD_MAX_CONCURRENT`
defaults to 1. Scribd's bot check often blocks data-centre IPs; the job then
fails with a reason like "Bot check (Cloudflare)". To update the downloader,
copy a newer `scribd-downloader.py` over `scribd/scribd-downloader.py`;
`scribd/run.py` is the only glue.

### Keeping yt-dlp current

The image pins `YTDLP_VERSION` (build arg). Either bump it and redeploy, or
set `YTDLP_AUTO_UPDATE=true` to run `yt-dlp -U` on every container start.

## API

| Method | Path | |
|---|---|---|
| GET | `/api/info?url=` | metadata, formats, caption tracks |
| GET | `/api/transcript?url=&lang=&kind=manual\|auto&format=txt\|pdf\|srt\|vtt[&timestamps=true][&download=true]` | transcript |
| GET/POST | `/api/jobs` | list (all kinds) / create video job (`{url, preset: best\|1080\|720\|480\|mp3\|m4a\|custom, format_id?}`) |
| POST | `/api/scribd/jobs` | create Scribd job (`{url, formats: [pdf, docx, txt]}`) |
| DELETE | `/api/jobs/{id}` | remove job and its file |
| GET | `/api/events` | SSE stream of job updates |
| GET | `/files/{id}[/{n}]` | download a finished job's first (or n-th) file |
| GET | `/share?url=\|text=` | PWA share target / deep link (auto-fetches; Scribd links go to `/scribd`) |
| GET | `/scribd?url=` | Scribd page / deep link (auto-starts) |
| GET | `/healthz` | unauthenticated health check |
