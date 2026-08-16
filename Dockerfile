# syntax=docker/dockerfile:1

# Multi-stage build for the `pkg-docker` distribution channel: pnpm builds the React SPA,
# cargo builds `bhtune`/`bhtune-server` (which embeds the SPA at compile time via
# rust-embed -- see crates/bhtune-server/build.rs and src/spa.rs), and the runtime stage
# ships only the two compiled binaries, nothing else from either toolchain.
#
# This is a *secondary* distribution channel for IT-managed Linux hosts. The Windows MSI
# (`pkg-windows-installer`) is the primary path, since OT sites frequently prohibit or
# simply lack container runtimes -- see AGENTS.md's "Packaging and distribution" section.

# ---- Frontend --------------------------------------------------------------------------
FROM node:22-slim AS frontend
WORKDIR /src
RUN corepack enable

# Copy only the workspace/package manifests first so `pnpm install` is cached across
# rebuilds that touch source but not dependencies. `website/` is a real pnpm workspace
# member (see pnpm-workspace.yaml) so its manifest must be present for `pnpm install` to
# resolve the lockfile, even though only `frontend/` is actually built below.
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY frontend/package.json frontend/package.json
COPY website/package.json website/package.json
RUN pnpm install --frozen-lockfile

# frontend/src/api/schema.d.ts is committed and drift-checked in CI (see openapi-contract),
# so this build needs no openapi.json input -- unlike `pnpm run generate:api`, `run build`
# never regenerates it.
COPY frontend/ frontend/
RUN pnpm --filter bhtune-frontend run build

# ---- Rust builder -----------------------------------------------------------------------
FROM rust:1-slim-bookworm AS builder

# protobuf-compiler: opcda-bridge-proto compiles bridge.proto via tonic-build at build time
# (matches checks.yml/release.yml's `taiki-e/install-action` protoc step -- a real, non-dev
# build requirement, not a test-only one). build-essential: bhtune-db's bundled SQLite
# (libsqlite3-sys) compiles a C amalgamation via the `cc` crate at build time, which this
# `slim` base image -- unlike the default `rust` image -- doesn't include by default.
RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# Must exist and be populated *before* the release build below: rust-embed only embeds
# frontend/dist/ into the binary for --release builds (a plain `cargo build`/`cargo run`
# reads it live off disk instead -- see spa.rs's doc comment). There is no after-the-fact
# embed step, so the copy order here is load-bearing, not just convenient.
COPY --from=frontend /src/frontend/dist/ frontend/dist/

RUN cargo build --release --locked -p bhtune-cli -p bhtune-server

# ---- Runtime ----------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /var/lib/bhtune --shell /usr/sbin/nologin bhtune

COPY --from=builder /src/target/release/bhtune /usr/local/bin/bhtune
COPY --from=builder /src/target/release/bhtune-server /usr/local/bin/bhtune-server

# BHTUNE_DB: both binaries' shared `CLI flag > env var > TOML config > platform default`
# config precedence (see AGENTS.md's "Config precedence") -- points them at the persistent
# volume below rather than the platform-default XDG path, which wouldn't survive a
# container recreate.
ENV BHTUNE_DB=/var/lib/bhtune/bhtune.db

# BHTUNE_BIND: the native binary's 127.0.0.1-only default is safe on a shared host running
# other local processes, but a container's loopback interface is invisible to `docker run
# -p`/`--publish` port mapping -- Docker's own network isolation (nothing is reachable from
# outside the container unless a port is explicitly published) is what makes 0.0.0.0 the
# correct default *inside* the container without regressing the "loud, explicit opt-in"
# posture described in AGENTS.md's "Web app architecture" section: running this image and
# choosing to publish a port already *is* that explicit opt-in.
ENV BHTUNE_BIND=0.0.0.0:8787

VOLUME ["/var/lib/bhtune"]
WORKDIR /var/lib/bhtune
USER bhtune
EXPOSE 8787
ENTRYPOINT ["/usr/local/bin/bhtune-server"]
