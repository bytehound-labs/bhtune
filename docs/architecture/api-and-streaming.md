---
sidebar_position: 6
---

# HTTP API and live streaming

The HTTP API is the shared contract between `bhtune-server`, the browser frontend, and
external HTTP clients. Route handlers and their request and response types define one
OpenAPI 3.1 document. Full mode serves it at `GET /api/openapi.json` and provides interactive
Scalar documentation at `GET /api/docs`. Demo omits both routes and exposes only its
[restricted simulator surface](../guides/public-simulator-demo.md#security-boundary).

The generated document is checked in as the repository-root `openapi.json`. CI regenerates it
from the server definitions and checks for a diff. The frontend's TypeScript API types are
generated from that same file and have their own regenerate-and-diff check. This keeps the
runtime routes, published contract, and browser client aligned. See the
[HTTP API reference](../reference/api.md).

## Adapter and runtime boundary

The HTTP adapter projects database records into explicit response types rather than exposing
storage rows as the wire contract. CLI and HTTP projections describe the same run without
making either adapter depend on the other.

`StartRunRequest` and CLI `TuneArgs` become a runtime-owned `ValidatedTuneRequest`.
The runtime resolves common defaults, validates inputs, prepares provenance, and drives the
tune; the server owns HTTP extraction, response mapping, and task registration. A successful
`POST /api/runs` returns `201` after preparation and runs the tune asynchronously. Cancellation
signals that task and waits for a terminal result; leaving a browser page is not cancellation.

Full mode's `POST /api/runs/preflight` uses the same request shape but calls only read-only
preflight, with a read-only template pool and `ReadOnlyDriver`. It does not call preparation,
insert history, start background tuning, write tags, or refresh a namespace.

Post-run write/revert operations use the shared runtime's pre-read, per-constant verification,
rollback where applicable, and audit services. Their in-memory HTTP reservation complements,
but does not replace, the [durable live-operation guard](safety-model.md#durable-ownership-and-recovery).
They use the run's recorded connection. Controller-ready result text and numeric write targets
come from the same snapshotted precision policy; reverting recorded previous values does not
apply that rounding.

Request/setup errors use typed HTTP error responses. A processed PID write can instead return
`200` with `writes[].success = false` and an `error_message`: HTTP success alone is not physical
write success. Clients must inspect the returned audit and restore/result evidence.

## Live run stream

`GET /api/runs/{id}/stream` sends run progress as Server-Sent Events (SSE). It emits the
persisted initial-readings snapshot, then persisted samples in tick order, and a final `done`
event when the run reaches a terminal outcome. Each connection can reconstruct the run from
its stored samples rather than depending on a client-specific in-memory history.

The stream reads the same SQLite records used by run history and the REST API. It does not
drive a second tuning loop or create another sample store. SSE is server-to-browser progress;
starting, cancelling, and reviewing a run remain ordinary HTTP API operations.

The browser closes its `EventSource` on `done` and refreshes the run-detail query so final
results, write audits, and restoration status come from the authoritative record. Reconnection
replays persisted evidence; it does not require a second broadcast store or a special in-memory
history for each visitor.

## Browser assets and fallback rules

Release builds embed `frontend/dist/` in `bhtune-server`; debug builds read those assets from
disk. Frontend assets must be built before compiling a standalone release server, and the
server must be rebuilt after asset changes.

| Request                                      | Response                                              |
| -------------------------------------------- | ----------------------------------------------------- |
| `index.html` or a client-side route          | SPA shell with `Cache-Control: no-cache`.             |
| A built static asset                         | Its MIME type and immutable asset caching.            |
| A missing path with a dotted final component | `404`, not an HTML response masquerading as an asset. |
| An unknown `/api/*` path                     | JSON `404`, not the SPA shell.                        |
| A UI request when no frontend is built       | Actionable `503`; API routes remain independent.      |

The server build script creates the asset directory before `rust-embed` expands its path guard.
An existing empty directory prevents a bad compile-time canonical path from rejecting assets
that are built later during debug development; it is not a substitute for building the frontend
before release embedding. The [frontend architecture](frontend.md) describes client state and
live/history rendering.

See the [Web GUI quickstart](../getting-started/web-gui-quickstart.md#explore-the-api-directly)
to inspect the API and its OpenAPI contract.
