# ---------------------------------------------------------------------------
# Stage 1: Web UI (Solid SPA → ui/build)
# ---------------------------------------------------------------------------
FROM node:24-bookworm-slim AS ui
# pnpm は mise.toml と同じバージョンへ固定する
RUN npm install --global pnpm@12.8.1
WORKDIR /app/ui
COPY ui/package.json ui/pnpm-lock.yaml ui/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY ui/ ./
RUN pnpm run build

# ---------------------------------------------------------------------------
# Stage 2: Rust build
# ---------------------------------------------------------------------------
FROM rust:1.94-slim-bookworm AS build
RUN apt-get update \
  && apt-get install --yes --no-install-recommends build-essential cmake pkg-config \
  && rm -rf /var/lib/apt/lists/*
WORKDIR /app
ENV SQLX_OFFLINE=true

COPY Cargo.toml Cargo.lock ./
COPY .sqlx/ ./.sqlx/
COPY crates/ ./crates/

COPY --from=ui /app/ui/build ./ui/build
COPY --from=ui /app/ui/public ./ui/public

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked --package fxg-cli \
  && cp target/release/fxg /usr/local/bin/fxg

# ---------------------------------------------------------------------------
# Stage 3: Executable image
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.title="FlexAgent (fxg)" \
      org.opencontainers.image.description="Self-hosted, local-first agent manager (CLI, node, hub and PWA in a single binary)" \
      org.opencontainers.image.source="https://github.com/nazo6/flexagent"

# Agents and provisioners are not bundled: install the CLIs you use (opencode,
# agy, npx/uvx, docker/incus, ...) in a derived image or mount them into the
# container. bash/tini/curl are the runtime baseline.
RUN apt-get update \
  && apt-get install --yes --no-install-recommends bash ca-certificates curl git openssh-client tini \
  && rm -rf /var/lib/apt/lists/* \
  && git config --system --add safe.directory /workspace \
  && useradd --create-home --uid 1000 --user-group fxg

COPY --from=build /usr/local/bin/fxg /usr/local/bin/fxg

ENV FXG_HOME=/var/lib/flexagent
ENV SHELL=/bin/bash
RUN install --directory --owner=fxg --group=fxg --mode=0750 /var/lib/flexagent
VOLUME ["/var/lib/flexagent"]
WORKDIR /var/lib/flexagent
USER fxg

EXPOSE 8080 7860
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/healthz || curl -fsS http://127.0.0.1:7860/healthz || exit 1

ENTRYPOINT ["/usr/bin/tini", "--", "fxg"]
CMD ["hub", "start"]
