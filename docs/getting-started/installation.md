---
sidebar_position: 1
---

# Installation

BHTune has not made its first tagged release yet — see the
[Releases](https://github.com/bytehound-labs/bhtune/releases) page for prebuilt binaries once
one exists. Until then, run the published Docker image or build from source. The repository also
contains the Windows NSIS installer workflow and validation sources; a stable release will attach
the resulting installer beside the matching Windows archive.

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

Full-mode Docker access does not require `BHTUNE_ORIGIN`. When no origin is configured, browser
mutations are accepted only when the browser origin's host and effective port match the request
`Host`, so the same image works through `localhost`, a LAN hostname, or a LAN address. Set
`BHTUNE_ORIGIN` (or the `origin` config key) when a reverse proxy rewrites `Host` or when a
single external origin must be pinned. This is CSRF protection, not authentication; keep
non-loopback Full-mode deployments on a trusted network.

When OPC DA gateway names are maintained in the Linux host's `/etc/hosts` file, bind that
file into the container so the server resolves the same names as the host:

```sh
docker run -d --name bhtune \
  -p 8787:8787 \
  --mount type=bind,source=/etc/hosts,target=/etc/hosts,readonly \
  -v bhtune-data:/var/lib/bhtune \
  ghcr.io/bytehound-labs/bhtune:edge
```

Docker does not copy arbitrary host `/etc/hosts` entries into containers automatically. The
bind mount is intended for Linux hosts that use local aliases such as `yok3`; it also exposes
the host's other hosts-file entries to the container. On Docker Desktop, or when aliases are
provided by DNS instead, use the platform's DNS configuration or explicit `--add-host` entries
instead. The public simulator Demo deployment does not need OPC gateway host mappings.

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
- Use the `stable` Rust toolchain for local builds to match the regular CI jobs. CI separately
  checks Rust 1.94.0 as the MSRV; on rustup-managed hosts, `rustup update stable` refreshes an
  older selected toolchain.
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

#### NSIS installer

A stable Windows release will provide an installer named
`bhtune-vX.Y.Z-windows-x86_64-installer.exe`. It installs the release payload under
`%ProgramFiles%\ByteHound\bhtune\`, stores configuration, SQLite data, logs, installer state,
and one verified rollback backup under `%ProgramData%\ByteHound\bhtune\`, and registers
`BhtuneServer` as an automatic `NT AUTHORITY\LocalService` service. The registered command line
contains an absolute `--config` path under ProgramData, so service startup does not depend on the
interactive user's profile.

The installer starts the service by default and checks
`http://127.0.0.1:8787/api/health` for both `status: "ok"` and the expected package version.
It does not create firewall rules or change the OPC DA gateway. Interactive installation keeps
machine `PATH` and clean-install service startup enabled by default. Silent installation supports:

```text
/S
/ADD_TO_PATH=0
/START_SERVICE=0
/CUSTOM_DB_BACKUP_CONFIRMED=1
```

An empty clean install has no prior database to protect, so it does not create a rollback
backup. Once installer-managed data exists, upgrades retain exactly one verified rollback backup
under the ProgramData installer state directory; the existing backup is replaced only after the
new manifest has been verified.

Silent installs use the same ownership, health, and rollback checks as interactive installs; the
flags only select the optional machine `PATH`, clean-install startup, and custom-database
acknowledgement behavior. Fatal errors in silent mode return a nonzero installer exit code
instead of waiting for an interactive error dialog, so scheduled-task and CI callers can detect
failure without a desktop session.

Upgrades preserve the service's previous running/stopped state, but validate configuration before
stopping the service. Automatic database backup and rollback apply only when the top-level `db`
setting resolves to the installer-managed ProgramData database. An external, malformed, relative,
ambiguous, missing, or inaccessible database path fails closed; the installer does not create an
external database file or its parent directory. After preparing and independently backing up an
existing external database, an operator may rerun with `/CUSTOM_DB_BACKUP_CONFIRMED=1`. That
override permits binary/service upgrade but does not claim to back up or roll back the external
database. If an upgrade fails, rollback also checks the managed rollback root, restored service
health, and package version before reporting recovery.

The external database is opened by the installed service as `NT AUTHORITY\LocalService`, not
by the elevated installer account. The database file's parent directory and the database file
itself therefore need to grant that account enough access to read and update SQLite state and
to create or update the `bhtune.db-wal` and `bhtune.db-shm` sidecars. Installer preflight can
confirm that the file exists and is readable in the installer context, but only the subsequent
service startup and health check proves that the service account can use the path. A confirmed
external database that fails that startup check is an operator configuration/ACL problem; the
installer does not broaden permissions on arbitrary operator-owned paths.

If an installer process is interrupted, the next installer invocation validates the recorded
transaction journal and either completes the safe recovery or refuses to continue without
guessing about ownership or data.

Uninstall is a two-pass transaction. The first pass removes the owned service, shortcut,
registry-facing uninstall state, and exact machine `PATH` entry, then leaves the transaction
journal and ownership metadata in place while NSIS removes the fixed Program Files tree. A
guarded finalization pass removes that metadata and the journal only after the tree is verified
absent; an interrupted or incomplete cleanup therefore remains retryable instead of being
reported as finished. Uninstall removes only installer-owned binaries and state, and preserves the entire
`%ProgramData%\ByteHound\bhtune\` tree, including configuration, databases, logs, and rollback
backup. There is no automated data-deletion option.

#### Manual archive installation

The release archive is also usable without the installer. `bhtune-server.exe` registers itself
directly with the Service Control Manager (SCM):

```powershell
bhtune-server.exe install    # registers the service (does not start it)
bhtune-server.exe start
bhtune-server.exe status
bhtune-server.exe stop
bhtune-server.exe uninstall  # stops it first if still running, then removes it
```

`install` registers a service named `BhtuneServer` ("BHTune Server" in `services.msc`), set to
start automatically and run as `LocalSystem`. This manual fallback is intentionally distinct from
the NSIS installer, which uses `LocalService` and an installer-owned ProgramData layout.

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
