# BHTune

[![CI](https://github.com/bytehound-labs/bhtune/actions/workflows/checks.yml/badge.svg)](https://github.com/bytehound-labs/bhtune/actions/workflows/checks.yml)
[![codecov](https://codecov.io/gh/bytehound-labs/bhtune/branch/main/graph/badge.svg?token=)](https://codecov.io/gh/bytehound-labs/bhtune)
[![Quality Gate Status](https://sonarcloud.io/api/project_badges/measure?project=bytehound-labs_bhtune&metric=alert_status)](https://sonarcloud.io/summary/new_code?id=bytehound-labs_bhtune)
[![License: AGPL v3](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://doc.rust-lang.org/edition-guide/rust-2024/)
[![Docs](https://img.shields.io/badge/docs-bytehound--labs.github.io%2Fbhtune-blue.svg)](https://bytehound-labs.github.io/bhtune/)

An open-source PID loop auto-tuner for industrial control systems (DCS/PLC), built by
[ByteHound](https://github.com/bytehound-labs).

## What it does

BHTune runs a Modified Relay Feedback Test (MRFT) against a live PID control loop, then
calculates and — with explicit confirmation — writes back tuned PID constants. It targets
Yokogawa CentumVP, Honeywell Experion, Schneider Modicon, and Allen-Bradley PlantPAx control
systems out of the box, with more addable via [templates](#dcsplc-templates) — contributions
welcome.

BHTune is designed around a few core principles:

- **Runs anywhere.** No Windows or COM/DCOM dependency in this application. Communication with
  OPC DA systems goes over the network to a small Windows-side gateway,
  [`opcda-bridge`](https://github.com/bytehound-labs/opcda-bridge) — BHTune itself runs on Linux,
  macOS, or Windows.
- **Has no proprietary dependencies.** Every dependency is open-source, machine-enforced in CI via
  `cargo deny` (see [`deny.toml`](deny.toml)). There is no licensed UI toolkit, no licensed OPC
  SDK, and no hardware/software dongle.
- **Has two faces, one engine.** A headless CLI for scheduled/scripted tuning and a browser-based
  web GUI for interactive use, both built on the same tuning engine and the same SQLite database.
- **Stores everything in a plain, open SQLite database.** No encryption, no usage gating, no
  license dongle — just a single database anyone can inspect.
- **Is built to be extended.** OPC DA is the primary, supported driver for v1. The tag-I/O
  interface (`Driver` trait) is deliberately protocol-agnostic — see [Roadmap](#roadmap) for
  planned OPC UA and Modbus drivers.

## Status

Pre-release, but functional: the CLI and web GUI both run a complete MRFT tune end to end —
against the built-in simulator with no setup at all, or against a real OPC DA loop via
[`opcda-bridge`](https://github.com/bytehound-labs/opcda-bridge) — including calculating PID
constants, writing them back with confirmation and rollback, and recording full run history.
See [Getting started](#getting-started) below to try it. No versioned release or prebuilt
binaries exist yet (see [Installation](#installation)). The tuning engine's golden-master
validation against a captured legacy trace is complete; [`AGENTS.md`](AGENTS.md) records the
full phased implementation plan.

## Getting started

- [Installation](docs/getting-started/installation.md) — build from source.
- [CLI quickstart](docs/getting-started/cli-quickstart.md) — run a tune from the command line
  against the built-in simulator, no plant connection required.
- [Web GUI quickstart](docs/getting-started/web-gui-quickstart.md) — the same tuning engine,
  driven from a browser.
- [MRFT concepts](docs/guides/mrft-concepts.md) and [Safety](docs/guides/safety.md) — what the
  test actually does, and the guardrails around running it unattended against live equipment.

All of this content also builds into a browsable, searchable documentation site, published at
**[bytehound-labs.github.io/bhtune](https://bytehound-labs.github.io/bhtune/)** — see
[`website/`](website/README.md) — sourced directly from [`docs/`](docs/), so the site can never
drift from what's in this repo.

## Architecture

A Cargo workspace of small, single-purpose crates:

| Crate           | Role                                                                                                                                                                                                 |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `bhtune-core`   | Pure domain logic: the MRFT state machine, tuning math, and data model. No I/O, no async, no clock reads — this is what makes deterministic, replayable testing possible.                            |
| `bhtune-driver` | The `Driver` trait (`read`/`write`/`browse`) and its implementations: OPC DA (via `opcda-bridge`), an in-process process simulator, and a golden-trace replay driver used for regression validation. |
| `bhtune-db`     | SQLite persistence (`sqlx`, WAL mode): DCS/PLC templates, loops, tune runs, samples, and results.                                                                                                    |
| `bhtune-cli`    | The headless `bhtune` binary — scriptable tuning for schedules and automation, no GUI required.                                                                                                      |
| `bhtune-server` | The web GUI adapter: an Axum HTTP API plus the embedded React SPA, served from one binary.                                                                                                           |

The frontend (`bhtune-frontend`: React + TypeScript + Vite + Tailwind CSS, for `bhtune-server`)
lives under `frontend/` — a pnpm workspace package, kept separate from the Cargo workspace. See
[`frontend/README.md`](frontend/README.md) for how to run it during development. Once built
(`pnpm run build`), `bhtune-server` embeds `frontend/dist/` directly into its own binary via
`rust-embed`, so a release build is one self-contained executable — no separate static file
server, Node, or nginx required on the target host.

The documentation site (`bhtune-website`: Docusaurus) lives under `website/`, a third pnpm
workspace package alongside `frontend/`. It has no runtime relationship to `bhtune-server` or
the CLI — it's a separate static site build that renders [`docs/`](docs/). See
[`website/README.md`](website/README.md).

### OPC DA bridge

The OPC DA driver uses the reusable [`opcda-bridge`](https://crates.io/crates/opcda-bridge)
library from crates.io:

```toml
[dependencies]
opcda-bridge = "0.2"
```

The library communicates with the separate Windows-side
[`opcda-bridge-gateway`](https://crates.io/crates/opcda-bridge-gateway) process over the network.
`bhtune-driver` uses the published `opcda-bridge` crate for its `OpcDaDriver` implementation.

The web form's **Browse servers** button opens an on-demand picker for the OPC DA servers
registered on the gateway. Its tag browser expands one namespace level at a time and supports
both dotted and slash-separated OPC item IDs. The first node is selected automatically when
the tree loads, and the selection panel remains in place while browsing. With a template
selected, confirming a tag selection replaces its final component with that template's
process-variable suffix before writing the value into the Tag name field. Use a
gateway release with recursive hierarchical browsing for servers that expose branch names
through `OPC_FLAT` without returning their descendants.
Both browser dialogs expose accessible dialog semantics and can be closed with their Close
button, the backdrop, or Escape.
Changing templates replaces a tag's final component with the new template's process-variable
suffix, regardless of what the previous component was, while preserving the tag path.
Confirming a tag selection performs a fresh read of the original item selected in the browser
and proceeds immediately only when its OPC quality is `Good`. `Uncertain` or `Bad` quality
requires an explicit choice to select a different tag or proceed anyway. Proceeding only
selects the item; a tune still applies its live-reading quality safeguards. The global Config
page controls whether `Uncertain` readings are accepted during tuning; `Bad` is always rejected.
Reopening the browser automatically expands the available path to the current Tag name, selects
that node, and scrolls it into view; if it is no longer present, browsing falls back to the root
level.

The New tune form's collapsible **Loop mapping** section is the single place to inspect and
adjust the effective mapping. Every row shows its effective value and source. Tag mappings use
**Template tag** or **Custom tag**; direction and range mappings use **Template tag**, **Custom tag**,
or **Fixed value**. Switching to a custom tag starts from the
template-derived value; fixed direction/range values must be entered explicitly. Per-row
**Reset** actions return to template/live values, and **Reset all mapping overrides** restores
every row.
The selected source and values are retained in the saved draft. Simulator direction and ranges
are stored separately from OPC fixed overrides, so changing drivers cannot turn simulator
settings into live OPC overrides.

In **Simulator** mode, the form disables the OPC DA connection, tag, quality, timeout, and
automatic write-back controls because the in-process simulator cannot use them. The DCS/PLC
template remains selectable: the simulator ignores its tag mappings, but its PID type and unit
conventions still format the calculated results (for example, Yokogawa uses proportional band
while the other built-in templates use gain). PV/MV ranges and controller direction remain
editable because the simulator has no live tags from which to read them.

## Installation

No `v0.1.0` tag has been cut yet, so there are no versioned release archives. Two ways to run
BHTune today:

### Docker

A multi-stage image (frontend build → `cargo build --release` → slim Debian runtime, ~110 MB)
is published to [GHCR](https://github.com/bytehound-labs/bhtune/pkgs/container/bhtune) —
tagged `edge` on every push to `main`, and additionally under the version and `latest` once a
release tag exists. It bundles both `bhtune` and `bhtune-server`; the host needs neither a
Rust toolchain, pnpm, nor a C compiler:

```sh
docker run -d --name bhtune \
  -p 8787:8787 \
  -v bhtune-data:/var/lib/bhtune \
  ghcr.io/bytehound-labs/bhtune:edge
```

Open `http://localhost:8787` for the web GUI. The `bhtune` CLI is available in the same
image, sharing the running server's database via the mounted volume:

```sh
docker exec bhtune bhtune history list
```

See the [`Dockerfile`](Dockerfile) for the full build and the image's baked-in defaults
(`BHTUNE_BIND=0.0.0.0:8787`, `BHTUNE_DB=/var/lib/bhtune/bhtune.db` — both overridable with
`docker run -e`). This is a _secondary_ distribution channel for IT-managed Linux hosts; a
Windows installer is the primary path for this project's actual users, since OT sites
frequently prohibit or simply lack container runtimes.

### Build from source

Requires a Rust toolchain supporting the 2024 edition (Rust 1.94 or newer, BHTune's declared
MSRV, verified in CI) and the Protocol Buffers compiler `protoc` on `PATH` (needed transitively
by [`opcda-bridge`](https://github.com/bytehound-labs/opcda-bridge)'s gRPC codegen build
script — see [Installation](docs/getting-started/installation.md#prerequisites) for
per-platform install commands):

```sh
git clone https://github.com/bytehound-labs/bhtune.git
cd bhtune
cargo build --workspace
```

The full `bhtune` command reference — every subcommand, flag, and default — is generated
directly from the CLI's own argument definitions and checked in at
[`docs/reference/cli.md`](docs/reference/cli.md), so it can never drift from what the binary
actually accepts. The same generation step also produces man pages (`man/*.1` — try `man
./man/bhtune-tune.1`) and shell completions (`completions/bhtune.bash`, `completions/_bhtune`,
`completions/bhtune.fish`); packaged releases install both into the usual system locations.

### What's still coming

Once a version tag is pushed, release tooling already in place
(`.github/workflows/release.yml`, `taiki-e/upload-rust-binary-action`) attaches prebuilt
Linux/macOS/Windows archives (each bundling both `bhtune` and `bhtune-server`) to the
[Releases](https://github.com/bytehound-labs/bhtune/releases) page automatically. A Windows
installer and an AUR package are committed follow-on distribution channels, not yet built.
Publishing to [crates.io](https://crates.io), Homebrew, and a few other channels is still an
open evaluation, not a commitment — see the [roadmap](docs/roadmap.md).

### Running the server

```sh
cargo run --bin bhtune-server
```

Binds `127.0.0.1:8787` by default (see the `bind` setting below) and exposes a JSON HTTP API —
`GET /api/health` (including the running application version); `GET`/`POST /api/templates`
and `GET`/`PUT`/`DELETE /api/templates/{name}`;
`GET /api/runs`/`GET /api/runs/{id}`/`DELETE /api/runs/{id}` for run history,
`GET /api/runs/{id}/export` for CSV/JSON sample export, `GET /api/runs/{id}/stream` for a live
Server-Sent Events feed of an in-progress run (initial readings are sent as soon as they are
recorded, before per-tick samples; independent OPC DA startup tags are collected in one
batched read, while mode-dependent setpoint reads remain conditional), `GET`/`PUT /api/runs/draft` for the
app-wide autosaved New tune form draft (all fields except Notes), and
`GET /api/runs/last-request` for the newest run's settings as a one-time fallback when no draft
exists. A missing draft is a normal first-use state and quietly falls back to the newest run or
built-in defaults; `POST /api/runs`/
`POST /api/runs/{id}/cancel` to start and cancel a tune, plus `POST /api/runs/{id}/write`/
`POST /api/runs/{id}/revert` to write or roll back PID constants after a run has finished;
`PUT`/`DELETE /api/runs/{id}/notes` to edit or clear operator notes while a run is active or
after it finishes. Multiple tune runs can execute concurrently; PID writes and reverts remain
exclusive so they cannot overlap an active tune;
and `GET /api/opc/servers`/`GET /api/opc/browse`/`GET /api/opc/read` for read-only OPC DA
server discovery, tag-tree browsing, and a live single-tag read — using the same SQLite
database and config precedence as the CLI. The full API contract is described by an OpenAPI
3.1 document, served as raw JSON at `GET /api/openapi.json` and as interactive documentation
at `/api/docs`
(a [Scalar](https://scalar.com/) UI — try it in a browser, or point any OpenAPI-aware tool at
the JSON endpoint). The same document is checked in at [`openapi.json`](openapi.json) at the
repo root for anyone who wants to read or diff it without running the server. Once the
frontend is built, `bhtune-server` serves the web UI itself directly at `/` (embedded into the
binary via `rust-embed` — see [Architecture](#architecture)):

```sh
corepack enable                  # or install the pnpm version declared in package.json
pnpm install                     # from the repo root — this is a pnpm workspace
pnpm --filter bhtune-frontend run build
cargo run --bin bhtune-server    # now also serves the built UI at http://127.0.0.1:8787/
```

The web GUI header includes a light/dark theme toggle and shows the server version beside a
vertically centered colored status dot based on that liveness endpoint. The selected theme is
remembered by the browser. Green means the BHTune HTTP service is reachable; it does not verify
OPC DA connectivity. Hover the dot for the full status detail.

During frontend development, running the Vite dev server alongside `bhtune-server` instead
gives hot-reload:

```sh
cd frontend
pnpm dev             # http://localhost:5173, proxies /api/* to the server above
```

See [`frontend/README.md`](frontend/README.md) for details. The
server shuts down gracefully on Ctrl+C (and on Unix, `SIGTERM`), draining in-flight requests
rather than dropping connections.

`bhtune-server` also accepts a `--config <path>` flag to pin an explicit config file, which
matters once it's running unattended as a background service rather than from an interactive
terminal.

### Run as a background service

For a shared, always-on deployment, `bhtune-server` registers with each platform's native
service manager instead of running from an interactive terminal:

- **Windows**: `bhtune-server.exe install`, then `start` — see
  `bhtune-server.exe --help` for `uninstall`/`stop`/`status` too.
- **Linux**: install the provided [systemd unit](packaging/systemd/bhtune-server.service).
- **macOS**: install the provided [launchd daemon](packaging/launchd/com.bytehound-labs.bhtune-server.plist).

Full step-by-step instructions — including a config/database path gotcha worth knowing about
before installing the Windows service — are in
[Run as a background service](docs/getting-started/installation.md#run-as-a-background-service).

## Configuration

Every setting resolves with the same precedence, highest first:

**CLI flag > environment variable > config file > built-in default**

A config file (or an individual key within it) is entirely optional — anything not set falls
back through the rest of the chain. If `--config` is omitted, `bhtune` looks for a config file
in a platform-specific location:

- Linux/macOS: `$XDG_CONFIG_HOME/bhtune/bhtune.toml`, falling back to
  `$HOME/.config/bhtune/bhtune.toml`.
- Windows: `%APPDATA%\bhtune\bhtune.toml`.

A missing file there is not an error, since it may simply not have been created yet. A file
that _does_ exist but fails to parse as TOML is always a hard error, pointing at the file and
the parse problem. See
[`crates/bhtune-cli/bhtune.example.toml`](crates/bhtune-cli/bhtune.example.toml) for every
available key, or [`docs/reference/config.md`](docs/reference/config.md) for the generated
JSON Schema (also covers one DCS/PLC template catalog entry — see below).

| Setting                                  | CLI flag           | Env var                 | Config key                | Default                                                                                                                                                                     |
| ---------------------------------------- | ------------------ | ----------------------- | ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Database path                            | `--db`             | `BHTUNE_DB`             | `db`                      | Linux/macOS: `$XDG_DATA_HOME/bhtune/bhtune.db` (or `$HOME/.local/share/bhtune/bhtune.db`); Windows: `%APPDATA%\bhtune\bhtune.db`                                            |
| opcda-bridge gateway                     | `--bridge-host`    | `BHTUNE_BRIDGE_HOST`    | `bridge_host`             | `localhost:7600`                                                                                                                                                            |
| Default OPC DA server                    | `--server`         | —                       | `server`                  | none — must be set one way or another                                                                                                                                       |
| User template catalog                    | `--templates`      | `BHTUNE_TEMPLATES`      | `templates`               | Linux/macOS: `$XDG_CONFIG_HOME/bhtune/templates.toml` (or `$HOME/.config/bhtune/templates.toml`); Windows: `%APPDATA%\bhtune\templates.toml` — missing is not an error here |
| Log level                                | `--log-level`      | `RUST_LOG`              | `log.level`               | `info`                                                                                                                                                                      |
| Log directory                            | `--log-dir`        | —                       | `log.dir`                 | Linux/macOS: `$XDG_DATA_HOME/bhtune/logs` (or `$HOME/.local/share/bhtune/logs`); Windows: `%APPDATA%\bhtune\logs`                                                           |
| Log format                               | `--log-format`     | —                       | `log.format`              | `pretty`                                                                                                                                                                    |
| Log rotation                             | `--log-rotation`   | —                       | `log.rotation`            | `daily`                                                                                                                                                                     |
| HTTP bind address (`bhtune-server` only) | —                  | `BHTUNE_BIND`           | `bind`                    | `127.0.0.1:8787`                                                                                                                                                            |
| Allow Uncertain OPC quality              | —                  | —                       | `allow_uncertain_quality` | `true`                                                                                                                                                                      |
| History retention                        | `--retention-days` | `BHTUNE_RETENTION_DAYS` | `retention_days`          | unset — retain forever; a configured value must be a positive whole number                                                                                                  |

The web GUI's **Config** page reads and updates the two global policy keys in the selected
TOML file. Updates preserve unrelated comments and keys, create a timestamped sibling backup
for an existing file, and take effect for new server operations without a restart. A revision
token prevents overwriting a file changed by another process; command-line and environment
overrides remain higher precedence than TOML values.

## DCS/PLC templates

A template maps one DCS/PLC vendor's PID conventions — tag suffixes, units, and raw mode
values — onto BHTune's tuning engine. Four ship built in (Yokogawa CentumVP, Honeywell
Experion, Schneider Modicon, Allen-Bradley PlantPAx), and more are added as a plain TOML
data file change, not a Rust change:

```sh
bhtune template list                                              # built-in, catalog, and user templates
bhtune template show "Yokogawa CentumVP"                          # full field detail as JSON
bhtune template export "Yokogawa CentumVP" out.toml --format toml # a PR-ready [[template]] block
bhtune template import ./site-catalog.toml                        # a single template or a multi-template catalog
bhtune template delete "My Custom Template"
```

You can also auto-load your own catalog file on every startup (see `templates`/
`--templates`/`BHTUNE_TEMPLATES` in the Configuration table above) — handy for sharing
site-specific templates across installations without contributing them upstream.

**Contributions of new templates are very welcome.** The goal is a full,
community-maintained library eventually covering as many control systems as possible. See
[`docs/dcs-templates.md`](docs/dcs-templates.md) for the complete field reference and a
worked example, and [`CONTRIBUTING.md`](CONTRIBUTING.md) for how to submit one.

## Automation

`bhtune tune`/`bhtune simulate` run fully non-interactively when scripted or scheduled (`cron`,
Windows Task Scheduler, CI):

- **`--write-pid <aggressive|moderate|sluggish>`** writes that response level's calculated PID
  constants back without the interactive confirmation prompt. It requires **`--yes`** — the
  combination is rejected before any driver connection or database write, so an unattended
  write-back is always an explicit, deliberate choice.
- **`--output json`** (also on `history list`/`show`/`revert`) prints exactly one
  machine-readable JSON value to stdout instead of the plain-text table, and nothing else —
  no prose, no interactive prompts — ever reaches stdout in that mode, so `stdout | jq` (or
  any JSON parser) always succeeds. `tune`/`simulate` fold the reason a PID write-back was
  skipped or failed into a `write_back_detail` field rather than only printing it, and skip
  the interactive write-back prompt entirely when `--write-pid` wasn't also given, since
  there's no human present in a scripted run to answer it.
- **Exit codes** distinguish outcomes for automated callers: `0` success, `1` a setup error
  (bad flags, unreachable driver/database), `2` aborted (Ctrl+C or `--timeout-secs` elapsing),
  `3` the test completed but the requested PID write-back failed, `4` the test was forcibly
  stopped for running past `--timeout-secs`, `5` a poor-quality OPC reading aborted the run, and
  `6` the post-run restore could not be confirmed (a second Ctrl+C, or `--restore-timeout-secs`
  elapsing) — distinct from `2` since "aborted and restored" and "aborted, restore abandoned"
  call for very different alerting. A caller never has to parse stdout just to find out whether
  a scheduled tune actually wrote anything, or why it stopped early.

## Safety

Scheduled/scripted tuning removes the one safeguard the interactive app always had: an operator
watching the trend, able to hit Stop. `bhtune tune`/`bhtune simulate` build in guardrails so
unattended runs against live plant equipment fail safe. This section is the technical reference;
[`docs/guides/safety.md`](docs/guides/safety.md) walks through the same guarantees in plain
language, including exactly what happens on the first and second Ctrl+C:

- **Relay amplitude is range-checked**, not just required to be non-blank — an out-of-range value
  is rejected before any driver connection or database write.
- **Every numeric input is validated before it can reach a live loop** — CLI flags reject
  non-finite (`NaN`/infinite), zero, or negative values at parse time with a clear error; loop
  configuration rejects an out-of-range cycle count or MRFT delay; and the PV/MV ranges plus the
  initial MV, whether they came from a flag or a driver tag read, are checked for finiteness and
  correct ordering immediately after the initial read and before the loop is switched to manual.
- **`--timeout-secs <seconds>`** (default `3600`) is a mandatory wall-clock limit on the whole
  test — there is no way to disable it. If it elapses, the loop is automatically restored to its
  pre-test mode and the process exits `4`, distinct from a deliberate Ctrl+C (`2`). Both Ctrl+C
  and the timeout stay effective even if a single driver read or write stalls mid-tick (a
  wedged gateway, a black-holed network): every driver call is separately capped by
  **`--op-timeout-secs`** (default `30`), so a hung call is abandoned rather than blocking the
  whole run indefinitely.
- **`--restore-timeout-secs <seconds>`** (default `30`) bounds putting the loop back afterwards,
  independently of `--timeout-secs`. If the restore can't be confirmed within that time, or a
  _second_ Ctrl+C arrives while it's in progress, the process prints which tag and value to
  check by hand and exits `6` — distinct from `2`, since "aborted and restored" and "aborted,
  restore abandoned" call for very different responses.
- **Restoration is guaranteed on every exit path and never gives up early** — a run only ever
  mutates a loop after switching it to manual, and _any_ way that run can end (a clean
  completion, an abort, or an error partway through setup) always attempts to put back exactly
  what was actually changed, never more and never less. If one part of the restore fails (say,
  the mode write is rejected), the rest are still attempted independently rather than the whole
  restore giving up — an operator checking `bhtune history show <run>` sees `confirmed` or
  `incomplete`, and an `incomplete` restore names every step that couldn't be confirmed.
- **`--write-pid <level>` always requires `--yes`** — there is no way to write PID constants to
  a live loop without explicitly confirming it, whether interactively or from a script.
- **Every run snapshots the exact template and resolved tags it used** — a historical run
  (`bhtune history show <run>`) stays interpretable even after the template it was configured
  against is later edited, re-versioned, or deleted from the catalog.
- **OPC quality is enforced on every tuning-critical read** — a tag reporting bad quality is
  never trusted for tuning, and a tag reporting uncertain quality follows the global
  `allow_uncertain_quality` policy in the TOML configuration (enabled by default, logged and
  recorded on the run either way). Disable it on the Config page when uncertain readings must
  be rejected. A poor-quality reading during the in-flight test aborts and restores the loop
  just like a Ctrl+C, and the exit code (`5`) is distinct from every other abort reason.
- **PID write-back is pre-read, verified, audited, and rolled back on partial failure.** Before
  any constant is written, the loop's current P/I/D values are read and recorded — if that
  pre-read fails, nothing is written at all. Each constant is then written and immediately read
  back to confirm it landed within tolerance, in order, stopping at the first failure; a
  constant that already wrote and confirmed successfully is automatically rolled back to its
  pre-write value if a later one in the same write-back fails, so a loop is never left with a
  mismatched, half-updated set of constants. `bhtune history show <run>` reports the previous
  value, the written value, and the confirmed readback for every constant, plus whether a
  rollback was needed and whether it succeeded — a rollback that itself fails is called out
  explicitly, since it means the loop may hold constants that need fixing by hand.
- **A past write-back can be undone with `bhtune history revert <run-id>`** — it writes the
  run's recorded pre-write values back to the live loop, under the same pre-read/verify/audit
  behavior and the same `--yes` confirmation gate as the original write-back. Useful when a
  write-back turns out to have been wrong days later and nobody wrote the old numbers down.

## Logging

Every `bhtune` invocation writes structured logs to a rotating file, using `--log-level`/
`--log-dir`/`--log-format`/`--log-rotation` (see the Configuration table above for the matching
env vars/config keys and defaults). Log lines never go to stdout: `bhtune tune`/`simulate
--output json` documents stdout as a single machine-readable JSON object, so logs are written to
the log file and, only when an actual console is attached (never for a `cron`/Task Scheduler
invocation), mirrored to stderr — stdout stays exactly what a scheduler expects to parse either
way.

## Validation

BHTune's tuning engine is validated by golden-master replay: recorded input/output traces are
replayed through the engine and the results are asserted to match exactly, so future changes can
never silently alter tuning behavior. The captured legacy trace replays tick-for-tick and
result-for-result through the Rust engine (`crates/bhtune-core/tests/golden_replay.rs`) —
proving the port behaviorally matches the original, not just arguing that it should. The full v1
feature checklist — what's required, what's deferred, and what's deliberately not planned — lives at
[`docs/internal/v1-checklist.md`](docs/internal/v1-checklist.md).

The repository also validates high-risk boundaries and delivery artifacts automatically:

- `proptest` covers configuration, template catalogs, bridge payload mappings, and template
  imports; standalone `cargo-fuzz` targets cover the same parsers with arbitrary byte streams.
- Pull requests compare the generated `openapi.json` with the base branch and reject removed
  operations, response shapes, enum values, or newly required request fields.
- Databases created before the current migration set are upgraded in a compatibility test that
  verifies representative settings survive the forward migration.
- CodeQL, Semgrep, Gitleaks, actionlint, and zizmor run in GitHub Actions with immutable action
  pins. Release assets carry keyless Sigstore signatures, a CycloneDX SBOM, and GitHub artifact
  provenance attestations.

Run the local parser and compatibility checks with:

```sh
cargo test --workspace
python3 scripts/check_openapi_breaking_test.py
```

Fuzzing requires [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz); for example:

```sh
cargo fuzz run config
```

Knip provides deeper dead-code and dependency analysis across the root pnpm workspace,
`frontend/`, and `website/`:

```sh
pnpm install --frozen-lockfile
pnpm run check:dead-code
```

The check examines unused files, dependencies, and exports, plus unresolved and unlisted
imports. CI runs it on every pull request and push that changes the JavaScript/TypeScript
workspace, its configuration, or the workflow that invokes it; it is not a weekly-only check.

SonarQube Cloud provides broader maintainability analysis for the Rust, TypeScript/TSX,
documentation-site, and repository-script sources:

- Rust coverage is imported from the `cargo llvm-cov` LCOV report.
- Generated files, build output, tests, fuzz targets, and documentation artifacts are excluded
  from the source analysis.
- Frontend and documentation-site code is analyzed for issues and duplication, but is excluded
  from coverage until those packages produce JavaScript LCOV reports.
- Relevant pull requests and pushes to `main` run the analysis, with a full scan every Wednesday
  at 04:17 UTC and an available manual dispatch. Fork pull requests report an intentional skip
  because repository secrets are unavailable.

With the SonarScanner CLI installed and `SONAR_TOKEN` exported, reproduce the analysis locally
after generating the Rust report:

```sh
cargo llvm-cov --workspace --locked --lcov --output-path lcov.info
sonar-scanner
```

## Roadmap

- OPC UA and Modbus `Driver` implementations, alongside OPC DA.
- Remote/multi-user access to the web GUI: authentication, TLS, and an audit log of who
  ran/wrote what.
- Step Test, a simpler alternative manual tuning method. Blocked on adding a
  push/subscription RPC to `opcda-bridge`, since Step Test observes PV changes via an OPC DA
  subscription rather than polling reads.
- Multi-loop and batch tuning campaigns.
- Cross-run comparison and overlay in the history explorer — charting several past runs of
  the same loop together (e.g. "has this valve degraded since last year?"). Everything else
  in the history explorer is already shipped: age-based retention, headless `history list`/
  `show`/`prune`, and a GUI run list/detail screen with a PV/MV trend chart that includes the
  initial readings and terminal restored-MV boundary, keeps short runs left-anchored for
  12 configured poll intervals without synthetic points, and supports export (CSV/JSON) and
  delete — see [`docs/roadmap.md`](docs/roadmap.md#history-explorer).

See the [full roadmap](docs/roadmap.md) for the reasoning behind each item and its current
status.

## Contributing

All changes use a short-lived feature branch and pull request, including documentation and
one-line fixes. Keep each pull request focused, run the applicable Rust/frontend/website checks,
and update the branch if `main` advances before merging. Pull requests are squash-merged only
after every required check and the applicable SonarQube analysis pass; a PR analysis must report
zero `OPEN`/`CONFIRMED` issues, while any intentional Accepted or False Positive finding needs a
documented rationale and related link. See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the complete
workflow. Contributions require signing the [Contributor License Agreement](CLA.md).

## License

[GNU AGPL v3.0-or-later](LICENSE). BHTune is open-source software licensed under the AGPL's
terms.
