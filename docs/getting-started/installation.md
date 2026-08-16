---
sidebar_position: 1
---

# Installation

BHTune has not made its first tagged release yet — see the
[Releases](https://github.com/bytehound-labs/bhtune/releases) page for prebuilt binaries once
one exists. Until then, run the published Docker image or build from source.

## Run via Docker

The fastest way to try BHTune: a multi-stage image (frontend build → `cargo build --release` →
slim Debian runtime) is published to
[GHCR](https://github.com/bytehound-labs/bhtune/pkgs/container/bhtune) on every push to `main`
(tagged `edge`), and additionally under the version and `latest` once a release tag exists. No
Rust toolchain, pnpm, or C compiler needed on the host — just Docker:

```sh
docker run -d --name bhtune \
  -p 8787:8787 \
  -v bhtune-data:/var/lib/bhtune \
  ghcr.io/bytehound-labs/bhtune:edge
```

Open `http://localhost:8787` for the web GUI. The image bundles both binaries, so the headless
CLI is available the same way, sharing the running server's database through the mounted
volume:

```sh
docker exec bhtune bhtune history list
```

The image sets `BHTUNE_BIND=0.0.0.0:8787` and `BHTUNE_DB=/var/lib/bhtune/bhtune.db` as its own
defaults — see [`Dockerfile`](https://github.com/bytehound-labs/bhtune/blob/main/Dockerfile)
for the full build and [Configuration precedence](../reference/config.md) for how to override
either with `docker run -e`. This is a secondary distribution channel aimed at IT-managed Linux
hosts; a Windows installer is the primary path for this project's actual users, since OT sites
frequently prohibit or simply lack container runtimes.

Skip to [Prerequisites](#prerequisites) below to build from source instead.

## Prerequisites

- A Rust toolchain supporting the 2024 edition (Rust 1.94 or newer — this is BHTune's declared
  MSRV, verified in CI).
- [`pnpm`](https://pnpm.io/) if you want to build or develop the web GUI's frontend. The CLI
  and server both build and run without it — the frontend is only needed to serve the browser
  UI from `bhtune-server`.
- Nothing else. No Windows, no Docker, no proprietary SDKs — every dependency is open-source
  (machine-enforced in CI via `cargo deny`).

## Build the CLI and server

```sh
git clone https://github.com/bytehound-labs/bhtune.git
cd bhtune
cargo build --workspace --release
```

This produces `target/release/bhtune` (the headless CLI) and `target/release/bhtune-server`
(the HTTP API + web GUI). Both link the same tuning engine and read/write the same SQLite
database — see [Introduction](../intro.md#design-principles).

## Build the web frontend (optional)

Skip this if you only want the CLI, or if you're developing the frontend itself with Vite's
dev server (see [Web GUI quickstart](web-gui-quickstart.md)).

```sh
pnpm install                              # from the repo root -- this is a pnpm workspace
pnpm --filter bhtune-frontend run build
```

`bhtune-server` embeds the built `frontend/dist/` directory directly into its own binary via
`rust-embed`, so once this step has been run once, `bhtune-server` is a single self-contained
executable — no separate static file server, Node runtime, or reverse proxy required on the
target host.

## Where BHTune stores its data

Both the CLI and the server resolve the same default, platform-standard data directory (unless
overridden — see [Configuration precedence](../reference/config.md)):

| Platform     | Default data directory                                             |
| ------------ | ------------------------------------------------------------------ |
| Linux, macOS | `$XDG_DATA_HOME/bhtune/`, falling back to `~/.local/share/bhtune/` |
| Windows      | `%APPDATA%\bhtune\`                                                |

(BHTune resolves this the same way on macOS as Linux — a plain XDG-style fallback, not
`~/Library/Application Support/` — see `default_db_path_from` in `bhtune-cli`'s `config.rs` if
you need the exact precedence.)

This holds `bhtune.db` (the SQLite database — every template, loop, tune run, sample, result,
and write-back audit row) and `logs/` (structured `tracing` output). Nothing here is
encrypted or hidden — it's a plain SQLite file you can open with any SQLite tool.

Every run is kept forever unless you opt in to a retention policy (`retention_days` in
`bhtune.toml`, or `bhtune history prune` on demand) — see
[CLI quickstart](cli-quickstart.md#look-at-what-it-calculated).

## Next steps

- [CLI quickstart](cli-quickstart.md) — run your first tune from the command
  line, no plant connection required.
- [Web GUI quickstart](web-gui-quickstart.md) — run the server and drive a
  tune from a browser.
