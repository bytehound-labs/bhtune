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
- The Protocol Buffers compiler, `protoc`, on `PATH` — needed transitively by
  [`opcda-bridge`](https://github.com/bytehound-labs/opcda-bridge)'s gRPC codegen build
  script. On Windows, `choco install protoc` is the most reliable option (`winget` can fail
  on hosts where its `msstore` source prompts for a one-time terms-of-service acceptance).
  On Linux, install your distro's `protobuf-compiler` package; on macOS,
  `brew install protobuf`.
- [`pnpm`](https://pnpm.io/) 11.22.0 if you want to build or develop the web GUI's frontend.
  The repository's `package.json` declares this version for Corepack. The CLI and server both
  build and run without it — the frontend is only needed to serve the browser UI from
  `bhtune-server`.

No Windows, no Docker, and no proprietary SDKs are required beyond the above — every
dependency, `protoc` included, is open-source (machine-enforced in CI via `cargo deny`).

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

Every run is kept forever unless you opt in to a retention policy (`retention_days` must be a
positive whole number in `bhtune.toml`, or use `bhtune history prune` on demand) — see
[CLI quickstart](cli-quickstart.md#look-at-what-it-calculated).

## Run as a background service

Running `bhtune-server` from an interactive terminal is fine for trying it out, but a shared,
always-on deployment should register it with the host OS's own service manager instead, so it
starts at boot and restarts automatically without anyone needing to keep a terminal open.

### Windows

`bhtune-server.exe` registers itself directly with the Service Control Manager (SCM) — no
separate installer or third-party service wrapper needed:

```powershell
bhtune-server.exe install    # registers the service (does not start it)
bhtune-server.exe start
bhtune-server.exe status
bhtune-server.exe stop
bhtune-server.exe uninstall  # stops it first if still running, then removes it
```

`install` registers a service named `BhtuneServer` ("BHTune Server" in `services.msc`), set to
start automatically and run as `LocalSystem`.

**A config/database gotcha worth knowing before you install.** BHTune's default config and
data paths live under `%APPDATA%` (see [above](#where-bhtune-stores-its-data)), which resolves
_per user account_. A Windows service normally runs as `LocalSystem`, whose `%APPDATA%` is a
hidden system-profile folder — a different location entirely from the one your own
interactive login resolves to. If you've been testing `bhtune-server` from your own terminal
and then install it as a service with no further changes, the service will _not_ see the
config or database you were using: it will look like a fresh install, with an empty database
and only the four built-in templates.

The fix is to pin an explicit, absolute config file at install time, and have that file itself
name absolute (not default-relative) paths for the database and logs, so nothing about it
depends on which account ends up running the service:

```powershell
mkdir C:\ProgramData\bhtune
```

```toml
# C:\ProgramData\bhtune\bhtune.toml
db = 'C:\ProgramData\bhtune\bhtune.db'

[log]
dir = 'C:\ProgramData\bhtune\logs'
```

```powershell
bhtune-server.exe --config C:\ProgramData\bhtune\bhtune.toml install
bhtune-server.exe start
```

`--config` is captured into the service's own registered launch command at install time (not
just used once, interactively), so every future start of the service — after a reboot, after
`stop`/`start`, after a Windows update — resolves the same config file and the same database,
regardless of which account the SCM happens to run it as.

### Linux (systemd)

```sh
sudo install -m755 target/release/bhtune-server /usr/local/bin/bhtune-server
sudo install -Dm644 packaging/systemd/bhtune-server.service \
    /etc/systemd/system/bhtune-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now bhtune-server
```

The provided [`packaging/systemd/bhtune-server.service`](https://github.com/bytehound-labs/bhtune/blob/main/packaging/systemd/bhtune-server.service)
unit uses `DynamicUser=true` (an ephemeral, unprivileged account systemd creates for the
service's lifetime — no separate `useradd` step) plus `StateDirectory=`/
`ConfigurationDirectory=` so the database and logs live at `/var/lib/bhtune/` and an optional
config file at `/etc/bhtune/bhtune.toml`, both owned correctly with no manual `chown` needed.
Unlike the Windows service above, this sidesteps the per-account path problem entirely — a
systemd-managed service's environment is set once, in the unit file itself, not inherited from
whichever user happens to be logged in. Check on it with `systemctl status bhtune-server` and
`journalctl -u bhtune-server -f`; stop it with `sudo systemctl disable --now bhtune-server`.

### macOS (launchd)

```sh
sudo install -m755 target/release/bhtune-server /usr/local/bin/bhtune-server
sudo mkdir -p /usr/local/etc/bhtune /usr/local/var/bhtune /usr/local/var/log
sudo install -m 644 packaging/launchd/com.bytehound-labs.bhtune-server.plist \
    /Library/LaunchDaemons/
sudo launchctl bootstrap system /Library/LaunchDaemons/com.bytehound-labs.bhtune-server.plist
```

The provided [`packaging/launchd/com.bytehound-labs.bhtune-server.plist`](https://github.com/bytehound-labs/bhtune/blob/main/packaging/launchd/com.bytehound-labs.bhtune-server.plist)
registers a LaunchDaemon (not a per-user LaunchAgent, since this is a network service that
should run regardless of login state) pointed at the Homebrew-style `/usr/local/etc`/
`/usr/local/var` paths (Apple Silicon Homebrew installs use `/opt/homebrew` instead — adjust
the binary path in the plist to match). Check on it with
`sudo launchctl print system/com.bytehound-labs.bhtune-server` and
`tail -f /usr/local/var/log/bhtune-server.log`; stop and unload it with
`sudo launchctl bootout system/com.bytehound-labs.bhtune-server`.

## Next steps

- [CLI quickstart](cli-quickstart.md) — run your first tune from the command
  line, no plant connection required.
- [Web GUI quickstart](web-gui-quickstart.md) — run the server and drive a
  tune from a browser.
