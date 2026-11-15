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
COPY --from=ui /app/ui/static ./ui/static

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

RUN apt-get update \
  && apt-get install --yes --no-install-recommends ca-certificates git openssh-client \
  && rm -rf /var/lib/apt/lists/* \
  && useradd --create-home --uid 1000 --user-group fxg

COPY --from=build /usr/local/bin/fxg /usr/local/bin/fxg

ENV FXG_HOME=/var/lib/flexagent
RUN install --directory --owner=fxg --group=fxg --mode=0750 /var/lib/flexagent
VOLUME ["/var/lib/flexagent"]
WORKDIR /var/lib/flexagent
USER fxg

EXPOSE 8080

ENTRYPOINT ["fxg"]
CMD ["hub", "start"]
