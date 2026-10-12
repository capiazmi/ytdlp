# syntax=docker/dockerfile:1

# ---------- build ----------
FROM rust:1-slim-bookworm AS build
WORKDIR /app
# Cache dependencies separately from source.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release && rm -rf src
COPY src ./src
COPY static ./static
RUN touch src/main.rs && cargo build --release

# ---------- runtime ----------
FROM debian:bookworm-slim
ARG TARGETARCH
# Pin for reproducible builds; set YTDLP_AUTO_UPDATE=true to update on start.
ARG YTDLP_VERSION=2026.08.19
ARG DENO_VERSION=2.9.7

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl ffmpeg unzip \
 && rm -rf /var/lib/apt/lists/*

# yt-dlp standalone binary (no Python needed)
RUN case "$TARGETARCH" in arm64) f=yt-dlp_linux_aarch64 ;; *) f=yt-dlp_linux ;; esac \
 && curl -fsSL -o /usr/local/bin/yt-dlp "https://github.com/yt-dlp/yt-dlp/releases/download/${YTDLP_VERSION}/${f}" \
 && chmod 755 /usr/local/bin/yt-dlp

# Deno: JS runtime yt-dlp needs to solve YouTube's player challenges
RUN case "$TARGETARCH" in arm64) a=aarch64 ;; *) a=x86_64 ;; esac \
 && curl -fsSL -o /tmp/deno.zip "https://github.com/denoland/deno/releases/download/v${DENO_VERSION}/deno-${a}-unknown-linux-gnu.zip" \
 && unzip /tmp/deno.zip -d /usr/local/bin && rm /tmp/deno.zip && chmod 755 /usr/local/bin/deno

RUN useradd --uid 1000 --create-home app \
 && mkdir -p /data/downloads \
 && chown -R app:app /data /usr/local/bin/yt-dlp

COPY --from=build /app/target/release/ytdlp-web /usr/local/bin/ytdlp-web

USER app
WORKDIR /app
ENV APP_PORT=3000 \
    DOWNLOAD_DIR=/data/downloads \
    XDG_CACHE_HOME=/data/.cache \
    RUST_LOG=info
VOLUME /data
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s \
  CMD curl -fs http://127.0.0.1:${APP_PORT}/healthz || exit 1
CMD ["ytdlp-web"]
