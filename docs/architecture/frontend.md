---
sidebar_position: 7
---

# Browser frontend

The browser interface is a React and TypeScript single-page application in `frontend/`. It
uses the generated TypeScript client for `bhtune-server`'s HTTP API. The browser talks to the
server rather than connecting to SQLite or the OPC DA gateway directly.

## One API client

`frontend/src/api/client.ts` is the single `openapi-fetch` client, typed by
`schema.d.ts` generated from the repository-root OpenAPI document. TanStack Query owns request
state and caching; there is no alternate transport interface or second client implementation.
Typed enum-label maps keep display labels separate from snake_case wire values.

Client-side validation gives prompt feedback, but shared runtime/server validation remains
authoritative. Forms use the shared UI fields, loading primitives, error banners, and modal
components rather than browser `alert`, `confirm`, or `prompt`.

## Form state and mapping

The Full-mode New Tune form uses this precedence: **Duplicate this run**, saved server draft,
newest-run request snapshot, then built-in defaults. The SQLite draft stores editable fields
except Notes and preserves inactive-driver values. Notes stay blank when prefilling a new tune;
operator context is not silently copied from an earlier operation. Demo has no server draft
route and keeps its restricted, expiring browser draft separately.

Base-tag changes share one form-state helper. Custom PV/MV/mode/PID mappings and custom
direction/range read tags reset to template sources; fixed direction/range values remain.
Duplicate/prefill preserves Template/Custom/Fixed source distinctions, including omitted
template-derived values. Process-type defaults for skip/count/noise are separate from DCS
template mappings. See [starting a tune](../guides/web-ui/starting-a-tune.md).

## Live trends and history

An `EventSource` consumes persisted initial readings and samples while a run is active.
Completion closes the stream and refreshes the run query; periodic detail queries provide a
fallback rather than a second tuning loop. Terminal views use the persisted history payload.

One `uPlot` component renders live and historical data, updating its existing instance rather
than recreating it per sample. Initial-reading and restored-MV boundary points are presentation
only. Short runs reserve twelve configured poll intervals before fitting the complete elapsed
history; blank future space is intentional, not missing samples.

PV and **Commanded MV** are the normal trend series. Measured MV stays in the actuation audit.
Sampling/latency diagnostics remain available through CLI/API/logs, not a browser tuning-quality
badge. Genuine safety failures and invalid/unwritable results remain visible.

Calculated results use backend-provided controller-ready strings, labels, and snapshotted
precision; the browser does not round targets independently. Write and Revert share a review
modal naming the loop, destination tags, and exact values. Closing that review after confirmation
does not prove a write succeeded: the returned audit and page-level errors describe the outcome.

## OPC navigation

The tag tree uses opaque gateway sessions, node keys, page tokens, and breadcrumbs. Tree
expansion is distinct from selecting an exact ItemID: a branch arrow is its own button, while
labels retain selection/double-click behavior and roving keyboard focus.

Saved-tag restoration keeps the tree mounted but covered until the selected row is rendered
and visible in the inner viewport. Missing tags, root errors, cancellation, and fallback
selection must clear the shared loading overlay rather than leave a stuck pending state.

Terminal index-error acknowledgement is page-local to the exact gateway/server pair. Closing
Browse acknowledges only notices displayed during that visit, never a hidden or later failure.
New attempts and accepted/observed builds re-arm notices. This presentation state does not clear
gateway diagnostics, change index usability, or persist in drafts/browser storage. Browse,
request, search, and selection errors remain separate. See the
[browse/search contract](../guides/opc-gateway-compatibility.md#browse-and-search-contract).

## Serving and development

In release builds, the built SPA is embedded in the `bhtune-server` executable. The server
serves the page and API from one process, so a released server does not need Node.js or a
separate static-file server on the target host. Debug builds read the built `frontend/dist/`
assets from disk.

The Vite development server is a separate development tool. Its `/api` proxy forwards requests
to a locally running `bhtune-server` while Vite provides frontend hot reload; that proxy is not
part of the release serving path.

The Docusaurus site in `website/` is a separate static documentation site. It reads the
repository's `docs/` directory and is not embedded in `bhtune-server`.

Playwright validates real browser behavior; the native OPC fixture validates the shared
backend, not React rendering or plant compatibility. See
[contributor testing guidance](https://github.com/bytehound-labs/bhtune/blob/main/CONTRIBUTING.md#testing).
