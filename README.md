# ytdlp-web

Small Rust (Axum) web UI around `yt-dlp`: fetch video info, download video/audio
with live progress, and get transcripts from existing YouTube captions
(manual, auto-generated, auto-translated) as TXT, timestamped TXT, SRT or VTT.

No database: jobs live in memory, files live under `DOWNLOAD_DIR` and are
deleted after `FILE_TTL_HOURS`.

## Dev

Needs `yt-dlp`, `ffmpeg` and `deno` on `PATH`.

```sh
cp .env.example .env.local   # set APP_PASSWORD
cargo run                    # http://localhost:3000
```

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

### Keeping yt-dlp current

The image pins `YTDLP_VERSION` (build arg). Either bump it and redeploy, or
set `YTDLP_AUTO_UPDATE=true` to run `yt-dlp -U` on every container start.

## API

| Method | Path | |
|---|---|---|
| GET | `/api/info?url=` | metadata, formats, caption tracks |
| GET | `/api/transcript?url=&lang=&kind=manual\|auto&format=txt\|ts\|srt\|vtt[&download=true]` | transcript |
| GET/POST | `/api/jobs` | list / create (`{url, preset: best\|1080\|720\|480\|mp3\|m4a\|custom, format_id?}`) |
| DELETE | `/api/jobs/{id}` | remove job and its file |
| GET | `/api/events` | SSE stream of job updates |
| GET | `/files/{id}` | download finished file |
| GET | `/healthz` | unauthenticated health check |
