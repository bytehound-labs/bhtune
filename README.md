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
  [`opcda-bridge`](https://github.com/mikeboiko/opcda-bridge) — BHTune itself runs on Linux,
  macOS, or Windows.
- **Has no proprietary dependencies.** Every dependency is FOSS, machine-enforced in CI via
  `cargo deny` (see [`deny.toml`](deny.toml)). There is no licensed UI toolkit, no licensed OPC
  SDK, and no hardware/software dongle.
- **Has two faces, one engine.** A headless CLI for scheduled/scripted tuning and a desktop GUI
  (Tauri) for interactive use, both built on the same tuning engine and the same SQLite database.
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
| `bhtune-cli`     | The headless `bhtune` binary — scriptable tuning for schedules and automation, no GUI required.                                                                                                        |
| `bhtune-desktop` | The Tauri v2 desktop GUI.                                                                                                                                                                              |
| `bhtune-server`  | An HTTP/REST adapter. Roadmap only — not part of v1.                                                                                                                                                   |

The frontend (React + TypeScript + Vite, for `bhtune-desktop`) lives under `frontend/` once that
phase begins; it does not exist yet in this early scaffold.

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

## Validation

BHTune's tuning engine is validated by golden-master replay: recorded input/output traces are
replayed through the engine and the results are asserted to match exactly, so future changes can
never silently alter tuning behavior. The full v1 feature checklist — what's required, what's
deferred, and what's deliberately not planned — lives at
[`docs/v1-checklist.md`](docs/v1-checklist.md).

## Roadmap

- OPC UA and Modbus `Backend` implementations, alongside OPC DA.
- An HTTP/REST adapter (`bhtune-server`) and Docker image, for driving BHTune without a local
  Tauri install.
- Step Test (a simpler, manual alternative to MRFT that observes PV changes via an OPC DA
  subscription rather than polling). This is blocked on adding a subscription/streaming RPC to
  `opcda-bridge`.
- Multi-loop and batch tuning campaigns.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). Contributions require signing the
[Contributor License Agreement](CLA.md).

## License

[GNU AGPL v3.0-or-later](LICENSE). BHTune is free software: free to use, study, modify, and
redistribute under the AGPL's terms. ByteHound may separately offer proprietary/commercial
licensing terms to enterprise customers who need different terms than the AGPL provides; this
does not change the terms available to everyone else under this license.
