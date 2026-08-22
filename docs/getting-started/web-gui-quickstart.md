---
sidebar_position: 3
---

# Web GUI quickstart

`bhtune-server` is a single binary that serves both a REST + Server-Sent-Events API and the
built React frontend — no separate Node process, reverse proxy, or Docker container required on
the target host. This walks through running a complete tune from a browser, against the
built-in simulator.

## Build and run

```sh
pnpm install
pnpm --filter bhtune-frontend run build   # embeds frontend/dist/ into the server binary
cargo run --release --bin bhtune-server
```

```text
bhtune-server listening on http://127.0.0.1:8787
```

Open that URL in a browser. If you skip the frontend build, the server still starts and serves
its API — every `/api/*` route works — but any other path returns a clear
`503 the web UI has not been built yet` instead of a blank page, naming the exact command to
run.

`bhtune-server` has no command-line flags of its own (unlike `bhtune`, it doesn't use `clap`
yet). Every setting — including which port it binds — is resolved from a config file or
environment variable, the same way the CLI resolves its own flags:

```sh
BHTUNE_BIND=0.0.0.0:8080 cargo run --release --bin bhtune-server   # bind elsewhere, e.g. for LAN access
```

Binding anywhere other than `127.0.0.1` exposes the server to your network with **no
authentication** — see [Safety](../guides/safety.md#network-exposure) before doing this outside
a trusted, isolated OT network. Authentication is a planned, not yet shipped, feature (see the
roadmap in the main [README](https://github.com/bytehound-labs/bhtune#readme)).

## Frontend development mode

If you're changing the frontend itself, run the Vite dev server alongside a running
`bhtune-server` instead of rebuilding on every change — Vite proxies `/api/*` requests through
to the real server so both stay in sync:

```sh
cargo run --bin bhtune-server &     # first, in one terminal
pnpm --filter bhtune-frontend run dev   # then, in another -- hot-reloads on save
```

## Run a tune

1. **Tune** (`/runs/new`) — the app's default landing page, and the first item in the header
   nav. One form covering everything `bhtune tune` takes as flags: connection (which driver —
   OPC DA or the simulator), tag mapping, test parameters (process type, controller type,
   relay amplitude, cycles), simulator parameters (gain/time constant/dead time/noise, when
   the simulator driver is selected), and automatic PID settings. Submitting POSTs to the same
   `/api/runs` endpoint the CLI's `bhtune-server` mode exposes — there's exactly one API,
   used by both the browser and any script that wants to drive a run over HTTP directly.
   Multiple tunes can run at the same time. PID writes and restores remain exclusive with
   active tunes because they modify live controller values directly.
   The **Start tune** and **Cancel** actions remain at the top of the page while you configure
   the tune.

   - Switching the driver to **Simulator** greys out every field the simulator genuinely
     ignores (OPC server ProgID, bridge host, tag name, automatic PID settings, quality/timeout options)
     rather than hiding them, so the form doesn't reflow and the greyed field itself explains
     what the simulator doesn't use. Fields the simulator still needs — template, PV/MV
     ranges, controller direction, and every engine parameter — stay enabled, since the
     template's unit conversions and PID type apply to every run regardless of driver.
   - Switching to **OPC DA** reveals a **Browse servers** button next to the ProgID field.
     It opens an on-demand picker populated by the bridge gateway, listing every OPC DA server
     registered on it as clickable buttons — no need to already know (or spell correctly) a
     ProgID like `Matrikon.OPC.Simulation`. A **Browse tags** button next to the Tag name field
     (enabled once a ProgID is entered) opens a lazily-expanding tag tree fetched one level at
     a time from the gateway. Hierarchical servers such as Yokogawa CSHIS can be expanded
     through nested controller/block levels until their PV leaves; dotted and slash-separated
     item IDs are supported. Selecting a leaf previews the complete tag set the active template
     would derive from it (the clearest way to see how a template's suffixes actually work),
     offers a **Read selected tag** action showing a live value and its quality, and **Use this
     tag** writes the selected tag with its final component replaced by the active template's
     process-variable suffix into the Tag name field.
   - A **Notes** field records optional operator context, observations, or follow-up actions.
     Notes are included when the run starts and can be edited or cleared from the run detail
     page while the run is active or after it finishes.
   - The form prefills from the newest run's own tune settings every time you open it fresh (or
     from a specific past run's settings via **Duplicate this run**, below) — remembered
     server-side, so it follows you across browsers and machines rather than living in
     `localStorage`. Notes are intentionally left blank for each new tune so operator context
     is not copied accidentally. A **Reset to defaults** button returns the form to the built-in
     defaults.

2. **Run detail** (`/runs/:id`) — while a run is in progress, a live PV/MV trend chart updates
   in real time over Server-Sent Events (`GET /api/runs/:id/stream`), alongside the current
   relay switch count and cycles remaining. A **Cancel** button stops the run early (the same
   Ctrl+C-triggered abort-and-restore path the CLI uses — see
   [Safety](../guides/safety.md#cancellation)). Once complete, the same page shows:
   - The calculated Aggressive/Moderate/Sluggish PID constants, each row with its own
     **Apply** button to send that response level's constants to the loop after the fact —
     independently of any `--write-pid` choice made before the run started. A confirmation
     dialog names the tag and the exact tag/value pairs before anything is sent.
   - A mutable **Notes** field with **Save notes** and **Clear notes** actions. Notes are
     metadata, so editing them does not interrupt an active tune.
   - A **PID change history** table of every PID change this tune has made, each with a pre-write
     readback, a post-write readback, and a rollback status. The newest successful write
     shows a **Restore previous values** button that writes the pre-write values back, also behind a
     confirmation dialog.

     Both buttons are disabled — with the reason shown as text, never a silent, unexplained
     grey button — unless the run is finished, used the OPC DA driver, has PID constant tags
     configured, and recorded which OPC server/bridge host it connected to.

   - **Export CSV**/**Export JSON** download links and a **Delete tune** button (with a
     confirmation prompt — deleting a tune also removes its recorded measurements and
     results, and cannot be undone).
   - A **Duplicate this run** button, returning to the New tune form prefilled from this run's
     tune settings instead of the newest run's; Notes starts blank.
3. **History** (`/runs`) — every past tune, shown by **Tag name** and filterable by outcome and
   process type, with the same detail view available for any completed run — not just the one
   you just started.
4. **Templates** (`/templates`) — the four built-in DCS/PLC templates are listed on first
   launch (seeded automatically into the database). Open one to see its full tag-suffix
   mapping, or create a new one instead of hand-editing a TOML file.

## Explore the API directly

Every route is documented at `/api/docs` (an interactive Scalar UI generated from the same
OpenAPI 3.1 spec the frontend's TypeScript client is generated from — the two can never drift
apart), and the raw spec is at `/api/openapi.json`. A minimal health check:

```sh
curl http://127.0.0.1:8787/api/health
# {"status":"ok","version":"<application-version>"}
```

The web GUI displays this application version beside its connection status in the header.

Starting a tune over HTTP directly (no browser) needs the same fields the CLI's `tune`/
`simulate` commands take — see `/api/docs` for the full request schema, including the extra
range/direction fields the simulator driver requires that a real OPC DA driver would instead
read live from the DCS.

## Next steps

- [CLI quickstart](cli-quickstart.md) — the same tuning engine, scriptable for scheduled/
  unattended runs.
- [Safety](../guides/safety.md) — cancellation, quality enforcement, and write-back rollback,
  all shared between the CLI and the server.
- [DCS/PLC templates](../dcs-templates.md) — the tag-mapping system behind the Templates screen.
