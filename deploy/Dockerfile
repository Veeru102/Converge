# syntax=docker/dockerfile:1.7
# Multi-stage build: web app (pnpm) + server (cargo) → slim runtime image.

FROM node:22-bookworm-slim AS web
RUN corepack enable
WORKDIR /src
COPY web/package.json web/pnpm-workspace.yaml web/pnpm-lock.yaml web/tsconfig.base.json web/
COPY web/packages web/packages
COPY web/apps web/apps
COPY proto proto
COPY fixtures fixtures
WORKDIR /src/web
RUN pnpm install --frozen-lockfile
RUN pnpm --filter @converge/canvas build

FROM rust:1.88-bookworm AS server
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY proto proto
COPY fixtures fixtures
RUN cargo build --release -p converge-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=server /src/target/release/converge-server /app/converge-server
COPY --from=web /src/web/apps/canvas/dist /app/web
ENV BIND=0.0.0.0:8080 STATIC_DIR=/app/web RUST_LOG=info
EXPOSE 8080
ENTRYPOINT ["/app/converge-server"]
