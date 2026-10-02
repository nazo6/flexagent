# syntax=docker/dockerfile:1
#
# FlexAgent (`fxg`) コンテナイメージ
#
# - **3 ステージ構成**: Web UI (SvelteKit SPA) → Rust リリースビルド → 実行イメージ。
#   UI は `rust-embed` で `fxg` バイナリに同梱されるため、実行イメージに Node.js は
#   不要 (`crates/fxg-ui-assets`)。
# - **SQLx オフライン**: コミット済みの `.sqlx/` (オフラインクエリデータ) を使うため
#   ビルド時に DB へ接続しない (`SQLX_OFFLINE=true`)。

# ---------------------------------------------------------------------------
# Stage 1: Web UI (SvelteKit SPA → ui/build)
# ---------------------------------------------------------------------------
FROM node:24-bookworm-slim AS ui
# pnpm は mise.toml と同じバージョンへ固定する
RUN npm install --global pnpm@12.8.1
WORKDIR /app/ui
COPY ui/package.json ui/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY ui/ ./
RUN pnpm run build

# ---------------------------------------------------------------------------
# Stage 2: Rust リリースビルド (`fxg` バイナリ)
# ---------------------------------------------------------------------------
FROM rust:1.94-slim-bookworm AS build
RUN apt-get update \
  && apt-get install --yes --no-install-recommends build-essential cmake pkg-config \
  && rm -rf /var/lib/apt/lists/*
WORKDIR /app
ENV SQLX_OFFLINE=true

# 依存クレートの registry / ビルド成果物を BuildKit キャッシュマウントに置き、
# 2 回目以降のビルドで再コンパイルを避ける
COPY Cargo.toml Cargo.lock ./
COPY .cargo/ ./.cargo/
COPY .sqlx/ ./.sqlx/
COPY crates/ ./crates/

# バイナリに同梱するファイル (ui/build は rust-embed、ui/static/icon-192.png は
# `crates/fxg-cli/src/tray/mod.rs` の include_bytes! が参照する)
COPY --from=ui /app/ui/build ./ui/build
COPY --from=ui /app/ui/static ./ui/static

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked --package fxg-cli \
  && cp target/release/fxg /usr/local/bin/fxg

# ---------------------------------------------------------------------------
# Stage 3: 実行イメージ
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.title="FlexAgent (fxg)" \
      org.opencontainers.image.description="Self-hosted, local-first agent manager (CLI/TUI, node daemon, central server and PWA in a single binary)" \
      org.opencontainers.image.source="https://github.com/nazo6/flexagent"

# - ca-certificates: HTTPS (Node Hub / Web Push / PWA 配信) 用
# - git / openssh-client: `fxg daemon` の Worktree・Shadow Git Tree・git+ssh 用
# (SQLite は bundled で静的リンク、TLS は rustls のため libssl は不要)
RUN apt-get update \
  && apt-get install --yes --no-install-recommends ca-certificates git openssh-client \
  && rm -rf /var/lib/apt/lists/* \
  && useradd --create-home --uid 1000 --user-group fxg

COPY --from=build /usr/local/bin/fxg /usr/local/bin/fxg

# データディレクトリ (`~/.flexagent` 相当) を FXG_HOME=/data に固定し、
# named volume で永続化する (server.db / node.db / config.toml / worktrees 等)
ENV FXG_HOME=/data
RUN install --directory --owner=fxg --group=fxg --mode=0750 /data
VOLUME ["/data"]
WORKDIR /data
USER fxg

# `fxg server` の既定待受 (0.0.0.0:8080)。コンテナ内部から `fxg daemon` を
# 使う場合の 127.0.0.1:7860 はコンテナ外へ公開されない。
EXPOSE 8080

ENTRYPOINT ["fxg"]
CMD ["server"]
