---
sidebar_position: 1
---

# Runtime topology

BHTune separates the operator interface, tuning logic, plant connectivity, and run history.
The command-line tool and browser interface are peer adapters over `bhtune-runtime`, which
owns shared configuration, database bootstrap, driver setup, tune orchestration, and safety.

| Component              | Typical placement                             | Responsibility                                                                                                                                    |
| ---------------------- | --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Browser                | Operator workstation                          | Loads the React interface and calls `bhtune-server` over HTTP. It does not connect directly to an OPC DA gateway.                                 |
| `bhtune-server`        | BHTune host                                   | Serves the HTTP API and browser interface, runs HTTP-started tunes, and reads or writes the configured SQLite database.                           |
| `bhtune` CLI           | BHTune host or operator workstation           | Runs headless tunes, simulation, history, and diagnostics without requiring `bhtune-server`.                                                      |
| SQLite database        | BHTune host                                   | Stores templates, run configuration and samples, calculated results, and write or actuation audit records.                                        |
| `opcda-bridge-gateway` | Windows host with access to the OPC DA server | Exposes the network interface used by BHTune and performs the Windows-side OPC DA integration. It is commonly installed beside the OPC DA server. |
| OPC DA server          | DCS/PLC environment                           | Resolves the server ProgID and ItemIDs to live controller values and writes.                                                                      |

The gateway is a separate process, not part of BHTune's executable. BHTune's OPC DA driver
communicates with it over the network; the gateway handles the Windows-specific integration
with the OPC DA server. The browser communicates with `bhtune-server`, which uses the same
driver and tune orchestration libraries as the CLI.

| Operation                   | Call path                                                                                     |
| --------------------------- | --------------------------------------------------------------------------------------------- |
| CLI tune against OPC DA     | `bhtune` -> `bhtune-runtime` -> `bhtune-driver` -> gateway -> OPC DA server                   |
| Browser tune against OPC DA | Browser -> `bhtune-server` -> `bhtune-runtime` -> `bhtune-driver` -> gateway -> OPC DA server |
| Simulator tune              | CLI or server -> shared runtime -> in-process simulator; no gateway is involved               |
| Run history                 | CLI or server -> shared runtime and `bhtune-db` -> configured SQLite file                     |

The CLI and server use the same database schema. They share a history when they resolve to the
same database file. The browser does not have its own database or plant connection.

## Driver choices

| Driver    | Role                                                                  | Time and I/O boundary                                                                                                              |
| --------- | --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| OPC DA    | Live DCS/PLC connectivity through the published `opcda-bridge` client | The Windows gateway performs COM calls. BHTune supplies UTC-anchored monotonic engine time rather than using a driver's timestamp. |
| Simulator | Plant-free tuning against an in-process FOPDT model                   | Each PV read advances one configured fixed step. There is no gateway or PID-constant write-back.                                   |
| Replay    | Validation against a recorded trace                                   | Recorded timestamps are the input. This is a library/test driver, not a selectable CLI production mode.                            |

Simulator and replay drivers explicitly report browse/search as unsupported; they do not
fabricate a plant namespace. The separate `VirtualPid` utility supports closed-loop validation,
not background control by the simulator driver. See the
[simulator model](../guides/simulator-model.md) and [time model](engine-and-time.md).

## Scope and deployment boundary

v1 tunes one loop with MRFT through the CLI or browser; there is no desktop application.
An external scheduler can invoke the CLI. OPC UA, Modbus, Step Test, multi-loop batch work,
and a built-in scheduler are [roadmap](../roadmap.md) items, not alternative v1 paths.

Product OPC I/O is cross-platform and COM-free. Windows service integration is platform-gated;
the unpublished native COM fixture is test-only. Keeping COM in the separate gateway avoids
making a BHTune host depend on a Windows OPC runtime.

Full mode is an unauthenticated trusted-network service with a loopback default. Demo mode
uses a separate simulator-only route surface and database, not the Full API with controls
hidden in the browser. See the [safety model](safety-model.md) and
[Demo deployment boundary](../guides/public-simulator-demo.md#self-hosting-requirements).

For installation locations and gateway requirements, see the
[installation guide](../getting-started/installation.md). The
[crate graph](crate-graph.md) describes which workspace components implement each boundary.
