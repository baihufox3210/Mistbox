# syntax=docker/dockerfile:1
# Multi-stage build. Targets:
#   app   – Rust server (API + static frontend), non-root, no TeX
#   latex – isolated sandbox worker: LaTeX compile (TeX Live) and PDF text
#           extraction (poppler), non-root, no data volume

############################ frontend
FROM node:24-trixie-slim AS web
WORKDIR /web
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm run build

############################ portable ONNX Runtime
# Use Microsoft's shared CPU runtime instead of ort's AVX2-only static bundle.
# Version matches fastembed's ort api-24 feature; verify release archives.
FROM debian:trixie-slim AS onnxruntime
ARG TARGETARCH
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && case "$TARGETARCH" in \
      amd64) arch=x64; sha=43725474ba5663642e17684717946693850e2005efbd724ac72da278fead25e6 ;; \
      arm64) arch=aarch64; sha=6715b3d19965a2a6981e78ed4ba24f17a8c30d2d26420dbed10aac7ceca0085e ;; \
      *) echo "Unsupported ONNX Runtime architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && curl -fL --retry 3 "https://github.com/microsoft/onnxruntime/releases/download/v1.24.2/onnxruntime-linux-${arch}-1.24.2.tgz" -o /tmp/ort.tgz \
 && echo "$sha  /tmp/ort.tgz" | sha256sum -c - \
 && mkdir /opt/onnxruntime \
 && tar xzf /tmp/ort.tgz -C /opt/onnxruntime --strip-components=1 \
 && rm /tmp/ort.tgz

############################ rust
FROM rust:1-trixie AS rust
ARG EMBED_MODEL=multilingual-e5-small-q8
COPY --from=onnxruntime /opt/onnxruntime/lib/ /opt/onnxruntime/lib/
ENV ORT_LIB_LOCATION=/opt/onnxruntime/lib \
    ORT_PREFER_DYNAMIC_LINK=1 \
    LD_LIBRARY_PATH=/opt/onnxruntime/lib
WORKDIR /src
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/migrations ./migrations
COPY server/src ./src
# With EMBED_MODEL=off, exclude ONNX Runtime entirely.
# The LaTeX worker is always built without embedding support.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin latex-worker \
 && install -m 0755 target/release/latex-worker /usr/local/bin/ \
 && if [ "$EMBED_MODEL" = off ]; then \
      cargo build --release --locked --bin mistbox; \
    else \
      cargo build --release --locked --features embed --bin mistbox; \
    fi \
 && install -m 0755 target/release/mistbox /usr/local/bin/

############################ embedding model (downloaded at build time only)
FROM rust AS model
ARG EMBED_MODEL=multilingual-e5-small-q8
RUN mkdir -p /models && if [ "$EMBED_MODEL" != off ]; then \
      MISTBOX_RAG_MODEL_DIR=/models mistbox rag download-model "$EMBED_MODEL"; fi

############################ app
FROM debian:trixie-slim AS app
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini libstdc++6 \
 && rm -rf /var/lib/apt/lists/*
# setgid + group-writable data dir: lets a one-off import container running as
# `--user <host uid>:10001` share the volume (see README).
RUN groupadd -g 10001 mistbox \
 && useradd -u 10001 -g 10001 -M -d /nonexistent -s /usr/sbin/nologin mistbox \
 && mkdir -p /data && chown 10001:10001 /data && chmod 2775 /data
ARG EMBED_MODEL=multilingual-e5-small-q8
COPY --from=onnxruntime /opt/onnxruntime/lib/ /opt/onnxruntime/lib/
COPY --from=rust /usr/local/bin/mistbox /usr/local/bin/mistbox
COPY --from=model /models /app/models
COPY --from=web /web/dist /app/web
# Default (empty) theme; compose bind-mounts ./theme over it read-only.
COPY theme/ /app/theme/
ENV MISTBOX_DATA_DIR=/data \
    LD_LIBRARY_PATH=/opt/onnxruntime/lib \
    MISTBOX_BIND=0.0.0.0:8080 \
    MISTBOX_STATIC_DIR=/app/web \
    MISTBOX_THEME_DIR=/app/theme \
    MISTBOX_RAG_MODEL=${EMBED_MODEL} \
    MISTBOX_RAG_MODEL_DIR=/app/models \
    HF_HUB_OFFLINE=1 \
    MALLOC_ARENA_MAX=2
USER 10001:10001
VOLUME ["/data"]
EXPOSE 8080
ENTRYPOINT ["tini", "--", "mistbox"]
CMD ["serve"]

############################ latex worker
FROM debian:trixie-slim AS latex
# All TeX packages and fonts are installed at build time; nothing is ever
# downloaded while compiling untrusted documents.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      tini fontconfig poppler-utils poppler-data \
      texlive-xetex texlive-latex-base texlive-latex-recommended texlive-latex-extra \
      texlive-fonts-recommended texlive-lang-chinese texlive-lang-cjk \
      texlive-science texlive-pictures texlive-plain-generic \
      lmodern fonts-noto-cjk fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/* /usr/share/doc/* /usr/share/man/* \
 && fc-cache -f
RUN useradd -u 10002 -M -d /nonexistent -s /usr/sbin/nologin latex
COPY docker/latex-warmup.tex /opt/warmup/warmup.tex
# Warm-up compile so formats / font maps are verified at build time.
RUN cd /tmp && cp /opt/warmup/warmup.tex . \
 && xelatex -no-shell-escape -interaction=nonstopmode -halt-on-error warmup.tex >/dev/null \
 && test -s warmup.pdf && rm -rf /tmp/*
COPY --from=rust /usr/local/bin/latex-worker /usr/local/bin/latex-worker
USER 10002:10002
ENV LATEX_BIND=0.0.0.0:8090 LATEX_WORK_DIR=/tmp
EXPOSE 8090
ENTRYPOINT ["tini", "--", "latex-worker"]
