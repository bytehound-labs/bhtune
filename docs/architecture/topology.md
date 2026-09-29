---
sidebar_position: 1
---

# Runtime topology

BHTune separates the operator interface, tuning logic, plant connectivity, and run history.
The command-line tool and browser interface use the same tuning and persistence libraries.

| Component | Typical placement | Responsibility |
| --- | --- | --- |
| Browser | Operator workstation | Loads the React interface and calls `bhtune-server` over HTTP. It does not connect directly to an OPC DA gateway. |
| `bhtune-server` | BHTune host | Serves the HTTP API and browser interface, runs HTTP-started tunes, and reads or writes the configured SQLite database. |
| `bhtune` CLI | BHTune host or operator workstation | Runs headless tunes, simulation, history, and diagnostics without requiring `bhtune-server`. |
| SQLite database | BHTune host | Stores templates, run configuration and samples, calculated results, and write or actuation audit records. |
| `opcda-bridge-gateway` | Windows host with access to the OPC DA server | Exposes the network interface used by BHTune and performs the Windows-side OPC DA integration. It is commonly installed beside the OPC DA server. |
| OPC DA server | DCS/PLC environment | Resolves the server ProgID and ItemIDs to live controller values and writes. |

The gateway is a separate process, not part of BHTune's executable. BHTune's OPC DA driver
communicates with it over the network; the gateway handles the Windows-specific integration
with the OPC DA server. The browser communicates with `bhtune-server`, which uses the same
driver and tune orchestration libraries as the CLI.

| Operation | Call path |
| --- | --- |
| CLI tune against OPC DA | `bhtune` -> `bhtune-driver` -> `opcda-bridge-gateway` -> OPC DA server |
| Browser tune against OPC DA | Browser -> `bhtune-server` -> shared tune orchestration and `bhtune-driver` -> `opcda-bridge-gateway` -> OPC DA server |
| Simulator tune | `bhtune` or browser -> simulator driver and FOPDT model in the BHTune process; no gateway is involved |
| Run history | CLI or server -> configured SQLite file |

The CLI and server use the same database schema. They share a history when they resolve to the
same database file. The browser does not have its own database or plant connection.

For installation locations and gateway requirements, see the
[installation guide](../getting-started/installation.md). The
[crate graph](crate-graph.md) describes which workspace components implement each boundary.
