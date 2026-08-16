# Rust API reference

This page indexes the full [rustdoc](https://doc.rust-lang.org/rustdoc/) reference for
every crate and binary in the BHTune workspace, published at
[`/api/`](pathname:///api/) on this site and regenerated from the current `main` branch
on every merge that touches `crates/**` — it can never drift from the code the way a
hand-written architecture document can.

This is **Rust API documentation for contributors** reading or extending the codebase —
internal module layout, types, and function signatures. It is not the same thing as
BHTune's **HTTP API**, which is what the web GUI and any external script use to drive a
tune: that is documented separately as an OpenAPI 3.1 spec, served as an interactive
Scalar UI at `/api/docs` by a _running_ `bhtune-server` instance (see
[Explore the API directly](../getting-started/web-gui-quickstart.md#explore-the-api-directly)).
None of the crates below are published to crates.io or intended as a stable public
dependency yet — see [`pkg-evaluate-others`](../roadmap.md) for that possibility.

| Crate                                                         | Kind             | What it covers                                                                                                                                                                  |
| ------------------------------------------------------------- | ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`bhtune_core`](pathname:///api/bhtune_core/index.html)       | library          | Pure domain logic: the MRFT tuning state machine, PID math, and data model. No I/O, no async, no clock reads.                                                                   |
| [`bhtune_backend`](pathname:///api/bhtune_backend/index.html) | library          | The `Backend` trait (tag read/write/browse) and its implementations: OPC DA (via `opcda-bridge`), an in-process FOPDT simulator, and a golden-trace replay backend for testing. |
| [`bhtune_db`](pathname:///api/bhtune_db/index.html)           | library          | SQLite persistence (`sqlx`, WAL mode): DCS/PLC templates, loops, tune runs, samples, and results. A single, un-encrypted, open database file.                                   |
| [`bhtune_cli`](pathname:///api/bhtune_cli/index.html)         | library          | The headless CLI's internals — argument parsing, config precedence, and the `prepare()`/`drive()` orchestration `bhtune-server` also reuses to start a tune over HTTP.          |
| [`bhtune`](pathname:///api/bhtune/index.html)                 | binary           | The `bhtune` executable entry point (a thin wrapper over `bhtune_cli`).                                                                                                         |
| [`bhtune_server`](pathname:///api/bhtune_server/index.html)   | binary + library | The Axum HTTP/REST adapter: the primary v1 GUI backend, serving the tuning engine and the embedded React SPA from one binary.                                                   |

## Reading the source directly

Every crate's `Cargo.toml` also has a one-line `description`, and `AGENTS.md`'s
["Crate map"](https://github.com/bytehound-labs/bhtune/blob/main/AGENTS.md) table in the
repository summarizes what each one is responsible for and which phase(s) of the project
built it — a good starting point before diving into the generated reference above.
