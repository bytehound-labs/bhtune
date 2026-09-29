---
sidebar_position: 2
---

# Crate and workspace graph

The Cargo workspace separates domain logic, tag I/O, persistence, and adapters. The frontend
and documentation site are pnpm workspace members rather than Rust crates.

| Component | Kind | Role and dependency boundary |
| --- | --- | --- |
| `bhtune-core` | Rust library | Domain model, MRFT state machine, and tuning calculations. It has no I/O, async runtime, or clock reads. |
| `bhtune-driver` | Rust library | Defines the `Driver` tag-I/O interface and provides OPC DA, simulator, and replay implementations. Its production interface is separate from the MRFT domain types. |
| `bhtune-db` | Rust library | Provides the SQLite schema, migrations, and typed repository operations. It uses core model types and is the persistence boundary for the adapters. |
| `bhtune-cli` | Rust library and `bhtune` binary | Combines the core, driver, and database for headless tune, simulation, history, and OPC diagnostic commands. |
| `bhtune-server` | Rust library and `bhtune-server` binary | Provides the Axum HTTP API, OpenAPI document, SSE run stream, and browser-asset handler. It uses the shared core, driver, and database libraries and reuses the CLI library's tune orchestration and bootstrap code. |
| `frontend/` | pnpm package | React and TypeScript browser interface. It calls the server API through the generated OpenAPI client; it is not a Rust crate. |
| `website/` | pnpm package | Docusaurus site built from the repository's `docs/` directory. It is separate from the runtime binaries. |

The main runtime paths are:

| Entry point | Runtime path |
| --- | --- |
| CLI | `bhtune` -> `bhtune-cli` -> `bhtune-core` + `bhtune-driver` + `bhtune-db` |
| Browser | React SPA -> HTTP/SSE -> `bhtune-server` -> shared tune orchestration and the core, driver, and database libraries |
| Documentation | Docusaurus site -> repository `docs/` |

An HTTP-started tune runs inside `bhtune-server` by calling `bhtune-cli`'s library functions;
the server does not launch a second `bhtune` process. The CLI can run independently of the
server. The website has no runtime dependency on either adapter.

The [runtime topology](topology.md) shows where these components run. See the
[Rust API reference](../reference/api.md) for crate-level source documentation.
