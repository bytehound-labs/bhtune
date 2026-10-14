---
sidebar_position: 2
---

# Crate and workspace graph

The Cargo workspace separates domain logic, tag I/O, persistence, and adapters. The frontend
and documentation site are pnpm workspace members rather than Rust crates.

| Component             | Kind                                                 | Role and dependency boundary                                                                                                                                                                                                                                                                                                                                                         |
| --------------------- | ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `bhtune-core`         | Rust library                                         | Domain model, MRFT state machine, and tuning calculations. It has no I/O, async runtime, or clock reads.                                                                                                                                                                                                                                                                             |
| `bhtune-driver`       | Rust library                                         | Defines the `Driver` tag-I/O interface and provides OPC DA, simulator, and replay implementations. Its production interface is separate from the MRFT domain types.                                                                                                                                                                                                                  |
| `bhtune-db`           | Rust library                                         | Provides the SQLite schema, migrations, and typed repository operations. It uses core model types and is the persistence boundary for the adapters.                                                                                                                                                                                                                                  |
| `bhtune-runtime`      | Rust library                                         | Owns configuration resolution, database bootstrap, logging, retention, driver setup, shared tune orchestration and safety, history operations, OPC helpers, and sample export serialization. Its source and direct dependencies use the core, driver, and database libraries, not either adapter; transport crates pulled by the OPC DA gRPC client are not exposed through its API. |
| `bhtune`              | Rust package (`bhtune_cli` library, `bhtune` binary) | Adapts command-line arguments and terminal interaction to the shared runtime for headless tuning, simulation, history, and OPC diagnostic commands.                                                                                                                                                                                                                                  |
| `bhtune-server`       | Rust library and `bhtune-server` binary              | Provides the Axum HTTP API, OpenAPI document, SSE run stream, and browser-asset handler. It adapts HTTP requests and responses to the shared runtime; it does not depend on the CLI.                                                                                                                                                                                                 |
| `bhtune-test-support` | Unpublished Rust test library                        | Shared mock gRPC bridge for driver, CLI, and server tests; never a product or release artifact.                                                                                                                                                                                                                                                                                      |
| `frontend/`           | pnpm package                                         | React and TypeScript browser interface. It calls the server API through the generated OpenAPI client; it is not a Rust crate.                                                                                                                                                                                                                                                        |
| `website/`            | pnpm package                                         | Docusaurus site built from the repository's `docs/` directory. It is separate from the runtime binaries.                                                                                                                                                                                                                                                                             |

The six product crates are distinct from test infrastructure. `fuzz/` and
`tests/opcda-yokogawa-fixture/` are separate Cargo workspaces, not product members. The server
is a product even though its package is not published to crates.io.

The main runtime paths are:

| Entry point   | Runtime path                                                                                                                    |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| CLI           | `bhtune` binary -> `bhtune_cli` library (`bhtune` package) -> `bhtune-runtime` -> `bhtune-core` + `bhtune-driver` + `bhtune-db` |
| Browser       | React SPA -> HTTP/SSE -> `bhtune-server` -> `bhtune-runtime` -> shared application services                                     |
| Documentation | Docusaurus site -> repository `docs/`                                                                                           |

An HTTP-started tune runs inside `bhtune-server` through the shared runtime; the server does
not call the CLI library or launch a second `bhtune` process. The CLI can run independently of
the server. The website has no runtime dependency on either adapter.

## Dependency boundaries

The core is dependency-minimal, not dependency-free: parsing its embedded TOML catalog and
deriving optional schemas do not perform runtime I/O. Network calls, clock reads, asynchronous
orchestration, and persistence belong outside it. See the [engine and time model](engine-and-time.md).

Runtime services expose application-owned inputs and outcomes. CLI argument types and prompts
stay in `bhtune`; HTTP requests, framework types, and OpenAPI aggregation stay in the server.
Transport crates can occur transitively through the OPC DA client without becoming runtime API
types. Neither adapter is the other's application-service dependency.

Only `bhtune-driver` has the production `opcda-bridge` dependency. Packaged builds use its
published crates.io release, not a git dependency or local source override. Shared dependencies
belong in the workspace manifest when a second crate needs them, not as unused reservations.

## Driver seam

`Driver` deals in tags and raw string values, not MRFT domain types. Connection, operation,
and unsupported errors remain distinct. A controller-rejected write is a successful transport
response containing `WriteOutcome`, not a fabricated transport failure; an accepted write is
not proof of [physical MV actuation](../guides/safety.md#mv-actuation-verification).

The OPC DA client uses `tokio::sync::Mutex` because its guard crosses asynchronous calls.
The simulator uses `std::sync::Mutex` because its state changes do not suspend. These are
different synchronization requirements, not interchangeable implementations to unify.

Gateway metadata and registered-server discovery are pre-connection helpers: they need a
gateway host but no selected OPC ProgID, so they are not methods on a connected driver.
Browse sessions, hierarchy keys, continuation tokens, and exact ItemIDs remain gateway-owned;
see [OPC browse and search](../guides/opc-gateway-compatibility.md#browse-and-search-contract).

The [runtime topology](topology.md) shows where these components run. See the
[Rust API reference](../reference/api.md) for crate-level source documentation.
