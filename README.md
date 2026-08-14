# BHTune

[![CI](https://github.com/bytehound-labs/bhtune/actions/workflows/checks.yml/badge.svg)](https://github.com/bytehound-labs/bhtune/actions/workflows/checks.yml)
[![codecov](https://codecov.io/gh/bytehound-labs/bhtune/branch/main/graph/badge.svg?token=)](https://codecov.io/gh/bytehound-labs/bhtune)
[![License: AGPL v3](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://doc.rust-lang.org/edition-guide/rust-2024/)

A free, open-source PID loop auto-tuner for industrial control systems (DCS/PLC), built by
[ByteHound](https://github.com/bytehound-labs).

## What it does

BHTune runs a Modified Relay Feedback Test (MRFT) against a live PID control loop, then
calculates and — with explicit confirmation — writes back tuned PID constants. It targets
Yokogawa CentumVP, Honeywell Experion, Schneider Modicon, and Allen-Bradley PlantPAx control
systems out of the box, with more addable via templates.

BHTune is designed around a few core principles:

- **Runs anywhere.** No Windows or COM/DCOM dependency in this application. Communication with
  OPC DA systems goes over the network to a small Windows-side gateway,
  [`opcda-bridge`](https://github.com/bytehound-labs/opcda-bridge) — BHTune itself runs on Linux,
  macOS, or Windows.
- **Has no proprietary dependencies.** Every dependency is FOSS, machine-enforced in CI via
  `cargo deny` (see [`deny.toml`](deny.toml)). There is no licensed UI toolkit, no licensed OPC
  SDK, and no hardware/software dongle.
- **Has two faces, one engine.** A headless CLI for scheduled/scripted tuning and a browser-based
  web GUI for interactive use, both built on the same tuning engine and the same SQLite database.
- **Stores everything in a plain, open SQLite database.** No encryption, no usage gating, no
  license dongle — just a single database anyone can inspect.
- **Is built to be extended.** OPC DA is the primary, supported driver for v1. The tag-I/O
  interface (`Backend` trait) is deliberately protocol-agnostic — see [Roadmap](#roadmap) for
  planned OPC UA and Modbus backends.

## Status

Early scaffolding. Nothing is released yet. Track progress via the
[issues](https://github.com/bytehound-labs/bhtune/issues)
and [`AGENTS.md`](AGENTS.md), which records the phased implementation plan.

## Architecture

A Cargo workspace of small, single-purpose crates:

| Crate            | Role                                                                                                                                                                                                   |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `bhtune-core`    | Pure domain logic: the MRFT state machine, tuning math, and data model. No I/O, no async, no clock reads — this is what makes deterministic, replayable testing possible.                              |
| `bhtune-backend` | The `Backend` trait (`read`/`write`/`browse`) and its implementations: OPC DA (via `opcda-bridge`), an in-process process simulator, and a golden-trace replay backend used for regression validation. |
| `bhtune-db`      | SQLite persistence (`sqlx`, WAL mode): DCS/PLC templates, loops, tune runs, samples, and results.                                                                                                      |
| `bhtune-cli`     | The headless `bhtune` binary — scriptable tuning for schedules and automation, no GUI required.                                                                                                       |
| `bhtune-server`  | The web GUI adapter: an Axum HTTP API plus the embedded React SPA, served from one binary.                                                                                                            |

The frontend (React + TypeScript + Vite, for `bhtune-server`) lives under `frontend/` once that
phase begins; it does not exist yet in this early scaffold.

### OPC DA bridge

The OPC DA backend uses the reusable [`opcda-bridge`](https://crates.io/crates/opcda-bridge)
library from crates.io:

```toml
[dependencies]
opcda-bridge = "0.2"
```

The library communicates with the separate Windows-side
[`opcda-bridge-gateway`](https://crates.io/crates/opcda-bridge-gateway) process over the network.
The dependency is added to `bhtune-backend` when its OPC DA implementation is introduced; the
scaffolding workspace intentionally does not declare unused dependencies.

## Installation

Not yet released. Once tagged, prebuilt binaries will be attached to the
[Releases](https://github.com/bytehound-labs/bhtune/releases) page, and `bhtune-cli` will be
published to [crates.io](https://crates.io) for `cargo install bhtune-cli`.

To build from source (requires a Rust toolchain with 2024 edition support, i.e. Rust 1.85+):

```sh
git clone https://github.com/bytehound-labs/bhtune.git
cd bhtune
cargo build --workspace
```

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
available key.

| Setting               | CLI flag        | Env var              | Config key    | Default                                                                                   |
| --------------------- | --------------- | -------------------- | ------------- | ------------------------------------------------------------------------------------------ |
| Database path         | `--db`          | `BHTUNE_DB`          | `db`          | Linux/macOS: `$XDG_DATA_HOME/bhtune/bhtune.db` (or `$HOME/.local/share/bhtune/bhtune.db`); Windows: `%APPDATA%\bhtune\bhtune.db` |
| opcda-bridge gateway  | `--bridge-host` | `BHTUNE_BRIDGE_HOST` | `bridge_host` | `localhost:7600`                                                                            |
| Default OPC DA server | `--server`      | —                    | `server`      | none — must be set one way or another                                                      |

## Automation

`bhtune tune`/`bhtune simulate` run fully non-interactively when scripted or scheduled (`cron`,
Windows Task Scheduler, CI):

- **`--write-pid <aggressive|moderate|sluggish>`** writes that response level's calculated PID
  constants back without the interactive confirmation prompt. It requires **`--yes`** — the
  combination is rejected before any backend connection or database write, so an unattended
  write-back is always an explicit, deliberate choice.
- **`--output json`** (also on `history list`/`show`) prints a single machine-readable JSON
  object or array to stdout instead of the plain-text table, for scripting.
- **Exit codes** distinguish outcomes for automated callers: `0` success, `1` a setup error
  (bad flags, unreachable backend/database), `2` aborted (Ctrl+C or `--timeout-secs` elapsing),
  `3` the test completed but the requested PID write-back failed, `4` the test was forcibly
  stopped for running past `--timeout-secs`. A caller never has to parse stdout just to find out
  whether a scheduled tune actually wrote anything, or why it stopped early.

## Safety

Scheduled/scripted tuning removes the one safeguard the interactive app always had: an operator
watching the trend, able to hit Stop. `bhtune tune`/`bhtune simulate` build in guardrails so
unattended runs against live plant equipment fail safe:

- **Relay amplitude is range-checked**, not just required to be non-blank — an out-of-range value
  is rejected before any backend connection or database write.
- **`--timeout-secs <seconds>`** (default `3600`) is a mandatory wall-clock limit on the whole
  test — there is no way to disable it. If it elapses, the loop is automatically restored to its
  pre-test mode and the process exits `4`, distinct from a deliberate Ctrl+C (`2`).
- **`--dry-run`** rehearses the full write-back — resolving the response level, validating tags
  and calculated results — without ever writing to the DCS/PLC, and lifts the
  `--write-pid`-requires-`--yes` requirement, since nothing live is touched.

## Validation

BHTune's tuning engine is validated by golden-master replay: recorded input/output traces are
replayed through the engine and the results are asserted to match exactly, so future changes can
never silently alter tuning behavior. The full v1 feature checklist — what's required, what's
deferred, and what's deliberately not planned — lives at
[`docs/v1-checklist.md`](docs/v1-checklist.md).

## Roadmap

- OPC UA and Modbus `Backend` implementations, alongside OPC DA.
- Free remote/multi-user access to the web GUI: authentication, TLS, and an audit log of
  who ran/wrote what. Free, like every other feature — there is no paid tier planned.
- Step Test, a simpler alternative manual tuning method. Blocked on adding a
  push/subscription RPC to `opcda-bridge`, since Step Test observes PV changes via an OPC DA
  subscription rather than polling reads.
- Multi-loop and batch tuning campaigns.
- A history explorer: browsing/trend view over past runs, configurable retention, and
  cross-run comparison/overlay.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). Contributions require signing the
[Contributor License Agreement](CLA.md).

## License

[GNU AGPL v3.0-or-later](LICENSE). BHTune is free software: free to use, study, modify, and
redistribute under the AGPL's terms.
