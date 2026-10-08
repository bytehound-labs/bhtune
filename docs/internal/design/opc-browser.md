# OPC browse, search, and tag selection

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Scalable OPC browse/search integration

The BHTune OPC integration now targets `opcda-bridge` 0.5.0 or newer. The old flat, globally
limited namespace model is not supported: `bhtune-driver` uses typed capabilities, bounded browse
pages, opaque browse sessions/node keys/page tokens, exact ItemIDs, branch/item/branch-and-item
node kinds, explicit session cleanup, and gateway-owned persistent indexed search. The HTTP API
and React browser proxy the same contract, while simulator and replay drivers return explicit
unsupported errors instead of fabricating a namespace.

The browser loads only the requested page, preserves continuation state, offers **Load more**,
keeps expandable-and-selectable nodes usable, and uses the persistent index for bounded,
debounced fzf-style search. Search exposes ranked exact ItemIDs, breadcrumbs, index state/
progress, and `has_more`; stale or partial results are labeled rather than presented as an
authoritative no-match. It uses gateway breadcrumbs/search to reveal a saved selection and never
splits `.`, `!`, or `/` to guess hierarchy. The CLI mirrors this with
`bhtune opc gateway-info`, `bhtune opc servers`, paged `bhtune opc browse` (or explicit
`--all` draining), live `bhtune opc search`, and
`bhtune opc search-index status|search|refresh|control`; progress and warnings stay on stderr so
JSON output remains machine-readable. CLI browse sessions remain open for continuation after a
page is printed and are released with `bhtune opc close <session-id>`.
If a completed gateway inventory reports a non-fatal diagnostic, the active index remains usable.
The diagnostic remains available through the gateway/API and CLI, while the browser only shows an
index error when the usable index state is `failed`.

Terminal index-error visibility is separate from the raw gateway status. `NewRunPage` owns
acknowledged failure identities for each exact bridge-host/OPC-server pair; closing Browse
records only diagnostics whose output was rendered during that visit. Reopening uses the
same acknowledgement for cached and freshly fetched status without rewriting `last_error`,
the failed state, or usable-generation/search availability. Attempt start/completion and
scheduler attempt timestamps distinguish later failures with identical text; accepted or
observed builds re-arm the notice when timestamps are absent. Starting a build also clears
the visit's earlier seen identities, so closing mid-build cannot acknowledge an unseen later
failure with identical metadata. Covered quality-warning/delete-confirmation panels do not
acknowledge hidden diagnostics. Nested confirmation dismissal does not close Browse.
Acknowledgements are page-local and are not stored with drafts or in browser storage.
Request, browse, search, and selection errors retain their separate handling.

Indexed search is deliberately not a prerequisite for tag selection or tuning. When a server is
not enrolled, still building its first generation, or has no usable index, the browser disables
only the global search input and offers **Build index** or **Retry build** while keeping the
lazy tree, exact ItemID entry, quality read, and selection controls available. It never falls
back automatically to the slow live whole-server search. A failed tree page retains already
loaded nodes and exposes a per-level **Retry** action;
when automatic refresh is enabled, the browser shows the next scheduled refresh as a relative
days-and-hours countdown and keeps the exact scheduled time in the hover tooltip.
unknown `/api/*` paths return JSON 404 responses instead of the SPA shell, making stale
server/frontend combinations diagnosable.

Live acceptance against `Yokogawa.CSHIS_OPC.1` confirmed the root page exposes the full controller
family set, navigation reaches `FCS0201` → `204FI00510` → `PV`, the exact ItemID
`FCS0201!204FI00510.PV` is preserved, a diagnostic read returns `Good`, and scoped search
returns breadcrumbs. A full unscoped exact-ItemID search can still hit the gateway's bounded
visit limit; callers should prefer scoped search or an exact display-name query when the
namespace is large, and the browser must continue to present any truncated result as partial.

## OPC server discovery and gateway info

The re-exported `bhtune_driver::opcda::get_opcda_gateway_info(bridge_host)` and
`list_opcda_servers(bridge_host)` free functions cover gateway-wide compatibility metadata and OPC
DA server discovery. They are deliberately **not** `Driver`/`OpcDaDriver` methods — both are
pre-connection operations requiring only a bridge host, not the OPC DA server ProgID that
`OpcDaDriver::connect` requires. `get_opcda_gateway_info` calls only the gateway-wide metadata RPC
and therefore works before OPCEnum or an OPC DA server is available; `list_opcda_servers` connects
for one discovery RPC and drops the connection immediately afterward. Note for anyone reading the
wire calls: `opcda_bridge::Client::list_servers` always sends `host: "localhost"`, i.e. it lists
servers registered on _the gateway's own_ machine, not on whatever machine bhtune itself runs on —
exactly right for this topology (the gateway runs next to the OPC DA server), and worth knowing so
it's never mistaken for a bug. On the CLI side, `bhtune opc gateway-info` reports the application
version and core/namespace/indexed-search ranges without contacting an OPC server, while `bhtune opc
servers [--bridge-host <HOST>]` fills the discovery gap in the existing `opc read`/`write`/`browse`
diagnostic family — it was previously impossible to discover a server's ProgID from bhtune at all,
forcing a round-trip to `opcda-bridge-client`'s own CLI just to find out what to pass to `--server`.
Both the shared smoke-test mock gateway (`bhtune-driver::opcda`'s own `smoke_tests` module) and
the `bhtune` package's separate `test_support::MockBridgeService` gained a settable `list_servers_response`
field to cover this — the latter's mock previously hardcoded an empty response with a comment noting
`list_servers` was never actually exercised by any CLI test, which is no longer true.

## OPC HTTP API

The read-only routes (`GET /api/opc/servers`, `/browse`, and `/read`) proxy the released
session-aware `opcda-bridge` contract: `GET /api/opc/capabilities`, `GET /api/opc/browse` with
opaque session/node/page-token parameters, `DELETE /api/opc/browse/sessions/{session_id}`, `GET
/api/opc/search`, and `GET /api/opc/read`. Calls remain independent of `AppState::active_run` and
are bounded by the 30-second `OPC_QUERY_TIMEOUT_SECS` deadline. Browse responses preserve exact
ItemIDs separately from display labels, expose branch/item/branch-and-item kinds, continuation
metadata, namespace source, and warnings. The gateway's indexed-search extension adds `GET
/api/opc/search-index/status`, `GET /api/opc/search-index/search`, and refresh/control endpoints
with persistent-index state, progress, ranked exact matches, breadcrumbs, and `has_more`.
`openapi.json` and `frontend/src/api/schema.d.ts` are regenerated from the route definitions.
Indexing is an optional search accelerator with per-server enrollment owned by the gateway database.
A fresh gateway has no enrolled servers; BHTune's tag browser can build an index for any exact
ProgID returned by the gateway, without a TOML allow-list or restart. After a successful first
build, automatic refresh is enabled by default under the gateway's configurable seven-day policy.
The browser can retry, refresh, disable or re-enable automatic refresh, and delete an index while
lazy browse, direct ItemID entry, live reads, and tuning remain independent of index state.
Index-status failures are compact diagnostics rather than browse failures.

## OPC browser UI

Two pieces wire the OPC routes into the New Run form, both rendered only when `form.driver ===
"opcda"`: `OpcServerDiscovery` (`components/OpcServerDiscovery.tsx`), a "Browse servers" button next
to the ProgID field that opens an on-demand modal rather than rendering every discovered server
inline (server discovery is itself a live network call), with clickable ProgIDs that fill the field
directly, or a clean error/empty state; and `OpcTagBrowserModal`
(`components/OpcTagBrowserModal.tsx`), opened by a new "Browse tags" button next to the Tag name
field (disabled, with an explanatory `title`, until a ProgID is entered) — a lazily-expanding tree
backed by one session-aware `GET /api/opc/browse` page per request, with opaque navigation keys and
continuation tokens, whose leaf selection renders a **derived tag set preview** in the main New Run
form's collapsed **Loop mapping** section: the exact tags the active template would derive from that
selection, via a new pure `frontend/src/lib/opcTags.ts::deriveTag` — a client-side mirror of
`bhtune_core::tags::derive_from_pv_tag`'s "replace everything after the last `.`/`!`/`/` with the
suffix" algorithm, used only for this preview; the server remains the actual source of truth once a
run starts — plus a "Test read" button (`GET /api/opc/read`, showing value and quality) and "Select
tag", which replaces the selected node's final component with the active template's process-variable
suffix before writing it into the Tag name field, after a fresh read of the exact original selected
ItemID confirms `Good` quality. A non-Good result requires an explicit choice to select another tag
or proceed anyway; a read failure leaves the browser open. Double-clicking a browsed or
indexed-search selectable node performs the same selection, while double-clicking an expandable node
expands or collapses it; branch-and-item nodes support both actions. Reopening the modal uses
bridge-provided breadcrumbs or bounded search to reveal the current selection, then scrolls it into
view; if the tag is no longer available, it falls back to the root level. The selection panel is
rendered before a node is clicked, and the first loaded node is selected automatically. Its detailed
template replacement list is inside a native `details` element on the main form, which is collapsed
by default; the browse modal deliberately does not duplicate that mapping preview. This is
deliberately not "strip the suffix and use the base name": since `deriveTag`/`derive_from_pv_tag`
both work by replacing everything after the last separator, a full leaf tag (e.g. `FIC101.PV`) is
already exactly the right input — the preview panel and a real tune's tag derivation agree because
they run the identical algorithm. Template changes reuse the same separator-aware logic to replace
any final component while preserving the tag path. A new shared `Modal` component
(`components/ui.tsx`) backs the tag browser and is reusable for future modals: closes on Escape, a
backdrop click, or an explicit close button.

No standalone "Test connection" button exists elsewhere on the form — the modal's own "Test
read" already covers that need, and a second, redundant affordance would just be one more thing
to keep in sync.

## Saved-tag restoration

Saved-tag restoration is visually atomic: `OpcTagBrowserModal` keeps the tree mounted for
measurement, but covers the tree and selected-tag panel with the shared `LoadingOverlay` until the
exact saved ItemID is selected, its gateway-provided path is expanded, and the row is verified
inside the inner tree viewport. The global search controls remain visible while this region settles.
Root errors, unavailable tags, cancellation, and fallback selection must settle this phase rather
than leave a loading state stuck. Prefer the shared `Spinner`, `LoadingStatus`, `LoadingOverlay`,
and `Button` loading primitives over local spinners or text-only pending feedback when adding
related asynchronous UI.

## OPC browser regression tests

`smoke_tests::MockBridgeService` ignores request contents, so it cannot demonstrate recursive tree
expansion or a template-specific derived-tag preview; a populated-tree check needs a path-aware
fake `Bridge` service.

The permanent regression tests are `frontend/e2e/opc-browser*.spec.ts`. They exercise the OPC DA
path against the suite's real, already-running `bhtune-server` — with no gateway started at its
default `localhost:7600` bridge host, every action fails at the connection step, which resolves in
single-digit milliseconds (`ECONNREFUSED`, empirically confirmed, well inside `with_timeout`'s 30s
budget) rather than hanging. That failure is still real coverage no other spec touches: the
driver-switch visibility of "Browse servers"/"Browse tags", the "Browse tags" button's
disabled-until- a-ProgID-is-entered state, both buttons' real HTTP request wiring, the modal opening
and rendering a visible connection-error message rather than silently swallowing it, and the modal
closing via both its "Close" button and Escape. They do not attempt to re-prove the populated-tree
happy path, since standing up a second, permanent mock gRPC service just for this suite would cost
more than it would additionally prove. The suites also cover paged browse, branch-and-item nodes,
exact ItemID selection, search-based reveal, session cleanup, and non-Good quality confirmation.

## Per-tune OPC tag overrides

`TagOverrides` carries optional replacements for the template-derived process-variable,
manipulated-variable/output, setpoint, mode, mode-attribute, and P/I/D constant tags; blank values
preserve the template defaults, nonblank values are trimmed and validated before a driver
connection, and overrides are applied only on the OPC DA path. `StartRunRequest`, `TuneArgs`, the
saved New Run draft, and the durable request/effective-tag snapshots carry the same object. The main
form's collapsed mapping section is the single preview and editing location; range and
controller-direction values remain separate fixed-value overrides. The browser modal only browses,
reads, checks quality, and selects the PV tag.
