---
sidebar_position: 6
---

# HTTP API and live streaming

The HTTP API is the shared contract between `bhtune-server`, the browser frontend, and
external HTTP clients. Route handlers and their request and response types define one
OpenAPI 3.1 document. The server serves it at `GET /api/openapi.json` and provides interactive
documentation at `GET /api/docs`.

The generated document is checked in as the repository-root `openapi.json`. CI regenerates it
from the server definitions and checks for a diff. The frontend's TypeScript API types are
generated from that same file and have their own regenerate-and-diff check. This keeps the
runtime routes, published contract, and browser client aligned. See the
[HTTP API reference](../reference/api.md).

## Live run stream

`GET /api/runs/{id}/stream` sends run progress as Server-Sent Events (SSE). It emits the
persisted initial-readings snapshot, then persisted samples in tick order, and a final `done`
event when the run reaches a terminal outcome. Each connection can reconstruct the run from
its stored samples rather than depending on a client-specific in-memory history.

The stream reads the same SQLite records used by run history and the REST API. It does not
drive a second tuning loop or create another sample store. SSE is server-to-browser progress;
starting, cancelling, and reviewing a run remain ordinary HTTP API operations.

See the [Web GUI quickstart](../getting-started/web-gui-quickstart.md#explore-the-api-directly)
to inspect the API and its OpenAPI contract.
